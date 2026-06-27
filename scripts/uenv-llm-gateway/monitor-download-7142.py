#!/usr/bin/env python3
"""Monitor 7142 AWQ model download; print DOWNLOAD_COMPLETE when ready."""
from __future__ import annotations

import subprocess
import sys
import time
from datetime import datetime

KEY = r"d:/code/UEnv/secrets/2a9f778a35e7d08c738c79493ba643ef_65c3b455afbe3c81a8a757c01b0faae8_8.142"
HOST = "219.147.100.43"
PORT = "7142"
MODEL_DIR = "/data/models/DeepSeek-V3-0324-AWQ"
MIN_GB = 300
INTERVAL_SEC = 120  # 每 2 分钟检查一次


def ssh_cmd(script: str) -> str:
    r = subprocess.run(
        [
            "ssh",
            "-o", "BatchMode=yes",
            "-o", "ConnectTimeout=30",
            "-i", KEY,
            "-p", PORT,
            f"root@{HOST}",
            script,
        ],
        capture_output=True,
        text=True,
        timeout=60,
    )
    return (r.stdout or "") + (r.stderr or "")


def check() -> tuple[float, bool, str]:
    out = ssh_cmd(
        f"du -sb {MODEL_DIR} 2>/dev/null | awk '{{print $1}}'; "
        f"pgrep -f 'hf download.*DeepSeek-V3-0324-AWQ' >/dev/null && echo RUNNING || echo STOPPED; "
        f"ls {MODEL_DIR}/*.safetensors 2>/dev/null | wc -l"
    ).strip()
    lines = [ln.strip() for ln in out.splitlines() if ln.strip()]
    size_bytes = int(lines[0]) if lines and lines[0].isdigit() else 0
    size_gb = size_bytes / (1024**3)
    running = "RUNNING" in out
    shard_count = 0
    for ln in reversed(lines):
        if ln.isdigit() and int(ln) <= 50:
            shard_count = int(ln)
            break
    detail = f"{size_gb:.1f}GB shards={shard_count} proc={'running' if running else 'stopped'}"
    # 完成条件：体积达标 + 下载进程已退出 + 分片数接近 36
    done = size_gb >= MIN_GB and not running and shard_count >= 35
    return size_gb, done, detail


def main() -> int:
    print(f"[monitor] started {datetime.now():%Y-%m-%d %H:%M:%S} target>={MIN_GB}GB", flush=True)
    last_gb = -1.0
    while True:
        try:
            size_gb, done, detail = check()
            if abs(size_gb - last_gb) >= 0.5 or done:
                pct = min(100, size_gb / 360 * 100)
                print(
                    f"[monitor] {datetime.now():%H:%M:%S} {detail} (~{pct:.0f}%)",
                    flush=True,
                )
                last_gb = size_gb
            if done:
                print("DOWNLOAD_COMPLETE", flush=True)
                print(
                    f"[monitor] Model ready at {MODEL_DIR} ({size_gb:.1f}GB). "
                    "Run on 7142: bash /root/UEnv/scripts/uenv-llm-gateway/start-vllm-when-ready-7142.sh",
                    flush=True,
                )
                return 0
        except Exception as exc:  # noqa: BLE001
            print(f"[monitor] error: {exc}", flush=True)
        time.sleep(INTERVAL_SEC)


if __name__ == "__main__":
    sys.exit(main())
