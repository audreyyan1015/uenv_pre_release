#!/usr/bin/env python3
"""QA (原 math) 多 dataset 实机 smoke：AdapterCoreService/ExecuteBatch + mock LLM response_text。

env_type=qa（单轮问答/分类验证环境）。Worker 侧 plugins/qa 复用 math 判分（按 dataset 路由）。
用法：python3 smoke_qa_datasets_grpcurl.py [server]（默认 8.130.75.157:8088）
"""

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


def _b64(obj: dict) -> str:
    return base64.b64encode(json.dumps(obj).encode()).decode()


def execute_batch(server: str, case: dict, request_id: str) -> dict:
    # 与当前 proto (SampleEnvelope) 对齐：env/episode/reward 为独立 bytes 字段 (base64)。
    # 免-LLM 联调：Worker 的 rule_reward 短路仅在 payload 无 question 时触发
    # (model_client.rs W-2)，返回 target 作为 action → plugin 判分 → reward=1.0，
    # 用于验证 qa env_type 全链路 (dispatch→plugin→score→report)，不校验真实判分逻辑。
    env_config = {
        "dataset": case["dataset"],
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
                "envType": "qa",
                "correlationId": request_id,
                "timeoutSeconds": 120,
                "envConfigJson": _b64(env_config),
                "episodeConfigJson": _b64({"max_steps": 1, "seed": 42}),
                "rewardConfigJson": _b64({"type": "rule_reward", "target": case["ground_truth"]}),
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
        rid = f"qa-{case['name']}-{int(time.time())}"
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
    print("OK: qa multi-dataset e2e passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
