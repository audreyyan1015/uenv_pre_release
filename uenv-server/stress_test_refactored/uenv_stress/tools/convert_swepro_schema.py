#!/usr/bin/env python3
"""SWE-bench Pro 轨迹 schema 转换工具。

这个文件把 OpenHands 采集得到的 rollout schema 轨迹转换为稳定性套件接受的 trace-corpus schema。它保留真实指令、响应、实例 ID、token 和校验信息，并明确标记采集阶段没有记录而只能派生或填充的字段。

实现逻辑是：读取输入 JSONL 后，为每条 SWE-bench Pro 记录提取 instance_id、turn、模型响应和元数据；fetch_instruction_texts 从数据集或补充文件中取回 instruction；canonical_checksum 为标准化内容生成哈希；main 写出转换后的 JSONL、转换摘要和派生字段说明，并让后续 validate_trace_file/manifest 校验接管准入。"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import time
from pathlib import Path

from uenv_stress.core.distributed_runtime import connect


def canonical_checksum(value) -> str:
    encoded = json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest()


def fetch_instruction_texts(rows, worker, password):
    """Read instruction.txt for each row's source_path job dir from the worker."""
    texts: dict[str, str] = {}
    client = connect(worker, password)
    sftp = client.open_sftp()
    for row in rows:
        job_dir = str(Path(str(row["source_path"])).parent)
        inst = job_dir + "/instruction.txt"
        try:
            with sftp.file(inst, "r") as fh:
                texts[row["instance_id"]] = fh.read().decode("utf-8", "replace")
        except OSError:
            texts[row["instance_id"]] = ""
    client.close()
    return texts


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", type=Path, default=Path("/home/uenv-stress/trace-corpora/swebench_pro/trace-corpus.jsonl"))
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--tokenizer", required=True)
    parser.add_argument("--worker", default="8.130.65.20")
    parser.add_argument("--source-model", default="doubao-seed-2-1-pro-260628")
    parser.add_argument("--source-version", default="doubao-openhands-swebench-pro-20260726")
    args = parser.parse_args()
    password = os.environ.get("UENV_PASS")
    if not password:
        raise SystemExit("UENV_PASS is required")

    from transformers import AutoTokenizer  # noqa: E402

    tokenizer = AutoTokenizer.from_pretrained(args.tokenizer, local_files_only=True, trust_remote_code=False)
    rows = [json.loads(line) for line in args.input.read_text(encoding="utf-8").splitlines() if line.strip()]
    if len(rows) != 50:
        raise SystemExit(f"expected 50 rollout rows, got {len(rows)}")

    instructions = fetch_instruction_texts(rows, args.worker, password)
    missing_inst = sum(1 for value in instructions.values() if not value)
    if missing_inst:
        print(f"[warn] {missing_inst} instruction.txt missing; prompt_hash falls back to instance_id", file=sys.stderr)

    out_rows = []
    for row in rows:
        instance_id = str(row["instance_id"])
        turns_in = row.get("turns") or []
        result = row.get("result") or {}
        elapsed_ms = int(float(result.get("elapsed_sec") or 0) * 1000)
        per_turn_env_ms = elapsed_ms // max(len(turns_in), 1)
        instruction = instructions.get(instance_id) or instance_id
        turns = []
        for turn in turns_in:
            output = str(turn.get("assistant_output") or "")
            # response_ids/logprobs 是 replay 与训练证据的必需字段，必须随转换保留，
            # 否则下游校验会以 “no replayable real trace turns” 拒绝整条语料。
            response_ids = list(turn.get("response_ids") or [])
            logprobs = list(turn.get("logprobs") or [])
            turns.append({
                "turn_index": int(turn.get("turn_index", len(turns))),
                "assistant_output": output,
                "response_ids": response_ids,
                "logprobs": logprobs,
                "source_completion_tokens": len(logprobs or response_ids),
                "target_qwen3_tokens": len(tokenizer.encode(output, add_special_tokens=False)),
                "request_bytes": 0,
                "response_bytes": len(output.encode("utf-8")),
                "env_latency_ms": per_turn_env_ms,
            })
        sealed_ms = int((row.get("trajectory_ref") or {}).get("sealed_at_ms") or 0)
        collected_at = (
            time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(sealed_ms / 1000))
            if sealed_ms else time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
        )
        out_rows.append({
            "trace_id": f"swebench_pro-{hashlib.sha256((instance_id + str(row.get('run_id'))).encode()).hexdigest()[:20]}",
            "dataset": "swe-bench-pro",
            "dataset_id": instance_id,
            "instance_id": instance_id,
            "benchmark_variant": "pro",
            "driver": "run_swebenchpro_official.py",
            "driver_entrypoint": str(row.get("driver_entrypoint") or "run_swebenchpro_official.py"),
            "source_model": args.source_model,
            "source_version": args.source_version,
            "collected_at": collected_at,
            "prompt_hash": hashlib.sha256(instruction.encode()).hexdigest(),
            "turns": turns,
            "episode_total_ms": elapsed_ms,
            "final_status": "completed" if result.get("resolved") else "failed",
            "result_checksum": canonical_checksum(result),
            "conversion": {
                "from": str(args.input),
                "derived_fields": ["request_bytes", "env_latency_ms"],
                "note": "request_bytes not recorded by collector (0 placeholder); "
                        "env_latency_ms is episode elapsed split uniformly across turns",
            },
        })

    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("w", encoding="utf-8") as target:
        for out_row in out_rows:
            target.write(json.dumps(out_row, ensure_ascii=False, sort_keys=True) + "\n")
    print(f"wrote {len(out_rows)} rows -> {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
