#!/usr/bin/env python3
"""Convert SWE-bench Pro rollout-schema trace corpus to the stability-suite schema.

Input : /opt/uenv-stress/trace-corpora/swebench_pro/trace-corpus.jsonl
        (OpenHands rollout schema produced by the Doubao collection run)
Output: stability-suite curated schema accepted by
        stability_test_common.validate_trace_file(minimum=50) and
        prepare_stability_trace_manifest.validate_swe.

Derivation notes (fields not recorded by the collector):
- request_bytes: no per-turn request size was recorded; set to 0 and marked
  in `conversion.derived_fields` (honest placeholder, not fabricated data).
- env_latency_ms: per-turn env time was not recorded; episode elapsed time is
  split uniformly across turns and marked in `conversion.derived_fields`.
- prompt_hash: sha256 of the job instruction text fetched from the worker
  job directory referenced by each row's `source_path`.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import time
from pathlib import Path

import paramiko


def canonical_checksum(value) -> str:
    encoded = json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest()


def fetch_instruction_texts(rows, worker, password):
    """Read instruction.txt for each row's source_path job dir from the worker."""
    texts: dict[str, str] = {}
    client = paramiko.SSHClient()
    client.set_missing_host_key_policy(paramiko.AutoAddPolicy())
    client.connect(worker, username="root", password=password, timeout=15)
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
    parser.add_argument("--input", type=Path, default=Path("/opt/uenv-stress/trace-corpora/swebench_pro/trace-corpus.jsonl"))
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--tokenizer", required=True)
    parser.add_argument("--worker", default="8.130.65.20")
    parser.add_argument("--worker-password", default=os.environ.get("UENV_PASS", ""))
    parser.add_argument("--source-model", default="doubao-seed-2-1-pro-260628")
    parser.add_argument("--source-version", default="doubao-openhands-swebench-pro-20260726")
    args = parser.parse_args()

    from transformers import AutoTokenizer  # noqa: E402

    tokenizer = AutoTokenizer.from_pretrained(args.tokenizer, local_files_only=True, trust_remote_code=False)
    rows = [json.loads(line) for line in args.input.read_text(encoding="utf-8").splitlines() if line.strip()]
    if len(rows) != 50:
        raise SystemExit(f"expected 50 rollout rows, got {len(rows)}")

    instructions = fetch_instruction_texts(rows, args.worker, args.worker_password)
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
            turns.append({
                "turn_index": int(turn.get("turn_index", len(turns))),
                "assistant_output": output,
                "source_completion_tokens": len(turn.get("logprobs") or turn.get("response_ids") or []),
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
