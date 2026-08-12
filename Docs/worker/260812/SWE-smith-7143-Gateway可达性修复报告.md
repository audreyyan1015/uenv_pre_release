# SWE-smith 7143 Gateway 可达性修复报告

> 日期：2026-08-12
> 范围：7143 Worker、Server/208.77 SSH Gateway tunnel、AgentJob gateway URL

## 1. 现象

Server 仍将 `worker-7143-pro` 注册为 ready 并继续接收 Episode，但从外部访问：

```text
219.147.100.43:28097 -> connection refused
219.147.100.43:28888 -> reachable
```

7143 本机访问 `127.0.0.1:28097` 正常，说明不是 Runtime Gateway 进程退出。

## 2. 根因

当前部署环境的公网 `:28097` NAT 没有实际打通。既有联调拓扑不是公网直连，而是：

```text
Server 127.0.0.1:28097 -> SSH tunnel -> 10.10.20.143:28097
208.77 127.0.0.1:28097 -> SSH tunnel -> 10.10.20.143:28097
```

7143 的标准重启脚本已经定义了：

```text
UENV_SWE_GATEWAY_PUBLIC_URL=http://127.0.0.1:28097
```

但本次运行进程的环境被手工启动命令覆盖为：

```text
UENV_SWE_GATEWAY_PUBLIC_URL=http://219.147.100.43:28097
```

这导致 Worker 注册给 Server 的 AgentJob Gateway 地址与实际可达拓扑不一致。

## 3. 修复

按当前已部署的 SSH tunnel 拓扑重新启动 Worker，使用：

```text
UENV_SWE_GATEWAY_PUBLIC_URL=http://127.0.0.1:28097
```

同时保留现有 Gateway/API key/超时配置。没有修改公网安全组或 NAT；如果未来要求
Agent 不经过隧道直接访问公网，仍需平台侧放通并映射 `219.147.100.43:28097`。

## 4. 验收

需要确认：

1. 7143 `28097` 本机 health 为 `ok`。
2. Server `127.0.0.1:28097` tunnel health 为 `ok`。
3. 208.77 `127.0.0.1:28097` tunnel health 为 `ok`。
4. Worker heartbeat 和 Server Worker registry 正常。
5. 新注册的 `gateway_public_url` 为当前拓扑可达地址，而不是公网未映射地址。
6. Server active episode/load 没有异常残留。

## 5. 边界

`127.0.0.1:28097` 只对安装了对应 SSH tunnel 的 Server/Agent 主机有效，不能作为
任意外部客户端的公网地址。多 Worker 扩容时，每个 Worker 必须有独立、可达的 Gateway
地址，不能复用同一个 localhost URL，除非每个调用方都建立了对应的端口隧道。

## 6. 实际修复与验收结果

7143 Worker 已按当前联调拓扑重新启动，实际使用：

```text
UENV_SWE_GATEWAY_PUBLIC_URL=http://127.0.0.1:28097
```

此前手工启动时使用的公网未映射地址：

```text
http://219.147.100.43:28097
```

已不再作为 Worker 上报的 Gateway URL。

验收结果：

```text
7143 127.0.0.1:28777/health                       -> ok
7143 127.0.0.1:28097/runtime/v1/health           -> ok
Server 127.0.0.1:28097/runtime/v1/health         -> ok
208.77 127.0.0.1:28097/runtime/v1/health         -> ok
Server worker-7143-pro                            -> ready
Server last_heartbeat_secs                        -> 0
```

Server 复核时 Worker 已有 `load=4`，说明该 Worker 已重新接收并处理 Episode，Gateway
tunnel 和 Agent 调用链恢复工作。

公网直连：

```text
219.147.100.43:28097 -> connection refused
```

该结果仍然符合当前部署设计，因为公网 NAT 没有配置，当前有效访问方式是 Server 和
208.77 通过各自的 `uenv-gateway-tunnel.service` 访问 7143 内网 Gateway。若要求任意
外部客户端直连公网 `:28097`，还需要在 A100 网络侧单独配置 NAT/安全组，不能通过
重启 Worker 进程解决。
