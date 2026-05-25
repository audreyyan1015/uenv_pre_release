# uenv-mock-scheduler — Mock ControlPlane（Worker Pool MVP）

独立 crate，模拟 UEnv Scheduler 控制面：接受 Worker `RegisterWorker` / `Heartbeat` / `ReportResult`，并 **主动** 向 Worker 发起 `DispatchEpisode`（checklist M1.2）。

## CLI

```bash
# 启动 Mock ControlPlane（M1 实现业务逻辑）
uenv-mock-scheduler serve --config config/uenv-mock-scheduler.yaml

# 版本
uenv-mock-scheduler version
```

## 配置

见 `config/uenv-mock-scheduler.yaml`。

## 与 uenv-server 关系

Worker Pool MVP（M1–M6）**不依赖**完整 `uenv-server`；使用本 crate 联调 Worker。M7 起与真实 Server 集成。
