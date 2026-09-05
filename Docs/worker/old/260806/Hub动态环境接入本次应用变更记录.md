# Hub 动态环境接入本次应用变更记录

> 日期：2026-08-06  
> 范围：本次围绕“Hub 已注册但 Worker 尚未支持的环境，按需动态拉取、部署入池、调度执行、前端可见”的实际代码与部署变更。  
> 关联：
> - [Hub注册未知环境的Worker动态接入补齐方案.md](./Hub注册未知环境的Worker动态接入补齐方案.md)
> - [UEnv框架整体使用手册.md](./UEnv框架整体使用手册.md)
> - [SWE-GRPO预热池与Agent-Worker调度讨论.md](./SWE-GRPO预热池与Agent-Worker调度讨论.md)

---

## 1. 本次目标

本次调整的直接目标是把“Worker 本地未预先声明的环境类型”从规划推进为可运行链路：

```text
Episode(env_type 未被 Worker 支持)
  -> Server 调度发现无 ready Worker
  -> Server 选择可 prepare 的 Worker
  -> Worker 从 Hub 拉取 env manifest
  -> Worker 准备环境 / 注册本地运行入口 / 加入实例池
  -> Worker 心跳上报新能力和池状态
  -> Server 重新 reserve 并 dispatch
  -> 前端实时显示支持环境、已准备包、实例池
```

首轮动态未知环境支持范围为：

- `process`：本地 process plugin；
- `openenv_http` / `generic_openenv_plugin`：由 Worker 动态生成本地 shim，并通过 `uenv-openenv-plugin` 转接 OpenEnv HTTP。

纯 `container` / `docker` manifest 本轮不自动进入通用动态 backend，避免把容器环境误当 process plugin 执行；SWE 仍走已有 `swe_instance_pool` / Runtime Gateway / EnvPackage 专用路径。

---

## 2. 协议变更

### 2.1 Scheduler proto

文件：`proto/uenv/v1/scheduler.proto`

新增/扩展：

- `EnvPackageState`
- `WorkerPoolSummary`
- `WorkerPoolSlot`
- `RegisterWorkerRequest` 增加：
  - `platform_features`
  - `backend_kinds`
  - `trajectory_schemas`
  - `tool_schemas`
  - `package_states`
  - `pool_summary`
  - `pool_slots`
- `HeartbeatRequest` 增加同类 Worker capability / pool snapshot 字段。
- `WorkerInfo` 增加同类字段，用于 admin / frontend 查询。

### 2.2 Worker service proto

文件：`uenv-worker/proto/worker_service.proto`

新增：

```text
PrepareEnvironment(PrepareEnvironmentRequest)
  -> PrepareEnvironmentResponse
```

用途：Server 在发现目标 `env_type` 当前无 ready Worker 时，远程命令 Worker 按 Hub manifest 准备环境。

---

## 3. Server 侧变更

主要文件：

- `uenv-server/src/scheduler/traits.rs`
- `uenv-server/src/scheduler/mod.rs`
- `uenv-server/src/control_plane.rs`
- `uenv-server/src/service/episode.rs`
- `uenv-server/src/admin_query.rs`
- `uenv-server/src/admin_http.rs`
- `uenv-server/src/service/admin.rs`
- `uenv-server/src/ports.rs`

已落地能力：

1. Scheduler WorkerInfo 扩展为 capability + package + pool 结构。
2. `reserve` 仍优先选择已经 ready 的 Worker。
3. 无匹配 Worker 时，Server 可调用 `select_preparable_worker`。
4. `submit_native_worker_episode` 在 `NoMatchingEnvType` / `NoMatchingEnvPackage` 时触发 `prepare_worker_for_request`。
5. prepare 成功后 Server 标记 Worker 环境已 ready，并重新 reserve / dispatch。
6. admin HTTP `/status` / `/fleet/workers` 输出新增能力字段、EnvPackage 状态和实例池快照。

---

## 4. Worker 侧变更

主要文件：

- `uenv-worker/src/control_plane/client.rs`
- `uenv-worker/src/grpc_server/worker_service.rs`
- `uenv-worker/src/pool/warmup_pool.rs`
- `uenv-worker/src/hub/mod.rs`
- `uenv-worker/src/hub/env_resolver.rs`
- `uenv-worker/src/runtime.rs`
- `uenv-worker/src/backend/process.rs`
- `uenv-worker/src/bin/uenv-openenv-plugin.rs`

已落地能力：

1. `WarmupPool::prepare_env` 暴露按 `env_type` 准备环境的入口。
2. `WarmupPool::snapshot` 输出 `pool_summary` 和 `pool_slots`。
3. Worker 注册/心跳共享 `SharedWorkerCapabilities`，可在 prepare 后即时更新 `supported_env_types` 和 `package_states`。
4. `WorkerGrpcServiceImpl::prepare_environment` 调用 `warmup_pool.prepare_env`，成功后标记 package ready。
5. 新增 `uenv-openenv-plugin`：
   - 作为 OpenEnv HTTP shim；
   - 转接 `/reset`、`/step`、`/close`；
   - 把 OpenEnv HTTP 返回值映射成现有 plugin episode 轨迹与 reward。
6. Hub manifest 映射规则：
   - `openenv_http` / `generic_openenv_plugin` / HTTP entrypoint 自动生成 `plugins/{env_type}/run.sh`；
   - shim entry 固定为 `./run.sh`，避免把 HTTP URL 当本地路径 spawn；
   - `supported_backends` 映射为 `process` + `generic_openenv_plugin`；
   - container-only manifest 不再注册进 process `PluginHost`。
7. `ProcessBackend` 使用 `/bin/bash` 并增强 spawn 错误上下文。
8. 7143 重启脚本增加：
   - `UENV_OPENENV_PLUGIN_BIN=/root/UEnv/target/release/uenv-openenv-plugin`

---

## 5. 前端变更

主要文件：

- `frontend/src/lib/types/chain-state.ts`
- `frontend/src/lib/worker-tree.ts`
- `frontend/src/hooks/use-worker-fleet-live.ts`
- `frontend/src/components/worker-detail.tsx`

已落地能力：

1. 类型层增加 Worker capability / package / pool 字段。
2. `/fleet/workers` hook 解析：
   - `supported_env_types`
   - `platform_features`
   - `backend_kinds`
   - `trajectory_schemas`
   - `tool_schemas`
   - `package_states`
   - `pool_summary`
   - `pool_slots`
3. Worker 详情页展示：
   - 支持环境类型；
   - endpoint；
   - 当前活跃 Episode；
   - 环境实例数；
   - Worker 平台能力；
   - 轨迹 / 工具协议；
   - 已准备 EnvPackage；
   - 实例池 summary 和 slots。
4. 修复了本次联调中发现的 Worker 详情页运行时问题：
   - `worker` 初始化前被引用导致的 TDZ runtime error；
   - 首屏 `Date.now()` 导致的 hydration mismatch。

---

## 6. 部署变更

### 6.1 Server

部署位置：`8.130.75.157`

实际运行：

```text
/home/uenv/UEnv/target/release/uenv-adapter-core
```

监听：

- gRPC：`0.0.0.0:8088`
- trajectory HTTP：`0.0.0.0:8077`
- admin HTTP：`127.0.0.1:50052`

说明：原生产入口实际为 `/usr/local/bin/uenv-adapter-core` / `/home/uenv-frontend-add` 相关进程。本次已切换到 `/home/uenv/UEnv/target/release/uenv-adapter-core`。

### 6.2 Worker

部署位置：A100 7143 `/root/UEnv`

实际运行：

```text
./target/release/uenv-worker --config config/uenv-worker.deploy-7143-swe-pro.yaml serve
```

监听：

- Runtime Gateway：`0.0.0.0:28097`
- health：`0.0.0.0:28777`
- Worker gRPC：`0.0.0.0:28888`

### 6.3 Frontend

部署位置：`8.130.75.157:/home/uenv-frontend-add/frontend`

实际运行：

```text
./node_modules/.bin/vite dev --port 8888 --host 0.0.0.0
```

访问：

```text
http://8.130.75.157:8888/server/worker?worker=worker-7143-pro
```

---

## 7. 验证结果

### 7.1 构建 / 测试

已通过：

- 远端 7143 release build：
  - `uenv-server`
  - `uenv-worker`
  - `uenv-adapter-core`
- Worker 单测：
  - OpenEnv HTTP manifest 使用生成 shim entry；
  - container-only manifest 不注册为 process plugin。
- 前端本地 build：
  - `pnpm -C frontend build`
- 前端远端 build：
  - `/home/uenv-frontend-add/frontend npm run build`

本地 macOS 未跑完整 Rust `cargo check`，原因是本机缺 `protoc`；Rust 编译验证以 7143 Linux 远端为准。

### 7.2 隔离 smoke

隔离目录：`7143:/root/UEnv-codex-dynamic`

场景：

- Worker 初始只支持 `math`；
- mock Hub 发布 `dyn-openenv`；
- mock OpenEnv HTTP 提供 `/reset` / `/step` / `/close`；
- Server 收到 `env_type=dyn-openenv` 后触发 Worker prepare；
- Worker 拉取 manifest、生成 shim、注册 env、执行 episode。

结果：

```text
status=completed
reward=1
supported_env_types=["dyn-openenv","math"]
pool_summary 含 dyn-openenv ready slot
```

### 7.3 正式链路 smoke

真实 Hub：`http://8.130.95.176:8088`

发布临时环境：

```text
dyn-openenv-prod@0.1.2
supported_backends=["openenv_http"]
entrypoint="http://127.0.0.1:19181"
```

正式链路：

```text
Server: 8.130.75.157:8088
Worker: worker-7143-pro
OpenEnv HTTP: 7143 127.0.0.1:19181
```

执行结果：

```text
requestId=dyn-prod-ep-1
status=completed
reward=1
terminationReason=terminated
```

Server admin 当前可见：

```text
supported_env_types=["code","dyn-openenv-prod","qa","swe"]
package_states=3
pool_summary:
  dyn-openenv-prod ready=5 capacity=5
  code ready=4 capacity=4
  qa ready=4 capacity=4
```

### 7.4 前端验收

Playwright 打开：

```text
http://8.130.75.157:8888/server/worker?worker=worker-7143-pro
```

页面已显示：

- `dyn-openenv-prod`
- `swe-bench-pro@0.3.4`
- `swe-bench-smith@0.1.0-local`
- `hub_dynamic_env`
- `trajectory_v2_2`
- `artifact_uri`
- `reward_adapter_v1`
- `runtime/v1`
- `browser-tools/v1`
- `dyn-openenv-prod` 实例池 ready/capacity

截图：

```text
output/playwright/worker-detail-dynamic.png
```

---

## 8. 当前边界与后续缺口

已闭环：

- Hub registered unknown `env_type`；
- Worker 本地未预声明；
- Server 按需 prepare；
- Worker 从 Hub 拉取；
- OpenEnv HTTP shim 接入；
- episode 执行；
- reward 与 trajectory JSON 返回；
- artifact URI 作为附件引用；
- admin/frontend 实时可见。

仍未完整落地：

- 通用 Docker/container backend：自动下载 EnvPackage、`docker load`、启动容器、端口发现、健康检查、资源限制、volume/artifact 挂载、销毁回收；
- Agent scaffold 按 Episode Stack 动态 sync；
- reward adapter 独立制品运行器；
- 浏览器 artifact manifest 的强 schema 校验；
- SWE 专用池完全纳入统一 lease 账本；
- Hub manifest 中 `adapters` 字段的标准枚举和服务端强校验。

因此，自建 Docker 环境当前推荐方式是：

```text
Docker 容器内部启动 OpenEnv HTTP 服务
  -> Hub manifest 声明 openenv_http
  -> Worker 生成 shim
  -> 通过已落地动态接入链路调度执行
```

而不是直接声明纯 `container` 后端并期望 Worker 自动启动任意容器。
