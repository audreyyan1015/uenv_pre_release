# UEnv 基础部署指南

本指南用于在一台 x86_64 Linux 主机上部署最小可用的 UEnv。安装完成后，这台主机上会运行两个基础组件：

- Adapter：由 `uenv-adapter-core.service` 运行；其内部使用 UEnv Server（`uenv-server`）模块完成 UEnv Worker 注册、Episode 调度和状态管理。
- UEnv Worker：运行环境插件，并把执行结果返回给 Adapter。

输入文件中的一条任务记录称为“任务样本”。UEnv 对一条任务样本的一次执行称为 Episode。当前 UEnv 版本内置 `qa` 和 `code` 环境插件；`math` 只作为旧请求的兼容名保留。

本指南安装、启动和检查 Adapter 与 UEnv Worker。

添加 UEnv Worker 见 [UEnv 多机部署指南](./UEnv多机部署指南.md)。环境注册和版本管理见 [UEnv Hub 使用指南](./UEnv%20Hub使用指南.md)。任务输入文件、模型和专用运行组件在评测或训练指南中准备。

## 1. 系统要求

目标主机需要：

- x86_64 Linux 和 systemd。
- Python 3.10 或更高版本。
- `sudo`、`tar`、`sha256sum` 和 `curl`。
- 至少 2 GiB 可用磁盘空间。

CPU 主机即可运行 Adapter 和 UEnv Worker。某个环境需要容器时，再在运行该环境的 UEnv Worker 主机上安装 Docker 或 Podman。

如果准备自己构建 UEnv 安装包，构建主机还需要：

- x86_64 Linux。
- Rust stable 和 Cargo。
- C/C++ 基础构建工具、OpenSSL 开发头文件、`pkg-config` 和 Protobuf compiler (`protoc`)。
- Python 3.10+ 及 pip。
- `git`。

## 2. 选择安装包来源

UEnv 有两种交付方式。选择任意一种后，都从第 3 节的同一条安装命令继续。

### 2.1 从 GitHub 源码构建

在全新 Ubuntu/Debian 构建主机上先安装系统依赖：

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

`protoc` 用于生成 Adapter 与 UEnv Worker 之间的通信代码，是必需的构建依赖。然后在构建主机执行：

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

`--version` 指定本次构建的 UEnv 版本。安装包内容改变时，使用新的版本号。

构建产物位于：

```text
dist/install.sh
dist/uenv-linux-x86_64.tar.gz
dist/uenv-linux-x86_64.tar.gz.sha256
```

将这些文件复制到目标主机的同一目录。例如：

```bash
scp dist/install.sh dist/uenv-linux-x86_64.tar.gz \
  dist/uenv-linux-x86_64.tar.gz.sha256 user@uenv-host:/home/uenv-install/
```

### 2.2 使用预构建的 UEnv 安装包

将收到的两个文件放在目标主机的同一目录：

```text
install.sh
uenv-linux-x86_64.tar.gz
```

如果同时收到 `uenv-linux-x86_64.tar.gz.sha256`，也放在该目录。安装器发现该文件时会自动校验安装包。

## 3. 在单台主机上安装 UEnv

在目标主机执行：

```bash
cd /home/uenv-install
sudo bash install.sh \
  --bundle ./uenv-linux-x86_64.tar.gz \
  --profile single-node
```

`--profile` 选择安装模式（profile）。`single-node` 表示在同一台主机上安装 Adapter 和一个 UEnv Worker。`--bundle` 指定 UEnv 安装包的路径。

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
| 当前已安装版本 | `/opt/uenv/current` |
| 版本目录 | `/opt/uenv/releases/<version>` |
| UEnv 命令行工具 | `/usr/local/bin/uenv` |
| 配置 | `/etc/uenv` |
| 运行数据 | `/var/lib/uenv` |
| 日志 | `/var/log/uenv` 和 systemd 服务日志 |

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
- `uenv status` 显示 1 个 UEnv Worker，状态为 `ready`。
- 两个 systemd 服务都返回 `active`。
- 两个健康检查均返回成功状态。

至此，Adapter 和 UEnv Worker 已正常运行。后续再根据评测或训练任务准备模型、任务输入文件和目标环境。

## 5. 基础端口

| 端口 | 组件 | 用途 | 默认监听 |
|---:|---|---|---|
| 50051 | Adapter | 评测或训练程序提交 Episode；UEnv Worker 注册、定期报告状态和上报结果 | `0.0.0.0` |
| 50052 | Adapter | 管理与健康检查 | `127.0.0.1` |
| 50053 | Adapter | 运行状态与指标 | `0.0.0.0` |
| 50054 | UEnv Worker | Adapter 发送 Episode | `0.0.0.0` |
| 8077 | Adapter | 交互轨迹（trajectory）查询 | `0.0.0.0` |
| 19090 | UEnv Worker | 健康检查和运行指标 | `0.0.0.0` |

单机部署也需要使用主机防火墙限制外部访问。50053、8077 和 19090 仅向需要远程查看运行数据的受控内网放行；Adapter 管理接口（50052）默认只监听回环地址。

## 6. 常用运维操作

查看状态和日志：

```bash
uenv status
uenv workers
uenv logs server -n 100
uenv logs worker -n 100
```

`uenv logs server` 中的 `server` 是 UEnv 命令中的固定子命令，对应 Adapter 内部的 UEnv Server 模块；该命令查看 `uenv-adapter-core.service` 日志。`uenv logs worker` 查看 UEnv Worker 日志。

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

安装新版本时，使用新的 UEnv 安装包重新执行第 3 节的命令。默认会保留现有 `/etc/uenv` 配置。`--force-config` 适用于已备份并完成新旧配置比对的明确替换操作。

某个服务无法启动时，查看对应的 systemd 服务日志：

```bash
sudo journalctl -u uenv-adapter-core.service -n 200 --no-pager
sudo journalctl -u uenv-worker.service -n 200 --no-pager
```

## 7. 下一步

- 增加独立 UEnv Worker：[UEnv 多机部署指南](./UEnv多机部署指南.md)。
- 在单机或多机部署中使用 UEnv Hub：[UEnv Hub 使用指南](./UEnv%20Hub使用指南.md)。
- 运行模型评测或接入新环境：[UEnv 评测指南](./UEnv评测指南.md)。
- 将 UEnv 接入后训练：[UEnv 训练指南](./UEnv训练指南.md)。
