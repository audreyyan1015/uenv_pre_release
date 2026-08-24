# 配置 UEnv Server

本页用于修改已经安装的 UEnv Server 的配置，单机和多机部署都适用。修改完成后，配置校验通过、服务恢复运行，并且所有预期 UEnv Worker 重新变为 `ready`。

## 配置文件

| 文件 | 用途 |
|---|---|
| `/etc/uenv/server.env` | 进程监听地址、观测和轨迹接口等环境变量 |
| `/etc/uenv/server.yaml` | 调度、心跳、Episode 超时、Admin HTTP 和持久化 |

UEnv Server 服务对应的代码名是 `uenv-adapter-core`：systemd 单元为 `uenv-adapter-core.service`，可执行文件为 `/opt/uenv/current/bin/uenv-adapter-core`。

## 监听地址与访问控制

各接口的推荐监听地址与访问范围如下：

| 接口 | 建议监听 | 谁需要访问 |
|---|---|---|
| gRPC `50051` | 单机用回环地址；多机用受控内网地址或 `0.0.0.0` 配合防火墙 | 框架接入代码、UEnv Worker |
| Admin HTTP `50052` | `127.0.0.1` | UEnv Server 本机或 SSH 隧道 |
| Obs `50053` | 默认保持 `127.0.0.1` | 受控观测端 |
| trajectory `8077` | 默认保持 `127.0.0.1` | 需要集中上传或查询轨迹的 UEnv Worker 与客户端 |

不要在未设置访问控制时把 Obs 或 trajectory 接口监听到所有网卡。安全的本机起点示例：

```ini
UENV_ADDR=127.0.0.1:50051
UENV_CONFIG_PATH=/etc/uenv/server.yaml
UENV_SERVER_CONFIG_STRICT=1
UENV_OBS_ENABLED=1
UENV_OBS_HTTP_LISTEN=127.0.0.1:50053
UENV_OBS_DATA_DIR=/var/lib/uenv/server/obs
UENV_TRAJECTORY_ENABLED=1
UENV_TRAJECTORY_HTTP_LISTEN=127.0.0.1:8077
UENV_TRAJECTORY_DATA_DIR=/var/lib/uenv/server/trajectory
```

多机轨迹采集确实需要 UEnv Worker 访问 `8077/TCP` 时，再把该接口改为受控内网地址，同时设置高熵 `UENV_TRAJECTORY_TOKEN`、在 UEnv Worker 使用相同 token，并只向需要的主机放行端口。UEnv Worker 侧的对应配置是 `/etc/uenv/worker.env` 的 `UENV_TRAJECTORY_ENDPOINT`（安装时可用 install.sh 的 `--trajectory-endpoint` 指定）和 `/etc/uenv/secrets/swe.env` 的 `UENV_TRAJECTORY_TOKEN`；安装器在每台主机各自生成回环地址和随机 token，多机部署需逐台修改并与 UEnv Server 保持一致，否则轨迹不会集中保存。注意 `prepare-swe --reset-swe-key` 会重新生成该 token，执行后需重新同步到各 UEnv Worker。远程 Obs 同理设置 `UENV_OBS_TOKEN`。跨不受信任网络时还需要 TLS 终止或 VPN；Bearer token 本身不加密链路。

## 常用调度设置

`/etc/uenv/server.yaml` 中常用的用户设置：

```yaml
port: 50051
admin_http_bind: "127.0.0.1"
admin_http_port: 50052

scheduler:
  strategy: round_robin
  heartbeat_interval_ms: 5000
  heartbeat_timeout_secs: 30
  worker_offline_timeout_secs: 90

episode:
  default_timeout_secs: 1800
  max_attempts: 3
  queue_dynamic: true

persistence:
  enabled: true
  db_path: /var/lib/uenv/server/server-state.db
```

| 字段 | 含义 |
|---|---|
| `port` | gRPC 端口（保留字段，实际监听由 `server.env` 的 `UENV_ADDR` 决定） |
| `admin_http_bind` / `admin_http_port` | Admin HTTP 运维接口的监听地址与端口 |
| `scheduler.strategy` | 调度策略；`round_robin` 表示在可用 UEnv Worker 之间轮转 |
| `scheduler.heartbeat_interval_ms` | UEnv Worker 心跳间隔，单位**毫秒**（`5000` 即 5 秒） |
| `scheduler.heartbeat_timeout_secs` | 心跳超时，单位**秒**；超时后该 UEnv Worker 暂停参与新调度 |
| `scheduler.worker_offline_timeout_secs` | 判定 UEnv Worker 离线的时间，单位**秒**；必须大于心跳超时 |
| `episode.default_timeout_secs` | 请求未指定超时时的 Episode 业务超时，单位**秒** |
| `episode.max_attempts` | 单条 Episode 的最大尝试次数 |
| `episode.queue_dynamic` | 按已注册 UEnv Worker 的总容量动态控制并发 |
| `persistence.enabled` / `persistence.db_path` | 是否持久化调度状态，以及状态数据库路径 |

`port` 是保留配置字段，实际 gRPC 监听由 `server.env` 的 `UENV_ADDR` 决定。不要只修改 `port`；确需改端口时同时修改两处，并同步 UEnv Worker 的 `server.endpoint` 和防火墙规则。

完整字段见[UEnv Server 与 UEnv Worker 配置参考](../6-查阅参考/02-configuration.md)，端口用途见[端口与连接方向](../6-查阅参考/03-ports.md)。

## 修改配置并重启

用 `sudoedit` 修改需要的文件（`sudoedit /etc/uenv/server.yaml` 或 `sudoedit /etc/uenv/server.env`），然后校验并重启：

```bash
sudo -u uenv env UENV_CONFIG_PATH=/etc/uenv/server.yaml \
  /opt/uenv/current/bin/uenv-adapter-core --validate-config
sudo systemctl restart uenv-adapter-core.service
sudo systemctl is-active uenv-adapter-core.service
curl -fsS http://127.0.0.1:50052/health
uenv workers
```

完成标志：配置校验退出码为 0，服务输出 `active`，健康请求成功，并且预期 UEnv Worker 在下一轮心跳后恢复为 `ready`。如果未恢复，按[故障排查](../5-运维UEnv/02-troubleshooting.md#uenv-server-重启后-uenv-worker-没有恢复)检查。
