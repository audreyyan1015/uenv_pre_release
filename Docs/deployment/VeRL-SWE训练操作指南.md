# UEnv + VeRL SWE 训练操作指南

本文只讲如何把 UEnv SWE 环境接入 VeRL 后训练，不讲模型评测。

当前示例的目标是验证下面这条链路：

```text
VeRL 生成 rollout → UEnv 创建 Episode → OpenHands 操作 SWE 环境
→ Worker 执行测试并返回 reward/轨迹 → VeRL 完成一个训练 step
```

它不是一套生产训练配置。默认只取 1 条任务、生成 2 条 rollout、训练 1 步，而且不保存模型 checkpoint。

## 1. 先选择单机还是双机

| 方式 | UEnv 放置位置 | VeRL 和模型放置位置 | 适用场景 |
|---|---|---|---|
| 单机 | GPU 主机 | 同一台 GPU 主机 | 第一次验证，部署最简单 |
| 双机 | 无 GPU 的 CPU 主机 | 独立 GPU 主机 | 把 SWE 容器执行与训练资源分离 |

双机方式不是 VeRL 多训练节点。当前脚本始终使用 `trainer.nnodes=1`，训练 GPU 全部位于同一台 GPU 主机。

## 2. 训练需要哪些资源

| 资源 | 单机位置 | 双机位置 | 是否自动准备 |
|---|---|---|---:|
| UEnv Adapter Core、Worker、Runtime Gateway | GPU 主机 | CPU 主机 | `install.sh --enable-swe` |
| OpenHands Agent | GPU 主机 | CPU 主机 | `prepare-uenv` |
| SWE-smith 环境镜像 | GPU 主机 | CPU 主机 | `prepare-uenv` 按需拉取 |
| VeRL 固定版本源码 | GPU 主机 | GPU 主机 | `prepare-gpu` 下载 |
| UEnv Bridge wheel 和配置 | GPU 主机 | GPU 主机 | 从 release 提取 |
| VeRL CUDA 镜像 | GPU 主机 | GPU 主机 | Docker 首次使用时拉取 |
| 训练 Parquet | GPU 主机 | GPU 主机 | `prepare-data` 从 catalog 转换 |
| Hugging Face 模型目录 | GPU 主机 | GPU 主机 | 用户自行下载 |

双机时要特别注意：SWE 环境镜像只放在 CPU/UEnv 主机；模型权重、VeRL 镜像和 Parquet 只放在 GPU
主机。两边都复制同一种资源既浪费空间，也不能解决链路问题。

## 3. 当前示例的能力边界

release 中的 `share/swe/smith-example.json` 只有 5 条 SWE-smith 元数据。本文使用 `--limit 1`，因此：

- `prepare-data` 只选择 1 条任务；
- 示例生成的 `train.parquet` 与 `test.parquet` 内容相同，只用于满足 VeRL 数据接口；
- `--rollouts 2` 只生成两条轨迹；
- `--steps 1` 不能证明模型训练有效；
- 脚本默认 `trainer.save_freq=-1`，不会保存可继续使用的模型 checkpoint；
- 成功标准是 Episode、reward、轨迹和 optimizer step 全部跑通，不是模型能力提升。

不要把 Verified 评测 catalog 传给 `prepare-data`。数据转换脚本只接受包含 `benchmark_variant=smith` 和
`image_cache_key` 的 SWE-smith catalog。

## 4. GPU 主机共同前置条件

无论单机还是双机，GPU 主机都需要：

- NVIDIA GPU 和已安装的驱动；
- Docker；
- NVIDIA Container Toolkit；
- 当前普通用户可以运行 GPU 容器；
- Python 3.10 或更高版本、`git`、`tar`、`sha256sum`；
- 能访问 GitHub、Python 包源、Docker Hub，以及模型下载源；
- 足够的显存和磁盘。

先检查宿主机驱动：

```bash
nvidia-smi
```

如果尚未安装 NVIDIA Container Toolkit，按
<https://docs.nvidia.com/datacenter/cloud-native/container-toolkit/latest/install-guide.html>
完成与当前 Linux 发行版匹配的安装，然后配置 Docker：

```bash
sudo nvidia-ctk runtime configure --runtime=docker
sudo systemctl restart docker
```

让当前登录用户可以使用 Docker：

```bash
sudo usermod -aG docker "$USER"
```

退出 SSH 并重新登录，然后以普通用户验证：

```bash
docker info
docker pull docker.io/verlai/verl:vllm017.latest
docker run --rm --gpus all --entrypoint nvidia-smi \
  docker.io/verlai/verl:vllm017.latest
```

如果最后一条命令失败，不要继续运行训练脚本；先修复驱动、Toolkit 或 Docker 权限。

## 5. 准备本地模型目录

VeRL 要更新模型参数，所以 `--model` 必须是 GPU 主机上的完整 Hugging Face 模型目录，不能填写：

- 模型 API URL；
- 方舟接入点 ID；
- 只有名称但尚未下载的 Hub 模型 ID。

目录至少需要模型配置、tokenizer 和权重文件，并且 Docker 能读取。使用 Hugging Face Hub 时，可以参考
<https://huggingface.co/docs/huggingface_hub/guides/cli> 下载到固定目录。例如：

```bash
python3 -m pip install --user -U huggingface_hub
export PATH="$HOME/.local/bin:$PATH"

export MODEL_ID='Qwen/Qwen2.5-Coder-1.5B-Instruct'
export MODEL_DIR="$HOME/models/Qwen2.5-Coder-1.5B-Instruct"
mkdir -p "$MODEL_DIR"
hf download "$MODEL_ID" --local-dir "$MODEL_DIR"
```

上面的模型只用于说明下载方法，不代表它已经在你的 GPU、驱动和本 release 上完成训练效果验收。正式使用时
应替换成团队确认兼容的模型和固定 revision。

检查目录：

```bash
test -s "$MODEL_DIR/config.json"
test -n "$(find "$MODEL_DIR" -maxdepth 2 -type f \
  \( -name '*.safetensors' -o -name 'pytorch_model*.bin' \) -print -quit)"
```

即使模型参数量较小，GRPO 仍需要同时容纳 rollout、actor、optimizer 等开销。显存不足时先更换更小模型；
增加 `--gpus` 还会受到 batch 整除和模型并行配置约束，不是无条件解决方案。

## 6. 单机流程：UEnv 与 VeRL 位于同一台 GPU 主机

### 6.1 启用 UEnv SWE

先按 [UEnv SWE 使用入口](./SWE评测与VeRL训练操作指南.md) 在这台 GPU 主机完成：

```bash
cd /home/uenv-trial
sha256sum -c uenv-linux-x86_64.tar.gz.sha256
sudo bash install.sh --bundle ./uenv-linux-x86_64.tar.gz \
  --enable-swe \
  --swe-image-policy allow_public
```

确认 Adapter Core、Worker、Gateway 和 `uenv` 用户 Docker 权限全部通过后再继续。

### 6.2 准备 OpenHands Agent 和 SWE 训练镜像

以 root 执行一次：

```bash
sudo bash /opt/uenv/current/examples/swe/train_verl.sh prepare-uenv \
  --uenv-release /opt/uenv/current
```

该命令会：

1. 安装并验证固定版本 OpenHands；
2. 拉取内置训练任务使用的 SWE-smith 环境镜像；
3. 配置并启动 `uenv-swe-agent.service`；
4. 等待 `openhands-default` Agent 注册成功。

检查：

```bash
sudo systemctl is-active uenv-swe-agent.service
curl -fsS http://127.0.0.1:50052/agents
```

返回中应有非 stale 的 `openhands-default`。

### 6.3 准备 VeRL 和 Bridge

后续都以可以使用 Docker/GPU 的普通用户执行：

```bash
mkdir -p "$HOME/uenv-verl-demo"
cd "$HOME/uenv-verl-demo"

bash /opt/uenv/current/examples/swe/train_verl.sh prepare-gpu \
  --uenv-release /opt/uenv/current \
  --work-dir "$PWD/.uenv-verl"
```

`prepare-gpu` 会从官方仓库浅克隆 VeRL `v0.7.1` 的固定提交
`bec9ef74768dd201881cd4e54cd0385e87caae27`，并从当前 release 复制 Bridge wheel、Agent Loop 配置和启动器。
它不会修改 VeRL 源码。

### 6.4 准备一条训练数据

```bash
bash /opt/uenv/current/examples/swe/train_verl.sh prepare-data \
  --catalog /opt/uenv/current/share/swe/smith-example.json \
  --output-dir "$PWD/swe-data" \
  --limit 1

cat "$PWD/swe-data/dataset_summary.json"
ls -lh "$PWD/swe-data/train.parquet" "$PWD/swe-data/test.parquet"
```

如果宿主机没有 `pandas` 和 `pyarrow`，脚本会使用 VeRL 容器完成转换；这可能触发 VeRL 镜像下载。

### 6.5 先预检，再训练一步

确认第 5 节设置的 `MODEL_DIR` 仍有效：

```bash
test -s "$MODEL_DIR/config.json"
```

先打印将要执行的容器命令：

```bash
bash /opt/uenv/current/examples/swe/train_verl.sh run \
  --uenv-release /opt/uenv/current \
  --work-dir "$PWD/.uenv-verl" \
  --model "$MODEL_DIR" \
  --data "$PWD/swe-data" \
  --gpus 1 \
  --steps 1 \
  --rollouts 2 \
  --train-batch-size 1 \
  --dry-run
```

`--dry-run` 不连接 UEnv，也不验证 GPU 显存。检查路径和参数后，去掉该参数真正运行：

```bash
bash /opt/uenv/current/examples/swe/train_verl.sh run \
  --uenv-release /opt/uenv/current \
  --work-dir "$PWD/.uenv-verl" \
  --model "$MODEL_DIR" \
  --data "$PWD/swe-data" \
  --gpus 1 \
  --steps 1 \
  --rollouts 2 \
  --train-batch-size 1
```

## 7. 双机流程：CPU/UEnv 主机 + GPU/VeRL 主机

以下示例约定：

```text
CPU/UEnv 主机：10.0.0.10
GPU/VeRL 主机：10.0.0.20
```

### 7.1 两台机器各自需要的网络

CPU/UEnv 主机需要访问：

- GitHub 和 Python 包源，用于安装 OpenHands；
- Docker Hub，用于拉取 SWE 环境镜像。

GPU/VeRL 主机需要访问：

- GitHub，用于下载固定版本 VeRL；
- Python 包源，用于在训练容器安装 Bridge 依赖；
- Docker Hub，用于拉取 VeRL CUDA 镜像；
- Hugging Face 或团队模型仓库，用于准备模型权重。

release 压缩包不包含这些外部资源。

### 7.2 准备 CPU/UEnv 主机

先按 [UEnv SWE 使用入口](./SWE评测与VeRL训练操作指南.md) 安装 Docker 并以 `--enable-swe` 部署。
随后执行：

```bash
sudo bash /opt/uenv/current/examples/swe/train_verl.sh prepare-uenv \
  --uenv-release /opt/uenv/current

sudo systemctl is-active uenv-adapter-core.service \
  uenv-worker.service \
  uenv-swe-agent.service
curl -fsS http://127.0.0.1:50052/agents
sudo ss -lntp | grep 50051
```

CPU 主机不需要 GPU。`prepare-uenv` 会在这里安装 OpenHands 并拉取 SWE-smith 环境镜像。

### 7.3 准备 GPU/VeRL 主机

先完成第 4、5 节的 GPU 容器和模型检查。GPU 主机不安装 UEnv systemd 服务，但需要同一个 release 包中的
Bridge、配置和示例入口。

将下面两个文件复制到 GPU 主机：

```text
uenv-linux-x86_64.tar.gz
uenv-linux-x86_64.tar.gz.sha256
```

然后以普通用户执行：

```bash
mkdir -p "$HOME/uenv-gpu"
cd "$HOME/uenv-gpu"
sha256sum -c uenv-linux-x86_64.tar.gz.sha256

export UENV_BUNDLE="$PWD/uenv-linux-x86_64.tar.gz"
mkdir -p "$PWD/tools"
tar -xzf "$UENV_BUNDLE" --wildcards --strip-components=3 -C "$PWD/tools" \
  'uenv-*/examples/swe/train_verl.sh' \
  'uenv-*/examples/swe/prepare_verl_data.py' \
  'uenv-*/share/swe/smith-example.json'

bash "$PWD/tools/train_verl.sh" prepare-gpu \
  --bundle "$UENV_BUNDLE" \
  --work-dir "$PWD/.uenv-verl"

bash "$PWD/tools/train_verl.sh" prepare-data \
  --catalog "$PWD/tools/smith-example.json" \
  --output-dir "$PWD/swe-data" \
  --limit 1

cat "$PWD/swe-data/dataset_summary.json"
```

### 7.4 配置两条网络连接

| 访问方向 | TCP 端口 | 用途 | 开放范围 |
|---|---:|---|---|
| GPU 主机 → CPU 主机 | 50051 | Bridge 向 Adapter Core 提交 Episode | 只允许 GPU 主机内网 IP |
| CPU 主机 → GPU 主机 | 18080 | OpenHands 调用当前训练模型 | 只允许 CPU 主机内网 IP |

Runtime Gateway `28999` 继续只监听 CPU 主机的 `127.0.0.1`，不要对外开放。优先使用内网、VPN 或受控
隧道，不要把 `50051` 和 `18080` 暴露给公网全部来源。

在 GPU 主机检查 Adapter Core：

```bash
nc -vz 10.0.0.10 50051
```

`18080` 由训练进程启动，训练前没有监听是正常现象。

### 7.5 从 GPU 主机启动训练

```bash
cd "$HOME/uenv-gpu"
export UENV_BUNDLE="$PWD/uenv-linux-x86_64.tar.gz"
export UENV_HOST='10.0.0.10'
export GPU_HOST='10.0.0.20'
export MODEL_DIR='/absolute/path/to/huggingface-model'

test -s "$MODEL_DIR/config.json"

bash "$PWD/tools/train_verl.sh" run \
  --bundle "$UENV_BUNDLE" \
  --work-dir "$PWD/.uenv-verl" \
  --model "$MODEL_DIR" \
  --data "$PWD/swe-data" \
  --uenv-endpoint "$UENV_HOST:50051" \
  --gateway-public-url "http://$GPU_HOST:18080/v1" \
  --gpus 1 \
  --steps 1 \
  --rollouts 2 \
  --train-batch-size 1
```

`--gateway-public-url` 中的 “public” 表示向 CPU/UEnv 主机公布的可达地址，不是公网地址。这里必须填写
CPU 主机能够访问的 GPU 内网 IP，不能写 `127.0.0.1`。

训练启动后，如果 CPU 侧一直无法调用模型，在 CPU 主机检查：

```bash
curl -v http://10.0.0.20:18080/v1/models
```

## 8. 怎样判断训练链路跑通

训练命令必须正常退出，并且 GPU 主机上存在：

```bash
test -s "$PWD/.uenv-verl/output/agent-loop-requests.jsonl"
test -s "$PWD/.uenv-verl/output/agent-loop-results.jsonl"
tail -n 1 "$PWD/.uenv-verl/output/agent-loop-results.jsonl"
ls -lh "$PWD/.uenv-verl/output"
```

UEnv 主机应同时满足：

```bash
uenv status
sudo systemctl is-active uenv-swe-agent.service
curl -fsS http://127.0.0.1:50052/agents
```

默认示例不保存 checkpoint。确认链路以后，如果确实需要保存并且其他训练参数已经审查，可以增加：

```text
--set trainer.save_freq=1
```

不要只改保存频率就把一步验证当作正式训练。正式训练还需要真实 train/validation 划分、足够任务、奖励分布、
显存规划、checkpoint 策略和恢复测试。

## 9. 扩展到真实 SWE-smith 数据

官方数据集见 <https://huggingface.co/datasets/SWE-bench/SWE-smith>，环境镜像说明见
<https://github.com/SWE-bench/SWE-smith-envs>。

当前 release 不提供完整数据集的一键下载和转换。Hugging Face 原始数据不能直接传给 `--catalog`；需要先
转换成 UEnv SWE-smith catalog，并确保每条任务至少具有：

- `instance_id`；
- `problem_statement`；
- `benchmark_variant=smith`；
- 指向可用容器的 `image_cache_key`。

对于已经转换好的 catalog：

```bash
bash train_verl.sh prepare-data \
  --catalog /absolute/path/to/smith-catalog.json \
  --output-dir /absolute/path/to/swe-data \
  --limit 0
```

`--limit 0` 表示选择全部记录。正式训练应自行生成独立的训练集与验证集；当前转换脚本会把同一批记录同时
写入 `train.parquet` 和 `test.parquet`，只适合链路验证。

镜像应按选中的任务逐步准备，不建议一次下载全部。多个 Worker 需要同步相同环境版本时再接入 UEnv Hub；
单 Worker 或 CPU/GPU 双机示例不依赖 Hub。

## 10. VeRL 接入时实际改了什么

用户不需要修改 VeRL 仓库文件。需要手工关注的 Hydra 配置只有两项：

```text
actor_rollout_ref.rollout.agent.default_agent_loop=uenv_agent
actor_rollout_ref.rollout.agent.agent_loop_config_path=/uenv-assets/uenv-agent-loop.yaml
```

但完整链路还依赖脚本自动处理的 Bridge wheel、兼容层、动态模型 Gateway、OpenHands Agent 和 rollout trace。
因此不要只复制两行配置而跳过 `prepare-uenv` 与 `prepare-gpu`。

## 11. 常见问题

### GPU 容器看不到显卡

```bash
nvidia-smi
docker info
docker run --rm --gpus all --entrypoint nvidia-smi \
  docker.io/verlai/verl:vllm017.latest
```

依次检查驱动、NVIDIA Container Toolkit、Docker daemon 和当前用户组权限。

### Agent 没有注册

在 UEnv 主机执行：

```bash
curl -fsS http://127.0.0.1:50052/agents
sudo systemctl status uenv-swe-agent.service --no-pager
sudo journalctl -u uenv-swe-agent.service -n 200 --no-pager
```

### SWE 环境镜像缺失

在 UEnv 主机执行：

```bash
sudo runuser -u uenv -- docker pull \
  jyangballin/swesmith.x86_64.oauthlib_1776_oauthlib.1fd52536:latest
```

### 双机端口不通

- GPU → CPU `50051` 失败：检查 Adapter Core 监听、安全组和 CPU 防火墙；
- CPU → GPU `18080` 失败：确认训练进程已启动、`--gateway-public-url` 使用 GPU 内网 IP，并检查 GPU 防火墙。

### CUDA OOM

先更换更小模型。增加 GPU 数之前，注意脚本要求：

```text
train-batch-size × rollouts ≥ GPU 数，并且能够被 GPU 数整除
```

模型并行参数也可能需要通过 `--set` 调整。

### 查看各侧日志

UEnv 主机：

```bash
uenv logs server -n 100
uenv logs worker -n 100
sudo journalctl -u uenv-swe-agent.service -n 200 --no-pager
```

GPU 主机：查看前台 VeRL/Ray 输出以及 `.uenv-verl/output/agent-loop-*.jsonl`。双机排障时必须明确错误发生在
哪一侧，不能只检查其中一台机器。
