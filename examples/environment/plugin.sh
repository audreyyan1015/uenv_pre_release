#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BOOTSTRAP_PYTHON="${UENV_PLUGIN_BOOTSTRAP_PYTHON:-python3}"
CLEANUP_DIR=""

cleanup() {
  if [[ -n "$CLEANUP_DIR" && -d "$CLEANUP_DIR" ]]; then
    find "$CLEANUP_DIR" -depth -delete
  fi
}
trap cleanup EXIT

fail() {
  echo "错误：$*" >&2
  exit 1
}

info() {
  echo "==> $*"
}

usage() {
  cat <<'EOF'
创建、测试、安装和发布 UEnv process plugin。

用法：
  plugin.sh create ENV_TYPE --dataset DATASET \
                   [--dir DIR] [--version VERSION] [--description TEXT]
  plugin.sh test [DIR] [--offline] [--logic-only]
  plugin.sh install-local [DIR] [--offline] [--skip-test] [--no-restart]
                          [--plugin-root DIR] [--store-root DIR]
  plugin.sh publish [DIR] [--offline] [--skip-test] [--package ID]
                    [--worker-min VERSION] [--publisher NAME]

常用流程：
  plugin.sh create my-environment --dataset my-dataset
  # 编辑 my-environment/environment.py
  plugin.sh test my-environment
  sudo plugin.sh install-local my-environment
  plugin.sh publish my-environment

--offline 只使用已有 wheelhouse，不访问 Python 包索引。
EOF
}

template_dir() {
  local candidate
  for candidate in \
    "${UENV_PROCESS_PLUGIN_TEMPLATE:-}" \
    "$SCRIPT_DIR/../../templates/process-plugin" \
    "$SCRIPT_DIR/../../share/templates/process-plugin" \
    "/opt/uenv/current/share/templates/process-plugin"; do
    if [[ -n "$candidate" && -f "$candidate/manifest.yaml" ]]; then
      (cd "$candidate" && pwd)
      return
    fi
  done
  fail "找不到 process-plugin 模板；可设置 UENV_PROCESS_PLUGIN_TEMPLATE"
}

manifest_value() {
  "$BOOTSTRAP_PYTHON" - "$1" "$2" <<'PY'
import json
import sys
from pathlib import Path

path, key = Path(sys.argv[1]), sys.argv[2]
for line in path.read_text(encoding="utf-8").splitlines():
    if line.startswith(f"{key}:"):
        value = line.split(":", 1)[1].strip()
        try:
            value = json.loads(value)
        except (json.JSONDecodeError, TypeError):
            value = value.strip("'\"")
        print(value)
        raise SystemExit(0)
raise SystemExit(f"manifest missing top-level field: {key}")
PY
}

validate_plugin() {
  local dir="$1" env_type version ipc entry
  [[ -d "$dir" ]] || fail "插件目录不存在：$dir"
  for file in manifest.yaml environment.py uenv_plugin_api.py plugin.py run.sh requirements.txt; do
    [[ -f "$dir/$file" ]] || fail "插件缺少 $file：$dir"
  done
  env_type="$(manifest_value "$dir/manifest.yaml" env_type)"
  version="$(manifest_value "$dir/manifest.yaml" version)"
  ipc="$(manifest_value "$dir/manifest.yaml" ipc)"
  entry="$(manifest_value "$dir/manifest.yaml" entry)"
  [[ "$env_type" =~ ^[a-z0-9][a-z0-9._-]*$ && "$env_type" != *..* ]] \
    || fail "env_type 只能使用小写字母、数字、点、下划线和连字符：$env_type"
  [[ "$version" =~ ^[0-9A-Za-z._+-]+$ ]] || fail "version 格式非法：$version"
  [[ "$ipc" == "proto-uds" ]] || fail "模板目前只支持 ipc: proto-uds"
  [[ "$entry" == "./run.sh" ]] || fail "通信入口必须保持 entry: ./run.sh"
}

copy_plugin_tree() {
  local source="$1" target="$2"
  mkdir -p "$target"
  (
    cd "$source"
    tar \
      --exclude='.venv' \
      --exclude='.git' \
      --exclude='__pycache__' \
      --exclude='.pytest_cache' \
      --exclude='.mypy_cache' \
      --exclude='*.pyc' \
      -cf - .
  ) | (cd "$target" && tar -xf -)
}

requirements_present() {
  grep -Eq '^[[:space:]]*[^#[:space:]]' "$1/requirements.txt"
}

wheelhouse_present() {
  compgen -G "$1/wheelhouse/*.whl" >/dev/null
}

prepare_venv() {
  local dir="$1" offline="$2" python="$dir/.venv/bin/python"
  if [[ ! -x "$python" ]]; then
    info "创建 Python 虚拟环境：$dir/.venv"
    "$BOOTSTRAP_PYTHON" -m venv "$dir/.venv" \
      || fail "无法创建 venv；请安装当前 Python 的 venv 组件"
  fi
  if ! requirements_present "$dir"; then
    return
  fi

  local -a install_args=(-m pip install --disable-pip-version-check --no-cache-dir)
  if [[ "$offline" -eq 1 ]]; then
    wheelhouse_present "$dir" \
      || fail "离线模式需要 $dir/wheelhouse/*.whl；请先在联网的同平台机器运行 publish"
    install_args+=(--no-index --find-links "$dir/wheelhouse")
  fi
  install_args+=(-r "$dir/requirements.txt")
  info "安装插件依赖"
  "$python" "${install_args[@]}"
}

prepare_wheelhouse() {
  local dir="$1" offline="$2"
  if ! requirements_present "$dir"; then
    return
  fi
  if [[ "$offline" -eq 1 ]]; then
    wheelhouse_present "$dir" || fail "离线发布需要已有 wheelhouse/*.whl"
    return
  fi
  mkdir -p "$dir/wheelhouse"
  info "下载目标 Worker 可离线安装的 wheels"
  "$dir/.venv/bin/python" -m pip download \
    --disable-pip-version-check \
    --only-binary=:all: \
    -r "$dir/requirements.txt" \
    -d "$dir/wheelhouse"
}

verify_wheelhouse() {
  local dir="$1" check_dir
  if ! requirements_present "$dir"; then
    return
  fi
  wheelhouse_present "$dir" || fail "wheelhouse 没有 wheel 文件"
  check_dir="$(mktemp -d -t uenv-wheelcheck.XXXXXXXX)"
  CLEANUP_DIR="$check_dir"
  "$BOOTSTRAP_PYTHON" -m venv "$check_dir/.venv" \
    || fail "无法创建 wheelhouse 验证环境"
  info "在全新 venv 中验证 wheelhouse 完整性"
  "$check_dir/.venv/bin/python" -m pip install \
    --disable-pip-version-check \
    --no-cache-dir \
    --no-index \
    --find-links "$dir/wheelhouse" \
    -r "$dir/requirements.txt"
  find "$check_dir" -depth -delete
  CLEANUP_DIR=""
}

run_logic_test() {
  local dir="$1"
  info "测试 environment.py"
  "$BOOTSTRAP_PYTHON" "$dir/tests/test_environment.py"
}

run_contract_test() {
  local dir="$1" offline="$2"
  prepare_venv "$dir" "$offline"
  info "通过真实 Unix socket 测试 UEnv 协议"
  "$dir/.venv/bin/python" "$dir/tests/test_contract.py"
}

source_digest() {
  "$BOOTSTRAP_PYTHON" - "$1" <<'PY'
import hashlib
import os
import sys
from pathlib import Path

root = Path(sys.argv[1]).resolve()
ignored = {".venv", ".git", "__pycache__", ".pytest_cache", ".mypy_cache"}
digest = hashlib.sha256()
for path in sorted(root.rglob("*")):
    relative = path.relative_to(root)
    if any(part in ignored for part in relative.parts) or path.suffix == ".pyc":
        continue
    if path.is_symlink():
        raise SystemExit(f"plugin source may not contain symlinks: {relative}")
    if not path.is_file():
        continue
    digest.update(relative.as_posix().encode())
    digest.update(b"\0")
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
print(digest.hexdigest()[:16])
PY
}

default_worker_min() {
  local file value
  if [[ -n "${UENV_WORKER_MIN:-}" ]]; then
    echo "$UENV_WORKER_MIN"
    return
  fi
  for file in "$SCRIPT_DIR/../../VERSION" /opt/uenv/current/VERSION; do
    if [[ -f "$file" ]]; then
      value="$(tr -d '[:space:]' < "$file")"
      if [[ "$value" =~ ^[0-9A-Za-z._+-]+$ ]]; then
        echo "$value"
        return
      fi
    fi
  done
  echo "0.1.2-trial"
}

uenv_command() {
  if [[ -n "${UENV_CLI:-}" ]]; then
    echo "$UENV_CLI"
  elif command -v uenv >/dev/null 2>&1; then
    command -v uenv
  else
    fail "找不到 uenv；请安装 UEnv 或设置 UENV_CLI=/path/to/uenv"
  fi
}

command_create() {
  [[ $# -ge 1 ]] || fail "create 需要 ENV_TYPE"
  local env_type="$1" dataset="" target="" version="0.1.0" description=""
  shift
  while (($#)); do
    case "$1" in
      --dir) target="${2:-}"; shift 2 ;;
      --dataset) dataset="${2:-}"; shift 2 ;;
      --version) version="${2:-}"; shift 2 ;;
      --description) description="${2:-}"; shift 2 ;;
      -h|--help) usage; exit 0 ;;
      *) fail "create 不认识参数：$1" ;;
    esac
  done
  [[ "$env_type" =~ ^[a-z0-9][a-z0-9._-]*$ && "$env_type" != *..* ]] \
    || fail "ENV_TYPE 只能使用小写字母、数字、点、下划线和连字符"
  [[ "$dataset" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ && "$dataset" != *..* ]] \
    || fail "create 需要 --dataset；只能使用字母、数字、点、下划线和连字符"
  [[ "$version" =~ ^[0-9A-Za-z._+-]+$ ]] || fail "version 格式非法"
  target="${target:-$PWD/$env_type}"
  description="${description:-Custom UEnv process environment $env_type}"
  [[ ! -e "$target" ]] || fail "目标已存在：$target"

  local template
  template="$(template_dir)"
  info "创建环境：$target"
  copy_plugin_tree "$template" "$target"
  "$BOOTSTRAP_PYTHON" - "$target/manifest.yaml" "$env_type" "$version" "$description" "$dataset" <<'PY'
import json
import sys
from pathlib import Path

path = Path(sys.argv[1])
updates = {"env_type": sys.argv[2], "version": sys.argv[3], "description": sys.argv[4]}
dataset = sys.argv[5]
lines = path.read_text(encoding="utf-8").splitlines()
seen = set()
for index, line in enumerate(lines):
    for key, value in updates.items():
        if line.startswith(f"{key}:"):
            lines[index] = f"{key}: {json.dumps(value, ensure_ascii=False)}"
            seen.add(key)
for key in updates.keys() - seen:
    lines.append(f"{key}: {json.dumps(updates[key], ensure_ascii=False)}")
start = next((i for i, line in enumerate(lines) if line == "datasets:"), None)
if start is None:
    lines.extend(["datasets:", f"  - {json.dumps(dataset, ensure_ascii=False)}"])
else:
    end = start + 1
    while end < len(lines) and (not lines[end] or lines[end][0].isspace()):
        end += 1
    lines[start:end] = ["datasets:", f"  - {json.dumps(dataset, ensure_ascii=False)}"]
path.write_text("\n".join(lines) + "\n", encoding="utf-8")
PY
  "$BOOTSTRAP_PYTHON" - "$target/example.jsonl" "$env_type" "$dataset" <<'PY'
import json
import sys
from pathlib import Path

path = Path(sys.argv[1])
env_type = sys.argv[2]
dataset = sys.argv[3]
row = {
    "id": f"{env_type}-example-1",
    "env_type": env_type,
    "dataset": dataset,
    "question": "Reply with exactly: ok",
    "env_config": {"expected_action": "ok"},
    "reward_config": {"type": "plugin", "target": "ok"},
    "max_steps": 1,
}
path.write_text(json.dumps(row, ensure_ascii=False) + "\n", encoding="utf-8")
PY
  chmod 0755 "$target/run.sh" "$target/plugin.py" "$target/generate_proto.sh" \
    "$target/tests/test_contract.py" "$target/tests/test_environment.py"
  echo
  echo "已创建 $env_type。下一步只需编辑："
  echo "  $target/environment.py"
  echo "需要更换示例输入时再编辑："
  echo "  $target/example.jsonl"
  echo "然后运行："
  echo "  $0 test '$target'"
  echo "安装后评测这份输入："
  echo "  uenv evaluate run-task --endpoint 127.0.0.1:50051 --env-type '$env_type' --dataset '$dataset' --input '$target/example.jsonl' --output '$target/results.jsonl' --max-steps 1"
}

command_test() {
  local dir="$PWD" offline=0 logic_only=0
  if (($#)) && [[ "$1" != --* ]]; then dir="$1"; shift; fi
  while (($#)); do
    case "$1" in
      --offline) offline=1; shift ;;
      --logic-only) logic_only=1; shift ;;
      -h|--help) usage; exit 0 ;;
      *) fail "test 不认识参数：$1" ;;
    esac
  done
  dir="$(cd "$dir" && pwd)"
  validate_plugin "$dir"
  run_logic_test "$dir"
  if [[ "$logic_only" -eq 0 ]]; then
    run_contract_test "$dir" "$offline"
  fi
  echo "插件测试通过。"
}

command_install_local() {
  local dir="$PWD" offline=0 skip_test=0 no_restart=0
  local plugin_root="${UENV_PLUGIN_ROOT:-/var/lib/uenv/plugins}"
  local store_root="${UENV_LOCAL_PLUGIN_STORE:-/var/lib/uenv/local-plugins}"
  if (($#)) && [[ "$1" != --* ]]; then dir="$1"; shift; fi
  while (($#)); do
    case "$1" in
      --offline) offline=1; shift ;;
      --skip-test) skip_test=1; shift ;;
      --no-restart) no_restart=1; shift ;;
      --plugin-root) plugin_root="${2:-}"; shift 2 ;;
      --store-root) store_root="${2:-}"; shift 2 ;;
      -h|--help) usage; exit 0 ;;
      *) fail "install-local 不认识参数：$1" ;;
    esac
  done
  dir="$(cd "$dir" && pwd)"
  validate_plugin "$dir"
  if [[ "$skip_test" -eq 0 ]]; then
    run_logic_test "$dir"
    run_contract_test "$dir" "$offline"
  fi
  if [[ "$no_restart" -eq 0 ]]; then
    command -v systemctl >/dev/null 2>&1 \
      || fail "找不到 systemctl；重新运行并加 --no-restart 后手工重启 Worker"
  fi

  local env_type version digest env_store final active pending previous=""
  env_type="$(manifest_value "$dir/manifest.yaml" env_type)"
  version="$(manifest_value "$dir/manifest.yaml" version)"
  digest="$(source_digest "$dir")"
  env_store="$store_root/$env_type"
  final="$env_store/$version-$digest"
  active="$plugin_root/$env_type"
  if [[ -e "$active" && ! -L "$active" ]]; then
    fail "拒绝覆盖非符号链接：$active；请先人工备份旧的本地插件"
  fi
  install -d -m 0755 "$env_store" "$plugin_root"
  if [[ -e "$final" ]]; then
    [[ -f "$final/.uenv-local-ready" ]] \
      || fail "发现未完成的本地安装：$final；确认没有进程使用后移走该目录再重试"
  else
    # Build directly in its final immutable directory. Only the active symlink
    # is switched atomically after dependency installation and permission
    # normalization have both succeeded.
    mkdir "$final"
    copy_plugin_tree "$dir" "$final"
    prepare_venv "$final" "$offline"
    printf 'env_type=%s\nversion=%s\nsource_digest=%s\n' \
      "$env_type" "$version" "$digest" > "$final/.uenv-local-ready"
  fi

  # A version/digest directory is immutable application material.  GNU tar
  # preserves source ownership when this command runs as root, so normalize
  # both ownership and write permissions even when reusing an older install.
  chown -R root:root "$final"
  chmod -R u=rwX,go=rX "$final"
  chmod 0755 "$final/run.sh" "$final/plugin.py"
  chmod 0644 "$final/.uenv-local-ready"

  if [[ -L "$active" ]]; then previous="$(readlink "$active")"; fi
  pending="$plugin_root/.$env_type.local-$$"
  [[ ! -e "$pending" && ! -L "$pending" ]] || fail "临时激活路径已存在：$pending"
  ln -s "$final" "$pending"
  mv -Tf "$pending" "$active"

  if [[ "$no_restart" -eq 0 ]]; then
    info "重启 uenv-worker"
    if ! systemctl restart uenv-worker.service \
      || ! systemctl is-active --quiet uenv-worker.service; then
      echo "新插件启动失败，正在恢复上一个激活版本。" >&2
      if [[ -n "$previous" ]]; then
        ln -s "$previous" "$pending"
        mv -Tf "$pending" "$active"
      else
        rm -f -- "$active"
      fi
      systemctl restart uenv-worker.service >/dev/null 2>&1 || true
      fail "uenv-worker 重启失败；请查看 journalctl -u uenv-worker -n 100"
    fi
  fi
  echo "已激活 $env_type@$version：$active -> $final"
  if [[ "$no_restart" -eq 1 ]]; then
    echo "尚未重启 Worker；完成后执行 sudo systemctl restart uenv-worker.service"
  fi
}

command_publish() {
  local dir="$PWD" offline=0 skip_test=0 package="" publisher=""
  local worker_min
  worker_min="$(default_worker_min)"
  if (($#)) && [[ "$1" != --* ]]; then dir="$1"; shift; fi
  while (($#)); do
    case "$1" in
      --offline) offline=1; shift ;;
      --skip-test) skip_test=1; shift ;;
      --package) package="${2:-}"; shift 2 ;;
      --worker-min) worker_min="${2:-}"; shift 2 ;;
      --publisher) publisher="${2:-}"; shift 2 ;;
      -h|--help) usage; exit 0 ;;
      *) fail "publish 不认识参数：$1" ;;
    esac
  done
  dir="$(cd "$dir" && pwd)"
  validate_plugin "$dir"
  local env_type version cli
  env_type="$(manifest_value "$dir/manifest.yaml" env_type)"
  version="$(manifest_value "$dir/manifest.yaml" version)"
  package="${package:-$env_type}"
  [[ "$worker_min" =~ ^[0-9A-Za-z._+-]+$ ]] || fail "worker-min 格式非法"

  if [[ "$skip_test" -eq 0 ]]; then
    run_logic_test "$dir"
    run_contract_test "$dir" "$offline"
  else
    prepare_venv "$dir" "$offline"
  fi
  prepare_wheelhouse "$dir" "$offline"
  # Verify in a fresh venv so already-installed dependencies cannot hide a
  # missing transitive wheel that would break activation on a Worker.
  verify_wheelhouse "$dir"

  cli="$(uenv_command)"
  local -a command=("$cli" env publish-plugin
    --plugin-dir "$dir"
    --package "$package"
    --version "$version"
    --worker-min "$worker_min")
  if [[ -n "$publisher" ]]; then command+=(--publisher "$publisher"); fi
  info "发布 $package@$version 到 Hub"
  "${command[@]}"
  echo "发布完成。Worker 安装命令："
  echo "  sudo uenv env sync '$package' --version '$version' --target-dir /var/lib/uenv --consumer worker --worker-version '$worker_min' --activate --plugin-dir /var/lib/uenv/plugins"
  echo "  sudo systemctl restart uenv-worker.service"
}

main() {
  local command="${1:-}"
  [[ -n "$command" ]] || { usage; exit 2; }
  shift || true
  case "$command" in
    create) command_create "$@" ;;
    test) command_test "$@" ;;
    install-local) command_install_local "$@" ;;
    publish) command_publish "$@" ;;
    -h|--help|help) usage ;;
    *) fail "未知命令：$command" ;;
  esac
}

main "$@"
