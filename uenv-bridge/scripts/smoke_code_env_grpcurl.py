#!/usr/bin/env python3
"""CodeEnv / DSCodeBench smoke：ExecuteBatch + inline test_code + response_text."""

from __future__ import annotations

import base64
import json
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "fixtures/code/samples/ds_smoke_001.json"


def execute_batch(server: str, sample: dict, request_id: str) -> dict:
    env_config = {
        "question": sample["question"],
        "dataset": sample.get("dataset", "dscodebench"),
        "task_id": sample.get("task_id"),
        "library": sample.get("library"),
        "test_code": sample.get("test_code"),
        "entry_point": sample.get("entry_point"),
        "num_tests": sample.get("num_tests"),
        "random_seed": sample.get("random_seed"),
        "timeout_secs": sample.get("timeout_secs"),
        "response_text": sample.get("response_text"),
    }
    env_config = {k: v for k, v in env_config.items() if v is not None}
    envelope = {
        "correlation_id": request_id,
        "env_config": env_config,
        "episode_config": {"max_steps": 1, "seed": 42},
        "reward_config": {"type": "rule_reward"},
        "timeout_seconds": int(sample.get("timeout_secs") or 120),
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
                "envType": "code",
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
    sample = json.loads(FIXTURE.read_text(encoding="utf-8"))
    rid = f"code-dscodebench-{int(time.time())}"
    data = execute_batch(server, sample, rid)
    first = (data.get("results") or [{}])[0]
    status = first.get("status")
    reward = first.get("reward")
    ok = status == "completed" and float(reward or 0) == 1.0
    print(json.dumps({"endpoint": server, "status": status, "reward": reward, "ok": ok}, indent=2))
    if not ok:
        print(json.dumps({"failed": first}, indent=2))
        return 1
    print("OK: code env e2e passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
