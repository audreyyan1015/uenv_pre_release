# 配置并注册 UEnv Worker

本页用于修复已经安装但没有出现在 `uenv workers` 中的 UEnv Worker：核对地址配置、校验并重启，最后在 UEnv Server 确认注册。完成后，UEnv Server 应显示该 UEnv Worker 为 `ready`，并且双方都能建立所需连接。

## 开始前：确认前提

在 UEnv Server 主机确认服务运行：

```bash
sudo systemctl is-active uenv-adapter-core.service
```

在 UEnv Worker 主机确认能访问 UEnv Server（把地址换成实际值）：

```bash
export SERVER_HOST=10.0.0.10
python3 -c 'import os, socket; socket.create_connection((os.environ["SERVER_HOST"], 50051), 5).close(); print("ok")'
```

预期输出 `ok`。再在两台主机分别执行 `uenv version`，确认安装版本一致。本文示例使用 UEnv Server `10.0.0.10`、UEnv Worker `10.0.0.21`。

## 区分两个地址

| 参数或配置 | 方向 | 正确值 |
|---|---|---|
| `--server` / `server.endpoint` | UEnv Worker → UEnv Server | UEnv Server 的可访问 gRPC 地址，例如 `10.0.0.10:50051` |
| `--advertise` / `worker.advertise_endpoint` | UEnv Server → UEnv Worker | UEnv Server 能回连的 UEnv Worker 地址，例如 `10.0.0.21:50054` |
| `worker.listen` | UEnv Worker 本机监听 | 通常为 `0.0.0.0:50054` |

`advertise_endpoint` 是 Worker 注册时上报给 UEnv Server 的回连地址，Server 通过它向 Worker 派发 Episode。跨主机时填写 `127.0.0.1` 会让 Server 连回自己，填写 `0.0.0.0` 则无法定位具体主机。跨 NAT、容器或多网卡部署时，本机监听地址和公布地址通常不同。

## 常用字段

`/etc/uenv/worker.yaml` 除地址外，日常运维最常接触的字段：

```yaml
worker:
  id: "auto"
  max_concurrent: 4

env:
  types: ["qa", "math", "code"]
  backend: "process"
  plugin_dir: "/opt/uenv/current/plugins"
  package_plugin_dir: "/var/lib/uenv/plugins"

pool:
  warmup_size: 2
  prewarm_on_startup: false

observability:
  metrics_listen: "0.0.0.0:19090"
  health_listen: "0.0.0.0:19090"

hub:
  enabled: false
  endpoint: "http://127.0.0.1:8080"
  token_file: ""

llm:
  env_file: "/etc/uenv/secrets/worker-llm.env"
```

| 字段 | 含义 |
|---|---|
| `worker.id` | `auto` 由 UEnv Server 分配，重装或重启可能变；生产环境建议固定为集群唯一值 |
| `worker.max_concurrent` | Episode 并发上限，即 `uenv workers` 中 `load=0/4` 的分母 |
| `env.types` | 上报给 UEnv Server 的环境类型；`math` 是兼容别名，新任务用 `qa`；SWE 由 `/etc/uenv/swe.env` 追加 |
| `env.backend` / `plugin_dir` / `package_plugin_dir` | 环境实现方式（进程插件）、内置插件目录、UEnv Hub 激活插件目录 |
| `pool.warmup_size` / `prewarm_on_startup` | 环境实例池目标大小；默认不预热，收到 Episode 按需创建 |
| `observability.metrics_listen` / `health_listen` | `/metrics` 与 `/health` 的监听地址；当前两者必须相同 |
| `hub.enabled` / `endpoint` / `token_file` | UEnv Hub 连接；见[部署和使用 UEnv Hub](./05-hub.md) |
| `llm.env_file` | 模型端点、模型名和密钥所在的环境文件；用 `uenv evaluate configure-model` 修改，不要手工写密钥 |

同名环境变量（如 `UENV_SERVER_ENDPOINT`、`UENV_MAX_CONCURRENT`、`UENV_ENV_TYPES`）优先于 YAML；systemd 服务会依次加载 `worker.env`、模型密钥文件和可选 SWE 环境文件。完整字段与环境变量对照见[配置参考](../6-查阅参考/02-configuration.md#uenv-worker-设置)。

## 修改配置并重启

如果 UEnv Worker 已安装但未出现在 `uenv workers` 中，先查看当前配置：

```bash
sudo grep -E 'endpoint|advertise|listen|id' /etc/uenv/worker.yaml
```

用 `sudoedit /etc/uenv/worker.yaml` 修正 `server.endpoint` 与 `worker.advertise_endpoint` 后，校验并重启：

```bash
sudo -u uenv /opt/uenv/current/bin/uenv-worker \
  --config /etc/uenv/worker.yaml validate-config
sudo systemctl restart uenv-worker.service
sudo systemctl is-active uenv-worker.service
curl -fsS http://127.0.0.1:19090/health
```

预期结果是配置校验成功、服务输出 `active`、健康请求退出码为 0。

## 验证双向网络

在 UEnv Worker 主机验证 UEnv Worker → UEnv Server：

```bash
export SERVER_HOST=10.0.0.10
python3 -c 'import os, socket; socket.create_connection((os.environ["SERVER_HOST"], 50051), 5).close()'
```

在 UEnv Server 主机验证 UEnv Server → UEnv Worker：

```bash
export WORKER_HOST=10.0.0.21
python3 -c 'import os, socket; socket.create_connection((os.environ["WORKER_HOST"], 50054), 5).close()'
```

两个命令都应无输出并以退出码 0 结束。连接失败时，分别检查 `server.endpoint`、`worker.advertise_endpoint`、监听地址和防火墙方向。

## 确认注册结果

在 UEnv Server 主机执行：

```bash
uenv status
uenv workers
```

`uenv workers` 应显示该 UEnv Worker 为 `[READY]`，endpoint 与实际地址一致，例如（Worker 名称以实际注册为准）：

```text
[READY] worker-01 (10.0.0.21:50054) load=0/4
  无运行中的 Episode
```

心跳时间应在 `uenv status` 中持续刷新。最后核对这台 Worker 上报的环境类型与本机配置一致。`uenv workers` 不显示环境类型，需要在 UEnv Server 主机查询 Admin HTTP（默认只监听 `127.0.0.1:50052`）：

```bash
curl -fsS http://127.0.0.1:50052/status | \
  jq -r '.workers[] | [.endpoint, .status, (.supported_env_types | join(","))] | @tsv'
```

每台 UEnv Worker 输出一行，依次是 endpoint、状态、支持的环境类型，例如：

```text
10.0.0.21:50054	ready	qa,math,code
```

在输出中找到 endpoint 为本机地址的那一行：状态应为 `ready`，第三列应与 `/etc/uenv/worker.yaml` 中 `env.types` 配置的类型一致。也可以在 UEnv Worker 本机用 `uenv environments` 查看它实际加载的环境。

## 安装新的 UEnv Worker

需要新增 UEnv Worker 主机时，按[多机部署](./02-multi-node.md#在第一台-uenv-worker-主机安装执行节点)的安装命令操作；安装完成后回到本页，从"修改配置并重启"一节开始核对。安装器把地址写入 `/etc/uenv/worker.yaml`；如果该文件已存在，安装器默认保留原配置。

## 自动恢复行为

- UEnv Worker 启动后会自动注册，并周期性上报心跳、负载和环境能力。
- UEnv Server 重启或丢失注册记录后，UEnv Worker 会在心跳响应中发现变化并重新注册。
- UEnv Worker 暂时无法上报最终结果时，会保留待确认结果并在恢复连接后重试。
- 心跳超时的 UEnv Worker 会暂停参与新调度，但记录不会因为一次超时立即删除。

这些行为由 UEnv Worker 与 UEnv Server 自动完成；协议字段细节见[协议与调用方向](../6-查阅参考/04-protocols.md#uenv-worker-注册与心跳)。

## 常见问题

| 现象 | 先检查 |
|---|---|
| UEnv Worker 健康但没有注册 | `server.endpoint` 和 UEnv Worker → UEnv Server `50051/TCP` |
| UEnv Worker 已注册但无法派发 | `worker.advertise_endpoint` 和 UEnv Server → UEnv Worker `50054/TCP` |
| 重启后出现新的 UEnv Worker ID | `worker.id` 是否为 `auto`；同时核对 endpoint |
| 重启后旧 ID 记录长期显示 `degraded` | 见下方说明 |
| 固定 ID 注册被拒绝 | 旧实例是否仍在运行或仍有活动 Episode |
| UEnv Worker 为 `degraded` | 心跳、当前负载、环境执行和结果上报是否正常 |

`worker.id` 为 `auto` 时，每次重装或重启都可能注册出一个新 ID，旧 ID 的记录不会自动删除：它转为 `degraded`、暂停参与新调度，但仍计入 `uenv status` 的 `Worker=N` 并在列表中长期保留（当前版本没有删除单条 Worker 记录的命令，记录随持久化数据保留）。验收 Worker 数量时应只统计 `ready` 记录。需要稳定 ID 时，在 `/etc/uenv/worker.yaml` 中为每台 Worker 设置集群内唯一的固定 `worker.id`。

日志和进一步处理见[故障排查](../5-运维UEnv/02-troubleshooting.md)。
