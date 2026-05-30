#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import socket
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from uenv.bridge.clients import RustCoreClientConfig, RustCoreEpisodeClient
from uenv.bridge.verl import VeRLAdapter


def free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def load_fixture() -> dict:
    fixture_path = ROOT / "tests" / "fixtures" / "verl_batch.json"
    return json.loads(fixture_path.read_text(encoding="utf-8"))


def main() -> int:
    parser = argparse.ArgumentParser(description="Verify Python -> Rust adapter core local gRPC loop")
    parser.add_argument("--endpoint", default=None)
    parser.add_argument("--reward", type=float, default=7.0)
    parser.add_argument("--startup-timeout", type=float, default=30.0)
    parser.add_argument("--skip-build", action="store_true")
    args = parser.parse_args()

    endpoint = args.endpoint or f"127.0.0.1:{free_port()}"

    subprocess.run([str(ROOT / "scripts" / "generate_adapter_core_proto.sh")], check=True)
    if not args.skip_build:
        subprocess.run(["cargo", "build"], cwd=ROOT / "core", check=True)

    env = os.environ.copy()
    env["UENV_ADAPTER_CORE_FAKE_REWARD"] = str(args.reward)
    binary = str(ROOT / "core" / "target" / "debug" / "uenv-adapter-core")
    old_env = os.environ.copy()
    os.environ.update(env)
    try:
        client = RustCoreEpisodeClient(
            RustCoreClientConfig(
                endpoint=endpoint,
                auto_start=True,
                binary=binary,
                startup_timeout_seconds=args.startup_timeout,
            )
        )
        try:
            output = VeRLAdapter(client=client).execute_batch(load_fixture())
        finally:
            client.close()
        rewards = [result["reward"] for result in output["results"]]
        if rewards != [args.reward] * len(rewards):
            raise RuntimeError(f"unexpected rewards: {rewards}")
        print(json.dumps({"endpoint": endpoint, "batch_id": output["batch_id"], "rewards": rewards}, ensure_ascii=False))
        return 0
    finally:
        os.environ.clear()
        os.environ.update(old_env)


if __name__ == "__main__":
    raise SystemExit(main())
