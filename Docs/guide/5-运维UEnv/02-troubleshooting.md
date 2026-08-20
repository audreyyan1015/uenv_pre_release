# 故障排查

先定位故障所在的组件或连接方向，再查看详细日志。不要只根据“进程存在”判断系统可用。

## 首轮检查

| 顺序 | 执行主机 | 命令或检查 | 正常结果 |
|---:|---|---|---|
| 1 | UEnv Server | `systemctl is-active uenv-adapter-core.service` | `active` |
| 2 | UEnv Worker | `systemctl is-active uenv-worker.service` | `active` |
| 3 | UEnv Worker | 连接 UEnv Server `50051/TCP` | 连接成功 |
| 4 | UEnv Server | 连接 UEnv Worker `50054/TCP` | 连接成功 |
| 5 | UEnv Server | `uenv workers` | 目标 UEnv Worker 为 `ready` |

示例网络命令使用 UEnv Server `10.0.0.10` 和 UEnv Worker `10.0.0.21`。请替换为实际内网地址。

在 UEnv Worker 主机：

```bash
export SERVER_HOST=10.0.0.10
python3 -c 'import os, socket; socket.create_connection((os.environ["SERVER_HOST"], 50051), 5).close()'
```

在 UEnv Server 主机：

```bash
export WORKER_HOST=10.0.0.21
python3 -c 'import os, socket; socket.create_connection((os.environ["WORKER_HOST"], 50054), 5).close()'
```

命令无输出且退出码为 0 表示连接成功。`Connection refused` 通常表示目标未监听，超时通常表示路由、防火墙或安全组阻断。

## UEnv Server 无法启动

在 UEnv Server 主机：

```bash
sudo -u uenv /opt/uenv/current/bin/uenv-adapter-core --validate-config
sudo systemctl status uenv-adapter-core.service --no-pager
sudo journalctl -u uenv-adapter-core.service -n 200 --no-pager
sudo ss -lntp | grep -E ':(50051|50052|50053|8077)\b'
```

依次检查：

1. `/etc/uenv/server.env` 的 `UENV_CONFIG_PATH` 是否指向可读 YAML。
2. `UENV_ADDR` 使用的端口是否被其他进程占用。
3. 持久化目录是否可由 `uenv` 用户写入，磁盘是否有剩余空间。
4. `worker_offline_timeout_secs` 是否严格大于 `heartbeat_timeout_secs`。

先修复第一条明确错误，再重新校验；不要同时修改多个无关设置。

## UEnv Worker 服务启动失败

在 UEnv Worker 主机：

```bash
sudo -u uenv /opt/uenv/current/bin/uenv-worker \
  --config /etc/uenv/worker.yaml validate-config
sudo systemctl status uenv-worker.service --no-pager
sudo journalctl -u uenv-worker.service -n 200 --no-pager
curl -v http://127.0.0.1:19090/health
```

常见原因是 YAML 字段非法、健康和指标监听地址不一致、插件目录不可读、WAL 目录不可写，或注册连续失败导致进程退出。

## UEnv Worker 健康但没有注册

如果本机健康接口成功，但 UEnv Server 上的 `uenv workers` 没有该节点：

1. 检查 `/etc/uenv/worker.yaml` 的 `server.endpoint` 是否为 UEnv Server 实际地址。
2. 在 UEnv Worker 主机执行首轮检查中的 UEnv Worker → UEnv Server 命令。
3. 检查 UEnv Worker 日志中的注册超时、`accepted=false` 或重试事件。
4. 固定 `worker.id` 时，确认没有另一个同 ID 实例仍在运行或持有活动 Episode。

```bash
sudo journalctl -u uenv-worker.service -n 200 --no-pager
```

## UEnv Worker 已注册但派发失败

如果 UEnv Worker 已出现但任务无法开始，在 UEnv Server 主机：

```bash
uenv workers
export WORKER_HOST=10.0.0.21
python3 -c 'import os, socket; socket.create_connection((os.environ["WORKER_HOST"], 50054), 5).close()'
uenv logs server -n 200
```

依次检查：

- `worker.advertise_endpoint` 是否误填为回环地址、通配监听地址或不可路由地址。
- UEnv Worker 是否监听 `50054`，UEnv Server → UEnv Worker 防火墙是否放行。
- UEnv Worker 是否声明任务要求的环境类型、资源或环境包版本。
- UEnv Worker 是否为 `degraded`、`draining` 或容量已满。

## UEnv Server 重启后 UEnv Worker 没有恢复

UEnv Server 重启会产生新的运行实例标识。正常情况下，UEnv Worker 在下一次心跳响应后自动重新注册。

在 UEnv Server 主机：

```bash
uenv status
uenv logs server -n 200
```

在 UEnv Worker 主机：

```bash
sudo journalctl -u uenv-worker.service -n 200 --no-pager
```

先确认 UEnv Worker → UEnv Server `50051/TCP` 已恢复，再等待下一轮心跳。如果仍没有重新注册，并且已经确认该 UEnv Worker 没有活动 Episode，再重启 UEnv Worker 服务。

## 结果一直没有返回

分别查看两侧同一时间窗口的日志，并使用 Episode ID 关联：

```bash
uenv logs server -n 300
sudo journalctl -u uenv-worker.service -n 300 --no-pager
```

按阶段检查：

| 阶段 | 检查内容 |
|---|---|
| 调度 | UEnv Worker 是否为 `ready`，环境能力和容量是否匹配 |
| 派发 | UEnv Worker 是否收到该 Episode；UEnv Server → UEnv Worker 网络是否正常 |
| 执行 | 环境是否仍在运行，是否达到 Episode timeout |
| 结果上报 | UEnv Worker WAL 是否有待确认记录；UEnv Worker → UEnv Server 网络是否正常 |
| 身份校验 | 日志是否出现运行实例、UEnv Worker、token、幂等或租约不匹配错误 |

不要手工删除 `/var/lib/uenv/worker/wal/` 来消除告警，其中可能包含尚未被 UEnv Server 确认的最终结果。错误码含义见[协议与调用方向](../6-查阅参考/04-protocols.md#派发与最终结果)。

## 从远程主机访问 `uenv status`

Admin HTTP 默认只监听 UEnv Server 的 `127.0.0.1:50052`。在远程运维主机先建立 SSH 隧道：

```bash
export SERVER_HOST=10.0.0.10
export SERVER_SSH_USER=uenv-admin
ssh -N -L 15052:127.0.0.1:50052 \
  "${SERVER_SSH_USER}@${SERVER_HOST}"
```

保持该终端运行，在另一个终端执行：

```bash
uenv --admin-url http://127.0.0.1:15052 status
```

预期得到与在 UEnv Server 本机运行 `uenv status` 相同的状态。如果配置了 `admin_http_token`，当前轻量 CLI 不会自动附加 token；使用能设置 `Authorization: Bearer` 或 `X-Admin-Token` 的 HTTP 客户端，或继续通过受控本机和隧道运维。

## 收集问题信息

提交问题前保留：

- 所有节点的 `uenv version`
- 去除密钥后的 UEnv Server 和 UEnv Worker 配置
- UEnv Server 与 UEnv Worker 相同时间窗口的 journal
- `uenv status` 和 `uenv workers` 输出
- 两个方向的 TCP 检查结果
- 失败 Episode 的 ID、尝试编号和错误码

不要附带 API key、UEnv Hub token、trajectory token 或 `/etc/uenv/secrets/` 内容。
