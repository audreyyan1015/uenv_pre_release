#!/usr/bin/env python3
"""Generate openhands-llm-7142.json from uenv-worker-llm.env."""
import json
import sys
from pathlib import Path

src = Path(sys.argv[1] if len(sys.argv) > 1 else "config/uenv-worker-llm.env")
dst = Path(sys.argv[2] if len(sys.argv) > 2 else "config/openhands-llm-7142.json")
env: dict[str, str] = {}
for line in src.read_text(encoding="utf-8").splitlines():
    line = line.strip()
    if not line or line.startswith("#") or "=" not in line:
        continue
    k, v = line.split("=", 1)
    env[k.strip()] = v.strip().strip('"').strip("'")
out = {
    "model": "openai/" + env.get("UENV_LLM_MODEL_NAME", "deepseek-v4-flash"),
    "base_url": env.get("UENV_LLM_ENDPOINT"),
    "api_key": env.get("UENV_LLM_API_KEY"),
    "temperature": float(env.get("UENV_LLM_TEMPERATURE", "0.2")),
    "max_output_tokens": int(env.get("UENV_LLM_MAX_TOKENS", "4096")),
}
dst.write_text(json.dumps(out, indent=2) + "\n", encoding="utf-8")
print(dst)
