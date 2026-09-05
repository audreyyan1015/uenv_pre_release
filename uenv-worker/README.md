# uenv-worker — UEnv Worker

UEnv Worker 是环境执行节点。它接收 UEnv Server 派发的 Episode，运行环境、调用模型接口、计算回报，并把轨迹与最终结果返回 Server。

```text
评测程序 / 强化学习框架 → UEnv Bridge → UEnv Server → UEnv Worker
```

本页主要面向 Worker 维护者。首次部署请先阅读 [配置并注册 UEnv Worker](../Docs/guide/2-部署UEnv/04-worker-registration.md)。

## 运行时职责

- `WorkerGrpcService`：接收 Server 的 `DispatchEpisode`、取消和环境准备请求。
- `ControlPlaneService` client：向 Server 执行 `RegisterWorker`、`WorkerHeartbeat` 和 `ReportResult`。
- 环境执行：通过 process plugin、SWE Runtime 或其他 backend 完成 reset/step/close。
- 实例池：按需创建并复用环境实例；发布配置默认不在启动时预热。
- 最终结果 WAL：Server 确认前持久化结果，并在暂时断连时重放。
- Hub 接入：可选地读取环境元数据并加载已经同步到本机的 EnvPackage。

## 安装部署

发布安装：

```bash
sudo bash install.sh \
  --bundle ./uenv-linux-x86_64.tar.gz \
  --profile worker \
  --server 10.0.0.10:50051 \
  --advertise 10.0.0.21:50054
```

`--server` 是 UEnv Server 地址；`--advertise` 是 Server 回连本 Worker 的地址。两个方向都必须可达。

安装后：

```bash
sudo systemctl is-active uenv-worker.service
curl -fsS http://127.0.0.1:19090/health
sudo -u uenv uenv doctor
```

然后在 Server 主机运行：

```bash
uenv status
uenv workers
```

验收标准是 Worker 状态为 `ready`、endpoint 与 `--advertise` 一致、心跳持续刷新，并且 Server 可连接该 endpoint。

## 配置

发布配置路径为 `/etc/uenv/worker.yaml`：

```yaml
server:
  endpoint: "10.0.0.10:50051"

worker:
  id: "auto"
  listen: "0.0.0.0:50054"
  advertise_endpoint: "10.0.0.21:50054"
  max_concurrent: 4

scheduler:
  mode: "remote"

env:
  types: ["qa", "math", "code"]
  backend: "process"
  plugin_dir: "/opt/uenv/current/plugins"
  package_plugin_dir: "/var/lib/uenv/plugins"

pool:
  warmup_size: 2
  prewarm_on_startup: false

wal:
  dir: "/var/lib/uenv/worker/wal"

observability:
  metrics_listen: "0.0.0.0:19090"
  health_listen: "0.0.0.0:19090"
```

配置优先级是 CLI 日志选项 > 环境变量 > YAML > 代码默认值。环境变量与完整字段表见 [UEnv Server 与 UEnv Worker 配置参考](../Docs/guide/6-查阅参考/02-configuration.md)。

校验配置：

```bash
sudo -u uenv /opt/uenv/current/bin/uenv-worker \
  --config /etc/uenv/worker.yaml validate-config
```

## 注册、心跳与重新注册

1. Worker 启动后向 Server 注册自己的地址、环境能力和容量。
2. Worker 定期上报健康状态和当前负载。
3. Server 通过 Worker 公布的地址派发 Episode。
4. Worker 完成任务后把最终结果返回 Server。
5. Server 重启或注册丢失时，Worker 自动重新注册。

面向用户的配置步骤见 [配置并注册 UEnv Worker](../Docs/guide/2-部署UEnv/04-worker-registration.md)；协议字段和可靠性机制见 [协议与调用方向](../Docs/guide/6-查阅参考/04-protocols.md)。

## 源码运行

开发配置示例位于 `config/uenv-worker.yaml`。启动前应把其中的 `server.endpoint`、`worker.listen` 和可选 `worker.advertise_endpoint` 改为当前开发拓扑：

```bash
cargo run -p uenv-worker -- \
  --config config/uenv-worker.yaml \
  validate-config

cargo run -p uenv-worker -- \
  --config config/uenv-worker.yaml \
  serve
```

查看版本和本地健康：

```bash
cargo run -p uenv-worker -- version
cargo run -p uenv-worker -- \
  --config config/uenv-worker.yaml \
  health
```

## 构建与测试

```bash
cargo check -p uenv-worker
cargo test -p uenv-worker
```

协议定义：

- Worker 接收服务：`uenv-worker/proto/worker_service.proto`
- 注册、心跳和结果：`proto/uenv/v1/scheduler.proto`
- Episode 数据：`proto/uenv/v1/episode.proto`
- process plugin：`plugin_proto/uenv/plugin/v1/plugin.proto`
