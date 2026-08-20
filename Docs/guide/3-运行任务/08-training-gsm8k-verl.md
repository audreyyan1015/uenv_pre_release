# 数学问答

## 任务与数据性质

本案例使用 `qa` 环境返回数学问答 reward，并让当前发布支持的强化学习 runner 完成 20 次模型更新。runner 当前基于 VeRL v0.7.1 和 GRPO，属于实现元信息；案例流程与具体 runner 解耦。

输入文件只有两条仓库自拟的 GSM8K 风格问题，仅用于演示训练字段与端到端数据流。

| 项目 | 本案例取值 |
|---|---|
| 环境 / 路由 | `qa` / `gsm8k` |
| 输入真源 | `examples/cases/training/qa-gsm8k.jsonl` |
| 样本数 | 2 |
| 更新步数 / rollout | 20 / 每样本 4 |
| 当前实现 | VeRL v0.7.1、GRPO |

## 执行主机

在有 NVIDIA GPU、模型目录和容器运行时的训练主机执行。环境 rollout 由 UEnv Worker 执行，模型更新与 checkpoint 保存在 GPU 主机。

## 前置检查与变量

先完成[强化学习训练指南](./07-post-training.md)的 UEnv、GPU 和网络检查，再设置实际模型路径。生产复现应把浮动镜像标签替换为团队批准的 digest。

```bash
export UENV_SERVER_ENDPOINT='127.0.0.1:50051'
export UENV_RELEASE_ROOT='/opt/uenv/current'
export MODEL_DIR='/absolute/path/to/huggingface-model'
export TRAIN_IMAGE='docker.io/verlai/verl:vllm017.latest'
export INPUT="$UENV_RELEASE_ROOT/examples/cases/training/qa-gsm8k.jsonl"
export TRAIN_CONFIG="$UENV_RELEASE_ROOT/examples/cases/training/verl-grpo-overrides.conf"
export RUN_ID="math-train-$(date +%Y%m%d-%H%M%S)"
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

多机部署替换 `UENV_SERVER_ENDPOINT`，并按通用训练指南增加 UEnv Worker 可达的 model gateway 地址。不要使用对远程 UEnv Worker 无效的 loopback。

## 执行

```bash
uenv train run-task \
  --model "$MODEL_DIR" \
  --work-dir "$WORK_DIR" \
  --uenv-endpoint "$UENV_SERVER_ENDPOINT" \
  --env-type qa \
  --dataset gsm8k \
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

`--steps 20`、`--rollouts 4` 和 batch size 2 只定义本案例规模，可按需要调整。

## 预期结果与验收

当前 runner 产生：

```text
WORK_DIR/
  episode-data/            # 转换后的训练数据
  .uenv-verl/output/       # 指标与 checkpoint
```

先验收目录和非空产物：

```bash
test -d "$WORK_DIR/episode-data"
test -d "$WORK_DIR/.uenv-verl/output"
find "$WORK_DIR/episode-data" -type f -print -quit | grep -q .
find "$WORK_DIR/.uenv-verl/output" -mindepth 1 -print -quit | grep -q .
echo 'math training artifacts present'
```

训练日志还必须证明：两条输入进入转换数据；每个参与更新的 rollout 有等长 response IDs/mask 与数值 reward；完成 20 个计划更新；第 20 步按示例配置保存；失败 Episode 没有被无提示地转换成普通低 reward。

## 替换为自己的数据与规模

| 目标 | 修改 |
|---|---|
| 模型 | `MODEL_DIR` |
| 自有数据 | `INPUT`，保持 QA 字段契约并记录数据版本/split |
| 训练规模 | `--steps`、`--rollouts`、`--train-batch-size` |
| GPU/并行 | `--gpus` 与框架并行配置一起调整 |
| 超参数 | `TRAIN_CONFIG` 和可重复的 `--set KEY=VALUE` |
| 复现镜像 | 用不可变 digest 替换 `TRAIN_IMAGE` |

## 失败定位

| 现象 | 处理 |
|---|---|
| UEnv Worker 无法访问训练模型 | 设置 UEnv Worker 可达的 gateway public URL/bind 并检查 18080/TCP |
| 数据转换失败 | 核对每行与命令的 env、dataset、max_steps |
| response token 缺失 | 检查 UEnv Worker 模型响应与接入 trace，不只看最终文本 |
| GPU OOM | 降低 batch/rollout/上下文或调整并行，保持整除关系 |
| 产物混入旧运行 | 每轮使用新的 `RUN_ID` / `WORK_DIR` |
