# UEnv 基础部署指南

本指南用于在一台 x86_64 Linux 服务器上部署最小可用的 UEnv。安装完成后，同一台机器上会运行：

- Adapter Core：接收 Episode，并将其调度给 Worker。
- Worker：运行环境插件，返回 reward 和 trajectory。
- 安装包内置的 `qa` 和 `code` 环境插件；`math` 仅作为旧请求的兼容名保留。

本指南不部署 Hub，也不准备任何具体任务的数据、模型或运行时。需要添加 Worker 时阅读 [UEnv 多机部署指南](./UEnv多机部署指南.md)；需要环境注册和版本管理时阅读 [UEnv Hub 使用指南](./UEnv%20Hub使用指南.md)。

## 1. 系统要求

目标服务器需要：

- x86_64 Linux 和 systemd。
- Python 3.10 或更高版本。
- `sudo`、`tar`、`sha256sum` 和 `curl`。
- 至少 2 GiB 可用磁盘空间。

只运行 UEnv 基础服务不需要 GPU，也不需要容器运行时。

如果准备自己构建 release，构建机还需要：

- x86_64 Linux。
- Rust stable 和 Cargo。
- C/C++ 基础构建工具、OpenSSL 开发头文件、`pkg-config` 和 Protobuf compiler (`protoc`)。
- Python 3.10+ 及 pip。
- `git`。

## 2. 选择安装包来源

UEnv 有两种交付方式。选择任意一种后，都从第 3 节的同一条安装命令继续。

### 2.1 从 GitHub 源码构建

在全新 Ubuntu/Debian 构建机先安装系统依赖：

```bash
sudo apt-get update
sudo apt-get install -y \
  build-essential libssl-dev pkg-config protobuf-compiler \
  python3 python3-pip python3-venv git curl
```

如果尚未安装 Rust，安装 stable toolchain：

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
  | sh -s -- -y --profile minimal --default-toolchain stable
. "$HOME/.cargo/env"
```

先确认关键工具真正可用：

```bash
cargo --version
rustc --version
protoc --version
python3 --version
```

`protoc` 不是可选依赖；Workspace 在编译 Server 和 Worker 的 Protobuf/gRPC 代码时会直接调用它。然后在构建机执行：

```bash
git clone https://github.com/audreyyan1015/uenv_pre_release.git
cd uenv_pre_release

python3 -m venv .uenv-build-venv
. .uenv-build-venv/bin/activate
python -m pip install --upgrade pip wheel

bash -n install.sh scripts/build-release.sh
python3 -m unittest tests.test_installation_assets -v

bash ./scripts/build-release.sh --version 0.1.2-trial
```

`--version` 是本次构建的 release 版本。内容改变后必须使用新版本，不能用不同内容覆盖已安装的同名版本。

构建产物位于：

```text
dist/install.sh
dist/uenv-linux-x86_64.tar.gz
dist/uenv-linux-x86_64.tar.gz.sha256
```

将这些文件复制到目标服务器的同一目录。例如：

```bash
scp dist/install.sh dist/uenv-linux-x86_64.tar.gz \
  dist/uenv-linux-x86_64.tar.gz.sha256 user@uenv-host:/home/uenv-install/
```

### 2.2 使用预构建安装包

将收到的两个文件放在目标服务器的同一目录：

```text
install.sh
uenv-linux-x86_64.tar.gz
```

如果同时收到 `uenv-linux-x86_64.tar.gz.sha256`，也放在该目录。安装器发现该文件时会自动校验安装包。

## 3. 安装单机 UEnv

在目标服务器执行：

```bash
cd /home/uenv-install
sudo bash install.sh \
  --bundle ./uenv-linux-x86_64.tar.gz \
  --profile single-node
```

如果有校验文件，还可以在安装前手动核对：

```bash
sha256sum -c uenv-linux-x86_64.tar.gz.sha256
```

安装器会创建 `uenv` 系统用户，并安装、启用以下服务：

```text
uenv-adapter-core.service
uenv-worker.service
```

主要路径：

| 内容 | 路径 |
|---|---|
| 当前 release | `/opt/uenv/current` |
| 版本目录 | `/opt/uenv/releases/<version>` |
| 统一 CLI | `/usr/local/bin/uenv` |
| 配置 | `/etc/uenv` |
| 运行数据 | `/var/lib/uenv` |
| 日志 | `/var/log/uenv` 和 systemd journal |

## 4. 验证部署结果

依次执行：

```bash
uenv version
sudo -u uenv uenv doctor
uenv status
sudo systemctl is-active \
  uenv-adapter-core.service \
  uenv-worker.service
curl -fsS http://127.0.0.1:50052/health
curl -fsS http://127.0.0.1:19090/health
```

正常结果应同时满足：

- `doctor` 的检查项全部通过。
- `uenv status` 显示 1 个 Worker，状态为 `ready`。
- 两个 systemd 服务都返回 `active`。
- 两个健康检查均返回成功状态。

这里验证的是 UEnv 平台已经启动，不代表模型、数据或某个具体环境已经准备完成。

## 5. 基础端口

| 端口 | 组件 | 用途 | 默认监听 |
|---:|---|---|---|
| 50051 | Adapter Core | Bridge 提交 Episode；Worker 注册、心跳和上报 | `0.0.0.0` |
| 50052 | Adapter Core | 管理与健康检查 | `127.0.0.1` |
| 50053 | Adapter Core | 可观测数据 | `0.0.0.0` |
| 50054 | Worker | Adapter Core 下发 Episode | `0.0.0.0` |
| 8077 | Adapter Core | trajectory 查询 | `0.0.0.0` |
| 19090 | Worker | 健康检查和指标 | `0.0.0.0` |

单机也应用主机防火墙限制外部访问。如果不需要远程查看可观测数据，不要向公网放行 50053、8077 和 19090。Admin HTTP 默认只在回环地址提供。

## 6. 常用运维操作

查看状态和日志：

```bash
uenv status
uenv workers
uenv logs server -n 100
uenv logs worker -n 100
```

重启基础服务：

```bash
sudo systemctl restart \
  uenv-adapter-core.service \
  uenv-worker.service
```

修改配置前先备份：

```bash
sudo cp -a /etc/uenv "/etc/uenv.backup.$(date +%Y%m%d-%H%M%S)"
```

安装新版本时，使用新 bundle 重新执行第 3 节的命令。默认会保留现有 `/etc/uenv` 配置。不要在未备份且未比对配置时使用 `--force-config`。

某个服务无法启动时，直接查看对应 journal：

```bash
sudo journalctl -u uenv-adapter-core.service -n 200 --no-pager
sudo journalctl -u uenv-worker.service -n 200 --no-pager
```

## 7. 下一步

- 增加独立 Worker：[UEnv 多机部署指南](./UEnv多机部署指南.md)。
- 单机或多机使用 Hub：[UEnv Hub 使用指南](./UEnv%20Hub使用指南.md)。
- 运行模型评测或接入新环境：[UEnv 评测指南](./UEnv评测指南.md)。
- 将 UEnv 接入后训练：[UEnv 训练指南](./UEnv训练指南.md)。

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
