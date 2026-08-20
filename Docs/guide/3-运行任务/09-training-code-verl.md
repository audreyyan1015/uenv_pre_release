# 代码生成

## 任务与数据性质

本案例用 Python 执行测试产生 reward，并让当前发布 runner 完成 20 次模型更新。输入只是一个自包含 `add(a,b)` 示例，使用 `code/dscodebench` 路由，仅用于演示端到端训练链路。

| 项目 | 本案例取值 |
|---|---|
| 环境 / 路由 | `code` / `dscodebench` |
| 输入真源 | `examples/cases/training/code-dscodebench.jsonl` |
| 测试 | `assert add(2, 3) == 5` |
| 更新步数 / rollout | 20 / 每样本 4 |
| 当前实现 | VeRL v0.7.1、GRPO |

## 执行主机

在有 NVIDIA GPU、代码模型和容器运行时的训练主机执行。代码测试由 UEnv Worker 的隔离环境执行，模型更新和 checkpoint 留在 GPU 主机。

## 前置检查与变量

除了[强化学习训练指南](./07-post-training.md)的共同检查，代码 UEnv Worker 必须启用不可信代码隔离、超时、进程和资源上限。

```bash
export UENV_SERVER_ENDPOINT='127.0.0.1:50051'
export UENV_RELEASE_ROOT='/opt/uenv/current'
export MODEL_DIR='/absolute/path/to/code-model'
export TRAIN_IMAGE='docker.io/verlai/verl:vllm017.latest'
export INPUT="$UENV_RELEASE_ROOT/examples/cases/training/code-dscodebench.jsonl"
export TRAIN_CONFIG="$UENV_RELEASE_ROOT/examples/cases/training/verl-grpo-overrides.conf"
export RUN_ID="code-train-$(date +%Y%m%d-%H%M%S)"
export WORK_DIR="$PWD/uenv-runs/$RUN_ID"

nvidia-smi -L
docker info >/dev/null
test -d "$MODEL_DIR"
test -r "$INPUT"
test -r "$TRAIN_CONFIG"
jq -e -c . "$INPUT" >/dev/null
test ! -e "$WORK_DIR"
mkdir -p "$(dirname "$WORK_DIR")"
docker image inspect "$TRAIN_IMAGE" >/dev/null || docker pull "$TRAIN_IMAGE"
```

检查 `uenv workers` 中存在支持 `code` 的 ready UEnv Worker。多机部署还要设置 UEnv Worker 可访问的 model gateway。

## 执行

```bash
uenv train run-task \
  --model "$MODEL_DIR" \
  --work-dir "$WORK_DIR" \
  --uenv-endpoint "$UENV_SERVER_ENDPOINT" \
  --env-type code \
  --dataset dscodebench \
  --input "$INPUT" \
  --max-steps 1 \
  --gpus 1 \
  --steps 20 \
  --rollouts 4 \
  --train-batch-size 2 \
  --runtime docker \
  --image "$TRAIN_IMAGE" \
  --verl-config "$TRAIN_CONFIG"
```

## 预期结果与验收

必有产物是转换数据与当前 runner 的训练输出；普通 code trajectory 通常随 Episode 结果内联，集中轨迹对本案例为可选项。

```bash
test -d "$WORK_DIR/episode-data"
test -d "$WORK_DIR/.uenv-verl/output"
find "$WORK_DIR/episode-data" -type f -print -quit | grep -q .
find "$WORK_DIR/.uenv-verl/output" -mindepth 1 -print -quit | grep -q .
echo 'code training artifacts present'
```

训练日志还应证明：`entry_point`、测试代码和超时传到 UEnv Worker；每条 rollout 返回代码 action、测试结果、数值 reward 和可训练 token/mask；完成 20 次更新并保存产物。代码测试不通过是有效业务低分，沙箱或模型不可达是基础设施失败。

## 替换为自己的任务

| 目标 | 修改 |
|---|---|
| 模型 | `MODEL_DIR` |
| 函数与测试 | JSONL 的 question、entry point、ground truth、test code |
| 测试预算 | timeout、测试数量与 UEnv Worker 资源上限 |
| 训练规模 | steps、rollouts、batch、GPU 与框架配置 |
| 复现 | 数据版本、依赖、镜像 digest 与唯一 `WORK_DIR` |

使用真实 DSCodeBench 时，先按许可取得并转换任务，固定 split、依赖和测试版本；不能把本例 `add` 任务当作 DSCodeBench 数据。

## 失败定位

| 现象 | 处理 |
|---|---|
| 生成文本不能执行 | 统一代码提取规则，检查 Markdown 与语言标记 |
| reward 长期为 0 | 核对入口、测试、依赖与模型输出 |
| 执行卡死 | 检查环境超时、子进程限制和 UEnv Worker 回收 |
| 测试失败导致训练中断 | 检查失败分类；业务测试失败应返回 completed 加低 reward |
| UEnv Worker 主机受影响 | 停止训练并修复隔离后再继续 |
