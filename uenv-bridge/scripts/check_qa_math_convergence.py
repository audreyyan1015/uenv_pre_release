#!/usr/bin/env python3
"""math 兼容收敛核对：确认 `qa` 可派发、`math` 已不再被任何 Worker 承接。

在能访问 Adapter Core 且有 grpcurl 的机器上（如 7143）从仓库根目录运行：
    python3 uenv-bridge/scripts/check_qa_math_convergence.py [server]

判定标准（Worker 的 env.types 已移除 math 时）：
    qa   → status=completed
    math → 派发失败 / no worker（说明 math 已下线，训练侧必须走 qa）
"""

from __future__ import annotations

import base64
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def _b64(obj: dict) -> str:
    return base64.b64encode(json.dumps(obj).encode()).decode()


def probe(server: str, env_type: str) -> dict:
    rid = f"convergence-{env_type}-check"
    req = {
        "requestId": rid,
        "batchId": f"batch-{rid}",
        "samples": [
            {
                "requestId": rid,
                "batchId": f"batch-{rid}",
                "sampleIndex": 0,
                "framework": "smoke",
                "envType": env_type,
                "correlationId": rid,
                "timeoutSeconds": 60,
                "envConfigJson": _b64({"dataset": "gsm8k"}),
                "episodeConfigJson": _b64({"max_steps": 1, "seed": 42}),
                "rewardConfigJson": _b64({"type": "rule_reward", "target": "72"}),
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
        return {"env_type": env_type, "rpc_error": (proc.stderr or proc.stdout).strip()[:300]}
    first = (json.loads(proc.stdout).get("results") or [{}])[0]
    return {
        "env_type": env_type,
        "status": first.get("status"),
        "reward": first.get("reward"),
        "error": str(first.get("errorMessage") or first.get("error") or "")[:300],
    }


def main() -> int:
    server = sys.argv[1] if len(sys.argv) > 1 else "8.130.75.157:8088"
    qa = probe(server, "qa")
    math = probe(server, "math")
    qa_ok = qa.get("status") == "completed"
    math_retired = math.get("status") != "completed"
    print(json.dumps({"endpoint": server, "qa": qa, "math": math}, ensure_ascii=False, indent=2))
    print(f"qa_dispatchable={qa_ok} math_retired={math_retired}")
    return 0 if (qa_ok and math_retired) else 1


if __name__ == "__main__":
    raise SystemExit(main())
