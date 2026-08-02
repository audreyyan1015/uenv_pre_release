#!/usr/bin/env python3
"""QA (原 math) 多 dataset 实机 smoke：AdapterCoreService/ExecuteBatch + mock LLM response_text。

env_type=qa（单轮问答/分类验证环境）。Worker 侧 plugins/qa 复用 math 判分（按 dataset 路由）。
用法：python3 smoke_qa_datasets_grpcurl.py [server]（默认 8.130.75.157:8088）
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import subprocess
import sys
import time
import uuid
from pathlib import Path
from urllib import error, request

ROOT = Path(__file__).resolve().parents[2]

CASES = [
    {
        "name": "gsm8k",
        "dataset": "gsm8k",
        "question": "Natalia sold clips to 48 friends in April and half as many in May. Total?",
        "ground_truth": "72",
        "response_text": "#### 72",
    },
    {
        "name": "pubmedqa",
        "dataset": "pubmedqa",
        "question": "Context: A randomized trial showed improved outcomes.\nQuestion: Does treatment X improve outcomes?",
        "ground_truth": "yes",
        "response_text": "Based on the abstract, the answer is yes.",
    },
    {
        "name": "scitab",
        "dataset": "scitab",
        "question": "Table: Group A mean=10, Group B mean=5.\nClaim: Group A outperformed Group B.",
        "ground_truth": "supports",
        "response_text": "The claim is supports by the table.",
    },
    {
        "name": "olymmath-easy",
        "dataset": "olymmath-easy",
        "question": "Find the value of 4+4+4+4.",
        "ground_truth": "16",
        "response_text": r"The final answer is \boxed{16}",
    },
]


def _b64(obj: dict) -> str:
    return base64.b64encode(json.dumps(obj).encode()).decode()


def _post_obs_event(obs_url: str, event_type: str, run_id: str, *, payload: dict | None = None) -> None:
    if not obs_url or not run_id:
        return
    token = os.environ.get("UENV_OBS_TOKEN", "").strip()
    body = json.dumps(
        {
            "event_id": str(uuid.uuid4()),
            "schema_version": "1",
            "correlation_id": f"smoke:{run_id}",
            "training_run_id": run_id,
            "source_id": f"bridge-smoke:{os.getpid()}",
            "module": "adapter",
            "entity_type": "training_run",
            "entity_id": run_id,
            "event_type": event_type,
            "seq": int(time.time() * 1000),
            "source_ts": int(time.time() * 1000),
            "payload": payload or {},
        }
    ).encode("utf-8")
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
        headers["X-Obs-Token"] = token
    req = request.Request(
        f"{obs_url.rstrip('/')}/api/v1/events",
        data=body,
        headers=headers,
        method="POST",
    )
    try:
        with request.urlopen(req, timeout=5.0) as resp:
            resp.read()
    except error.URLError as exc:
        print(f"WARN: obs {event_type} failed: {exc}", file=sys.stderr)


def execute_batch(server: str, case: dict, request_id: str, run_id: str = "") -> dict:
    # 与当前 proto (SampleEnvelope) 对齐：env/episode/reward 为独立 bytes 字段 (base64)。
    # 免-LLM 联调：Worker 的 rule_reward 短路仅在 payload 无 question 时触发
    # (model_client.rs W-2)，返回 target 作为 action → plugin 判分 → reward=1.0，
    # 用于验证 qa env_type 全链路 (dispatch→plugin→score→report)，不校验真实判分逻辑。
    env_config = {
        "dataset": case["dataset"],
    }
    batch_id = f"batch-{request_id}"
    sample_context = {
        "case": case["name"],
        "benchmark": case["dataset"],
        "batch_id": batch_id,
    }
    if run_id:
        sample_context["training_run_id"] = run_id
    req = {
        "requestId": request_id,
        "batchId": batch_id,
        "samples": [
            {
                "requestId": request_id,
                "batchId": batch_id,
                "sampleIndex": 0,
                "framework": "smoke",
                "envType": "qa",
                "parallelMode": "sync",
                "correlationId": request_id,
                "timeoutSeconds": 120,
                "envConfigJson": _b64(env_config),
                "episodeConfigJson": _b64({"max_steps": 1, "seed": 42}),
                "rewardConfigJson": _b64({"type": "rule_reward", "target": case["ground_truth"]}),
                "sampleContextJson": _b64(sample_context),
            }
        ],
    }
    proc = subprocess.run(
        [
            "grpcurl",
            "-plaintext",
            "-import-path",
            str(ROOT / "proto"),
            "-proto",
            "uenv/v1/adapter_core.proto",
            "-d",
            json.dumps(req),
            server,
            "uenv.bridge.v1.AdapterCoreService/ExecuteBatch",
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    if proc.returncode != 0:
        raise RuntimeError(proc.stderr or proc.stdout)
    return json.loads(proc.stdout)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("server", nargs="?", default="8.130.75.157:8088")
    parser.add_argument(
        "--run-id",
        default=os.environ.get("UENV_TRAINING_RUN_ID", ""),
        help="training_run_id for Obs/UI validation.",
    )
    parser.add_argument(
        "--obs-url",
        default=os.environ.get("UENV_OBS_URL", ""),
        help="Obs base URL, e.g. http://8.130.75.157:8888/obs.",
    )
    parser.add_argument(
        "--repeat",
        type=int,
        default=1,
        help="Repeat the QA smoke cases to keep the UI run visible longer.",
    )
    parser.add_argument(
        "--delay-seconds",
        type=float,
        default=0.0,
        help="Sleep after each completed episode before submitting the next one.",
    )
    parser.add_argument(
        "--close-delay-seconds",
        type=float,
        default=0.0,
        help="Sleep before emitting RUN_CLOSED so the frontend can show an active run.",
    )
    args = parser.parse_args()

    server = args.server
    run_id = args.run_id.strip()
    obs_url = args.obs_url.strip()
    repeat = max(1, args.repeat)
    delay_seconds = max(0.0, args.delay_seconds)
    close_delay_seconds = max(0.0, args.close_delay_seconds)
    results = []
    if run_id:
        _post_obs_event(obs_url, "RUN_STARTED", run_id, payload={"entry": "smoke_qa_datasets_grpcurl"})
    exit_code = 0
    for round_index in range(repeat):
        for case in CASES:
            rid = f"qa-r{round_index + 1}-{case['name']}-{int(time.time())}"
            data = execute_batch(server, case, rid, run_id=run_id)
            first = (data.get("results") or [{}])[0]
            status = first.get("status")
            reward = first.get("reward")
            ok = status == "completed" and float(reward or 0) == 1.0
            results.append(
                {
                    "round": round_index + 1,
                    "case": case["name"],
                    "request_id": rid,
                    "status": status,
                    "reward": reward,
                    "ok": ok,
                }
            )
            print(json.dumps(results[-1], ensure_ascii=False), flush=True)
            if not ok:
                print(json.dumps({"failed": first, "case": case["name"]}, indent=2))
                exit_code = 1
                break
            if delay_seconds > 0:
                time.sleep(delay_seconds)
        if exit_code != 0:
            break
    if close_delay_seconds > 0:
        time.sleep(close_delay_seconds)
    if run_id:
        _post_obs_event(
            obs_url,
            "RUN_CLOSED",
            run_id,
            payload={"entry": "smoke_qa_datasets_grpcurl", "ok": exit_code == 0},
        )
    print(json.dumps({"endpoint": server, "run_id": run_id, "results": results}, indent=2, ensure_ascii=False))
    if exit_code == 0:
        print("OK: qa multi-dataset e2e passed")
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
