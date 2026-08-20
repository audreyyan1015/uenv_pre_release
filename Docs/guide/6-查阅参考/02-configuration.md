# UEnv Server 与 UEnv Worker 配置参考

本页汇总 UEnv Server 与 UEnv Worker 的全部配置字段。首次安装按[单机部署](../2-部署UEnv/01-single-node.md)和[多机部署](../2-部署UEnv/02-multi-node.md)操作即可，默认值已经可用。

## 文件和生效方式

| 文件 | 主要内容 | 修改后操作 |
|---|---|---|
| `/etc/uenv/server.env` | UEnv Server 进程监听、Obs、trajectory | 校验 UEnv Server 配置并重启 UEnv Server |
| `/etc/uenv/server.yaml` | 调度、心跳、Episode、Admin、持久化 | 校验 UEnv Server 配置并重启 UEnv Server |
| `/etc/uenv/worker.env` | UEnv Worker 进程环境变量 | 校验 UEnv Worker 配置并重启 UEnv Worker |
| `/etc/uenv/worker.yaml` | UEnv Server 地址、UEnv Worker 地址、环境和容量 | 校验 UEnv Worker 配置并重启 UEnv Worker |

修改前备份 `/etc/uenv`。密钥文件放在 `/etc/uenv/secrets/`，不要提交到仓库或写入普通日志。

## UEnv Server 进程设置

### `/etc/uenv/server.env`

| 变量 | 发布模板值 | 什么时候修改 |
|---|---|---|
| `UENV_ADDR` | `0.0.0.0:50051` | 修改 UEnv Server gRPC 实际监听地址 |
| `UENV_CONFIG_PATH` | `/etc/uenv/server.yaml` | YAML 位于其他路径时 |
| `UENV_SERVER_CONFIG_STRICT` | `1` | 生产环境保持严格校验 |
| `UENV_OBS_ENABLED` | `1` | 是否启用观测接口 |
| `UENV_OBS_HTTP_LISTEN` | `127.0.0.1:50053` | 集中观测确需远程访问时，改为受控内网地址并配置 token |
| `UENV_OBS_DATA_DIR` | `/var/lib/uenv/server/obs` | 观测数据需要独立磁盘时 |
| `UENV_OBS_TOKEN` | 未设置 | Obs 非回环监听时必须设置高熵 token |
| `UENV_TRAJECTORY_ENABLED` | `1` | 是否启用集中轨迹接口 |
| `UENV_TRAJECTORY_HTTP_LISTEN` | `127.0.0.1:8077` | UEnv Worker 确需集中上传时，改为受控内网地址并配置 token |
| `UENV_TRAJECTORY_DATA_DIR` | `/var/lib/uenv/server/trajectory` | 轨迹数据需要独立磁盘时 |
| `UENV_TRAJECTORY_TOKEN` | 未设置 | trajectory 非回环监听时必须设置高熵 token |

安全要求：Obs 或 trajectory 绑定非回环地址时，必须同时配置 token 和网络访问限制；跨不受信任网络还需要 TLS 终止或 VPN。

## UEnv Server 调度和持久化

### `/etc/uenv/server.yaml`

常用字段：

| 配置键 | 发布默认值 | 说明 |
|---|---:|---|
| `port` | 50051 | 保留端口字段；实际 bind 由 `UENV_ADDR` 控制 |
| `admin_http_bind` | `127.0.0.1` | Admin HTTP 监听地址 |
| `admin_http_port` | 50052 | 设为 0 可禁用 Admin HTTP |
| `admin_http_token` | 空 | 非空时 `/status` 等要求 Bearer 或 `X-Admin-Token`；`/health` 例外 |
| `scheduler.heartbeat_interval_ms` | 5000 | UEnv Server 建议的 UEnv Worker 心跳间隔 |
| `scheduler.heartbeat_timeout_secs` | 30 | 超时后 UEnv Worker 暂停参与新调度 |
| `scheduler.worker_offline_timeout_secs` | 90 | 观测接口显示离线的阈值；必须大于心跳超时 |
| `episode.default_timeout_secs` | 1800 | 请求未指定时的默认 Episode 超时 |
| `episode.max_attempts` | 3 | Episode 最大尝试次数 |
| `episode.queue_dynamic` | `true` | 按已注册 UEnv Worker 总容量动态控制并发 |
| `persistence.enabled` | `true` | 启用 UEnv Server 状态持久化 |
| `persistence.db_path` | `/var/lib/uenv/server/server-state.db` | SQLite 数据库路径 |

高级调度字段：

| 配置键 | 发布默认值 | 说明 |
|---|---:|---|
| `scheduler.strategy` | `round_robin` | 当前支持的调度策略 |
| `scheduler.schedule_retry_interval_ms` | 500 | 暂无可用 UEnv Worker 时的重试间隔 |
| `scheduler.worker_degraded_threshold_secs` | 400 | 有负载但长时间无最终结果时标记 `degraded` |

不要只修改 `port`。变更 gRPC 端口时，同时修改 `UENV_ADDR`、`port`、所有 UEnv Worker 的 `server.endpoint` 和防火墙规则，否则配置显示值与实际监听会不一致。

`deploy/config/server.yaml` 是发布模板；`config/server.yaml` 是源码开发示例。生产环境以 `/etc/uenv/server.yaml` 为准。

## UEnv Worker 设置

### `/etc/uenv/worker.yaml`

| 配置键 | 发布默认或生成值 | 说明 |
|---|---|---|
| `server.endpoint` | 安装器 `--server` | UEnv Server gRPC 地址 |
| `worker.id` | `auto` | 由 UEnv Server 分配；固定值须集群唯一 |
| `worker.listen` | `0.0.0.0:50054` | UEnv Worker gRPC 本机监听 |
| `worker.advertise_endpoint` | 安装器 `--advertise` | UEnv Server 实际能够回连的地址 |
| `worker.max_concurrent` | 4 | UEnv Worker Episode 并发上限 |
| `scheduler.mode` | `remote` | 发布运行使用远程 UEnv Server |
| `env.types` | `qa, math, code` | UEnv Worker 声明的环境类型；`math` 为兼容值，新任务使用 `qa` |
| `env.plugin_dir` | `/opt/uenv/current/plugins` | 内置插件目录 |
| `env.package_plugin_dir` | `/var/lib/uenv/plugins` | UEnv Hub 激活插件目录 |
| `pool.warmup_size` | 2 | 环境实例池目标大小 |
| `pool.prewarm_on_startup` | `false` | 默认收到 Episode 后按需创建实例 |
| `wal.dir` | `/var/lib/uenv/worker/wal` | 最终结果待确认记录目录 |
| `observability.metrics_listen` | `0.0.0.0:19090` | `/metrics`；当前必须与 health 地址相同 |
| `observability.health_listen` | `0.0.0.0:19090` | `/health` |
| `hub.enabled` | `false` | 是否连接 UEnv Hub |
| `hub.endpoint` | `http://127.0.0.1:8080` | UEnv Hub 地址；仅在 enabled 时使用 |
| `hub.token_file` | 空 | reader token 文件 |

常用环境变量覆盖：

| 环境变量 | 覆盖字段 |
|---|---|
| `UENV_SERVER_ENDPOINT` | `server.endpoint` |
| `UENV_WORKER_LISTEN` | `worker.listen` |
| `UENV_WORKER_ADVERTISE_ENDPOINT` | `worker.advertise_endpoint` |
| `UENV_WORKER_ID` | `worker.id` |
| `UENV_MAX_CONCURRENT` | `worker.max_concurrent` |
| `UENV_ENV_TYPES` | `env.types`，逗号分隔 |
| `UENV_PLUGIN_DIR` | `env.plugin_dir` |
| `UENV_PACKAGE_PLUGIN_DIR` | `env.package_plugin_dir` |
| `UENV_METRICS_LISTEN` | `observability.metrics_listen` |
| `UENV_HEALTH_LISTEN` | `observability.health_listen` |
| `UENV_HUB_ENDPOINT` | `hub.endpoint`，并启用 UEnv Hub |
| `UENV_HUB_TOKEN_FILE` | `hub.token_file` |
| `UENV_WORKER_LLM_ENV` | 模型密钥环境文件路径 |

UEnv Worker 加载优先级是：CLI 日志选项 > 环境变量 > YAML > 代码默认值。发布 systemd 服务还会依次加载 `worker.env`、模型密钥和可选 SWE 环境文件，因此同名环境变量可以覆盖 YAML。

UEnv Hub 服务自身的配置和鉴权流程见[部署和使用 UEnv Hub](../2-部署UEnv/05-hub.md)。

## 校验配置

在对应主机运行：

```bash
sudo -u uenv env UENV_CONFIG_PATH=/etc/uenv/server.yaml \
  /opt/uenv/current/bin/uenv-adapter-core --validate-config
sudo -u uenv /opt/uenv/current/bin/uenv-worker \
  --config /etc/uenv/worker.yaml validate-config
```

两个命令都应以退出码 0 结束。校验只检查配置结构和基本约束，不代替服务健康、UEnv Worker 注册与双向网络验收。
