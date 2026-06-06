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


def parse_ids(value: str) -> list[int]:
    raw = value.strip()
    if raw.startswith("[") and raw.endswith("]"):
        raw = raw[1:-1]
    return [int(item.strip()) for item in raw.split(",") if item.strip()]


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Verify pre-rollout AgentLoop -> Rust adapter core local gRPC loop",
    )
    parser.add_argument("--endpoint", default=None)
    parser.add_argument("--reward", type=float, default=0.73)
    parser.add_argument("--response-ids", default="201,202,203")
    parser.add_argument("--response-text", default="static external rollout")
    parser.add_argument("--startup-timeout", type=float, default=60.0)
    parser.add_argument("--skip-build", action="store_true")
    args = parser.parse_args()

    endpoint = args.endpoint or f"127.0.0.1:{free_port()}"
    response_ids = parse_ids(args.response_ids)

    subprocess.run([str(ROOT / "scripts" / "generate_adapter_core_proto.sh")], check=True)
    if not args.skip_build:
        subprocess.run(["cargo", "build"], cwd=ROOT / "core", check=True)

    env = os.environ.copy()
    env["UENV_ADAPTER_CORE_BACKEND"] = "static_rollout"
    env["UENV_ADAPTER_CORE_STATIC_REWARD"] = str(args.reward)
    env["UENV_ADAPTER_CORE_STATIC_RESPONSE_IDS"] = json.dumps(response_ids)
    env["UENV_ADAPTER_CORE_STATIC_RESPONSE_TEXT"] = args.response_text

    binary = os.getenv("UENV_ADAPTER_CORE_BINARY") or str(ROOT / "core" / "target" / "debug" / "uenv-adapter-core")
    old_env = os.environ.copy()
    os.environ.update(env)
    client = None
    try:
        client = RustCoreEpisodeClient(
            RustCoreClientConfig(
                endpoint=endpoint,
                auto_start=True,
                binary=binary,
                startup_timeout_seconds=args.startup_timeout,
            )
        )
        loop = UEnvAgentLoop(
            tokenizer=FakeTokenizer(),
            client=client,
            default_model_endpoint="http://policy.example/v1",
        )
        output = asyncio.run(
            loop.run(
                {"temperature": 0.0, "top_p": 1.0},
                raw_prompt=[{"role": "user", "content": "What is 2 + 2?"}],
                data_source="openai/gsm8k",
                reward_model={"ground_truth": "4"},
                extra_info={
                    "batch_id": "layer3-pre-rollout",
                    "sample_index": 0,
                    "question": "What is 2 + 2?",
                },
            )
        )
        if output.response_ids != response_ids:
            raise RuntimeError(f"unexpected response_ids: {output.response_ids}")
        if output.response_mask != [1] * len(response_ids):
            raise RuntimeError(f"unexpected response_mask: {output.response_mask}")
        if float(output.reward_score) != args.reward:
            raise RuntimeError(f"unexpected reward_score: {output.reward_score}")

        print(
            json.dumps(
                {
                    "endpoint": endpoint,
                    "prompt_ids": output.prompt_ids,
                    "response_ids": output.response_ids,
                    "response_mask": output.response_mask,
                    "reward_score": output.reward_score,
                    "uenv_status": output.extra_fields.get("uenv_status"),
                    "uenv_termination_reason": output.extra_fields.get("uenv_termination_reason"),
                },
                ensure_ascii=False,
            )
        )
        return 0
    finally:
        if client is not None:
            client.close()
        os.environ.clear()
        os.environ.update(old_env)


if __name__ == "__main__":
    raise SystemExit(main())
