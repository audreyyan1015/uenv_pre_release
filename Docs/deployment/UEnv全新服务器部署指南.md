# UEnv 全新服务器部署指南（从 GitHub 源码开始）

> 本文档描述从 GitHub 克隆 `uenv_pre_release` 开始，在一台全新 x86_64 Linux 服务器上完成 UEnv 部署的完整流程。
> 仓库已自包含全部安装资产（install.sh / 构建脚本 / 测试 / 配置模板），**无需切换分支、无需叠加任何外部文件**。
> 基础安装流程已在 Ubuntu 24.04 x86_64 服务器上验证。SWE 与 GPU 训练的验收范围见后续操作指南。
>
> 本文只讲 UEnv 基础服务的安装。要运行 SWE 任务，请在基础安装后先阅读
> [UEnv SWE 使用入口](./SWE评测与VeRL训练操作指南.md)，再根据用途进入
> [SWE 评测操作指南](./SWE评测操作指南.md)或是
> [VeRL SWE 训练操作指南](./VeRL-SWE训练操作指南.md)。

## 0. 总体流程

```
构建机（需要 Rust）                    目标服务器（无需 Rust / GPU）
--------------------                  ---------------------------
git clone uenv_pre_release
  ↓
检查 + build-release.sh 打包
  ↓
dist/ 安装包  ──── scp ────→  校验 sha256
                                ↓
                              install.sh 安装
                                ↓
                              doctor / status 验收
```

## 1. 环境要求

### 构建机

- x86_64 Linux
- Rust stable + Cargo（`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`）
- Python ≥ 3.10 + pip（构建 Bridge wheel 用）
- `git`、`tar`、`sha256sum`

### 目标服务器

- x86_64 Linux，使用 systemd
- Python ≥ 3.10
- `sudo` 权限、`tar`、`sha256sum`
- ≥ 2 GiB 空闲磁盘
- **不需要 GPU，不需要 Rust**

以上只适用于 Adapter Core、Worker 和 Hub。若这台机器还要执行 SWE 环境，则另需：

- Docker，或已为 `uenv` 系统用户配置好的 Podman
- 能容纳所选 SWE 镜像的磁盘空间；单个镜像可能达到数十 GiB
- 运行 Worker 的 `uenv` 用户有权访问容器引擎

SWE 环境本身不使用 GPU；GPU 只供本地模型推理或 VeRL 训练使用。

## 2. 从 GitHub 克隆源码

```bash
git clone https://github.com/audreyyan1015/uenv_pre_release.git
cd uenv_pre_release
```

默认 `main` 分支即可，仓库已包含构建发布包所需的全部文件：

```
install.sh                        # 安装脚本
scripts/build-release.sh          # 发布包构建脚本
tests/test_installation_assets.py # 安装资产单元测试
deploy/config/                    # server/worker/hub 配置模板
deploy/systemd/                   # systemd 单元模板
uenv                              # 统一 CLI（doctor / status / logs）
```

## 3. 构建发布包

```bash
cd uenv_pre_release

# 语法检查 + 安装资产单元测试
bash -n install.sh scripts/build-release.sh
python3 -m unittest tests.test_installation_assets -v

# 打包（编译 workspace + Hub + Bridge wheel，首次全量编译约 10~30 分钟）
source $HOME/.cargo/env
./scripts/build-release.sh --version 0.1.1-trial
```

产物：

```text
dist/install.sh
dist/uenv-linux-x86_64.tar.gz
dist/uenv-linux-x86_64.tar.gz.sha256
```


## 4. 传输到目标服务器

```bash
# 在目标服务器上
mkdir -p /home/uenv-trial && cd /home/uenv-trial

scp user@<构建机>:/path/to/uenv_pre_release/dist/install.sh \
user@<构建机>:/path/to/uenv_pre_release/dist/uenv-linux-x86_64.tar.gz \
user@<构建机>:/path/to/uenv_pre_release/dist/uenv-linux-x86_64.tar.gz.sha256 .

```

## 5. 安装

```bash
cd /home/uenv-trial
sha256sum -c uenv-linux-x86_64.tar.gz.sha256   # 必须先校验
sudo bash install.sh --bundle ./uenv-linux-x86_64.tar.gz
```

默认 single-node 模式安装内容：

| 项目 | 位置 |
|---|---|
| systemd 服务 | `uenv-adapter-core.service`、`uenv-worker.service` |
| 统一 CLI | `/usr/local/bin/uenv` |
| 配置 | `/etc/uenv` |
| 程序 | `/opt/uenv/current` |
| 数据 | `/var/lib/uenv` |
| 日志 | `/var/log/uenv` |

> 同一个安装包可以重复执行，会复用不可变的 release 并保留现有配置；**不要使用 `--force-config`**（会覆盖配置）。
> 如果代码内容有变化，构建时必须使用新的版本号，安装器不会用同版本的新包覆盖正在运行的程序。
> 若目标机已有 `/etc/uenv`，先备份：`sudo cp -a /etc/uenv "/etc/uenv.backup.$(date +%Y%m%d-%H%M%S)"`

如果这台机器要执行 SWE 环境，先安装并启动 Docker 或 Podman，再在安装命令中增加
`--enable-swe`。例如允许首次运行时从公网拉取实例镜像：

```bash
sudo bash install.sh --bundle ./uenv-linux-x86_64.tar.gz \
  --enable-swe --swe-image-policy allow_public
```

该选项会启用 Worker Runtime Gateway，默认只监听 `127.0.0.1:28999`。不要为了多机训练而把
这个端口直接暴露到公网；把 OpenHands Agent 放在 UEnv 机器上即可。

`--enable-swe` 只配置 SWE Runtime、Gateway 和安装包内的少量任务元数据，不会安装 OpenHands，也不会
下载 SWE 数据集、环境镜像或模型。后续脚本会按选中的实例拉取镜像，具体见 UEnv SWE 使用入口。


## 6. 验收

```bash
uenv version                        # 应显示 0.1.1-trial
sudo -u uenv uenv doctor            # 应 6/6 全部通过
uenv status                         # Worker=1，endpoint 127.0.0.1:50054，状态 ready

curl -fsS http://127.0.0.1:50052/health   # ok（Admin）
curl -fsS http://127.0.0.1:19090/health   # ok（Worker）
```

如果使用了 `--enable-swe`，还要检查 Gateway 和 Worker 的容器权限：

```bash
curl -fsS http://127.0.0.1:28999/runtime/v1/health
sudo runuser -u uenv -- docker info >/dev/null
```

如 Worker 未及时注册，等 5 秒后重试 `uenv status`。

日志排查：

```bash
uenv logs server -n 100
uenv logs worker -n 100
sudo journalctl -u uenv-adapter-core.service -n 200 --no-pager
sudo journalctl -u uenv-worker.service -n 200 --no-pager
```

## 7. 端口表

| 端口 | 用途 | 默认暴露 |
|---|---|---|
| 50051 | Adapter Core gRPC（Worker 注册） | 0.0.0.0 |
| 50052 | Admin HTTP | 127.0.0.1 |
| 50053 | Obs HTTP（可观测平台） | 0.0.0.0 |
| 50054 | Worker gRPC | 0.0.0.0 |
| 19090 | Worker 指标 + health | 0.0.0.0 |
| 8077 | Trajectory HTTP | 0.0.0.0 |
| 8080 | Hub HTTP（可选） | 127.0.0.1 |
| 28999 | SWE Runtime Gateway（仅 `--enable-swe`） | 127.0.0.1 |

需要改端口时编辑 `/etc/uenv/server.yaml`（gRPC `port`、`admin_http_port`）和
`/etc/uenv/server.env`（`UENV_OBS_HTTP_LISTEN`、`UENV_TRAJECTORY_HTTP_LISTEN`），
然后 `sudo systemctl restart uenv-adapter-core.service`。

> `uenv status` / `uenv doctor` 默认查询 `http://127.0.0.1:50052`。
> 改过 Admin 端口后，用 `UENV_ADMIN_URL=http://127.0.0.1:<port>` 或 `--admin-url` 指定。


## 附录 A：国内网络环境准备

默认脚本直连 Docker Hub、PyPI 和 GitHub。在国内服务器上这三条链路都可能只有几十 KB/s，
建议先做以下准备，再执行安装与评测。

### Docker 镜像加速

编辑 `/etc/docker/daemon.json`：

```json
{
  "registry-mirrors": ["https://docker.m.daocloud.io", "https://docker.1ms.run"]
}
```

**修改后必须重启 dockerd 才会生效**（`sudo systemctl restart docker`）。只写配置不重启是
最常见的无效原因。重启前确认没有正在运行的容器会被打断。

### PyPI 依赖加速

普通 pip/uv 安装可临时指定镜像源：

```bash
export UV_DEFAULT_INDEX='https://mirrors.aliyun.com/pypi/simple/'
```

注意：OpenHands benchmarks 仓库的 `uv.lock` 把下载地址钉死在 `files.pythonhosted.org`，
`uv sync --frozen` 会以 lock 为准，忽略镜像源环境变量。网络较慢时可先把 lock 中的 URL
替换为镜像地址（文件哈希不变，不影响安装内容）：

```bash
cd /opt/uenv/agent/openhands-benchmarks
cp uv.lock /tmp/uv.lock.bak
sed -i 's|https://files.pythonhosted.org/packages/|https://mirrors.aliyun.com/pypi/packages/|g; \
        s|https://pypi.org/simple|https://mirrors.aliyun.com/pypi/simple/|g' uv.lock
```

依赖装完后用 `git checkout -- uv.lock` 恢复原文件。若重跑安装器时因 venv 已按镜像源地址
安装而被 uv 判定为来源不一致、触发全量重装，可再次执行同样的替换。

### GitHub 访问

OpenHands 安装器需要从 GitHub 克隆固定提交的 benchmarks 与 SDK。无法直连时先配置代理或
内网镜像，不要替换为未经验证的提交。

## 8. 可选：Hub 与多节点

Hub 负责环境包的注册、版本和多 Worker 分发，不参与一次 SWE 任务的调度与执行。单 Worker
评测和入门训练直接使用安装包内的本地 catalog，不需要安装 Hub；只有需要统一管理大量环境
或向多个 Worker 同步镜像时再启用它。

加装 Hub（在基础安装之后）：

```bash
sudo bash install.sh --bundle ./uenv-linux-x86_64.tar.gz --profile hub
export UENV_HUB_ENDPOINT=http://127.0.0.1:8080
curl -fsS http://127.0.0.1:8080/healthz
uenv hub status
```

多节点：

```bash
# 控制面节点
sudo bash install.sh --bundle ./uenv-linux-x86_64.tar.gz --profile control-plane

# Worker 节点
sudo bash install.sh --bundle ./uenv-linux-x86_64.tar.gz --profile worker \
  --server <控制面IP>:50051 --advertise <本机IP>:50054
```

要求：Worker 能连控制面 TCP 50051；控制面能回连 Worker TCP 50054；
`--advertise` 不要写 `0.0.0.0`。

## 9. 可视化前端（可选）

前端源码在仓库 `frontend/` 目录（Vite + TanStack Start）：

```bash
cd frontend
npm install
npm run dev -- --port 8888 --host 0.0.0.0
```

- 未显式配置端口时默认 5173
- `/obs/*` 由 Vite 代理到 `http://127.0.0.1:50053`（可用 `VITE_OBS_PROXY_TARGET` 覆盖）
- 生产部署建议 `npm run build` 后静态托管，或封装为 systemd 服务
