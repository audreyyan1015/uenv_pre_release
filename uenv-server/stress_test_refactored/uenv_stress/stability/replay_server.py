#!/usr/bin/env python3
"""OpenAI-compatible, per-Episode real-trace replay with target-model delays."""

from __future__ import annotations

import argparse
import asyncio
import csv
import json
import re
import time
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any
from urllib.parse import parse_qs, urlsplit

from uenv_stress.core.stability_test_common import (
    LATENCY_SOURCE_MEDIAN,
    latency_imputation_medians,
    percentile,
    select_trace_for_sequence,
    source_model_family,
    trace_pair_id,
    trace_turn_waits,
    validate_paired_trace_order,
)


class ReplayService:
    def __init__(self, config: dict[str, Any], *, run_seed: int, log_path: Path) -> None:
        self.run_seed = run_seed
        self.task_config = config["tasks"]
        self.max_missing_ratio = float(config["latency_replay"]["max_missing_ratio"])
        self.traces: dict[str, list[dict[str, Any]]] = {}
        self.trace_by_id: dict[str, dict[str, dict[str, Any]]] = {}
        self.waits_by_id: dict[str, dict[str, dict[str, Any]]] = {}
        for task, task_config in self.task_config.items():
            path = Path(task_config["trace_file"])
            self.traces[task] = [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]
            if not self.traces[task]:
                raise ValueError(f"trace file is empty: {path}")
            self.trace_by_id[task] = {str(item["trace_id"]): item for item in self.traces[task]}
            if len(self.trace_by_id[task]) != len(self.traces[task]):
                raise ValueError(f"trace file contains duplicate trace_id values: {path}")
            sampling_policy = str(task_config["sampling_policy"])
            if sampling_policy == "paired_doubao_qwen_alternating":
                validate_paired_trace_order(
                    self.traces[task],
                    expected_pairs=int(task_config["expected_pairs"]),
                )
            elif sampling_policy == "doubao_only_round_robin":
                if any(
                    source_model_family(trace.get("source_model")) != "doubao"
                    for trace in self.traces[task]
                ):
                    raise ValueError(f"{task} must contain Doubao traces only")
            medians = latency_imputation_medians(
                self.traces[task],
                max_missing_ratio=self.max_missing_ratio,
            )
            self.waits_by_id[task] = {
                str(trace["trace_id"]): trace_turn_waits(
                    trace, imputation_medians=medians
                )
                for trace in self.traces[task]
            }
        self.trace_cursors = {task: 0 for task in self.traces}
        self.replay_calls = {task: 0 for task in self.traces}
        self.replay_hits = {task: 0 for task in self.traces}
        self.replay_misses = {task: 0 for task in self.traces}
        self.unknown_task_calls = 0
        self.unknown_task_misses = 0
        self.trace_usage = {
            task: {str(trace["trace_id"]): 0 for trace in traces}
            for task, traces in self.traces.items()
        }
        self.source_model_usage = {task: Counter() for task in self.traces}
        self.source_family_usage = {task: Counter() for task in self.traces}
        self.pair_usage = {task: Counter() for task in self.traces}
        self.latency_source_usage = {task: Counter() for task in self.traces}
        self.wait_samples = {task: [] for task in self.traces}
        self.token_samples = {task: [] for task in self.traces}
        self.model_wait_samples = {
            task: defaultdict(list) for task in self.traces
        }
        self.model_token_samples = {
            task: defaultdict(list) for task in self.traces
        }
        self.state: dict[tuple[str, str], tuple[str, int, int, int]] = {}
        self.lock = asyncio.Lock()
        self.log_path = log_path
        self.log_path.parent.mkdir(parents=True, exist_ok=True)
        with self.log_path.open("w", encoding="utf-8", newline="") as target:
            csv.writer(target).writerow([
                "timestamp", "episode_id", "task", "trace_id", "turn_index",
                "trace_slot", "trace_corpus_size", "selection_strategy",
                "planned_sequence", "source_model", "pair_id", "latency_source",
                "episode_elapsed_proxy_ms", "turn_proxy_wait_seconds",
                "target_qwen3_tokens", "request_bytes", "response_bytes", "wait_seconds",
            ])

    @staticmethod
    def planned_sequence(
        query: dict[str, list[str]], episode_id: str
    ) -> int:
        raw = str((query.get("uenv_sequence") or [""])[0]).strip()
        if raw:
            sequence = int(raw)
        else:
            match = re.search(r"-(\d+)$", episode_id)
            if match is None:
                raise ValueError(
                    "uenv_sequence is required when episode_id has no numeric suffix"
                )
            sequence = int(match.group(1))
        if sequence < 0:
            raise ValueError("uenv_sequence must be non-negative")
        return sequence

    def assign_trace(
        self, task: str, sequence: int
    ) -> tuple[dict[str, Any], int]:
        trace, slot = select_trace_for_sequence(
            self.traces[task],
            sequence=sequence,
            sampling_policy=str(self.task_config[task]["sampling_policy"]),
        )
        self.trace_cursors[task] += 1
        self.trace_usage[task][str(trace["trace_id"])] += 1
        source_model = str(trace["source_model"])
        self.source_model_usage[task][source_model] += 1
        self.source_family_usage[task][source_model_family(source_model)] += 1
        self.pair_usage[task][trace_pair_id(trace)] += 1
        return trace, slot

    async def complete(
        self,
        headers: dict[str, str],
        body: bytes,
        query: dict[str, list[str]] | None = None,
    ) -> tuple[int, dict[str, Any]]:
        query = query or {}
        document = json.loads(body)
        task = (
            headers.get("x-uenv-dataset", "").strip()
            or str((query.get("uenv_dataset") or [""])[0]).strip()
            or str(document.get("model", "")).removeprefix("uenv-trace-").replace("-", "_")
        )
        episode_id = (
            headers.get("x-uenv-episode-id", "").strip()
            or str((query.get("uenv_episode_id") or [""])[0]).strip()
        )
        if not episode_id:
            if task in self.replay_calls:
                self.replay_calls[task] += 1
                self.replay_misses[task] += 1
            else:
                self.unknown_task_calls += 1
                self.unknown_task_misses += 1
            return 400, {"error": "X-UEnv-Episode-Id or uenv_episode_id query parameter is required"}
        if task not in self.traces:
            self.unknown_task_calls += 1
            self.unknown_task_misses += 1
            return 400, {"error": f"unknown replay task {task!r}"}
        sequence = self.planned_sequence(query, episode_id)
        async with self.lock:
            self.replay_calls[task] += 1
            single_turn = int(self.task_config[task].get("max_steps", 1)) == 1
            state_key = (task, episode_id)
            if single_turn:
                trace, trace_slot = self.assign_trace(task, sequence)
                trace_id, turn_index = str(trace["trace_id"]), 0
            else:
                if state_key not in self.state:
                    trace, trace_slot = self.assign_trace(task, sequence)
                    self.state[state_key] = (
                        str(trace["trace_id"]),
                        trace_slot,
                        0,
                        sequence,
                    )
                trace_id, trace_slot, turn_index, sequence = self.state[state_key]
                trace = self.trace_by_id[task][trace_id]
            expected = {
                "uenv_trace_id": str(trace["trace_id"]),
                "uenv_source_model": str(trace["source_model"]),
                "uenv_pair_id": trace_pair_id(trace),
            }
            for field, value in expected.items():
                requested = str((query.get(field) or [""])[0]).strip()
                if requested and requested != value:
                    raise ValueError(
                        f"{field}={requested!r} does not match selected {value!r}"
                    )
            turns = trace["turns"]
            if turn_index >= len(turns):
                self.state.pop(state_key, None)
                self.replay_misses[task] += 1
                return 409, {"error": f"trace exhausted for episode {episode_id}"}
            turn = turns[turn_index]
            self.replay_hits[task] += 1
            if not single_turn:
                if turn_index + 1 >= len(turns):
                    self.state.pop(state_key, None)
                else:
                    self.state[state_key] = (
                        trace_id,
                        trace_slot,
                        turn_index + 1,
                        sequence,
                    )
        wait_profile = self.waits_by_id[task][trace_id]
        token_count = int(turn["target_qwen3_tokens"])
        wait_seconds = float(
            wait_profile["turn_proxy_wait_seconds"][turn_index]
        )
        self.latency_source_usage[task][str(wait_profile["latency_source"])] += 1
        self.wait_samples[task].append(wait_seconds)
        self.token_samples[task].append(token_count)
        source_model = str(trace["source_model"])
        self.model_wait_samples[task][source_model].append(wait_seconds)
        self.model_token_samples[task][source_model].append(token_count)
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
                time.time(), episode_id, task, trace_id, turn_index,
                trace_slot, len(self.traces[task]),
                self.task_config[task]["sampling_policy"],
                sequence, trace["source_model"], trace_pair_id(trace),
                wait_profile["latency_source"],
                f"{float(wait_profile['episode_elapsed_proxy_ms']):.6f}",
                f"{wait_seconds:.9f}",
                token_count,
                len(body), response_bytes, f"{wait_seconds:.9f}",
            ])
        return 200, response

    async def health(self) -> tuple[int, dict[str, Any]]:
        async with self.lock:
            replay = {
                task: {
                    "corpus_size": len(self.traces[task]),
                    "calls": self.replay_calls[task],
                    "hits": self.replay_hits[task],
                    "misses": self.replay_misses[task],
                    "hit_rate": (
                        self.replay_hits[task]
                        / (self.replay_hits[task] + self.replay_misses[task])
                        if self.replay_hits[task] + self.replay_misses[task]
                        else 0.0
                    ),
                    "assigned_episodes": self.trace_cursors[task],
                    "completed_cycles": self.trace_cursors[task] // len(self.traces[task]),
                    "sampling_policy": self.task_config[task]["sampling_policy"],
                    "trace_usage": dict(self.trace_usage[task]),
                    "source_model_usage": dict(self.source_model_usage[task]),
                    "source_family_usage": dict(self.source_family_usage[task]),
                    "pair_usage": dict(self.pair_usage[task]),
                    "latency_source_usage": dict(self.latency_source_usage[task]),
                    "wait_seconds_p50": percentile(
                        self.wait_samples[task], 0.50
                    ),
                    "wait_seconds_p95": percentile(
                        self.wait_samples[task], 0.95
                    ),
                    "completion_tokens_p50": percentile(
                        self.token_samples[task], 0.50
                    ),
                    "completion_tokens_p95": percentile(
                        self.token_samples[task], 0.95
                    ),
                    "source_model_replay_stats": {
                        model: {
                            "turns": len(self.model_token_samples[task][model]),
                            "completion_tokens_p50": percentile(
                                self.model_token_samples[task][model], 0.50
                            ),
                            "completion_tokens_p95": percentile(
                                self.model_token_samples[task][model], 0.95
                            ),
                            "wait_seconds_p50": percentile(
                                self.model_wait_samples[task][model], 0.50
                            ),
                            "wait_seconds_p95": percentile(
                                self.model_wait_samples[task][model], 0.95
                            ),
                        }
                        for model in sorted(self.model_token_samples[task])
                    },
                    "imputed_turns": int(
                        self.latency_source_usage[task].get(
                            LATENCY_SOURCE_MEDIAN, 0
                        )
                    ),
                }
                for task in sorted(self.traces)
            }
        return 200, {
            "ok": True,
            "datasets": sorted(self.traces),
            "selection_strategy": "paired_alternating_episode",
            "latency_basis": "observed_episode_elapsed_proxy",
            "calls": sum(self.replay_calls.values()) + self.unknown_task_calls,
            "hits": sum(self.replay_hits.values()),
            "misses": sum(self.replay_misses.values()) + self.unknown_task_misses,
            "unknown_task_calls": self.unknown_task_calls,
            "replay": replay,
        }

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
