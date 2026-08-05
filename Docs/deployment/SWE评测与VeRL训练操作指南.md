# UEnv SWE 使用入口：先准备环境，再选择评测或训练

UEnv 基础服务安装成功，只表示 Adapter Core 和 Worker 已经运行。要真正执行 SWE 任务，还需要启用
SWE Runtime、准备容器镜像，并连接模型。

本文只完成两件事：

1. 帮你判断应该阅读哪本手册；
2. 完成评测与训练共用的 UEnv 主机准备。

之后不要继续在两条路径之间来回跳转：

- 要衡量一个模型能否解决 SWE 问题，阅读 [SWE 评测操作指南](./SWE评测操作指南.md)；
- 要把 UEnv 环境接入 VeRL 后训练，阅读 [VeRL SWE 训练操作指南](./VeRL-SWE训练操作指南.md)。

构建 release 和安装基础服务的方法见
[UEnv 全新服务器部署指南](./UEnv全新服务器部署指南.md)。

## 1. 选择使用方式

| 你的目标 | UEnv 主机 | 模型位置 | 后续手册 |
|---|---|---|---|
| 用火山引擎方舟 API 评测 | 无 GPU 的 CPU 主机即可 | 方舟推理接入点 | SWE 评测操作指南 |
| 用本地模型服务评测 | CPU 或 GPU 主机 | 同机或另一台 GPU 主机上的 OpenAI-compatible API | SWE 评测操作指南 |
| UEnv 和 VeRL 在同一台机器训练 | 一台 GPU 主机 | 该主机的本地模型目录 | VeRL SWE 训练操作指南的单机流程 |
| UEnv 在 CPU 主机、VeRL 在 GPU 主机 | 一台无 GPU 的 CPU 主机 | 另一台 GPU 主机的本地模型目录 | VeRL SWE 训练操作指南的双机流程 |

建议第一次先使用方舟 API 评测一个实例。它不需要本地 GPU，最容易确认以下链路是否完整：

```text
模型生成操作 → UEnv 创建环境容器 → OpenHands 操作代码 → 容器运行测试 → 返回 reward
```

## 2. 先分清数据、镜像和模型

这些资源互不相同，不能互相替代。

| 资源 | 作用 | release 是否包含 | 准备方式 |
|---|---|---:|---|
| UEnv 程序 | 调度 Episode、管理 Worker 和环境 | 是 | `install.sh` 安装 |
| 任务清单（catalog） | 保存实例 ID、题目、测试和镜像名称等元数据 | 是，但只有少量入门实例 | release 提供 |
| SWE 环境镜像 | 提供代码仓库、依赖和测试环境 | 否 | 运行脚本自动拉取，或用户提前拉取 |
| OpenHands | 调用模型并在环境中操作代码 | 否 | 评测安装器或训练准备命令下载 |
| 本地模型权重 | 本地评测或 VeRL 训练使用 | 否 | 用户自行准备 |
| VeRL 源码 | 后训练框架 | 否 | 训练准备命令下载固定版本 |
| VeRL CUDA 镜像 | 提供 CUDA、PyTorch、vLLM 等训练依赖 | 否 | GPU 主机拉取 |

release 内置：

- `share/swe/verified.json`：10 条 SWE-bench Verified 入门评测元数据；
- `share/swe/smith-example.json`：5 条 SWE-smith 入门训练元数据；
- 不包含完整 SWE-bench 或 SWE-smith 数据集；
- 不包含容器镜像、模型权重或 VeRL CUDA 镜像。

默认示例需要的镜像是：

```text
评测环境：swebench/sweb.eval.x86_64.astropy_1776_astropy-7166:latest
训练环境：jyangballin/swesmith.x86_64.oauthlib_1776_oauthlib.1fd52536:latest
VeRL 环境：docker.io/verlai/verl:vllm017.latest
```

`--swe-image-policy allow_public` 只是允许 Worker 按需拉取镜像，不会在安装 UEnv 时下载全部镜像。

## 3. 准备 UEnv 主机

本节在运行 UEnv 服务的主机执行：

- 单机训练时，它就是 GPU 主机；
- 双机训练时，它是 CPU 主机；
- API 评测时，它可以是没有 GPU 的 CPU 主机。

### 3.1 安装 Docker

全新 Ubuntu 主机建议使用 Docker：

```bash
sudo apt-get update
sudo apt-get install -y docker.io
sudo systemctl enable --now docker
sudo docker info
```

Podman 也可以使用，但必须提前为 `uenv` 系统用户配置好 rootless Podman。第一次验证建议使用 Docker。

### 3.2 安装或升级为 SWE 配置

以下假设新 release 已复制到 `/home/uenv-trial`：

```bash
cd /home/uenv-trial
sha256sum -c uenv-linux-x86_64.tar.gz.sha256
sudo cp -a /etc/uenv "/etc/uenv.backup.$(date +%Y%m%d-%H%M%S)" 2>/dev/null || true
sudo bash install.sh --bundle ./uenv-linux-x86_64.tar.gz \
  --enable-swe \
  --swe-image-policy allow_public
```

已经部署过基础版 UEnv 的机器也必须执行这一步。基础安装没有启用 Runtime Gateway，也没有给 Worker 注册
SWE 类型。

安装器会：

- 安装或升级 Adapter Core 和 Worker；
- 启用 Worker 的 SWE Runtime；
- 在 `127.0.0.1:28999` 启动 Runtime Gateway；
- 生成 Server 与 Worker 共用的 Gateway 密钥；
- 注册 release 内的两份任务清单；
- 让系统用户 `uenv` 可以访问 Docker。

安装器不会安装 OpenHands、拉取实例镜像或下载模型。

### 3.3 完成共同验收

下面几项必须全部成功：

```bash
uenv version
sudo -u uenv uenv doctor
uenv status
sudo systemctl is-active uenv-adapter-core.service uenv-worker.service
curl -fsS http://127.0.0.1:28999/runtime/v1/health
sudo runuser -u uenv -- docker info >/dev/null
test -s /opt/uenv/current/share/swe/verified.json
test -s /opt/uenv/current/share/swe/smith-example.json
echo "UEnv SWE 基础环境已就绪"
```

如果这里失败，先查看：

```bash
uenv logs server -n 100
uenv logs worker -n 100
sudo journalctl -u uenv-adapter-core.service -n 200 --no-pager
sudo journalctl -u uenv-worker.service -n 200 --no-pager
```

## 4. 镜像什么时候下载

基础安装不会下载环境镜像。后续行为如下：

| 命令 | 缺少镜像时的行为 |
|---|---|
| `evaluate.sh` | 拉取所选 SWE-bench Verified 实例镜像 |
| `train_verl.sh prepare-uenv` | 拉取内置训练实例所需的 SWE-smith 镜像 |
| `train_verl.sh prepare-data` | 主机缺少 `pandas/pyarrow` 时使用 VeRL 镜像转换数据 |
| `train_verl.sh run` | 使用 VeRL CUDA 镜像；本地没有时由 Docker 拉取 |

为了提前发现镜像仓库、权限或磁盘问题，也可以手动下载。

评测镜像：

```bash
sudo runuser -u uenv -- docker pull \
  swebench/sweb.eval.x86_64.astropy_1776_astropy-7166:latest
```

训练环境镜像：

```bash
sudo runuser -u uenv -- docker pull \
  jyangballin/swesmith.x86_64.oauthlib_1776_oauthlib.1fd52536:latest
```

VeRL 镜像只在 GPU 主机下载：

```bash
docker pull docker.io/verlai/verl:vllm017.latest
```

检查磁盘：

```bash
df -h /var/lib/docker
sudo docker system df
```

SWE 镜像可能很大，不要在第一次试用时下载全部 benchmark 镜像。

## 5. 继续阅读哪一本手册

### 进行模型评测

阅读 [SWE 评测操作指南](./SWE评测操作指南.md)。它分别给出：

- 方舟 API 评测；
- 本地 OpenAI-compatible 模型评测；
- OpenHands 安装；
- 模型连通性检查；
- 结果和 reward 判断；
- 切换实例及完整数据集边界。

### 接入 VeRL 后训练

阅读 [VeRL SWE 训练操作指南](./VeRL-SWE训练操作指南.md)。它分别给出：

- 单机 GPU 流程；
- CPU/UEnv + GPU/VeRL 双机流程；
- NVIDIA 容器环境检查；
- 模型、任务数据和三类镜像分别放在哪里；
- Agent 注册和双向端口检查；
- 一步训练的成功标准及当前示例不保存 checkpoint 的边界。

## 6. 完整数据集与 Hub 的边界

当前交付是“少量内置实例的端到端入口”，不是完整数据集下载器。

- SWE-bench Verified 官方数据集：<https://huggingface.co/datasets/SWE-bench/SWE-bench_Verified>
- SWE-smith 官方数据集：<https://huggingface.co/datasets/SWE-bench/SWE-smith>
- SWE-smith 环境镜像说明：<https://github.com/SWE-bench/SWE-smith-envs>

下载官方数据集后，不能把 Hugging Face 原始目录直接传给 `--catalog`。必须先转换为 UEnv catalog JSON，
并保证其中的实例 ID、测试信息和环境镜像能够对应。当前 release 尚未提供完整数据集的一键下载和转换。

单 Worker 使用本地 catalog 即可，不需要 UEnv Hub。只有需要统一管理大量环境版本或向多个 Worker 分发时，
才需要 Hub。把 UEnv 和 VeRL 分到两台机器不等于多个 Worker，也不需要 Hub。

## 7. 离线使用要额外准备什么

`--offline` 只禁止脚本访问镜像仓库，不代表 release 已包含运行所需的资源。完全离线前还要转移：

- 选中实例的 SWE 环境镜像；
- OpenHands 安装目录和 Python 依赖；
- VeRL 源码及 CUDA 镜像（训练时）；
- 本地模型权重（不使用远程 API 时）。

Docker 镜像可用 `docker save` 和 `docker load` 转移。导入后用 Worker 用户确认：

```bash
sudo runuser -u uenv -- docker image inspect 'your-swe-image:tag'
```

仅复制 UEnv release 压缩包，无法在完全断网的机器上从零完成评测或训练。
