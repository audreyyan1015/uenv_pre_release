# 多机部署

本章把 UEnv Server 和 UEnv Worker 分别安装到不同主机，再增加第二台 UEnv Worker。完成后，UEnv Server 应看到两台 `ready` UEnv Worker，且两个网络方向都可达。

## 开始前

- 已通读[单机部署](./01-single-node.md)，了解安装器参数和验收命令。多机部署是在各主机上分别全新安装，不要求任何主机上保留单机部署时的进程或状态。
- UEnv Server 与所有 UEnv Worker 使用同一版本的安装包。
- 每台 UEnv Worker 主机有稳定的内网 IP 或可解析主机名。

本文使用下列示例地址。请在所有命令中把它们替换为你的实际内网地址：

| 角色 | 示例地址 |
|---|---|
| UEnv Server | `10.0.0.10` |
| UEnv Worker 1 | `10.0.0.21` |
| UEnv Worker 2 | `10.0.0.22` |

## 配置双向网络

| 来源 | 目标 | 必须放通 | 用途 |
|---|---|---:|---|
| 框架接入代码、每台 UEnv Worker | UEnv Server | 50051/TCP | 请求、注册、心跳和结果上报 |
| UEnv Server | 每台 UEnv Worker | 50054/TCP | Episode 派发、取消和环境准备 |

云主机请在安全组中放通上述端口；物理机用防火墙工具（如 `firewalld`、`ufw`）放通。

`advertise_endpoint` 是 UEnv Worker 注册时上报给 UEnv Server 的回连地址，UEnv Server 通过它向 UEnv Worker 派发 Episode、取消任务和准备环境。它必须填写 **UEnv Server 实际能够访问** 的地址：多机部署中填写 `127.0.0.1` 会让 UEnv Server 连回自己，填写 `0.0.0.0` 则无法定位具体主机。UEnv Server 的 Admin HTTP 默认保持在 `127.0.0.1:50052`，只供本机运维使用。

**检查 UEnv Server 的实际监听地址。** gRPC 监听由 `/etc/uenv/server.env` 的 `UENV_ADDR` 决定：`control-plane` 全新安装写入 `0.0.0.0:50051`，而 `single-node`/`full` 安装写入 `127.0.0.1:50051`。安装器默认保留已有 `/etc/uenv`：在同一台主机上从单机部署升级为多机（本指南推荐的顺序）时，旧的回环地址会被保留，远端 UEnv Worker 无法注册——Worker 日志报 `serve failed: transport error` 并被 systemd 反复重启（注意服务启动后立刻 `systemctl is-active` 会显示 `active`，数秒后才开始失败重启）。在 UEnv Server 主机执行 `ss -lntp | grep 50051` 确认监听的是内网地址；如果仍是 `127.0.0.1`，按[配置 UEnv Server](./03-server.md) 把 `UENV_ADDR` 改为受控内网地址（或 `0.0.0.0` 配合防火墙），校验并重启后再继续。

## 在所有主机准备相同版本

把 `install.sh` 和发布压缩包复制到每台主机的安装目录。部署后用 `uenv version` 确认所有节点版本相同。版本不一致的 UEnv Worker 可能仍能注册和心跳，但不保证协议与行为一致；验收以所有节点 `uenv version` 完全相同为准，不要混用不同版本长期运行。

发布压缩包的获取：使用团队发布渠道提供的 `uenv-linux-x86_64.tar.gz`（连同 `.sha256` 校验文件）；从源码构建时，在源码仓库执行 `scripts/build-release.sh`（需要 Rust 工具链、`protoc` 和 Python 3，产物在仓库 `dist/` 目录，包含 `install.sh`、压缩包和校验文件）。

## 在 UEnv Server 主机安装中心服务

在示例 UEnv Server `10.0.0.10` 上执行：

```bash
INSTALL_DIR=/home/uenv-install
cd "$INSTALL_DIR"
sudo bash install.sh \
  --bundle ./uenv-linux-x86_64.tar.gz \
  --profile control-plane

sudo systemctl is-active uenv-adapter-core.service
curl -fsS http://127.0.0.1:50052/health
sudo -u uenv uenv doctor
uenv status
```

`control-plane` 是当前安装器中“只安装中心服务”的 profile 名称，装出的就是 UEnv Server 本身。此时 `uenv status` 显示 0 台 UEnv Worker 是正常的。

注意：`uenv doctor` 会检查本机全部组件，在只安装 control-plane 的主机上，`uenv-worker.service` 不存在、Worker 健康检查失败（输出为 `4/6 项通过`）是预期结果；只需确认 `uenv-adapter-core.service: active` 和 `控制面健康检查: ok` 两项通过。

## 在第一台 UEnv Worker 主机安装执行节点

在示例 UEnv Worker 1 `10.0.0.21` 上执行：

```bash
INSTALL_DIR=/home/uenv-install
export SERVER_HOST=10.0.0.10
export WORKER_HOST=10.0.0.21
cd "$INSTALL_DIR"

sudo bash install.sh \
  --bundle ./uenv-linux-x86_64.tar.gz \
  --profile worker \
  --server "${SERVER_HOST}:50051" \
  --advertise "${WORKER_HOST}:50054"

sudo systemctl is-active uenv-worker.service
curl -fsS http://127.0.0.1:19090/health
sudo -u uenv uenv doctor
```

- `--server` 是 UEnv Worker 主动连接的 UEnv Server 地址。
- `--advertise` 是 UEnv Worker 上报给 UEnv Server 的回连地址，即上文的 `advertise_endpoint`。

**多机轨迹采集（必须配置，否则轨迹静默丢失）。** UEnv Worker 在 Episode 结束后自动把轨迹上传到 UEnv Server 的轨迹接口（8077/TCP），无需手工触发；但安装器为 UEnv Worker 默认写入 `UENV_TRAJECTORY_ENDPOINT=http://127.0.0.1:8077`，这只在单机部署时正确。上传失败不会阻塞任务，只在 UEnv Worker 的 `/var/log/uenv/worker.log` 中留下 WARN（重试后放弃），UEnv Server 上查询该轨迹会得到 404。多机部署需要同时满足三点：

1. 安装时增加 `--trajectory-endpoint http://<SERVER_HOST>:8077`，或事后修改 `/etc/uenv/worker.env` 的 `UENV_TRAJECTORY_ENDPOINT` 并重启 `uenv-worker.service`；
2. 每台 UEnv Worker 的 `/etc/uenv/secrets/swe.env` 中 `UENV_TRAJECTORY_TOKEN` 必须与 UEnv Server 主机同文件中的值一致——安装器在每台主机各自随机生成，多机部署需要把 UEnv Server 的值手工同步到每台 UEnv Worker（不一致时上传返回 `401 bad upload token`）。注意每次执行 `prepare-swe` 都会重写该文件并重新生成该 token，执行后需要重新同步；
3. UEnv Server 的 `UENV_TRAJECTORY_HTTP_LISTEN` 监听 UEnv Worker 可达的地址，见[配置 UEnv Server](./03-server.md)。

## 验证网络和注册

在 UEnv Worker 1 主机验证 UEnv Worker → UEnv Server 方向：

```bash
export SERVER_HOST=10.0.0.10
python3 -c 'import os, socket; socket.create_connection((os.environ["SERVER_HOST"], 50051), 5).close()'
```

在 UEnv Server 主机验证 UEnv Server → UEnv Worker 方向：

```bash
export WORKER_HOST=10.0.0.21
python3 -c 'import os, socket; socket.create_connection((os.environ["WORKER_HOST"], 50054), 5).close()'
uenv status
uenv workers
```

两个 Python 命令均应无输出并以退出码 0 结束。`uenv workers` 应显示 UEnv Worker 1 为 `[READY]`，endpoint 为 `10.0.0.21:50054`，例如（UEnv Worker 名称以实际注册为准）：

```text
[READY] worker-01 (10.0.0.21:50054) load=0/4
  无运行中的 Episode
```

## 增加第二台 UEnv Worker

在示例 UEnv Worker 2 `10.0.0.22` 上执行：

```bash
INSTALL_DIR=/home/uenv-install
export SERVER_HOST=10.0.0.10
export WORKER_HOST=10.0.0.22
cd "$INSTALL_DIR"

sudo bash install.sh \
  --bundle ./uenv-linux-x86_64.tar.gz \
  --profile worker \
  --server "${SERVER_HOST}:50051" \
  --advertise "${WORKER_HOST}:50054"
```

再次执行上一节的两个方向检查。

## 确认完成状态

在 UEnv Server 主机执行：

```bash
sudo systemctl is-active uenv-adapter-core.service
uenv status
uenv workers
```

预期结果：

1. systemd 单元输出 `active`。
2. `uenv status` 显示 `Worker=2`。
3. `uenv workers` 列出两台 UEnv Worker，状态均为 `[READY]`，endpoint 分别为 `10.0.0.21:50054` 与 `10.0.0.22:50054`（UEnv Worker 名称以实际注册为准）：

```text
[READY] worker-01 (10.0.0.21:50054) load=0/4
[READY] worker-02 (10.0.0.22:50054) load=0/4
```

再在两台 UEnv Worker 主机分别执行 `sudo systemctl is-active uenv-worker.service`（预期 `active`）和 `uenv version`，所有节点版本应一致。

`worker.id` 为 `auto` 时，每次重装或重启都可能注册出一个新 ID。同一 endpoint 的重新注册会替换旧记录，`uenv status` 的 `Worker=N` 保持准确；如果重新注册时上报的 endpoint 发生了变化（例如更换网卡或地址），旧 ID 的记录会转为 `[DEGRADED]`、暂停参与新调度，并可能长期保留、计入 `Worker=N`（当前版本没有删除单条 Worker 记录的命令，记录随持久化数据保留）。验收 Worker 数量时应只统计 `[READY]` 记录。需要稳定 ID 时，在 `/etc/uenv/worker.yaml` 中为每台 Worker 设置集群内唯一的固定 `worker.id`（见[配置并注册 UEnv Worker](./04-worker-registration.md#常见问题)）。

安装器默认保留已有 `/etc/uenv`。重复安装时，新传入的地址不会覆盖旧配置；确认要用本次命令行参数替换旧配置时，在安装命令中增加 `--force-config`，或先备份再手动替换。变更前建议阅读[运行维护](../5-运维UEnv/01-operations.md)中的安全变更流程。

## 在同一台主机运行多个组件

机器数量不足时，UEnv Server、UEnv Worker 和 UEnv Hub 可以任意组合部署在同一台主机：各组件端口（50051、50054、19090、8080）互不冲突，`worker` profile 的安装也不会停用本机已有的 `uenv-adapter-core.service`。注意两点：

1. 安装器不带 Worker 的 `control-plane` profile 会停用本机的 `uenv-worker.service`；要在同机补齐 Worker，最后再执行一次 `--profile worker` 安装。
2. 与 UEnv Server 同机的 UEnv Worker，`--advertise` 可以填本机内网地址（如 `10.0.0.10:50054`）；仅在确定 UEnv Server 永远同机时才可以保留 `127.0.0.1`。

完成本章后，继续主线流程的[通用评测流程](../3-运行任务/03-evaluation.md)。UEnv Worker 注册的单独操作见[配置并注册 UEnv Worker](./04-worker-registration.md)。
