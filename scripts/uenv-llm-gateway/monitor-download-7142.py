#!/usr/bin/env python3
"""Monitor 7142 AWQ model download; optional auto-restart via resume-download-7142.sh."""
from __future__ import annotations

import argparse
import os
import subprocess
import sys
import time
from datetime import datetime
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_KEY = REPO_ROOT / "secrets/2a9f778a35e7d08c738c79493ba643ef_65c3b455afbe3c81a8a757c01b0faae8_8.142"

HOST = os.environ.get("UENV_ADAPTER_HOST", "219.147.100.43")
PORT = os.environ.get("UENV_ADAPTER_SSH_PORT", "7142")
MODEL_DIR = os.environ.get("UENV_MODEL_DIR", "/data/models/DeepSeek-V3-0324-AWQ")
MIN_GB = float(os.environ.get("UENV_MODEL_MIN_GB", "300"))
INTERVAL_SEC = int(os.environ.get("UENV_MONITOR_INTERVAL_SEC", "120"))
REMOTE_RESUME = "/root/UEnv/scripts/uenv-llm-gateway/resume-download-7142.sh"


def resolve_key() -> str:
    env = os.environ.get("UENV_SSH_KEY")
    if env and Path(env).is_file():
        return env
    if DEFAULT_KEY.is_file():
        return str(DEFAULT_KEY)
    raise FileNotFoundError("Set UENV_SSH_KEY to 7142 private key")


def ssh_cmd(key: str, script: str, timeout: int = 60) -> str:
    r = subprocess.run(
        [
            "ssh",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=30",
            "-i",
            key,
            "-p",
            PORT,
            f"root@{HOST}",
            script,
        ],
        capture_output=True,
        text=True,
        timeout=timeout,
    )
    return (r.stdout or "") + (r.stderr or "")


def check(key: str) -> tuple[float, bool, bool, str]:
    out = ssh_cmd(
        key,
        f"du -sb {MODEL_DIR} 2>/dev/null | awk '{{print $1}}'; "
        f"pgrep -f 'resume-download-7142.sh run' >/dev/null && echo RESUME_RUNNING || echo RESUME_STOPPED; "
        f"pgrep -f 'download-awq-resumable.py' >/dev/null && echo PY_RUNNING || echo PY_STOPPED; "
        f"pgrep -f 'hf download.*DeepSeek-V3-0324-AWQ' >/dev/null && echo HF_RUNNING || echo HF_STOPPED; "
        f"ls {MODEL_DIR}/*.safetensors 2>/dev/null | wc -l",
    ).strip()
    lines = [ln.strip() for ln in out.splitlines() if ln.strip()]
    size_bytes = int(lines[0]) if lines and lines[0].isdigit() else 0
    size_gb = size_bytes / (1024**3)
    running = "RESUME_RUNNING" in out or "PY_RUNNING" in out or "HF_RUNNING" in out
    shard_count = 0
    for ln in reversed(lines):
        if ln.isdigit() and int(ln) <= 50:
            shard_count = int(ln)
            break
    detail = (
        f"{size_gb:.1f}GB shards={shard_count} "
        f"proc={'running' if running else 'stopped'}"
    )
    done = size_gb >= MIN_GB and not running and shard_count >= 35
    return size_gb, done, running, detail


def restart_download(key: str) -> None:
    out = ssh_cmd(key, f"bash {REMOTE_RESUME} start", timeout=30)
    print(f"[monitor] auto-restart: {out.strip()}", flush=True)


def main() -> int:
    parser = argparse.ArgumentParser(description="Monitor 7142 AWQ download")
    parser.add_argument(
        "--auto-restart",
        action="store_true",
        help="if download supervisor is stopped and incomplete, run resume-download start",
    )
    args = parser.parse_args()

    key = resolve_key()
    print(
        f"[monitor] started {datetime.now():%Y-%m-%d %H:%M:%S} "
        f"target>={MIN_GB}GB auto_restart={args.auto_restart}",
        flush=True,
    )
    last_gb = -1.0
    stalled_checks = 0
    while True:
        try:
            size_gb, done, running, detail = check(key)
            if abs(size_gb - last_gb) >= 0.5 or done or not running:
                pct = min(100, size_gb / 360 * 100)
                print(
                    f"[monitor] {datetime.now():%H:%M:%S} {detail} (~{pct:.0f}%)",
                    flush=True,
                )
                if abs(size_gb - last_gb) < 0.1:
                    stalled_checks += 1
                else:
                    stalled_checks = 0
                last_gb = size_gb

            if done:
                print("DOWNLOAD_COMPLETE", flush=True)
                print(
                    f"[monitor] Model ready at {MODEL_DIR} ({size_gb:.1f}GB). "
                    "Run on 7142: bash /root/UEnv/scripts/uenv-llm-gateway/start-vllm-when-ready-7142.sh",
                    flush=True,
                )
                return 0

            if args.auto_restart and not running and size_gb < MIN_GB:
                restart_download(key)
                stalled_checks = 0
            elif not running and size_gb < MIN_GB and stalled_checks >= 1:
                print(
                    "[monitor] download stopped and incomplete; "
                    f"run: bash scripts/uenv-llm-gateway/remote-start-resume-download-7142.sh",
                    flush=True,
                )
        except Exception as exc:  # noqa: BLE001
            print(f"[monitor] error: {exc}", flush=True)
        time.sleep(INTERVAL_SEC)


if __name__ == "__main__":
    sys.exit(main())
