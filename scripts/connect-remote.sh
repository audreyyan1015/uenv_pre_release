#!/usr/bin/env bash
# 远端 A100 联调辅助脚本（依据 ../README.md 四端拓扑）。
#
# 私钥不入库：默认读取 UENV_SSH_KEY（缺省回退到本机常见路径或 secrets/）。
# 用法:
#   scripts/connect-remote.sh ssh-worker            # 登录 Worker 7143
#   scripts/connect-remote.sh ssh-adapter           # 登录 Adapter 7142
#   scripts/connect-remote.sh worker '<cmd>'        # 在 Worker 上执行命令
#   scripts/connect-remote.sh sync                  # rsync 本地源码 -> Worker 隔离目录
#   scripts/connect-remote.sh build                 # 在 Worker 隔离目录 proto+cargo build
#   scripts/connect-remote.sh swe-run <instance_id> [--no-gold]
#   scripts/connect-remote.sh health                # Worker 健康/端口
set -euo pipefail

WORKER_HOST="${UENV_WORKER_HOST:-219.147.100.43}"
WORKER_PORT="${UENV_WORKER_SSH_PORT:-7143}"
ADAPTER_PORT="${UENV_ADAPTER_SSH_PORT:-7142}"
HUB_HOST="${UENV_HUB_HOST:-8.130.95.176}"
REMOTE_DIR="${UENV_REMOTE_DIR:-/root/UEnv}"
INSTANCES_FILE="${UENV_SWE_INSTANCES:-config/swe/pro.json}"

# 解析私钥路径
resolve_key() {
  if [[ -n "${UENV_SSH_KEY:-}" && -f "${UENV_SSH_KEY}" ]]; then echo "${UENV_SSH_KEY}"; return; fi
  for k in "$HOME/Documents/143key" \
           "secrets/9aa460dab6678381f86a1022b8a54c9f_32e42d1c7902ce68ba6719d551645e02_8.143"; do
    [[ -f "$k" ]] && { echo "$k"; return; }
  done
  echo "ERROR: no SSH key found; set UENV_SSH_KEY" >&2; exit 1
}
KEY="$(resolve_key)"
chmod 600 "$KEY" 2>/dev/null || true
SSH_OPTS=(-o BatchMode=yes -o ConnectTimeout=10 -o StrictHostKeyChecking=accept-new -i "$KEY")
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

ssh_worker() { ssh "${SSH_OPTS[@]}" -p "$WORKER_PORT" root@"$WORKER_HOST" "$@"; }

cmd="${1:-}"; shift || true
case "$cmd" in
  ssh-worker)  exec ssh -i "$KEY" -p "$WORKER_PORT" root@"$WORKER_HOST" ;;
  ssh-adapter) exec ssh -i "$KEY" -p "$ADAPTER_PORT" root@"$WORKER_HOST" ;;
  worker)      ssh_worker "$@" ;;
  health)
    echo "== local probe Worker =="
    curl -sS --max-time 8 "http://${WORKER_HOST}:28777/health" && echo
    ssh_worker 'ss -tlnp 2>/dev/null | grep -E "28888|28777"; pgrep -af uenv-worker | grep -v pgrep'
    ;;
  sync)
    echo "== rsync $REPO_ROOT -> $WORKER_HOST:$REMOTE_DIR =="
    rsync -az --delete \
      --exclude 'target/' --exclude '.git/' --exclude 'frontend/' \
      --exclude '*.parquet' --exclude 'node_modules/' --exclude '__pycache__/' \
      -e "ssh ${SSH_OPTS[*]} -p $WORKER_PORT" \
      "$REPO_ROOT/" root@"$WORKER_HOST":"$REMOTE_DIR/"
    echo "synced."
    ;;
  build)
    ssh_worker "source ~/.cargo/env; cd $REMOTE_DIR && bash scripts/gen-worker-proto.sh && cargo build -p uenv-worker --release 2>&1 | tail -15"
    ;;
  swe-run)
    iid="${1:-}"; shift || true
    gold_flag="--gold true"
    [[ "${1:-}" == "--no-gold" ]] && gold_flag="--gold false"
    ssh_worker "source ~/.cargo/env; cd $REMOTE_DIR && ./target/release/uenv-worker swe-run --instances-file $INSTANCES_FILE --instance '$iid' $gold_flag"
    ;;
  *)
    grep -E '^#( |$)' "$0" | sed 's/^# \?//'; exit 1 ;;
esac
