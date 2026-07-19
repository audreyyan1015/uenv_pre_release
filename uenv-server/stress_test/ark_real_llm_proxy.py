#!/usr/bin/env python3
"""Loopback-only Ark proxy for real-LLM Gate3 runs.

The proxy reads the API key from a mode-0600 OpenHands LLM JSON file, forwards
OpenAI-compatible chat requests to Ark, and enriches the real provider response
with token IDs from Ark's tokenization API. It never logs prompts, responses, or
credentials.
"""

from __future__ import annotations

import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import re
import threading
import urllib.error
import urllib.request
from urllib.parse import parse_qs, urlparse


parser = argparse.ArgumentParser()
parser.add_argument("--port", type=int, required=True)
parser.add_argument("--config", required=True)
args = parser.parse_args()

raw_config = json.loads(Path(args.config).read_text(encoding="utf-8"))
api_key = str(raw_config["api_key"])
base_url = str(raw_config["base_url"]).rstrip("/")
model = str(raw_config["model"]).removeprefix("volcengine/")
request_timeout = int(raw_config.get("timeout", 1200))

stats_lock = threading.Lock()
calls_by_task: dict[str, int] = {}
tokens_by_task: dict[str, int] = {}


def request_json(url: str, payload: dict) -> dict:
    request = urllib.request.Request(
        url,
        data=json.dumps(payload, separators=(",", ":")).encode(),
        method="POST",
    )
    request.add_header("Content-Type", "application/json")
    request.add_header("Authorization", f"Bearer {api_key}")
    with urllib.request.urlopen(request, timeout=request_timeout) as response:
        return json.loads(response.read().decode())


def task_id_from_request(document: dict) -> str:
    for message in document.get("messages") or []:
        content = message.get("content", "") if isinstance(message, dict) else ""
        if not isinstance(content, str):
            continue
        found = re.search(r"Task ID: ([A-Za-z0-9_.:-]+)", content)
        if found:
            return found.group(1)
    return "unknown"


def add_real_token_metadata(document: dict) -> int:
    records = (
        (((document.get("choices") or [{}])[0].get("logprobs") or {}).get("content"))
        or []
    )
    if not records:
        raise RuntimeError("Ark response did not contain content logprobs")
    token_text = "".join(str(record.get("token", "")) for record in records)
    tokenized = request_json(
        base_url + "/tokenization",
        {"model": model, "text": [token_text]},
    )
    data = tokenized.get("data") or []
    token_ids = data[0].get("token_ids", []) if len(data) == 1 else []
    if len(token_ids) != len(records):
        raise RuntimeError(
            f"Ark token/logprob alignment mismatch ids={len(token_ids)} "
            f"logprobs={len(records)}"
        )
    normalized_ids = [int(value) for value in token_ids]
    for record, token_id in zip(records, normalized_ids, strict=True):
        record["token_id"] = token_id
    document["uenv_response_ids"] = normalized_ids
    document["uenv_model_version"] = {
        "rollout_param_version": 0,
        "rollout_policy_version": model,
    }
    return len(normalized_ids)


class Handler(BaseHTTPRequestHandler):
    def send_json(self, status: int, document: dict) -> None:
        body = json.dumps(document, separators=(",", ":")).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:
        parsed = urlparse(self.path)
        if parsed.path == "/health":
            self.send_json(200, {"ok": True, "model": model})
            return
        if parsed.path != "/stats":
            self.send_error(404)
            return
        prefix = parse_qs(parsed.query).get("prefix", [""])[0]
        with stats_lock:
            counts = {
                task: count
                for task, count in calls_by_task.items()
                if not prefix or task.startswith(prefix)
            }
            token_counts = {
                task: count
                for task, count in tokens_by_task.items()
                if not prefix or task.startswith(prefix)
            }
        ordered = sorted(counts.values())
        histogram: dict[str, int] = {}
        for count in ordered:
            histogram[str(count)] = histogram.get(str(count), 0) + 1
        self.send_json(
            200,
            {
                "prefix": prefix,
                "task_count": len(ordered),
                "total_model_calls": sum(ordered),
                "total_response_tokens": sum(token_counts.values()),
                "min_steps": min(ordered) if ordered else 0,
                "max_steps": max(ordered) if ordered else 0,
                "step_histogram": histogram,
                "examples": dict(list(sorted(counts.items()))[:20]),
                "provider": "ark_chat_completions+ark_tokenization",
                "model": model,
            },
        )

    def do_POST(self) -> None:
        if not urlparse(self.path).path.endswith("/chat/completions"):
            self.send_error(404)
            return
        size = int(self.headers.get("content-length", "0"))
        try:
            incoming = json.loads(self.rfile.read(size).decode() if size else "{}")
            task_id = task_id_from_request(incoming)
            outgoing = dict(incoming)
            outgoing["model"] = model
            outgoing["stream"] = False
            outgoing["thinking"] = {"type": "disabled"}
            outgoing["logprobs"] = True
            outgoing["top_logprobs"] = max(1, int(outgoing.get("top_logprobs", 0)))
            document = request_json(base_url + "/chat/completions", outgoing)
            token_count = add_real_token_metadata(document)
            with stats_lock:
                calls_by_task[task_id] = calls_by_task.get(task_id, 0) + 1
                tokens_by_task[task_id] = tokens_by_task.get(task_id, 0) + token_count
            self.send_json(200, document)
        except urllib.error.HTTPError as exc:
            self.send_json(
                exc.code,
                {"error": {"message": f"Ark request failed with HTTP {exc.code}"}},
            )
        except Exception as exc:
            self.send_json(502, {"error": {"message": str(exc)}})

    def log_message(self, *_args: object) -> None:
        pass


ThreadingHTTPServer(("127.0.0.1", args.port), Handler).serve_forever()
