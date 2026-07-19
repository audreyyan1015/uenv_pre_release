#!/usr/bin/env python3
"""Safe live preflight for the loopback Gate3 Ark proxy."""

from __future__ import annotations

import argparse
import json
import time
import urllib.error
import urllib.request


parser = argparse.ArgumentParser()
parser.add_argument("--url", required=True)
args = parser.parse_args()

payload = {
    "model": "proxy-selects-versioned-model",
    "messages": [
        {
            "role": "user",
            "content": (
                "Return only this Python code: def add(a, b): return a + b\n"
                "Task ID: gate3-real-llm-preflight"
            ),
        }
    ],
    "max_tokens": 64,
    "temperature": 0,
    "logprobs": True,
    "top_logprobs": 1,
}
request = urllib.request.Request(
    args.url.rstrip("/") + "/chat/completions",
    data=json.dumps(payload).encode(),
    method="POST",
    headers={"Content-Type": "application/json"},
)
deadline = time.monotonic() + 30
while True:
    try:
        with urllib.request.urlopen(request, timeout=180) as response:
            document = json.loads(response.read().decode())
        break
    except urllib.error.URLError:
        if time.monotonic() >= deadline:
            raise
        time.sleep(0.5)

content = document["choices"][0]["message"]["content"]
records = document["choices"][0]["logprobs"]["content"]
response_ids = document["uenv_response_ids"]
version = document["uenv_model_version"]
if not content or not records or len(records) != len(response_ids):
    raise SystemExit("real Ark preflight returned invalid training trace")
if not version.get("rollout_policy_version"):
    raise SystemExit("real Ark preflight returned no policy version")
print(
    "real_llm_preflight=PASS "
    f"response_tokens={len(response_ids)} "
    "ids_logprobs_aligned=true provider=ark"
)
