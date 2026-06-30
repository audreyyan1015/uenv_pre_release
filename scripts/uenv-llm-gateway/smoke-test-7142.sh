#!/usr/bin/env bash
# 7142 LLM Gateway + vLLM 冒烟测试（在 7142 本机执行）
set -euo pipefail

GATEWAY_ENV="${UENV_GATEWAY_ENV_FILE:-/root/.uenv-llm-gateway.env}"
GATEWAY_URL="${UENV_GATEWAY_URL:-http://127.0.0.1:18888}"
HEALTH_URL="${UENV_HEALTH_URL:-http://127.0.0.1:18777/health}"
MODEL="${UENV_LLM_MODEL:-deepseek-v3-0324-awq}"

if [[ -f "$GATEWAY_ENV" ]]; then
  # shellcheck disable=SC1090
  source "$GATEWAY_ENV"
fi

if [[ -z "${UENV_LLM_GATEWAY_API_KEY:-}" ]]; then
  echo "ERROR: UENV_LLM_GATEWAY_API_KEY not set" >&2
  exit 1
fi

AUTH=(-H "Authorization: Bearer $UENV_LLM_GATEWAY_API_KEY")

echo "== health =="
curl -sS "$HEALTH_URL"
echo

echo "== auth check (no key -> 401) =="
CODE=$(curl -sS -o /dev/null -w '%{http_code}' "$GATEWAY_URL/v1/models")
[[ "$CODE" == "401" ]] && echo "PASS: 401 without key" || echo "WARN: expected 401 got $CODE"

echo "== gateway /v1/models =="
MODELS=$(curl -sS "${AUTH[@]}" "$GATEWAY_URL/v1/models")
echo "$MODELS" | head -c 500
echo
if echo "$MODELS" | grep -q '"error":"backend_starting"'; then
  echo "INFO: backend still starting (vLLM loading or model downloading)"
  echo "== partial smoke OK (gateway up, backend pending) =="
  exit 0
fi
echo "$MODELS" | grep -q '"data"' || { echo "FAIL: unexpected /v1/models response"; exit 1; }

echo "== gateway chat =="
CHAT=$(curl -sS "${AUTH[@]}" -H "Content-Type: application/json" \
  "$GATEWAY_URL/v1/chat/completions" \
  -d "{\"model\":\"$MODEL\",\"messages\":[{\"role\":\"user\",\"content\":\"Say OK in one word\"}],\"max_tokens\":16}")
echo "$CHAT" | head -c 600
echo
echo "$CHAT" | grep -q '"content"' || { echo "FAIL: no chat content"; exit 1; }

echo "== gateway tool call =="
TOOL=$(curl -sS "${AUTH[@]}" -H "Content-Type: application/json" \
  "$GATEWAY_URL/v1/chat/completions" \
  -d "{
    \"model\": \"$MODEL\",
    \"messages\": [{\"role\": \"user\", \"content\": \"What is 2+2? Use the bash tool.\"}],
    \"tools\": [{
      \"type\": \"function\",
      \"function\": {
        \"name\": \"bash\",
        \"description\": \"Run a command\",
        \"parameters\": {\"type\": \"object\", \"properties\": {\"command\": {\"type\": \"string\"}}, \"required\": [\"command\"]}
      }
    }],
    \"tool_choice\": \"auto\",
    \"max_tokens\": 256
  }")
echo "$TOOL" | head -c 800
echo
if echo "$TOOL" | grep -q 'tool_calls'; then
  echo "PASS: tool_calls present"
else
  echo "WARN: tool_calls not found (may need longer warmup)"
fi

echo "== all smoke checks completed =="
