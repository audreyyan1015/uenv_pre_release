#!/usr/bin/env python3
"""Math 多 dataset 实机 smoke：AdapterCoreService/ExecuteBatch + mock LLM response_text."""

from __future__ import annotations

import base64
import json
import subprocess
import sys
import time
from pathlib import Path

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


def execute_batch(server: str, case: dict, request_id: str) -> dict:
    env_config = {
        "question": case["question"],
        "dataset": case["dataset"],
        "response_text": case["response_text"],
    }
    envelope = {
        "correlation_id": request_id,
        "env_config": env_config,
        "episode_config": {"max_steps": 1, "seed": 42},
        "reward_config": {"type": "rule_reward", "target": case["ground_truth"]},
        "timeout_seconds": 120,
    }
    req = {
        "requestId": request_id,
        "batchId": f"batch-{request_id}",
        "samples": [
            {
                "requestId": request_id,
                "batchId": f"batch-{request_id}",
                "sampleIndex": 0,
                "framework": "smoke",
                "envType": "math",
                "payloadJson": base64.b64encode(json.dumps(envelope).encode()).decode(),
                "metaJson": "",
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
    server = sys.argv[1] if len(sys.argv) > 1 else "8.130.75.157:8088"
    results = []
    for case in CASES:
        rid = f"math-{case['name']}-{int(time.time())}"
        data = execute_batch(server, case, rid)
        first = (data.get("results") or [{}])[0]
        status = first.get("status")
        reward = first.get("reward")
        ok = status == "completed" and float(reward or 0) == 1.0
        results.append({"case": case["name"], "status": status, "reward": reward, "ok": ok})
        if not ok:
            print(json.dumps({"failed": first, "case": case["name"]}, indent=2))
            return 1
    print(json.dumps({"endpoint": server, "results": results}, indent=2, ensure_ascii=False))
    print("OK: math multi-dataset e2e passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
