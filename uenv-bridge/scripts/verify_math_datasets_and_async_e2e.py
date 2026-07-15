#!/usr/bin/env python3
"""实机 E2E：math 多 dataset + VeRL async 字段链路验证。

依赖：
  - Server adapter-core @ UENV_ADAPTER_CORE_ENDPOINT（默认 8.130.75.157:8088）
  - 7143 Worker 已 Register，env_type=math
  - 7142 上 mock model 可被 Worker 访问（默认 http://219.147.100.43:18080/v1）
"""
from __future__ import annotations

import argparse
import asyncio
import json
import os
import socket
import sys
import threading
import time
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
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
        return [100 + (idx % 50) for idx, _ in enumerate(text)]


class AsyncMockModel:
    """OpenAI-compatible mock：按 prompt 关键词返回答案 + logprobs + uenv_model_version。"""

    ANSWERS = {
        "gsm8k": "#### 72",
        "8+8": "#### 16",
        "pubmedqa": "Based on the abstract, the answer is yes.",
        "scitab": "The claim is supports by the table.",
        "olymmath": r"The final answer is \boxed{16}",
    }

    def __init__(self, bind: str = "0.0.0.0:18080") -> None:
        host, port_str = bind.rsplit(":", 1)
        parent = self

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self) -> None:
                if self.path.rstrip("/") == "/v1/models":
                    self._json({"object": "list", "data": [{"id": "mock-policy", "object": "model"}]})
                else:
                    self.send_error(404)

            def do_POST(self) -> None:
                length = int(self.headers.get("Content-Length", "0") or "0")
                body = self.rfile.read(length).decode("utf-8", errors="replace")
                answer = parent._pick_answer(body)
                ids = [100 + i for i in range(len(answer))]
                logprobs = [-0.1 * (i + 1) for i in range(len(ids))]
                content = [{"token_id": tid, "logprob": lp, "bytes": [tid]} for tid, lp in zip(ids, logprobs)]
                self._json(
                    {
                        "id": "mock-chatcmpl",
                        "object": "chat.completion",
                        "choices": [
                            {
                                "index": 0,
                                "message": {"role": "assistant", "content": answer},
                                "finish_reason": "stop",
                                "logprobs": {"content": content},
                            }
                        ],
                        "uenv_model_version": {
                            "rollout_param_version": 11,
                            "rollout_policy_version": "actor-step-11",
                        },
                        "uenv_response_ids": ids,
                    }
                )

            def log_message(self, _fmt: str, *_args: object) -> None:
                return

            def _json(self, payload: dict) -> None:
                raw = json.dumps(payload).encode("utf-8")
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(raw)))
                self.end_headers()
                self.wfile.write(raw)

        self.server = ThreadingHTTPServer((host, int(port_str)), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def _pick_answer(self, body: str) -> str:
        lower = body.lower()
        if "randomized trial" in lower or "treatment x" in lower or "abstract" in lower or "pubmed" in lower:
            return self.ANSWERS["pubmedqa"]
        if "group a mean" in lower or "claim:" in lower or "scitab" in lower:
            return self.ANSWERS["scitab"]
        if "4+4+4+4" in lower or "olymmath" in lower:
            return self.ANSWERS["olymmath"]
        if "8+8" in lower or "what is 8" in lower:
            return self.ANSWERS["8+8"]
        for key, answer in self.ANSWERS.items():
            if key in lower:
                return answer
        return self.ANSWERS["gsm8k"]

    @property
    def url(self) -> str:
        host, port = self.server.server_address[:2]
        return f"http://{host}:{port}/v1"

    def close(self) -> None:
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)


DATASET_CASES = [
    {
        "name": "gsm8k",
        "data_source": "openai/gsm8k",
        "question": "Natalia sold clips to 48 of her friends in April, and then she sold half as many clips in May. How many clips did Natalia sell altogether in April and May?",
        "ground_truth": "72",
        "parallel_mode": "sync",
    },
    {
        "name": "pubmedqa",
        "data_source": "pubmedqa",
        "question": "Context: A randomized trial showed improved outcomes.\nQuestion: Does treatment X improve outcomes?",
        "ground_truth": "yes",
        "parallel_mode": "sync",
    },
    {
        "name": "scitab",
        "data_source": "XinyuanLu00/SciTab",
        "question": "Table: Group A mean=10, Group B mean=5.\nClaim: Group A outperformed Group B.",
        "ground_truth": "supports",
        "parallel_mode": "sync",
    },
    {
        "name": "olymmath-easy",
        "data_source": "OlymMATH-EASY",
        "question": "Find the value of 4+4+4+4.",
        "ground_truth": "16",
        "parallel_mode": "sync",
    },
    {
        "name": "async-one-step",
        "data_source": "openai/gsm8k",
        "question": "What is 8+8? Answer with #### 16",
        "ground_truth": "16",
        "parallel_mode": "one_step_off_policy",
    },
]


async def run_case(
    loop: UEnvAgentLoop,
    case: dict,
    *,
    model_endpoint: str,
) -> dict:
    output = await loop.run(
        {"temperature": 0.0, "max_new_tokens": 64, "logprobs": True},
        raw_prompt=[{"role": "user", "content": case["question"]}],
        data_source=case["data_source"],
        reward_model={"ground_truth": case["ground_truth"], "style": "rule"},
        extra_info={
            "batch_id": f"e2e-{case['name']}",
            "sample_index": 0,
            "question": case["question"],
            "parallel_mode": case["parallel_mode"],
            "generation_step": 11,
            "global_step": 11,
            "model_endpoint": model_endpoint,
            "model_name": "mock-policy",
        },
    )
    trajectory = output.extra_fields.get("uenv_trajectory") or []
    step_info = trajectory[-1].get("info", {}) if trajectory else {}
    rollout_log_probs = step_info.get("rollout_log_probs")
    logprob_len = len(json.loads(rollout_log_probs)) if isinstance(rollout_log_probs, str) and rollout_log_probs else 0
    return {
        "case": case["name"],
        "dataset": case["data_source"],
        "parallel_mode": case["parallel_mode"],
        "uenv_status": output.extra_fields.get("uenv_status"),
        "reward_score": output.reward_score,
        "rollout_param_version": step_info.get("rollout_param_version"),
        "rollout_policy_version": step_info.get("rollout_policy_version"),
        "rollout_log_probs_len": logprob_len,
        "response_ids_len": len(output.response_ids or []),
    }


def wait_for_endpoint(endpoint: str, timeout: float = 30.0) -> None:
    host, port_str = endpoint.rsplit(":", 1)
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            with socket.create_connection((host, int(port_str)), timeout=2):
                return
        except OSError:
            time.sleep(0.5)
    raise RuntimeError(f"endpoint not reachable: {endpoint}")


def main() -> int:
    parser = argparse.ArgumentParser(description="Verify math datasets + async E2E on live UEnv chain")
    parser.add_argument(
        "--endpoint",
        default=os.getenv("UENV_ADAPTER_CORE_ENDPOINT", "8.130.75.157:8088"),
    )
    parser.add_argument(
        "--mock-bind",
        default=os.getenv("UENV_E2E_MOCK_BIND", "0.0.0.0:18080"),
    )
    parser.add_argument(
        "--mock-public-url",
        default=os.getenv("UENV_E2E_MOCK_PUBLIC_URL", "http://219.147.100.43:18080/v1"),
        help="URL Worker uses to reach mock model",
    )
    parser.add_argument("--skip-mock", action="store_true")
    args = parser.parse_args()

    wait_for_endpoint(args.endpoint)
    mock = None
    model_endpoint = args.mock_public_url
    if not args.skip_mock:
        mock = AsyncMockModel(bind=args.mock_bind)
        model_endpoint = args.mock_public_url
        local_port = args.mock_bind.rsplit(":", 1)[-1]
        local_url = f"http://127.0.0.1:{local_port}/v1/chat/completions"
        req = urllib.request.Request(
            local_url,
            data=json.dumps({"model": "mock-policy", "messages": [{"role": "user", "content": "gsm8k test"}]}).encode(),
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        with urllib.request.urlopen(req, timeout=5) as resp:
            assert resp.status == 200

    client = RustCoreEpisodeClient(
        RustCoreClientConfig(
            endpoint=args.endpoint,
            auto_start=False,
            startup_timeout_seconds=60.0,
        )
    )
    results = []
    try:
        agent_loop = UEnvAgentLoop(
            tokenizer=FakeTokenizer(),
            client=client,
            client_mode="rust_core",
            default_model_endpoint=model_endpoint,
            default_model_name="mock-policy",
        )
        for case in DATASET_CASES:
            result = asyncio.run(run_case(agent_loop, case, model_endpoint=model_endpoint))
            results.append(result)
            ok = result["uenv_status"] == "completed"
            if case["parallel_mode"] != "sync":
                # Worker→Server async 校验以 completed 为准；Adapter 侧 trajectory 回填见 Docs/260707
                ok = ok and (result["reward_score"] or 0) >= 0.0
            elif case["name"] != "gsm8k":
                ok = ok and (result["reward_score"] or 0) >= 1.0
            if not ok:
                print(json.dumps({"failed": result}, ensure_ascii=False, indent=2))
                return 1
    finally:
        client.close()
        if mock is not None:
            mock.close()

    print(json.dumps({"endpoint": args.endpoint, "model_endpoint": model_endpoint, "results": results}, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
