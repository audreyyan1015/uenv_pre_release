#!/usr/bin/env python3
"""OpenAI-compatible, per-Episode real-trace replay with target-model delays."""

from __future__ import annotations

import argparse
import asyncio
import csv
import hashlib
import json
import time
from pathlib import Path
from typing import Any
from urllib.parse import parse_qs, urlsplit

from stability_test_common import deterministic_lognormal_base_seconds


class ReplayService:
    def __init__(self, config: dict[str, Any], *, run_seed: int, log_path: Path) -> None:
        self.run_seed = run_seed
        self.profiles = config["latency_profiles"]
        self.task_config = config["tasks"]
        self.traces: dict[str, list[dict[str, Any]]] = {}
        self.trace_by_id: dict[str, dict[str, dict[str, Any]]] = {}
        for task, task_config in self.task_config.items():
            path = Path(task_config["trace_file"])
            self.traces[task] = [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]
            if not self.traces[task]:
                raise ValueError(f"trace file is empty: {path}")
            self.trace_by_id[task] = {str(item["trace_id"]): item for item in self.traces[task]}
            if len(self.trace_by_id[task]) != len(self.traces[task]):
                raise ValueError(f"trace file contains duplicate trace_id values: {path}")
        self.state: dict[str, tuple[str, int, float]] = {}
        self.lock = asyncio.Lock()
        self.log_path = log_path
        self.log_path.parent.mkdir(parents=True, exist_ok=True)
        with self.log_path.open("w", encoding="utf-8", newline="") as target:
            csv.writer(target).writerow([
                "timestamp", "episode_id", "task", "trace_id", "turn_index",
                "target_qwen3_tokens", "request_bytes", "response_bytes", "wait_seconds",
            ])

    def choose_trace(self, task: str, episode_id: str) -> dict[str, Any]:
        digest = hashlib.sha256(f"{self.run_seed}:{task}:{episode_id}".encode()).digest()
        return self.traces[task][int.from_bytes(digest[:8], "big") % len(self.traces[task])]

    async def complete(
        self,
        headers: dict[str, str],
        body: bytes,
        query: dict[str, list[str]] | None = None,
    ) -> tuple[int, dict[str, Any]]:
        query = query or {}
        episode_id = (
            headers.get("x-uenv-episode-id", "").strip()
            or str((query.get("uenv_episode_id") or [""])[0]).strip()
        )
        if not episode_id:
            return 400, {"error": "X-UEnv-Episode-Id or uenv_episode_id query parameter is required"}
        task = (
            headers.get("x-uenv-dataset", "").strip()
            or str((query.get("uenv_dataset") or [""])[0]).strip()
        )
        document = json.loads(body)
        if not task:
            task = str(document.get("model", "")).removeprefix("uenv-trace-").replace("-", "_")
        if task not in self.traces:
            return 400, {"error": f"unknown replay task {task!r}"}
        async with self.lock:
            trace = self.choose_trace(task, episode_id)
            profile = self.profiles[self.task_config[task]["latency_profile"]]
            single_turn = int(self.task_config[task].get("max_steps", 1)) == 1
            if single_turn:
                trace_id, turn_index = str(trace["trace_id"]), 0
                base = deterministic_lognormal_base_seconds(
                    profile, run_seed=self.run_seed, episode_id=episode_id
                )
            else:
                if episode_id not in self.state:
                    base = deterministic_lognormal_base_seconds(
                        profile, run_seed=self.run_seed, episode_id=episode_id
                    )
                    self.state[episode_id] = (str(trace["trace_id"]), 0, base)
                trace_id, turn_index, base = self.state[episode_id]
                trace = self.trace_by_id[task][trace_id]
            turns = trace["turns"]
            if turn_index >= len(turns):
                self.state.pop(episode_id, None)
                return 409, {"error": f"trace exhausted for episode {episode_id}"}
            turn = turns[turn_index]
            if not single_turn:
                if turn_index + 1 >= len(turns):
                    self.state.pop(episode_id, None)
                else:
                    self.state[episode_id] = (trace_id, turn_index + 1, base)
        profile = self.profiles[self.task_config[task]["latency_profile"]]
        token_count = int(turn["target_qwen3_tokens"])
        wait_seconds = base * token_count / float(profile["base_tokens"])
        await asyncio.sleep(wait_seconds)
        output = str(turn["assistant_output"])
        response = {
            "id": f"uenv-{episode_id}-{turn_index}", "object": "chat.completion",
            "created": int(time.time()), "model": document.get("model", f"uenv-trace-{task}"),
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": output},
                "finish_reason": "stop",
                "logprobs": {
                    "content": [
                        {"token": character, "logprob": 0.0, "top_logprobs": [{"token": character, "logprob": 0.0}]}
                        for character in output
                    ]
                },
            }],
            "usage": {"prompt_tokens": 0, "completion_tokens": token_count, "total_tokens": token_count},
        }
        response_bytes = len(json.dumps(response, ensure_ascii=False).encode())
        with self.log_path.open("a", encoding="utf-8", newline="") as target:
            csv.writer(target).writerow([
                time.time(), episode_id, task, trace_id, turn_index, token_count,
                len(body), response_bytes, f"{wait_seconds:.9f}",
            ])
        return 200, response

    async def health(self) -> tuple[int, dict[str, Any]]:
        return 200, {"ok": True, "datasets": sorted(self.traces)}

    async def tokenize(self, body: bytes) -> tuple[int, dict[str, Any]]:
        document = json.loads(body)
        texts = document.get("text")
        if isinstance(texts, str):
            texts = [texts]
        if not isinstance(texts, list):
            return 400, {"error": "text must be a string or list of strings"}
        return 200, {
            "data": [
                {"index": index, "token_ids": [ord(character) for character in str(value)]}
                for index, value in enumerate(texts)
            ]
        }


async def read_request(reader: asyncio.StreamReader) -> tuple[str, str, dict[str, str], bytes]:
    header_block = await reader.readuntil(b"\r\n\r\n")
    lines = header_block.decode("iso-8859-1").split("\r\n")
    method, path, _version = lines[0].split(" ", 2)
    headers: dict[str, str] = {}
    for line in lines[1:]:
        if ":" in line:
            key, value = line.split(":", 1)
            headers[key.strip().lower()] = value.strip()
    content_length = int(headers.get("content-length", "0"))
    if content_length > 64 * 1024 * 1024:
        raise ValueError("request body exceeds 64 MiB")
    body = await reader.readexactly(content_length) if content_length else b""
    return method, path, headers, body


async def handle_client(service: ReplayService, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
    status = 500
    document: dict[str, Any] = {"error": "internal server error"}
    try:
        method, raw_path, headers, body = await asyncio.wait_for(read_request(reader), timeout=30)
        parsed = urlsplit(raw_path)
        query = parse_qs(parsed.query, keep_blank_values=True)
        if method == "GET" and parsed.path == "/health":
            status, document = await service.health()
        elif method == "POST" and parsed.path == "/v1/chat/completions":
            status, document = await service.complete(headers, body, query)
        elif method == "POST" and parsed.path == "/v1/tokenization":
            status, document = await service.tokenize(body)
        else:
            status, document = 404, {"error": "not found"}
    except (asyncio.IncompleteReadError, asyncio.LimitOverrunError, json.JSONDecodeError, ValueError) as exc:
        status, document = 400, {"error": f"{type(exc).__name__}: {exc}"}
    except Exception as exc:
        status, document = 500, {"error": f"{type(exc).__name__}: {exc}"}
    payload = json.dumps(document, ensure_ascii=False, separators=(",", ":")).encode()
    reason = {200: "OK", 400: "Bad Request", 404: "Not Found", 409: "Conflict", 500: "Internal Server Error"}.get(status, "Error")
    writer.write(
        f"HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {len(payload)}\r\nConnection: close\r\n\r\n".encode()
        + payload
    )
    await writer.drain()
    writer.close()
    await writer.wait_closed()


async def serve(service: ReplayService, host: str, port: int) -> None:
    server = await asyncio.start_server(lambda reader, writer: handle_client(service, reader, writer), host, port)
    addresses = ", ".join(str(sock.getsockname()) for sock in server.sockets or [])
    print(f"trace replay listening on {addresses}", flush=True)
    async with server:
        await server.serve_forever()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=0)
    parser.add_argument("--run-seed", type=int, default=20260722)
    parser.add_argument("--round-log", type=Path, required=True)
    args = parser.parse_args()
    config = json.loads(args.config.read_text(encoding="utf-8"))
    service = ReplayService(config, run_seed=args.run_seed, log_path=args.round_log)
    asyncio.run(serve(service, args.host, args.port))


if __name__ == "__main__":
    main()
