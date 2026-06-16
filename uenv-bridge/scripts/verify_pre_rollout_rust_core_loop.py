#!/usr/bin/env python3
from __future__ import annotations

import argparse
import asyncio
import json
import os
import socket
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from uenv.bridge.clients import RustCoreClientConfig, RustCoreEpisodeClient
from uenv.bridge.verl_agent_loop import UEnvAgentLoop


class FakeTokenizer:
    pad_token_id = 0

    def apply_chat_template(self, messages, tokenize=True, add_generation_prompt=True):
        return [10, 11, 12]

    def encode(self, text, add_special_tokens=False):
        return [ord(char) for char in text]


def free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Verify pre-rollout AgentLoop -> adapter-core(server) -> Worker real chain",
    )
    parser.add_argument(
        "--endpoint",
        default=os.getenv("UENV_ADAPTER_CORE_ENDPOINT", "8.130.86.71:8088"),
        help="adapter-core endpoint (must have a registered Worker for env_type=math)",
    )
    parser.add_argument("--startup-timeout", type=float, default=60.0)
    parser.add_argument("--skip-build", action="store_true")
    parser.add_argument(
        "--auto-start",
        action="store_true",
        help="Start local adapter-core; requires a Worker registered to that endpoint",
    )
    args = parser.parse_args()

    endpoint = args.endpoint
    if args.auto_start and args.endpoint == os.getenv("UENV_ADAPTER_CORE_ENDPOINT", "8.130.86.71:8088"):
        endpoint = f"127.0.0.1:{free_port()}"

    generated_stub = SRC / "uenv" / "bridge" / "gen" / "adapter_core_pb2_grpc.py"
    if not args.skip_build or not generated_stub.exists():
        subprocess.run([str(ROOT / "scripts" / "generate_adapter_core_proto.sh")], check=True)
    if not args.skip_build:
        subprocess.run(["cargo", "build"], cwd=ROOT / "core", check=True)

    env = os.environ.copy()
    env["UENV_ADAPTER_CORE_BACKEND"] = "server"

    binary = os.getenv("UENV_ADAPTER_CORE_BINARY") or str(ROOT / "core" / "target" / "debug" / "uenv-adapter-core")
    old_env = os.environ.copy()
    os.environ.update(env)
    client = None
    output = None
    try:
        client = RustCoreEpisodeClient(
            RustCoreClientConfig(
                endpoint=endpoint,
                auto_start=args.auto_start,
                binary=binary,
                startup_timeout_seconds=args.startup_timeout,
            )
        )
        loop = UEnvAgentLoop(
            tokenizer=FakeTokenizer(),
            client=client,
            client_mode="rust_core",
            default_model_endpoint=os.getenv(
                "UENV_ROLLOUT_MODEL_ENDPOINT", "https://openrouter.ai/api/v1"
            ),
        )
        output = asyncio.run(
            loop.run(
                {"temperature": 0.0, "max_new_tokens": 32},
                raw_prompt=[{"role": "user", "content": "Natalia sold clips to 48 friends. How many clips did she sell?"}],
                data_source="openai/gsm8k",
                reward_model={"ground_truth": "72", "style": "rule"},
                extra_info={
                    "batch_id": "verify-batch",
                    "question": "Natalia sold clips to 48 friends. How many clips did she sell?",
                },
            )
        )
    finally:
        os.environ.clear()
        os.environ.update(old_env)
        if client is not None:
            client.close()

    if output is None:
        raise RuntimeError("pre-rollout verification failed before AgentLoop returned output")

    print(json.dumps({
        "endpoint": endpoint,
        "backend": "server",
        "reward_score": output.reward_score,
        "response_ids": output.response_ids,
        "response_mask": output.response_mask,
        "uenv_request_id": output.extra_fields.get("uenv_request_id"),
        "uenv_status": output.extra_fields.get("uenv_status"),
        "uenv_termination_reason": output.extra_fields.get("uenv_termination_reason"),
    }, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
