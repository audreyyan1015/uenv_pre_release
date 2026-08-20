# 单机部署

本章在一台 x86_64 Linux 主机上安装 UEnv Server 和一台 UEnv Worker。完成后，UEnv Server 应能看到一个状态为 `ready` 的本机 UEnv Worker。

## 开始前

| 项目 | 要求 |
|---|---|
| 系统 | x86_64 Linux、systemd、Python 3.10 或更高版本 |
| 工具 | `sudo`、`tar`、`curl` |
| 空间 | 至少 2 GiB 可用磁盘空间 |
| 安装文件 | `install.sh` 和发布压缩包 |

安装文件指 `install.sh` 与发布压缩包 `uenv-linux-x86_64.tar.gz`（建议连同 `.sha256` 校验文件）。从团队发布渠道获取；从源码构建时，在源码仓库执行 `scripts/build-release.sh`（需要 Rust 工具链、`protoc` 和 Python 3），产物在仓库 `dist/` 目录，包含这三者。

普通环境可以运行在 CPU 主机上。使用 SWE 或其他容器环境时，还需要 Docker 或 Podman，并保证 `uenv` 用户有对应权限。

以下命令把安装文件目录设为 `/home/uenv-install`。如果文件位于其他目录，只修改 `INSTALL_DIR`。

## 安装 UEnv Server 和 UEnv Worker

```bash
INSTALL_DIR=/home/uenv-install
cd "$INSTALL_DIR"
sudo bash install.sh \
  --bundle ./uenv-linux-x86_64.tar.gz \
  --profile single-node
```

安装器会创建 `uenv` 系统用户，并启用 UEnv Server 和 UEnv Worker 两个服务。其中 UEnv Server 的 systemd 单元名是 `uenv-adapter-core.service`（兼容旧版的名称）；后续命令中看到这个单元名时，它就是 UEnv Server。

单机配置使用本机地址：UEnv Worker 连接 `127.0.0.1:50051`，并向 UEnv Server 公布 `127.0.0.1:50054`。

## 验证服务和注册

在这台主机依次运行：

```bash
uenv version
sudo -u uenv uenv doctor
sudo systemctl is-active \
  uenv-adapter-core.service \
  uenv-worker.service
curl -fsS http://127.0.0.1:50052/health
curl -fsS http://127.0.0.1:19090/health
uenv status
uenv workers
```

完成标志：

1. 两个 systemd 单元都输出 `active`。
2. 两个健康请求成功返回，命令退出码为 0。
3. `uenv workers` 列出这台 UEnv Worker，状态为 `[READY]`，endpoint 为 `127.0.0.1:50054`，例如（UEnv Worker 名称以实际注册为准）：

```text
[READY] worker-01 (127.0.0.1:50054) load=0/4
  无运行中的 Episode
```

部署阶段只验证服务与注册；任务样本的提交与结果验证见[通用评测流程](../3-运行任务/03-evaluation.md)、[强化学习训练流程](../3-运行任务/07-post-training.md)和[轨迹采集](../3-运行任务/12-trajectory.md)。

## 找到配置和日志

| 内容 | 位置 |
|---|---|
| UEnv Server 配置 | `/etc/uenv/server.yaml`、`/etc/uenv/server.env` |
| UEnv Worker 配置 | `/etc/uenv/worker.yaml`、`/etc/uenv/worker.env` |
| 密钥 | `/etc/uenv/secrets/` |
| 服务日志 | `uenv logs server`、`uenv logs worker` |

需要修改中心服务时阅读[配置 UEnv Server](./03-server.md)，需要理解 UEnv Worker 地址时阅读[配置并注册 UEnv Worker](./04-worker-registration.md)。

## 继续扩展到多机

完成本章后继续阅读[多机部署](./02-multi-node.md)。下一章沿用同一安装包和验收方法，把 UEnv Server 与 UEnv Worker 分到不同主机，并增加第二台 UEnv Worker。
