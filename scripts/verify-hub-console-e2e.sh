#!/usr/bin/env bash
# Hub 运维控制台 + 聚合总览端点的端到端验证。
#
# 两种用法：
#   1) 起一个临时 Hub 自验（默认）：
#        scripts/verify-hub-console-e2e.sh
#   2) 打到一台已经在跑的 Hub（真机联调）：
#        UENV_HUB_BASE_URL=http://127.0.0.1:18088 \
#        UENV_HUB_TOKEN=$(cat data/.admin_token) \
#        scripts/verify-hub-console-e2e.sh
#
# 验证内容：控制台三个静态资源可取且类型正确、根路径重定向、总览端点各区块自洽、
# 控制台引用的每一个 API 端点都真实可用（逐个实打实地请求一遍）。
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HUB_SERVER_BIN="${UENV_HUB_SERVER_BIN:-$REPO_ROOT/uenv-hub/target/debug/uenv-hub-server}"
PORT="${UENV_HUB_TEST_PORT:-18091}"
RUN_DIR="$(mktemp -d "${TMPDIR:-/tmp}/uenv-hub-console-e2e.XXXXXX")"
SERVER_PID=""
CHROME_AUDIT_PID=""
BASE_URL="${UENV_HUB_BASE_URL:-}"
TOKEN="${UENV_HUB_TOKEN:-}"

cleanup() {
  if [[ -n "$SERVER_PID" ]]; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  # 审计用的 Chrome 会一直往 user-data-dir 里写，没退干净就删目录会报
  # "Directory not empty"，所以先等它落地。
  if [[ -n "$CHROME_AUDIT_PID" ]]; then
    kill "$CHROME_AUDIT_PID" 2>/dev/null || true
    wait "$CHROME_AUDIT_PID" 2>/dev/null || true
  fi
  rm -rf "$RUN_DIR"
}
trap cleanup EXIT

# 只有在没有指定外部 Hub 时才自起一个。
if [[ -z "$BASE_URL" ]]; then
  if [[ ! -x "$HUB_SERVER_BIN" ]]; then
    echo "required binary is not executable: $HUB_SERVER_BIN" >&2
    echo "先构建：cd uenv-hub && cargo build -p uenv-hub-server" >&2
    exit 1
  fi
  mkdir -p "$RUN_DIR/artifacts"
  UENV_HUB_SERVER__HOST=127.0.0.1 \
  UENV_HUB_SERVER__PORT="$PORT" \
  UENV_HUB_DATABASE__URL="sqlite://$RUN_DIR/hub.db" \
  UENV_HUB_AUTH__REQUIRE_TOKEN=false \
  UENV_HUB_PACKAGES__ARTIFACT_DIR="$RUN_DIR/artifacts" \
  UENV_HUB_PACKAGES__CATALOG_SEED_DIR="$REPO_ROOT/config/swe" \
  UENV_HUB_PACKAGES__SEED_EXAMPLES=true \
  UENV_HUB_SWE_CATALOG_DIR="$REPO_ROOT/config/swe" \
    "$HUB_SERVER_BIN" >"$RUN_DIR/server.log" 2>&1 &
  SERVER_PID=$!
  BASE_URL="http://127.0.0.1:$PORT"

  ready=0
  for _ in {1..40}; do
    if curl -sf "$BASE_URL/healthz" >/dev/null 2>&1; then ready=1; break; fi
    sleep 1
  done
  if [[ "$ready" != 1 ]]; then
    echo "Hub did not become ready; log follows:" >&2
    cat "$RUN_DIR/server.log" >&2
    exit 1
  fi
fi

AUTH=()
[[ -n "$TOKEN" ]] && AUTH=(-H "Authorization: Bearer $TOKEN")

echo "== target: $BASE_URL =="

# --- 1. 控制台静态面 -------------------------------------------------------

# 根路径必须把操作者引到控制台。
redirect_code="$(curl -s -o /dev/null -w '%{http_code}' "$BASE_URL/")"
redirect_to="$(curl -s -o /dev/null -D - "$BASE_URL/" | tr -d '\r' | awk 'tolower($1)=="location:"{print $2}')"
[[ "$redirect_code" =~ ^30[0-9]$ ]] || { echo "FAIL: / 未重定向，得到 $redirect_code" >&2; exit 1; }
[[ "$redirect_to" == "/console" ]] || { echo "FAIL: / 重定向到 $redirect_to" >&2; exit 1; }
echo "ok   /            -> $redirect_code $redirect_to"

check_asset() {
  # 逐条 local：bash 3.2（macOS 自带）会在赋值前展开同一 local 语句的所有右值，
  # 合并写法下 out 里的 $path 会在 set -u 下报未绑定。
  local path="$1"
  local want_type="$2"
  local out="$RUN_DIR/$(basename "$path")"
  local code
  local type
  code="$(curl -s -o "$out" -w '%{http_code}' "$BASE_URL$path")"
  type="$(curl -s -o /dev/null -w '%{content_type}' "$BASE_URL$path")"
  [[ "$code" == 200 ]] || { echo "FAIL: $path -> $code" >&2; exit 1; }
  [[ "$type" == "$want_type"* ]] || { echo "FAIL: $path content-type=$type" >&2; exit 1; }
  [[ -s "$out" ]] || { echo "FAIL: $path 内容为空" >&2; exit 1; }
  echo "ok   $path ($(wc -c <"$out" | tr -d ' ') bytes, $type)"
}
check_asset /console            text/html
check_asset /console/app.css    text/css
check_asset /console/app.js     application/javascript

# 外壳引用的资源必须都已验证过，避免控制台加载一个 404 的文件。
python3 - "$RUN_DIR/console" <<'PY'
import pathlib, re, sys
html = pathlib.Path(sys.argv[1]).read_text()
refs = set(re.findall(r'/console/[A-Za-z0-9._-]+', html))
assert refs <= {"/console/app.css", "/console/app.js"}, f"外壳引用了未验证的资源: {refs}"
print("ok   外壳只引用已验证的静态资源")
PY

# --- 2. 聚合总览端点 -------------------------------------------------------

curl -sf ${AUTH[@]+"${AUTH[@]}"} "$BASE_URL/api/v1/system/overview" >"$RUN_DIR/overview.json" || {
  echo "FAIL: /api/v1/system/overview 不可用（若 Hub 开启鉴权，请设置 UENV_HUB_TOKEN）" >&2
  exit 1
}

python3 - "$RUN_DIR/overview.json" <<'PY'
import json, pathlib, sys

ov = json.loads(pathlib.Path(sys.argv[1]).read_text())

assert ov["service"]["name"] == "uenv-hub", ov["service"]
assert ov["db_up"] is True, "数据库探针为假"
assert ov["server_time"] >= ov["started_at"], "服务端时间早于启动时刻"
assert ov["uptime_seconds"] >= 0

reg, sto, host, pos = ov["registry"], ov["storage"], ov["host"], ov["posture"]

# 计数必须非负，且 yanked 是总数的子集而非并列项。
for k, v in reg.items():
    assert isinstance(v, int) and v >= 0, f"registry.{k} = {v!r}"
assert reg["yanked_env_versions"] <= reg["env_versions"]
assert reg["yanked_package_versions"] <= reg["package_versions"]
assert reg["yanked_stack_versions"] <= reg["stack_versions"]
assert reg["deprecated_envs"] <= reg["envs"]
# 有版本就必然有环境；反之不成立（可以先建环境再发版本）。
if reg["env_versions"] > 0:
    assert reg["envs"] > 0

# 存储：目录不存在时不得报出非零用量。
if not sto["artifact_dir_exists"]:
    assert sto["artifact_files"] == 0 and sto["artifact_bytes"] == 0
assert sto["artifact_dir"] and sto["database_url"]

# 主机：/proc 派生字段要么缺席，要么自洽——不允许出现伪造的 0。
assert host["os"] and host["arch"]
if host.get("cpu_usage_percent") is not None:
    assert 0.0 <= host["cpu_usage_percent"] <= 100.0
if host.get("memory_total_bytes") and host.get("memory_available_bytes") is not None:
    assert host["memory_available_bytes"] <= host["memory_total_bytes"]
if host.get("load_average") is not None:
    assert len(host["load_average"]) == 3

assert isinstance(pos["require_token"], bool)
assert isinstance(pos["cors_allow_origins"], list)

print(
    "ok   /api/v1/system/overview",
    f"envs={reg['envs']}",
    f"pkgs={reg['packages']}",
    f"stacks={reg['stacks']}",
    f"bridges={reg['agent_bridges']}",
    f"artifacts={reg['package_artifacts']}",
    f"disk={sto['artifact_bytes']}B",
    f"cpu={host.get('cpu_usage_percent')}",
)
PY

# --- 3. 控制台用到的每一个端点都实打实地打一遍 -----------------------------

probe() {
  local path="$1"
  local required="${2:-required}"
  local code
  code="$(curl -s -o "$RUN_DIR/probe.json" -w '%{http_code}' ${AUTH[@]+"${AUTH[@]}"} "$BASE_URL$path")"
  if [[ "$code" == 200 ]]; then
    echo "ok   $path"
  elif [[ "$required" == "optional" ]]; then
    echo "skip $path -> $code（该 Hub 未提供此内容，控制台会按空态渲染）"
  else
    echo "FAIL: $path -> $code" >&2
    cat "$RUN_DIR/probe.json" >&2
    exit 1
  fi
}

probe /healthz
probe /version
probe /metrics
probe /api/v1/envs
probe /api/v1/packages
probe /api/v1/episode-stacks
probe /api/v1/agent-bridges
probe /api/v1/templates
probe "/api/v1/search?q=swe"
probe /api/v1/admin/audit-log optional
for variant in verified lite pro smith; do
  probe "/api/v1/swe/$variant/instances" optional
done

# 列表里挑第一条，把详情/解析链路也走通——只验列表不验详情等于没验。
python3 - "$BASE_URL" "$RUN_DIR" "${TOKEN}" <<'PY'
import json, subprocess, sys, urllib.parse

base, run_dir, token = sys.argv[1], sys.argv[2], sys.argv[3]

def get(path):
    cmd = ["curl", "-s", "-o", f"{run_dir}/detail.json", "-w", "%{http_code}"]
    if token:
        cmd += ["-H", f"Authorization: Bearer {token}"]
    cmd.append(base + path)
    code = subprocess.run(cmd, capture_output=True, text=True).stdout.strip()
    body = open(f"{run_dir}/detail.json").read()
    return code, body

def first(path, key):
    code, body = get(path)
    assert code == "200", f"{path} -> {code}"
    data = json.loads(body)
    items = data["items"] if isinstance(data, dict) and "items" in data else data
    return items[0][key] if items else None

checked = 0

env_type = first("/api/v1/envs?per_page=1", "env_type")
if env_type:
    q = urllib.parse.quote(env_type)
    for path in (
        f"/api/v1/envs/{q}",
        f"/api/v1/envs/{q}/versions",
        f"/api/v1/envs/{q}/versions/latest",
        f"/api/v1/envs/{q}/versions/latest/interface",
        f"/api/v1/envs/{q}/versions/latest/examples",
    ):
        code, _ = get(path)
        assert code == "200", f"{path} -> {code}"
        checked += 1
    print(f"ok   环境详情链路 ({env_type})")

pkg = first("/api/v1/packages?per_page=1", "package_id")
if pkg:
    q = urllib.parse.quote(pkg)
    code, body = get(f"/api/v1/packages/{q}/versions/latest")
    assert code == "200", f"package manifest -> {code}"
    manifest = json.loads(body)
    checked += 1
    code, body = get(f"/api/v1/packages/{q}/versions/latest/sync-plan")
    assert code == "200", f"sync-plan -> {code}"
    plan = json.loads(body)
    checked += 1
    # 控制台展示的 bundle_digest 必须与 sync 用的同一个值。
    assert plan["bundle_digest"].startswith("sha256:"), plan["bundle_digest"]
    if manifest.get("artifacts"):
        name = urllib.parse.quote(manifest["artifacts"][0]["name"])
        code, _ = get(f"/api/v1/packages/{q}/versions/latest/artifacts/{name}")
        assert code == "200", f"artifact -> {code}"
        checked += 1
    print(f"ok   环境包详情链路 ({pkg}, bundle={plan['bundle_digest'][:19]}…)")

stack = first("/api/v1/episode-stacks?per_page=1", "stack_id")
if stack:
    q = urllib.parse.quote(stack)
    code, _ = get(f"/api/v1/episode-stacks/{q}/versions")
    assert code == "200", f"stack versions -> {code}"
    checked += 1
    code, body = get(f"/api/v1/episode-stacks/{q}/versions/latest/resolve")
    assert code == "200", f"stack resolve -> {code}"
    resolved = json.loads(body)
    assert resolved["stack_digest"].startswith("sha256:")
    assert resolved["components"], "解析结果没有组件"
    checked += 1
    print(
        f"ok   Stack 解析链路 ({stack}, "
        f"{len(resolved['components'])} 个组件, digest={resolved['stack_digest'][:19]}…)"
    )

print(f"ok   详情链路共验证 {checked} 个端点")
PY

# --------------------------------------------------------------------------
# 无头浏览器渲染回归。
#
# 上面的检查只证明「API 有数据、资源能取到」，证明不了页面真的画得出来——一个
# 字段类型不合预期就足以让整视图白屏。有 Chrome 时就把每条路由真渲染一遍，
# 断言面包屑出现且没有错误块；没有 Chrome 时跳过而不失败（CI 容器通常没有）。
# --------------------------------------------------------------------------

CHROME_BIN="${UENV_HUB_CONSOLE_CHROME:-}"
if [[ -z "$CHROME_BIN" ]]; then
  for candidate in \
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
    "/Applications/Chromium.app/Contents/MacOS/Chromium" \
    "$(command -v google-chrome || true)" \
    "$(command -v chromium || true)" \
    "$(command -v chromium-browser || true)"; do
    [[ -n "$candidate" && -x "$candidate" ]] && { CHROME_BIN="$candidate"; break; }
  done
fi

echo
if [[ -z "$CHROME_BIN" ]]; then
  echo "skip 无头渲染回归：未找到 Chrome/Chromium（设 UENV_HUB_CONSOLE_CHROME 可指定）"
else
  ROUTES=(
    overview envs packages stacks bridges swe templates audit health settings
    "swe?variant=smith" "swe?variant=pro" "search?q=swe"
  )
  # 再补上从真实数据里取到的详情页，避免只渲染空壳列表。
  [[ -n "${env_type_probe:=}" ]] || true
  for pair in "envs:/api/v1/envs?per_page=1:env_type" \
              "packages:/api/v1/packages?per_page=1:package_id" \
              "stacks:/api/v1/episode-stacks?per_page=1:stack_id"; do
    view="${pair%%:*}"; rest="${pair#*:}"; api_path="${rest%%:*}"; field="${rest##*:}"
    id="$(curl -s ${AUTH[@]+"${AUTH[@]}"} "$BASE_URL$api_path" \
      | python3 -c "
import json,sys
try:
    d=json.load(sys.stdin); items=d.get('items') if isinstance(d,dict) else d
    print((items or [{}])[0].get('$field',''))
except Exception:
    print('')
")"
    [[ -n "$id" ]] && ROUTES+=("$view/$id")
  done

  RENDER_DIR="$RUN_DIR/render"; mkdir -p "$RENDER_DIR"
  render_failed=0
  for route in "${ROUTES[@]}"; do
    safe="$(echo "$route" | tr '/?=&' '____')"
    "$CHROME_BIN" --headless --disable-gpu --no-sandbox --hide-scrollbars \
      --virtual-time-budget=7000 --dump-dom \
      "$BASE_URL/console#/$route" > "$RENDER_DIR/$safe.html" 2>/dev/null || true
    if ! python3 - "$RENDER_DIR/$safe.html" "$route" <<'PY'
import re, sys
dom = open(sys.argv[1], encoding="utf-8", errors="replace").read()
route = sys.argv[2]
if 'id="crumbs"' not in dom:
    print(f"FAIL {route}: 未渲染出控制台外壳（{len(dom)} 字节，多半是连不上）")
    raise SystemExit(1)
errors = re.findall(r'class="err"[^>]*>(.*?)</div>', dom, re.S)
if errors:
    msg = re.sub("<[^>]+>", "", errors[0]).strip()[:160]
    print(f"FAIL {route}: 视图渲染出错误块 — {msg}")
    raise SystemExit(1)
crumbs = re.search(r'id="crumbs"[^>]*>(.*?)</div>', dom, re.S)
title = " ".join(re.sub("<[^>]+>", " ", crumbs.group(1)).split())
cards = len(re.findall(r'class="card"', dom))
rows = len(re.findall(r"<tr[ >]", dom))
print(f"ok   渲染 #/{route:<32s} 卡片 {cards:2d} · 表行 {rows:3d} | {title[:52]}")
PY
    then
      render_failed=1
    fi
  done
  [[ $render_failed -eq 0 ]] || { echo "无头渲染回归失败"; exit 1; }
  echo "ok   无头渲染回归：${#ROUTES[@]} 条路由全部画出且无错误块"

  # --------------------------------------------------------------- 对比度审计
  #
  # 浅色主题下配色是否可读，肉眼看截图判断不了——把颜色实测出来算。连上 Chrome
  # 的调试端口，在页面里取每个可见文字节点 getComputedStyle 的实际前景色、沿祖先
  # 链合成出实际背景色，按 WCAG 2.1 算对比度。浏览器已经把 oklch / color-mix 全部
  # 解析完，量到的就是用户真正看到的颜色。
  echo
  if ! command -v node > /dev/null 2>&1; then
    echo "skip 对比度审计：未找到 node"
  else
    CDP_PORT="${UENV_HUB_CONSOLE_CDP_PORT:-9222}"
    "$CHROME_BIN" --headless --disable-gpu --no-sandbox \
      --remote-debugging-port="$CDP_PORT" \
      --user-data-dir="$RUN_DIR/chrome-audit" \
      --window-size=1600,1200 about:blank > "$RUN_DIR/chrome-audit.log" 2>&1 &
    CHROME_AUDIT_PID=$!
    for _ in $(seq 1 20); do
      curl -sf "http://127.0.0.1:$CDP_PORT/json/version" > /dev/null && break
      sleep 1
    done

    audit_rc=0
    CDP_PORT="$CDP_PORT" BASE="$BASE_URL" \
      ROUTES="$(IFS=,; echo "${ROUTES[*]}")" \
      node --experimental-websocket "$REPO_ROOT/scripts/hub-console-contrast-audit.mjs" \
      || audit_rc=$?
    kill "$CHROME_AUDIT_PID" 2> /dev/null || true
    wait "$CHROME_AUDIT_PID" 2> /dev/null || true
    CHROME_AUDIT_PID=""
    [[ $audit_rc -eq 0 ]] || { echo "对比度审计未通过"; exit 1; }
  fi
fi

echo
echo "HUB_CONSOLE_E2E_OK  base=$BASE_URL"
