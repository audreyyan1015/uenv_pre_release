# 端口与连接方向

本页列出发布安装包的默认端口。最终以目标主机 `/etc/uenv` 中的生效配置为准。

## 按部署方式选择

| 场景 | 必须验证 | 按需开放 |
|---|---|---|
| 单机 | 本机 UEnv Server `50051`、UEnv Worker `50054`、健康接口 | UEnv Hub、观测、轨迹、SWE Runtime |
| 多机 | UEnv Worker → UEnv Server `50051`；UEnv Server → UEnv Worker `50054` | UEnv Worker 监控 `19090` |
| 使用 UEnv Hub | UEnv Worker/发布端 → UEnv Hub `8080` | 无 |
| 集中轨迹 | UEnv Worker/查询端 → UEnv Server `8077` | Obs `50053` |

Admin 是运维接口，Obs 是观测状态和事件的接口；执行任务只需接入下表的公开入口，Admin 与 Obs 供运维和观测使用。

## 默认端口

| 端口 | 所属组件 | 协议或用途 | 发布默认监听 | 谁需要访问 |
|---:|---|---|---|---|
| 50051 | UEnv Server | gRPC：接入请求、UEnv Worker 控制和 Admin RPC | `0.0.0.0` | 框架接入代码、UEnv Worker、受控运维客户端 |
| 50052 | UEnv Server | Admin HTTP：`/health`、`/status`、`/agents` | `127.0.0.1` | UEnv Server 本机或 SSH 隧道 |
| 50053 | UEnv Server | Obs HTTP 和事件流 | `127.0.0.1` | 受控观测端 |
| 8077 | UEnv Server | trajectory HTTP | `127.0.0.1` | 需要上传或查询轨迹的 UEnv Worker 与客户端 |
| 50054 | UEnv Worker | gRPC：派发、取消、准备环境和健康检查 | `0.0.0.0` | UEnv Server |
| 19090 | UEnv Worker | HTTP：`/health`、`/metrics` | `0.0.0.0` | 本机或受控监控网络 |
| 8080 | UEnv Hub（可选） | HTTP API、`/healthz` | `127.0.0.1` | UEnv Worker、发布和运维主机 |
| 28999 | UEnv Worker SWE Runtime（可选） | Runtime Gateway HTTP | 默认关闭 | SWE 执行组件 |

不要仅因为服务监听就把端口开放到公网。Obs、trajectory 和 UEnv Hub 在非回环监听时必须同时配置鉴权、网络限制，并在不受信任网络上使用 TLS 或 VPN。

## 多机最小放行

| 来源 | 目标 | 必须放行 |
|---|---|---|
| 框架接入代码 | UEnv Server | `50051/TCP` |
| UEnv Worker | UEnv Server | `50051/TCP` |
| UEnv Server | UEnv Worker | `50054/TCP` |

只验证 UEnv Worker → UEnv Server 不够。UEnv Server 必须能访问 UEnv Worker 注册时上报的 `worker.advertise_endpoint`。

## 配置来源

| 接口 | 配置位置 |
|---|---|
| UEnv Server gRPC | `/etc/uenv/server.env` 的 `UENV_ADDR` |
| UEnv Server Admin HTTP | `/etc/uenv/server.yaml` 的 `admin_http_bind`、`admin_http_port` |
| Obs | `/etc/uenv/server.env` 的 `UENV_OBS_HTTP_LISTEN` |
| trajectory | `/etc/uenv/server.env` 的 `UENV_TRAJECTORY_HTTP_LISTEN` |
| UEnv Worker gRPC | `/etc/uenv/worker.yaml` 的 `worker.listen` |
| UEnv Worker 公布地址 | `/etc/uenv/worker.yaml` 的 `worker.advertise_endpoint` |
| UEnv Worker 健康和指标 | `/etc/uenv/worker.yaml` 的 `observability.*_listen` |
| UEnv Hub | `/etc/uenv/hub.toml` 的 `[server] host`、`port` |

## 检查监听与连通性

在目标主机查看监听进程：

```bash
sudo ss -lntp | grep -E ':(50051|50052|50053|50054|8077|19090|8080)\b'
```

预期看到已经启用组件的监听地址。没有输出表示对应端口未监听；先检查服务状态和配置，不要直接扩大防火墙范围。

在 UEnv Worker 主机检查 UEnv Worker → UEnv Server：

```bash
export SERVER_HOST=10.0.0.10
python3 -c 'import os, socket; socket.create_connection((os.environ["SERVER_HOST"], 50051), 5).close()'
```

在 UEnv Server 主机检查 UEnv Server → UEnv Worker：

```bash
export WORKER_HOST=10.0.0.21
python3 -c 'import os, socket; socket.create_connection((os.environ["WORKER_HOST"], 50054), 5).close()'
```

示例地址必须替换为实际内网地址。命令无输出且退出码为 0 表示 TCP 连接成功；`Connection refused` 通常表示目标未监听，超时通常表示路由或防火墙阻断。
