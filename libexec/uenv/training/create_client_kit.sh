#!/usr/bin/env bash
# Export only the files a remote GPU/VeRL host needs. This is intentionally
# smaller than a complete UEnv release and never installs systemd services.
set -euo pipefail

RELEASE="/opt/uenv/current"
OUTPUT="$PWD/uenv-training-client.tar.gz"

fail() {
  echo "错误：$*" >&2
  exit 1
}

usage() {
  cat <<'EOF'
从已安装 UEnv release 生成 GPU 训练客户端包

用法：
  create_client_kit.sh [--release DIR] [--output FILE]

选项：
  --release DIR   已安装 release（默认 /opt/uenv/current）
  --output FILE   输出 tar.gz（默认 ./uenv-training-client.tar.gz）

客户端包只包含 VeRL 入口、UEnv Bridge wheel/config 和示例数据，不包含
UEnv Server、UEnv Worker、UEnv Hub 或 systemd 服务。把它复制到 GPU 主机解压即可。
EOF
}

while (($#)); do
  case "$1" in
    --release) RELEASE="${2:-}"; shift 2 ;;
    --output) OUTPUT="${2:-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) fail "未知参数：$1" ;;
  esac
done

[[ -d "$RELEASE" ]] || fail "release 目录不存在：$RELEASE"
RELEASE="$(cd "$RELEASE" && pwd)"
OUTPUT="$(mkdir -p "$(dirname "$OUTPUT")" && cd "$(dirname "$OUTPUT")" && printf '%s/%s\n' "$PWD" "$(basename "$OUTPUT")")"
[[ ! -e "$OUTPUT" ]] || fail "输出文件已存在：$OUTPUT"

required=(
  VERSION
  manifest.json
  bin/uenv-train
  libexec/uenv/training/train_verl.sh
  libexec/uenv/training/verl_runner.sh
  libexec/uenv/training/prepare_episode_data.py
  libexec/uenv/swe/prepare_verl_data.py
  examples/cases/training/qa-gsm8k.jsonl
  examples/cases/training/code-dscodebench.jsonl
  examples/cases/training/process-plugin.jsonl
  examples/cases/training/verl-grpo-overrides.conf
  examples/cases/training/README.md
  share/uenv-bridge/configs/uenv-agent-loop.yaml
  share/uenv-bridge/scripts/run_verl_main_ppo.py
  share/swe/smith-sample-catalog.json
)
for relative in "${required[@]}"; do
  [[ -f "$RELEASE/$relative" ]] || fail "release 缺少训练客户端文件：$relative"
done

wheel="$(find "$RELEASE/wheels" -maxdepth 1 -type f \
  \( -name 'uenv_bridge-*.whl' -o -name 'uenv-bridge-*.whl' \) \
  -print | head -n1)"
[[ -n "$wheel" ]] || fail "release 缺少 UEnv Bridge wheel"

temporary="$(mktemp -d -p "$(dirname "$OUTPUT")" .uenv-training-client.XXXXXXXX)"
cleanup() {
  [[ -n "${temporary:-}" && -d "$temporary" ]] && rm -rf -- "$temporary"
}
trap cleanup EXIT
root="$temporary/uenv-training-client"

for relative in "${required[@]}"; do
  install -D -m 0644 "$RELEASE/$relative" "$root/$relative"
done
chmod 0755 \
  "$root/bin/uenv-train" \
  "$root/libexec/uenv/training/train_verl.sh" \
  "$root/libexec/uenv/training/verl_runner.sh" \
  "$root/libexec/uenv/training/prepare_episode_data.py" \
  "$root/libexec/uenv/swe/prepare_verl_data.py" \
  "$root/share/uenv-bridge/scripts/run_verl_main_ppo.py"
install -D -m 0644 "$wheel" "$root/wheels/$(basename "$wheel")"

cat > "$root/README.txt" <<'EOF'
这是 UEnv GPU 训练客户端包，不是 UEnv 服务安装包。

解压后运行：
  cd /path/to/uenv-training-client
  ./bin/uenv-train run-task \
    --uenv-release "$PWD" \
    --model /absolute/model/path \
    --input /absolute/task/cases.jsonl \
    --env-type qa \
    --dataset gsm8k \
    --max-steps 1 \
    --work-dir /absolute/output/uenv-training \
    --uenv-endpoint <CPU_UENV_IP>:50051 \
    --gateway-public-url http://<GPU_IP>:18080/v1 \
    --gateway-bind <GPU_IP> \
    --gpus 1 \
    --steps 100 \
    --rollouts 4 \
    --train-batch-size 8 \
    --runtime docker \
    --image docker.io/verlai/verl:vllm017.latest

UEnv 主机必须已经运行 UEnv Server 和 UEnv Worker；本包不会安装服务。
EOF

tar -czf "$OUTPUT" -C "$temporary" uenv-training-client
(cd "$(dirname "$OUTPUT")" && \
  sha256sum "$(basename "$OUTPUT")" > "$(basename "$OUTPUT").sha256")
echo "训练客户端包：$OUTPUT"
echo "校验文件：$OUTPUT.sha256"
helper="$(dirname "$OUTPUT")/install-training-client.sh"
install -m 0755 "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/install_client_kit.sh" "$helper"
echo "GPU 解压脚本：$helper"
