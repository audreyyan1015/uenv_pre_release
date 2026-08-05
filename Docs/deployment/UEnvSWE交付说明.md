# UEnv SWE 交付包替换、构建与服务器试用

本文给负责部署的同事使用，只说明怎样替换代码、构建 release、升级服务器，以及应该按什么顺序验收。
具体评测和训练命令以 release 中的三份使用手册为准。

## 1. 本次交付解决什么问题

本次 release 在 UEnv 基础部署上增加：

- SWE Runtime Gateway；
- SWE-bench Verified 单实例评测脚本；
- 火山引擎方舟 API 和本地 OpenAI-compatible 模型两种评测方式；
- VeRL SWE 单机及 CPU/GPU 双机接入脚本；
- 固定版本 OpenHands 安装器；
- 少量入门任务元数据；
- 分开的评测手册与训练手册。

安装包不包含完整数据集、SWE 环境镜像、模型权重或 VeRL CUDA 镜像。这些资源会由后续脚本按需下载，
或由使用者提前准备。

## 2. 替换构建机源码

交付目录包含两种压缩格式：

- `uenv_pre_release_swe_0.1.1_source.zip`：完整源码，Windows 下载后可直接解压一次；
- `uenv_pre_release_swe_0.1.1_patch.zip`：相对已有仓库的修改和新增文件，Windows 推荐；
- 同名 `.tar.gz`：内容相同，适合直接在 Linux 上使用；
- `SHA256SUMS`：交付文件校验值。

先校验：

```bash
cd /path/to/deliverables
sha256sum -c SHA256SUMS
```

### 方式 A：在新目录使用完整源码（Linux）

```bash
mkdir -p /path/to/uenv-swe-0.1.1
tar -xzf uenv_pre_release_swe_0.1.1_source.tar.gz \
  --strip-components=1 \
  -C /path/to/uenv-swe-0.1.1
cd /path/to/uenv-swe-0.1.1
```

### 方式 B：覆盖已有仓库（Linux）

先确认仓库中没有同事尚未保存的修改，再备份：

```bash
cd /path/to
cp -a uenv_pre_release "uenv_pre_release.backup.$(date +%Y%m%d-%H%M%S)"
tar -tzf /path/to/deliverables/uenv_pre_release_swe_0.1.1_patch.tar.gz
tar -xzf /path/to/deliverables/uenv_pre_release_swe_0.1.1_patch.tar.gz \
  -C /path/to/uenv_pre_release
cd /path/to/uenv_pre_release
```

使用 ZIP 时，在 Linux 构建机执行：

```bash
cd /path/to/uenv_pre_release
unzip -o /path/to/deliverables/uenv_pre_release_swe_0.1.1_patch.zip
```

ZIP 只需解压一次。`.tar.gz` 在部分 Windows 解压工具中会分成 GZip 和 TAR 两层；如果第一次解压后只得到
一个没有扩展名的文件，应将它改名为 `.tar` 后再解压，或直接改用 ZIP 包。

不要把源码压缩包直接解压到服务器的 `/opt/uenv/current`。服务器只能安装构建脚本生成的 release bundle，
否则二进制、systemd unit、Bridge wheel、任务清单和示例脚本会版本不一致。

## 3. 在构建机验证并生成 release

```bash
cd /path/to/uenv_pre_release
bash -n install.sh scripts/build-release.sh examples/swe/*.sh
python3 -m unittest tests.test_installation_assets tests.test_swe_examples -v

source "$HOME/.cargo/env"
./scripts/build-release.sh --version 0.1.1-trial
```

产物：

```text
dist/install.sh
dist/uenv-linux-x86_64.tar.gz
dist/uenv-linux-x86_64.tar.gz.sha256
```

本次必须使用新的 `0.1.1-trial` 版本号。不要用内容不同的新包覆盖已经安装的同版本 release。

把上面三个文件复制到 UEnv 服务器的 `/home/uenv-trial`。

## 4. 升级 UEnv 服务器并启用 SWE

以下以 Ubuntu 和 Docker 为例：

```bash
sudo apt-get update
sudo apt-get install -y docker.io
sudo systemctl enable --now docker
sudo docker info

cd /home/uenv-trial
sha256sum -c uenv-linux-x86_64.tar.gz.sha256
sudo cp -a /etc/uenv "/etc/uenv.backup.$(date +%Y%m%d-%H%M%S)" 2>/dev/null || true
sudo bash install.sh --bundle ./uenv-linux-x86_64.tar.gz \
  --enable-swe \
  --swe-image-policy allow_public
```

该命令只安装 UEnv 并启用 SWE Runtime，不会下载完整数据集、OpenHands、实例镜像或模型。

## 5. 完成基础验收

```bash
uenv version
sudo -u uenv uenv doctor
uenv status
sudo systemctl is-active uenv-adapter-core.service uenv-worker.service
curl -fsS http://127.0.0.1:28999/runtime/v1/health
sudo runuser -u uenv -- docker info >/dev/null
test -s /opt/uenv/current/share/swe/verified.json
test -s /opt/uenv/current/share/swe/smith-example.json
```

预期：

- `uenv version` 显示 `0.1.1-trial`；
- Adapter Core 和 Worker 都是 `active`；
- Runtime Gateway 可访问；
- `uenv` 用户可以使用 Docker；
- 两份入门任务清单存在。

## 6. 推荐的服务器试用顺序

### 第一步：先做 API 评测

如果有方舟接入点，优先用 API 评测。该路径不要求 UEnv 服务器有 GPU，最适合验证 UEnv、OpenHands、
Docker 环境和结果回传是否完整。

阅读：

```text
/opt/uenv/current/share/docs/SWE评测操作指南.md
```

第一次评测会安装 OpenHands 并拉取所选 SWE-bench Verified 环境镜像。

### 第二步：再做本地模型评测

确认 UEnv 主机可以访问模型服务的 `/v1/models`，且 `--model` 与返回的模型 ID 一致。本地模型权重和
模型服务由用户准备，UEnv 不负责下载或启动。

仍按 `SWE评测操作指南.md` 操作。

### 第三步：最后验证 VeRL 接入

训练需要 NVIDIA GPU、NVIDIA Container Toolkit、本地 Hugging Face 模型目录和 VeRL CUDA 镜像。

阅读：

```text
/opt/uenv/current/share/docs/VeRL-SWE训练操作指南.md
```

该手册明确区分单机和双机。默认命令只验证 1 条任务、2 条 rollout 和 1 个训练 step，不保存 checkpoint，
不能作为模型效果或正式训练验收。

## 7. 数据与镜像的实际边界

release 内置的是任务元数据：

- 10 条 SWE-bench Verified 评测元数据；
- 5 条 SWE-smith 训练元数据。

默认脚本按需拉取：

```text
评测环境：swebench/sweb.eval.x86_64.astropy_1776_astropy-7166:latest
训练环境：jyangballin/swesmith.x86_64.oauthlib_1776_oauthlib.1fd52536:latest
VeRL 环境：docker.io/verlai/verl:vllm017.latest
```

当前交付不提供完整 SWE-bench/SWE-smith 数据集的一键下载和转换。完整数据使用方法、catalog 格式和 Hub
适用条件见两本操作指南。

## 8. 出错时先收集这些信息

UEnv 主机：

```bash
uenv version
uenv status
uenv logs server -n 100
uenv logs worker -n 100
sudo systemctl status uenv-swe-agent.service --no-pager
sudo journalctl -u uenv-swe-agent.service -n 200 --no-pager
sudo runuser -u uenv -- docker info
sudo docker system df
```

GPU/VeRL 主机：

```bash
nvidia-smi
docker info
ls -lh .uenv-verl/output 2>/dev/null || true
tail -n 1 .uenv-verl/output/agent-loop-results.jsonl 2>/dev/null || true
```

反馈问题时同时提供实际执行的命令、退出码和对应主机日志；不要只提供“服务启动了但任务不能跑”。
