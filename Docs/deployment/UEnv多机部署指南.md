# UEnv 多机部署指南

本指南将 Adapter Core 和 Worker 拆到不同服务器，并可继续添加 Worker。这样做适合：

- 需要将调度控制面与环境执行资源分离。
- 需要提高 Episode 并发能力。
- 不同 Worker 需要不同资源或隔离策略。

Hub 和多机是两个独立选择：多机可以不部署 Hub，单机也可以使用 Hub。本指南只讲控制面和 Worker 的多机部署；需要环境注册、制品同步或版本回滚时，同时阅读 [UEnv Hub 使用指南](./UEnv%20Hub使用指南.md)。

## 1. 部署结构

最小多机结构由 1 个控制面和 1 个或多个 Worker 组成：

```text
评测或训练程序
        |
        | Episode
        v
Adapter Core（控制面）
        |
        | 调度、下发、结果回收
        +-------------------+
        v                   v
     Worker 1            Worker 2 ...
```

Adapter Core 不会代替 Worker 执行环境。Worker 会主动向 Adapter Core 注册和发送心跳，Adapter Core 还必须能够连回 Worker 对外公告的 gRPC 地址。

## 2. 主机与网络规划

先为每台机器确定稳定的内网 IP 或可解析主机名。以下示例使用：

```text
控制面：10.0.0.10
Worker 1：10.0.0.21
Worker 2：10.0.0.22
```

节点之间至少需要放行：

| 来源 | 目标 | 端口 | 用途 |
|---|---|---:|---|
| Bridge、Worker | 控制面 | 50051/TCP | 提交 Episode；Worker 注册、心跳和上报 |
| 控制面 | 每台 Worker | 50054/TCP | 下发 Episode |
| 运维网络（可选） | Worker | 19090/TCP | 健康检查和指标 |

控制面的 Admin HTTP 默认只监听 `127.0.0.1:50052`，不应直接暴露到公网。需要远程运维时，优先使用 SSH 或受控内网。

在部署前检查双向连通性。例如，控制面安装后可以从 Worker 检查 50051；Worker 安装后再从控制面检查 50054：

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

每台节点必须使用同一个 bundle。如果有 `.sha256` 文件，也一并复制并在每台节点校验。

多机可以不使用 Hub，但运维人员必须自行保证每台 Worker 的 UEnv 版本、插件和任务制品一致。

## 4. 部署控制面

在 `10.0.0.10` 执行：

```bash
cd /home/uenv-install
sudo bash install.sh \
  --bundle ./uenv-linux-x86_64.tar.gz \
  --profile control-plane
```

检查：

```bash
sudo systemctl is-active uenv-adapter-core.service
curl -fsS http://127.0.0.1:50052/health
sudo -u uenv uenv doctor
```

刚安装完时 `uenv status` 显示没有 Worker 是正常的；下一节完成后 Worker 才会注册。

## 5. 部署第一台 Worker

在 `10.0.0.21` 执行：

```bash
cd /home/uenv-install
sudo bash install.sh \
  --bundle ./uenv-linux-x86_64.tar.gz \
  --profile worker \
  --server 10.0.0.10:50051 \
  --advertise 10.0.0.21:50054
```

两个地址的含义不同：

- `--server`：Worker 用来注册、发送心跳和上报结果的控制面地址。
- `--advertise`：控制面下发 Episode 时用来连接该 Worker 的地址。必须是控制面可达的地址，不能填 `127.0.0.1`。

在 Worker 上检查：

```bash
sudo systemctl is-active uenv-worker.service
curl -fsS http://127.0.0.1:19090/health
sudo -u uenv uenv doctor
```

然后回到控制面检查注册结果：

```bash
uenv status
uenv workers
```

应能看到 Worker 状态为 `ready`，且 endpoint 为 `10.0.0.21:50054`。

## 6. 增加更多 Worker

在新 Worker 上重复上一节，只需替换 `--advertise` 地址。例如 `10.0.0.22`：

```bash
sudo bash install.sh \
  --bundle ./uenv-linux-x86_64.tar.gz \
  --profile worker \
  --server 10.0.0.10:50051 \
  --advertise 10.0.0.22:50054
```

添加后在控制面执行：

```bash
uenv status
```

确认 Worker 数量与预期一致，并检查每台 Worker 的 endpoint、状态、容量和心跳时间。

## 7. 可选：让 Worker 使用 Hub

多机不要求 Hub。当需要中心化的环境版本、制品同步或回滚时，先完成 [UEnv Hub 使用指南](./UEnv%20Hub使用指南.md) 中的 Hub 部署和鉴权配置。对受 Token 保护的 Hub，为 Worker 安装命令同时增加：

```text
--hub http://<hub-ip>:8080
--hub-token-file ./worker-reader.token
```

这只是让 Worker 连接 Hub，不会自动将 Hub 中的所有环境安装到 Worker。Hub 的发布、同步、激活和版本锁定以 Hub 指南为准。

## 8. 多机验收

建议按以下顺序检查：

1. 控制面上 `uenv-adapter-core.service` 为 `active`。
2. 每台 Worker 上 `uenv-worker.service` 为 `active`。
3. 每台 Worker 都能连接控制面 `50051`。
4. 控制面都能连接每台 Worker 公告的 `50054`。
5. `uenv status` 中的 Worker 数量、endpoint 和状态都正确。

注意：Worker 配置 `id: "auto"` 时，每次重启都会以新的 Worker ID 重新注册，旧记录在心跳超时后先转为 `degraded` 再被清除。刚重启过 Worker 的几分钟内，`uenv status` 的 Worker 数量短暂多于实际节点数属于正常现象，验收应在心跳稳定后再核对数量。
6. 所有节点的 `uenv version` 相同。

完成平台部署后，再根据用途执行 [UEnv 评测指南](./UEnv评测指南.md) 或 [UEnv 训练指南](./UEnv训练指南.md) 中的单 Episode 验证。

## 9. 扩容、升级和下线

### 扩容

新 Worker 准备完与现有节点一致的 release、插件和环境制品后，按第 6 节安装即可。不要在环境尚未就绪时就将 Worker 加入生产调度。

### 升级

优先逐台升级 Worker：

1. 确认该 Worker 没有正在运行的 Episode。
2. 在该节点用新 bundle 重新执行 `--profile worker` 安装命令。
3. 确认 Worker 重新注册并可执行任务。
4. 再处理下一台。

所有 Worker 验证完成后再升级控制面。跨版本升级前应备份 `/etc/uenv` 和 `/var/lib/uenv/server`。

### 下线

先确认 Worker 上没有运行中的 Episode，再执行：

```bash
sudo systemctl disable --now uenv-worker.service
```

控制面会在心跳超时后将该 Worker 标记为不可用。不要直接关机一台仍在执行 Episode 的 Worker。

## 10. 排障

### Worker 没有出现在 `uenv status`

在 Worker 检查：

```bash
sudo journalctl -u uenv-worker.service -n 200 --no-pager
python3 -c 'import socket; socket.create_connection(("10.0.0.10", 50051), 5).close()'
sed -n '1,80p' /etc/uenv/worker.yaml
```

重点核对 `server.endpoint`、`worker.advertise_endpoint` 和防火墙。

### Worker 已注册，但下发失败

在控制面检查它是否能连接 Worker 公告的地址：

```bash
python3 -c 'import socket; socket.create_connection(("10.0.0.21", 50054), 5).close()'
uenv logs server -n 200
```

常见原因是 `--advertise` 填成了回环地址、NAT 后的不可达地址，或 50054/TCP 没有放行。

### 各 Worker 行为不一致

先比对：

```bash
uenv version
cat /opt/uenv/current/.bundle.sha256 2>/dev/null || true
```

然后核对插件、配置和任务制品。需要中心化管理这些版本时，使用 Hub，不要在每台 Worker 上手工覆盖文件。
