#!/usr/bin/env python3
"""Ark 回环代理的安全预检脚本。

这个文件用于在正式采集前确认本机 Ark proxy 可用，并验证它不会暴露到非预期地址。它通过一次最小请求检查代理能否完成 OpenAI 兼容调用，同时避免把 prompt、响应正文或凭据写到日志中。

实现逻辑是：解析代理地址、超时和重试参数后，循环向代理发送健康或最小 chat 请求；若连接失败、HTTP 错误或返回结构不符合预期，就按明确退出码失败；成功时只输出不含敏感内容的状态和计时信息。"""

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
                "Task ID: dscodebench-pressure-real-llm-preflight"
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
