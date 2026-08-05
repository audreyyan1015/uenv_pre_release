# UEnv 全新服务器部署指南（从 GitHub 源码开始）

> 本文档描述从 GitHub 克隆 `uenv_pre_release` 开始，在一台全新 x86_64 Linux 服务器上完成 UEnv 部署的完整流程。
> 仓库已自包含全部安装资产（install.sh / 构建脚本 / 测试 / 配置模板），**无需切换分支、无需叠加任何外部文件**。
> 流程已在 2026-08-04 于 8.130.75.157（Aliyun，Ubuntu 24.04）完整验证通过。

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
                              修 secrets 属主 bug
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

# 语法检查 + 安装资产单元测试（应 5/5 通过）
bash -n install.sh scripts/build-release.sh
python3 -m unittest tests.test_installation_assets -v

# 打包（编译 workspace + Hub + Bridge wheel，首次全量编译约 10~30 分钟）
source $HOME/.cargo/env
./scripts/build-release.sh --version 0.1.0-trial
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

scp root@8.130.75.157:/home/uenv-frontend-add/dist/install.sh \
root@8.130.75.157:/home/uenv-frontend-add/dist/uenv-linux-x86_64.tar.gz \
root@8.130.75.157:/home/uenv-frontend-add/dist/uenv-linux-x86_64.tar.gz.sha256 .
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

> 重复安装直接执行同一命令即可，会保留现有配置；**不要使用 `--force-config`**（会覆盖配置）。
> 若目标机已有 `/etc/uenv`，先备份：`sudo cp -a /etc/uenv "/etc/uenv.backup.$(date +%Y%m%d-%H%M%S)"`


## 6. 验收

```bash
uenv version                        # 应显示 0.1.0-trial
sudo -u uenv uenv doctor            # 应 6/6 全部通过
uenv status                         # Worker=1，endpoint 127.0.0.1:50054，状态 ready

curl -fsS http://127.0.0.1:50052/health   # ok（Admin）
curl -fsS http://127.0.0.1:19090/health   # ok（Worker）
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

需要改端口时编辑 `/etc/uenv/server.yaml`（gRPC `port`、`admin_http_port`）和
`/etc/uenv/server.env`（`UENV_OBS_HTTP_LISTEN`、`UENV_TRAJECTORY_HTTP_LISTEN`），
然后 `sudo systemctl restart uenv-adapter-core.service`。

> `uenv status` / `uenv doctor` 默认查询 `http://127.0.0.1:50052`。
> 改过 Admin 端口后，用 `UENV_ADMIN_URL=http://127.0.0.1:<port>` 或 `--admin-url` 指定。

## 8. 可选：Hub 与多节点

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
