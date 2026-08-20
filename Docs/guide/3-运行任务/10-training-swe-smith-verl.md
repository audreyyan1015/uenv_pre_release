# 代码修复

## 任务与数据性质

本案例在一个 Smith catalog 实例上产生多步代码修改、测试 reward 与完整轨迹，并让当前发布 runner 完成 20 次模型更新。当前实现使用 VeRL v0.7.1、GRPO 和固定版本的 OpenHands Agent；这些是实现元信息，不进入案例标题或通用训练流程。

| 项目 | 本案例取值 |
|---|---|
| variant | `smith` |
| catalog | `config/swe/smith-sample-catalog.json` |
| 实例 | `oauthlib__oauthlib.1fd52536.combine_file__09vlzwgc` |
| 更新步数 / rollout | 20 / 每样本 4 |

catalog 行保存仓库、base commit、问题、测试和 `image_cache_key`；runner 将所选行转换为强化学习训练数据。发布包自带的 Smith catalog 是 5 条冒烟样例；catalog 的生成方式和字段说明见[代码修复评测](./06-evaluation-swe-verified.md#输入与-catalog)，训练入口当前只支持 `smith` variant。

## 执行主机

训练命令在有 NVIDIA GPU、代码模型和容器运行时的 GPU 主机执行。UEnv Worker 必须启用 SWE Runtime、Runtime Gateway 与 Agent 运行服务（启用方法见[代码修复](./06-evaluation-swe-verified.md#启用-swe-runtime)），并准备实例镜像。

## 前置检查与变量

```bash
export UENV_SERVER_ENDPOINT='127.0.0.1:50051'
export UENV_RELEASE_ROOT='/opt/uenv/current'
export MODEL_DIR='/absolute/path/to/code-model'
export TRAIN_IMAGE='docker.io/verlai/verl:vllm017.latest'
export SWE_CATALOG="$UENV_RELEASE_ROOT/share/swe/smith-sample-catalog.json"
export INSTANCE_ID='oauthlib__oauthlib.1fd52536.combine_file__09vlzwgc'
export TRAIN_CONFIG="$UENV_RELEASE_ROOT/examples/cases/training/verl-grpo-overrides.conf"
export RUN_ID="software-repair-train-$(date +%Y%m%d-%H%M%S)"
export WORK_DIR="$PWD/uenv-runs/$RUN_ID"
```

在 GPU 主机检查：

```bash
nvidia-smi -L
docker info >/dev/null
test -d "$MODEL_DIR"
test -r "$SWE_CATALOG"
test -r "$TRAIN_CONFIG"
jq -e --arg id "$INSTANCE_ID" 'has($id)' "$SWE_CATALOG" >/dev/null
test ! -e "$WORK_DIR"
mkdir -p "$(dirname "$WORK_DIR")"
docker image inspect "$TRAIN_IMAGE" >/dev/null || docker pull "$TRAIN_IMAGE"
```

从源码工作区运行时，先把 `REPO_ROOT` 设为仓库根目录的绝对路径，再把 catalog 单独设为 `SWE_CATALOG="$REPO_ROOT/config/swe/smith-sample-catalog.json"`；安装包使用上面的 `share/swe/` 路径。

在 SWE UEnv Worker 检查服务、容器权限与实例镜像策略：

```bash
sudo systemctl is-active uenv-worker.service uenv-swe-agent.service
sudo -u uenv docker info >/dev/null
```

多机部署把 `UENV_SERVER_ENDPOINT` 换成 UEnv Server 内网地址，并设置 UEnv Worker 可达的 `--gateway-public-url` / `--gateway-bind`。GPU 主机到 UEnv Server 50051/TCP、UEnv Worker 到 GPU model gateway 18080/TCP 必须同时成立。

## 执行

```bash
uenv train run-swe \
  --model "$MODEL_DIR" \
  --work-dir "$WORK_DIR" \
  --uenv-endpoint "$UENV_SERVER_ENDPOINT" \
  --catalog "$SWE_CATALOG" \
  --benchmark-variant smith \
  --instance "$INSTANCE_ID" \
  --max-iterations 30 \
  --gpus 1 \
  --steps 20 \
  --rollouts 4 \
  --train-batch-size 2 \
  --runtime docker \
  --image "$TRAIN_IMAGE" \
  --verl-config "$TRAIN_CONFIG"
```

`--max-iterations` 限制一条 Agent 轨迹的迭代数。`TRAIN_IMAGE` 是 GPU 主机的训练镜像；Smith 实例镜像由 UEnv Worker 根据 catalog 选择，两者各自独立。

## 预期结果与验收

```text
WORK_DIR/
  swe-data/                 # catalog 转换后的训练数据与 metadata
  .uenv-verl/output/        # 指标与 checkpoint
```

```bash
test -d "$WORK_DIR/swe-data"
test -d "$WORK_DIR/.uenv-verl/output"
find "$WORK_DIR/swe-data" -type f -print -quit | grep -q .
find "$WORK_DIR/.uenv-verl/output" -mindepth 1 -print -quit | grep -q .
echo 'software repair training artifacts present'
```

训练日志还必须证明：转换数据只含所选 Smith 实例；每条 rollout 使用独立 session/container；可训练结果含等长 response IDs/mask、reward 和 trajectory ID；完成 20 次更新并保存产物。`resolved=false` 是有效业务结果，模型、Gateway 或容器失败必须单独记录。

启用集中轨迹后，从实例产物的 `trajectory_ref.json` 取得实际 training run ID 和 trajectory ID，再按[轨迹采集指南](./12-trajectory.md)查询；不要假设本地 `RUN_ID` 与 bundle `run_id` 相同。

## 替换实例与规模

| 目标 | 修改 |
|---|---|
| 多个明确实例 | 重复 `--instance ID` |
| catalog 前 N 个实例 | 用 `--limit N` 替代全部 `--instance`；两者不可同时使用 |
| Agent 预算 | `--max-iterations` |
| 环境并发 | 接入代码、UEnv Server、Agent、Gateway 与 UEnv Worker 容量共同限制 |
| 模型/训练规模 | model、steps、rollouts、batch、GPU 与框架配置 |
| 复现 | catalog、实例镜像、训练镜像 digest 与唯一工作目录 |

## 失败定位

| 现象 | 处理 |
|---|---|
| 拒绝非 Smith variant | 当前发布的 SWE 训练入口只支持 `smith` |
| 找不到实例或镜像 | 核对 catalog ID、image cache key 和镜像命名空间 |
| response trace 缺失 | 检查 Agent 模型调用的 token/logprob 记录；不要回退为不可验证文本 |
| reward 全 0 | 分别检查 patch、测试、上下文长度和基础设施错误分类 |
| 并发残留容器 | 降低并发并修复 session/timeout/cleanup |
