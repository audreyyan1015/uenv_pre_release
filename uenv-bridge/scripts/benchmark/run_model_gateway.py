#!/usr/bin/env python3
from __future__ import annotations

import argparse
import signal
import sys
import threading

from uenv.bridge.model_gateway import ModelGateway, ModelGatewayConfig


def main() -> int:
    parser = argparse.ArgumentParser(description="Run the UEnv adapter model gateway.")
    parser.add_argument("--upstream", action="append", required=True, help="OpenAI-compatible upstream /v1 endpoint.")
    parser.add_argument("--bind-host", default="0.0.0.0")
    parser.add_argument("--port", type=int, default=18088)
    parser.add_argument("--public-url", default="")
    parser.add_argument("--log-path", default="")
    parser.add_argument("--request-timeout-seconds", type=float, default=300.0)
    parser.add_argument("--disable-thinking", action="store_true")
    parser.add_argument("--enable-thinking", action="store_true")
    parser.add_argument("--preserve-thinking", action="store_true")
    parser.add_argument("--strip-reasoning", action="store_true")
    parser.add_argument("--thinking-token-budget", type=int, default=None)
    args = parser.parse_args()

    gateway = ModelGateway(
        ModelGatewayConfig(
            enabled=True,
            bind_host=args.bind_host,
            port=args.port,
            public_url=args.public_url,
            request_timeout_seconds=args.request_timeout_seconds,
            log_path=args.log_path,
            disable_thinking=args.disable_thinking,
            force_enable_thinking=args.enable_thinking,
            preserve_thinking=args.preserve_thinking,
            strip_reasoning=args.strip_reasoning,
            thinking_token_budget=args.thinking_token_budget,
        )
    )
    stopped = threading.Event()

    def stop(_signum: int, _frame: object) -> None:
        gateway.stop()
        stopped.set()

    signal.signal(signal.SIGINT, stop)
    signal.signal(signal.SIGTERM, stop)

    gateway_url = gateway.start(args.upstream)
    print(f"gateway_url={gateway_url}", flush=True)
    print(f"upstreams={gateway.upstreams}", flush=True)
    stopped.wait()
    print("gateway_stopped", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
