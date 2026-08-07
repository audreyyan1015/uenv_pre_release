# UEnv 多机部署指南

Adapter 由 `uenv-adapter-core.service` 运行；其内部使用 UEnv Server（`uenv-server`）模块完成 UEnv Worker 注册、Episode 调度和状态管理。

本指南将 Adapter 和 UEnv Worker 安装在不同主机上，并说明如何继续添加 UEnv Worker。UEnv 对一条任务样本的一次执行称为 Episode。以下情况适合使用多机部署：

- 需要将 Adapter 与运行环境的主机分开。
- 需要提高 Episode 并发能力。
- 不同 UEnv Worker 需要不同资源或隔离设置。

本指南说明 Adapter 与 UEnv Worker 的多机部署。运维人员需要保证各主机使用相同的 UEnv 版本、环境插件和任务文件。

需要统一发布环境、固定版本或回滚时，再按 [UEnv Hub 使用指南](./UEnv%20Hub使用指南.md) 配置 UEnv Hub。

## 1. 部署结构

最小多机结构由 1 个 Adapter 和 1 个或多个 UEnv Worker 组成：

```text
评测或训练程序
        |
        | Episode
        v
Adapter
        |
        | 分配、发送、收集结果
        +-------------------+
        v                   v
 UEnv Worker 1       UEnv Worker 2 ...
```

UEnv Worker 主动向 Adapter 注册，并定期报告运行状态。Adapter 把 Episode 分配给 UEnv Worker，再通过 UEnv Worker 公布的 gRPC 地址发送 Episode。

Adapter 主机与每台 UEnv Worker 主机都需要能够访问对方指定的端口。

## 2. 主机与网络规划

先为每台主机确定稳定的内网 IP 或可解析主机名。以下示例使用：

```text
Adapter：10.0.0.10
UEnv Worker 1：10.0.0.21
UEnv Worker 2：10.0.0.22
```

主机之间至少需要放行：

| 来源 | 目标 | 端口 | 用途 |
|---|---|---:|---|
| 评测程序、训练程序和 UEnv Worker | Adapter | 50051/TCP | 提交 Episode；UEnv Worker 注册、定期报告状态和上报结果 |
| Adapter | 每台 UEnv Worker | 50054/TCP | 发送 Episode |
| 运维网络（可选） | UEnv Worker | 19090/TCP | 健康检查和运行指标 |

Adapter 管理接口默认监听 `127.0.0.1:50052`。远程运维可通过 SSH 或受控内网访问该主机。

在部署前检查两个方向的网络连接。Adapter 安装后，从 UEnv Worker 主机检查 50051；UEnv Worker 安装后，从 Adapter 主机检查 50054：

```bash
python3 -c 'import socket; socket.create_connection(("10.0.0.10", 50051), 5).close()'
python3 -c 'import socket; socket.create_connection(("10.0.0.21", 50054), 5).close()'
```

## 3. 准备安装包

先按 [UEnv 基础部署指南](./UEnv基础部署指南.md#2-选择安装包来源) 获得：

```text
install.sh
uenv-linux-x86_64.tar.gz
```

每台主机必须使用同一个 UEnv 安装包。如果有 `.sha256` 文件，也一并复制并在每台主机上校验。

本指南的默认流程由运维人员保证每台 UEnv Worker 的 UEnv 版本、环境插件和任务文件一致。需要统一管理环境版本时，可在第 7 节配置 UEnv Hub。

## 4. 部署 Adapter

在 `10.0.0.10` 执行：

```bash
cd /home/uenv-install
sudo bash install.sh \
  --bundle ./uenv-linux-x86_64.tar.gz \
  --profile control-plane
```

`--profile` 选择安装模式（profile）。`control-plane` 是安装模式的固定代码值，该模式只安装 Adapter。本指南正文统一使用组件名“Adapter”。`--bundle` 指定 UEnv 安装包的路径。

检查：

```bash
sudo systemctl is-active uenv-adapter-core.service
curl -fsS http://127.0.0.1:50052/health
sudo -u uenv uenv doctor
```

此时只安装了 Adapter，因此 `uenv status` 中的 UEnv Worker 数量为 0。完成下一节后，第一台 UEnv Worker 会注册到 Adapter。

## 5. 部署第一台 UEnv Worker

在 `10.0.0.21` 执行：

```bash
cd /home/uenv-install
sudo bash install.sh \
  --bundle ./uenv-linux-x86_64.tar.gz \
  --profile worker \
  --server 10.0.0.10:50051 \
  --advertise 10.0.0.21:50054
```

`worker` 安装模式只安装 UEnv Worker。

安装命令中的 `--server` 是固定参数名，其中的 `server` 对应 Adapter 内部的 UEnv Server 模块。这个参数填写 Adapter 地址。两个地址的含义如下：

| 参数 | 填写的地址 | 连接方向 |
|---|---|---|
| `--server` | Adapter 的 gRPC 地址，例如 `10.0.0.10:50051` | UEnv Worker 连接 Adapter，用于注册、定期报告状态和上报结果 |
| `--advertise` | Adapter 可访问的 UEnv Worker gRPC 地址，例如 `10.0.0.21:50054` | Adapter 连接 UEnv Worker，用于发送 Episode |

在 UEnv Worker 主机上检查：

```bash
sudo systemctl is-active uenv-worker.service
curl -fsS http://127.0.0.1:19090/health
sudo -u uenv uenv doctor
```

然后回到 Adapter 主机检查注册结果：

```bash
uenv status
uenv workers
```

应能看到 UEnv Worker 状态为 `ready`，且状态输出中的 `endpoint` 字段为 `10.0.0.21:50054`。

## 6. 增加更多 UEnv Worker

在新 UEnv Worker 主机上重复上一节，并将 `--advertise` 替换为该主机的地址。例如 `10.0.0.22`：

```bash
sudo bash install.sh \
  --bundle ./uenv-linux-x86_64.tar.gz \
  --profile worker \
  --server 10.0.0.10:50051 \
  --advertise 10.0.0.22:50054
```

添加后在 Adapter 主机执行：

```bash
uenv status
```

确认 UEnv Worker 数量与预期一致，并检查每台 UEnv Worker 的地址、状态、容量和最后报告时间。

## 7. 可选：让 UEnv Worker 使用 UEnv Hub

需要统一管理环境版本、向多台 UEnv Worker 分发 EnvPackage（环境包）或回滚环境版本时，先按 [UEnv Hub 使用指南](./UEnv%20Hub使用指南.md) 部署 UEnv Hub。

完成 UEnv Hub 访问令牌（Token）配置后，在 UEnv Worker 安装命令中增加：

```text
--hub http://<hub-ip>:8080
--hub-token-file ./worker-reader.token
```

这两个参数让 UEnv Worker 连接 UEnv Hub。随后将环境版本发布为 EnvPackage，在每台 UEnv Worker 上下载并激活同一版本，再重启 UEnv Worker 加载该版本。完整操作见 UEnv Hub 使用指南。

## 8. 多机验收

建议按以下顺序检查：

1. Adapter 主机上的 `uenv-adapter-core.service` 为 `active`。
2. 每台 UEnv Worker 主机上的 `uenv-worker.service` 为 `active`。
3. 每台 UEnv Worker 都能连接 Adapter 的 `50051/TCP`。
4. Adapter 能连接每台 UEnv Worker 公布的 `50054/TCP`。
5. `uenv status` 中 UEnv Worker 的数量、`endpoint` 字段和状态都正确。
6. 所有主机的 `uenv version` 相同。

当 UEnv Worker 配置为 `id: "auto"` 时，重启后的旧记录会保留到状态报告超时。重启后等待几分钟，再核对 `uenv status` 中的 UEnv Worker 数量。

完成部署后，再根据用途执行 [UEnv 评测指南](./UEnv评测指南.md) 或 [UEnv 训练指南](./UEnv训练指南.md) 中的任务验证。

## 9. 扩容、升级和下线

### 扩容

新 UEnv Worker 主机准备好与现有主机相同的 UEnv 版本、环境插件和任务文件后，按第 6 节安装，并在任务验收通过后加入生产调度。

### 升级

先逐台升级 UEnv Worker：

1. 确认该 UEnv Worker 没有正在运行的 Episode。
2. 在该主机上使用新的 UEnv 安装包重新执行 `--profile worker` 安装命令。
3. 确认 UEnv Worker 重新注册并可执行任务。
4. 再处理下一台。

所有 UEnv Worker 验证完成后再升级 Adapter。跨版本升级前应备份 `/etc/uenv` 和 `/var/lib/uenv/server`。路径中的 `server` 是固定目录名，用于保存 Adapter 内部 UEnv Server 模块的运行数据。

### 下线

先确认 UEnv Worker 上没有运行中的 Episode，再执行：

```bash
sudo systemctl disable --now uenv-worker.service
```

Adapter 会在状态报告超时后将该 UEnv Worker 标记为不可用。关机前等待当前 Episode 结束，再使用上面的 systemd 命令完成下线。

## 10. 排障

### UEnv Worker 没有出现在 `uenv status`

在 UEnv Worker 主机上检查：

```bash
sudo journalctl -u uenv-worker.service -n 200 --no-pager
python3 -c 'import socket; socket.create_connection(("10.0.0.10", 50051), 5).close()'
sed -n '1,80p' /etc/uenv/worker.yaml
```

重点核对 `server.endpoint`、`worker.advertise_endpoint` 和防火墙。`server.endpoint` 中的 `server` 对应 Adapter 内部的 UEnv Server 模块；这个固定配置键保存 Adapter 地址。

### UEnv Worker 已注册，但 Adapter 发送 Episode 失败

在 Adapter 主机检查它是否能连接 UEnv Worker 公布的地址：

```bash
python3 -c 'import socket; socket.create_connection(("10.0.0.21", 50054), 5).close()'
uenv logs server -n 200
```

`uenv logs server` 中的 `server` 是 UEnv 命令中的固定子命令，对应 Adapter 内部的 UEnv Server 模块；该命令查看 `uenv-adapter-core.service` 日志。

常见原因是 `--advertise` 填成了回环地址、网络地址转换（NAT）后 Adapter 无法访问的地址，或 50054/TCP 没有放行。

### 各 UEnv Worker 行为不一致

先比对：

```bash
uenv version
cat /opt/uenv/current/.bundle.sha256 2>/dev/null || true
```

然后核对环境插件、配置和任务文件。需要统一管理环境版本时，使用 UEnv Hub 的发布、同步和激活流程。
