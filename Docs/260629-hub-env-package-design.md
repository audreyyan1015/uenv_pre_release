# Hub 环境组合包（EnvPackage）设计

> **文档版本**：v1.0  
> **日期**：2026-06-29  
> **状态**：**方案冻结（待实施）**  
> **定位**：将 Hub 从「元数据索引站」升级为 **环境组合包注册与制品分发中心**；明确组合包边界、预制内容与批量部署路径。  
> **关联**：`260627-swe-openhands-integration-plan.md`（当前联调态）、`hub/uenv-hub服务指南.md`、`260628-local-vllm-deepseekv3-openhands-swe-test-report.md` §8

---

## 1. 背景与问题

当前联调态下，除 Hub 元数据外，大量能力以 **散落在外部的配套设施** 形式存在：

| 位置 | 配套设施 | 现状问题 |
|------|----------|----------|
| Agent 机（如 208.77） | OpenHands + `integrations/openhands` 集成层（tool → Gateway HTTP） | 人工 rsync、版本与 Worker 不同步 |
| Worker 机（如 7143） | `uenv-worker` + Runtime Gateway + 轨迹 seal/upload | yaml/env 分散；SWE 镜像 miss 时联网 pull 第三方 |
| 独立节点 | Trajectory Server、LLM Gateway | endpoint 硬编码在各机 env 文件 |

**Hub 仅提供 catalog / manifest 索引** 时，无法表达「一个可版本化、可批量克隆的 SWE 或 Agent 环境整体」，导致：

1. 每台 Agent / Worker 需单独部署、单独对齐版本；  
2. 镜像与元数据脱节，带宽与可用性受第三方 registry 制约；  
3. `gateway_url`、集成层、grader 配置无法作为 **同一环境版本** 发布与回滚。

本文采用 **组合包（EnvPackage）方案**：Hub 注册 **多制品 + 配置清单 + 版本约束** 的 bundle，**不是** 将 SWE 容器、Gateway、Server、Agent 打进同一个 OCI 镜像。

---

## 2. 设计原则

### 2.1 三层分离（必须同时满足）

```text
┌──────────────────────────────────────────────────────────────┐
│ A. UEnv 平台（Platform Release）                              │
│    随 uenv-worker / uenv-server 二进制或镜像部署一次           │
│    全 env_type 复用；版本随 Git tag / 发布流水线               │
└──────────────────────────────────────────────────────────────┘
                              ▲ 读取 EnvPackage 配置与制品
┌──────────────────────────────────────────────────────────────┐
│ B. 环境组合包（Hub EnvPackage）                               │
│    环境注册方发布；版本化；Worker/Agent 节点 bulk sync         │
└──────────────────────────────────────────────────────────────┘
                              ▲ 任务调度时注入
┌──────────────────────────────────────────────────────────────┐
│ C. 运行时调度态（Server / 控制面）                            │
│    每次 Episode 不同：gateway_url、session_id、run_id、租约    │
└──────────────────────────────────────────────────────────────┘
```

| 原则 | 说明 |
|------|------|
| **组合非单体** | 一个 EnvPackage 引用多类制品（镜像集、catalog、配置 overlay、agent bridge 包），各制品独立 digest |
| **平台与业务解耦** | Gateway、轨迹 seal 协议、池化逻辑属 **A**，不随每个 SWE 实例重复打包 |
| **制品内网预制** | B 中镜像与大数据文件走 **内网 registry / 对象存储 + manifest 索引**；Worker/Agent **默认不** 访问公网第三方 pull |
| **URL 调度下发** | 具体 `gateway_url` 属 **C**，由 Server 选 Worker 后写入 `AgentJob`；Hub 只声明能力与默认值 |
| **密钥不进 Hub** | API key、token 走部署密钥系统；Hub 仅引用 secret 名称 |

### 2.2 明确不做

| 不做 | 原因 |
|------|------|
| 单一「大镜像」含 SWE + Gateway + Server + Agent | 生命周期、扩缩维度、安全边界完全不同 |
| 在 SWE 评测容器内安装 Gateway / 轨迹 Agent / OpenHands | 沙箱应保持官方 harness 语义；审计在 Worker 宿主机 Gateway 边界 |
| Hub 参与 Episode 热路径调度 | Hub 保持注册与分发；调度仍在 Server |

---

## 3. 组合包（EnvPackage）定义

### 3.1 概念

**EnvPackage** = 一个可版本化的环境发布单元，包含：

- **制品清单（artifacts）**：可批量同步的 blobs（镜像、catalog JSON、agent bridge 包等）  
- **Worker 配置 overlay**：grader、policy、轨迹策略默认值等  
- **Agent 桥接引用**：使用哪个 framework bridge 包及版本  
- **平台约束**：所需的 `uenv_worker` 最低版本与 platform feature 标志  

环境注册方在 Hub **发布** `env_type@version`；Worker / Agent 节点执行 **`uenv env sync <package>`**（待 CLI 实现）完成本地预制。

### 3.2 Manifest 结构（草案）

```yaml
# Hub: GET /api/v1/envs/{env_type}/versions/{version}
# 或 GET /api/v1/packages/{package_id}/versions/{version}

package_id: swe-bench-pro          # 逻辑名，可与 env_type 相同
version: 2.1.0
published_at: 1730000000
publisher: org-uenv-swe              # 环境注册方

# ── 平台依赖（A 层）────────────────────────────────────────
platform:
  uenv_worker_min: "0.9.0"
  uenv_server_min: "0.8.0"           # 可选
  features:                          # Worker 必须启用的能力
    - runtime_gateway
    - trajectory_v2_2
    - swe_instance_pool

# ── 制品（B 层：可 bulk 部署）──────────────────────────────
artifacts:

  # 1) SWE 沙箱镜像集（内网预制，按 digest 锁定）
  images:
    bundle_ref: hub://artifacts/swe-bench-pro/2.1.0/images.manifest.json
    bundle_digest: sha256:...
    sync_mode: registry              # registry | tarball | rsync
    registry_prefix: registry.internal/uenv/swe-pro
    # images.manifest.json 内为 [{instance_id, image, digest}, ...]

  # 2) Benchmark 实例目录（与本地 swe_instances.json 同构）
  catalog:
    ref: hub://artifacts/swe-bench-pro/2.1.0/catalog.json
    digest: sha256:...
    variant: pro                       # pro | verified

  # 3) Grader / 评测规格（可选独立文件）
  eval_spec:
    ref: hub://artifacts/swe-bench-pro/2.1.0/eval_spec.yaml
    digest: sha256:...

  # 4) Agent 桥接包（Agent 节点 bulk 部署）
  agent_bridge:
    package: uenv-agent-openhands      # 独立包 ID
    version: 1.0.2
    ref: hub://artifacts/uenv-agent-openhands/1.0.2/package.tar.zst
    digest: sha256:...
    openhands_sdk_pin: "1.27.0"        # 声明兼容的 OpenHands 版本

# ── Worker 配置 overlay（B 层：合并进本地 worker yaml）────
worker_overlay:
  swe:
    benchmark_variant: pro
    command_mode: FullShell
    grader: swebench_pro
    image_pull_policy: local_only      # local_only | mirror | allow_public
  runtime_gateway:
    enabled: true
    # listen / capacity 可由节点 profile 覆盖
  trajectory:
    enabled: true
    # endpoint 模板；实际 URL 用部署 env 或 secret 注入
    upload_endpoint_ref: secret://trajectory-server-url
    artifact_dir: /var/lib/uenv/trajectories

# ── Agent 默认（B 层：Agent 镜像/venv 构建用）──────────────
agent_defaults:
  driver_entrypoint: run_swebenchpro_official.py
  workspace_dir: /app
  tools: [terminal, file_editor]
  integration_patch: patch_openhands_tools_for_uenv
  max_iterations_default: 30

# ── 接口契约版本（非运行时 URL）────────────────────────────
contracts:
  runtime_gateway_api: runtime/v1      # 与 uenv-worker gateway 路由版本对齐
  trajectory_bundle_schema: v2.2
  tool_bridge_schema: openhands-uenv-v1
```

---

## 4. 纳入组合包的内容（确认清单）

### 4.1 ✅ 应纳入 Hub 组合包（可预制、可批量部署）

| 类别 | 具体内容 | 同步目标 | 说明 |
|------|----------|----------|------|
| **沙箱镜像集** | SWE-bench Pro/Verified 实例镜像列表 + digest | Worker 节点 docker/containerd store | 通过内网 registry 或 tarball 批量 load；**禁止**生产默认公网 pull |
| **Benchmark catalog** | `instances.json`（instance_id、patch、F2P、image 引用等） | Worker `InstanceStore` / 本地 catalog 目录 | 与镜像 manifest 版本绑定 |
| **评测规格** | grader 名、log_parser、外部 `UENV_SWE_PRO_EVAL_CMD` 模板、install_cmd 策略 | Worker 配置或 catalog 内嵌 | 与 variant 绑定 |
| **Worker 配置 overlay** | `benchmark_variant`、`command_mode`、池容量建议、镜像 pull 策略 | 合并进 `uenv-worker.yaml` | 节点级 listen/port 可覆盖 |
| **轨迹策略默认值** | 是否 seal、artifact 根目录、upload 是否启用 | Worker env / yaml | **不含** token 明文 |
| **Agent bridge 包** | `uenv-agent-openhands`（含 `uenv_runtime/`、driver、patch 逻辑） | Agent 池节点 `/opt/uenv/agents/` 或容器层 | 版本与 OpenHands SDK pin 绑定 |
| **Agent 默认参数** | driver 入口、tool 列表、workspace_dir、默认 max_iterations | Agent Job 模板 | 运行时可被 Server 覆盖 |
| **契约版本号** | Gateway API 版本、trajectory schema、tool_bridge schema | 校验用 | 防止 Worker/Agent 与包不兼容 |
| **平台最低版本** | `uenv_worker_min`、`features[]` | 部署前检查 | sync 前拒绝不兼容组合 |

### 4.2 ⚠️ 部分纳入（Hub 给模板/引用，部署层填具体值）

| 类别 | Hub 提供 | 部署/调度层提供 |
|------|----------|-----------------|
| Trajectory Server URL | `upload_endpoint_ref` 或模板 `http://{trajectory_cluster}:8077` | 实际 host、token（secret） |
| LLM endpoint | `model_profile` 引用（如 `deepseek-v3-awq@7142`） | Server `AgentJob.model_endpoint` 具体 URL |
| Gateway listen / capacity | 建议值 `capacity: 8` | 节点硬件与 `uenv-worker.yaml` 覆盖 |
| Hub / Server 地址 | 环境包不重复定义 | 各层 `UENV_HUB_ENDPOINT` 等部署常量 |

### 4.3 ❌ 不纳入组合包（归属其他层）

| 内容 | 归属 | 原因 |
|------|------|------|
| `uenv-worker` / `uenv-server` 二进制与核心 Rust 代码 | **A. UEnv 平台 Release** | 全环境复用；随平台升级 |
| Runtime Gateway 实现、`push_step`、seal、upload 逻辑 | **A. 平台** | 通用机制；非 SWE 专属脚本 |
| Trajectory Server 服务进程 | **独立平台服务** | 聚合层；多 Worker 共享 |
| LLM Gateway / vLLM | **独立推理层** | GPU 与 SWE 分离 |
| **`gateway_url`（含 host:port）** | **C. Server 调度态** | 取决于本次派发的 Worker |
| **`session_id`、`run_id`** | **C. 调度态** | 每次 Episode 生成 |
| **API key、token 明文** | **密钥系统** | 不进 Hub 明文存储 |
| OpenHands 上游完整源码树 | **Agent 基础镜像** 或 vendor 缓存 | 体积大；由 `agent_bridge.openhands_sdk_pin` 约束版本，不重复打包进每个 swe 包 |
| SWE 容器内轨迹脚本 | **不应存在** | 轨迹在 Worker Gateway 边界采集 |

### 4.4 两大配套设施专项拆分（Agent 集成层 × Worker 轨迹）

联调态下最显眼的两个「外部配套」是：**Agent 机上的 tool→API 映射**，以及 **Worker 上的轨迹采集**。二者都**不应**塞进 SWE 沙箱镜像，也**不应**全部手搓在每台机器上；拆分如下。

#### 4.4.1 Agent 集成层（tool_call → Runtime Gateway HTTP）

**职责**：让 Agent 框架自带的 `terminal` / `file_editor` 等 tool，最终变成对 Worker `POST /runtime/v1/sessions/...` 的调用。

**当前代码映射**（`integrations/openhands/uenv_runtime/`）：

| 文件 / 模块 | 作用 |
|-------------|------|
| `client.py` | `UEnvGatewayClient` / `UEnvSession`：无 OpenHands 依赖的 HTTP 客户端 |
| `workspace.py` | `UEnvWorkspace`：继承 OpenHands `LocalWorkspace`，`exec`→`session.exec` |
| `gateway_tools.py` | `UEnvGateway*Executor` + `patch_openhands_tools_for_uenv()` 运行时挂钩 |
| `runtime.py` | `UEnvRuntime`：不 import openhands 的 duck-type 驱动（备用路径） |
| `run_swebenchpro_official.py` 等 | 环境 driver 入口 |

**归属判定**：

| 组件 | 归属 | 是否进 EnvPackage | 是否进 UEnv 平台代码 | 说明 |
|------|------|-----------------|----------------------|------|
| **Gateway HTTP 契约**（路径、JSON 字段） | **平台** | 仅版本号引用 | ✅ `uenv-worker` + `uenv-common` + 契约文档 | 所有 env 共用；随 Gateway 演进 |
| **`UEnvGatewayClient`**（纯 HTTP，无框架依赖） | **平台 SDK** | ❌ | ✅ 建议抽为 `uenv-agent-sdk`（Python/Rust 客户端库，随平台发版） | Agent 桥接包依赖它，不重复拷贝 |
| **OpenHands `UEnvWorkspace` 子类** | **AgentBridgePackage** | ❌ | ❌ | 与 OpenHands SDK 类型强绑定 |
| **`UEnvGatewayTerminalExecutor` / `FileEditorExecutor`** | **AgentBridgePackage** | ❌ | ❌ | 框架专用映射实现 |
| **`patch_openhands_tools_for_uenv()`** | **AgentBridgePackage** | ❌ | ❌ | 运行时 hook；随 OpenHands 版本变 |
| **`UEnvRuntime`（无 openhands）** | **AgentBridgePackage 或平台 SDK 示例** | ❌ | ⚠️ 可保留在平台作参考实现 | 新框架可仿照；非热路径 |
| **driver（`run_swebenchpro_official.py`）** | **EnvPackage 引用 + Bridge 包内携带** | ⚠️ `agent_defaults.driver_entrypoint` | ❌ | 入口名进 EnvPackage；脚本随 Bridge 包版本 |
| **默认 tool 列表、workspace_dir** | **EnvPackage `agent_defaults`** | ✅ | ❌ | 如 Pro 用 `/app`、tools=`[terminal,file_editor]` |
| **`gateway_url`、`session_id`** | **Server 调度态 C** | ❌ | ❌ | Job 下发，不写 Hub 明文 |
| **OpenHands 上游安装树** | **Agent 基础镜像** | ❌ | ❌ | `/opt/openhands/benchmarks`；Bridge 包只声明 `openhands_sdk_pin` |

**结论（Agent 侧）**：

- **进 UEnv 框架**：Gateway **协议** + **语言无关/轻依赖客户端**（`UEnvGatewayClient` 一类）。  
- **进 AgentBridgePackage**（Hub 发版、Agent 池 bulk sync）：**框架绑定**的 Executor、Workspace 子类、patch、driver 脚本。  
- **进 EnvPackage**：只 **引用** 哪个 Bridge 版本 + **默认参数**（driver 名、tools、workspace_dir）；**不**把映射代码打进每个 SWE 包副本。  
- **不进任何包**：具体 `gateway_url`（Server 每次 Job 注入）。

```text
LLM tool_call
  → OpenHands Tool（平台自带 schema，不变）
  → AgentBridgePackage：Executor + patch（Hub 版本化）
  → uenv-agent-sdk：UEnvGatewayClient（UEnv 平台发版）
  → Worker Runtime Gateway（UEnv 平台 Rust）
```

#### 4.4.2 Worker 轨迹采集（push_step → seal → upload）

**职责**：在 Gateway 边界记录 `(action, observation)`，submit 时封存为 `TrajectoryBundle`，可选异步上传 Server。

**当前代码映射**（`uenv-worker/src/swe/`）：

| 模块 | 作用 |
|------|------|
| `session.rs` `push_step` | exec/read/write 后写内存 `Vec<StepTrace>` |
| `trajectory.rs` | `StepAction` / `StepObservation` / `TrajectoryBundle` 类型 |
| `instance_pool.rs` `submit` | evaluate → seal → enqueue |
| `trajectory_upload.rs` | spool + 后台 POST Server |
| `runtime_gateway/mod.rs` | HTTP 入口，转调 pool |

**归属判定**：

| 组件 | 归属 | 是否进 EnvPackage | 是否进 UEnv 平台代码 | 说明 |
|------|------|-----------------|----------------------|------|
| **`push_step` / 内存 trace** | **平台** | ❌ | ✅ `uenv-worker` | 凡走 Gateway 的 env 通用 |
| **`StepAction` / `StepObservation` 契约** | **平台** | 仅 schema 版本 | ✅ `uenv-common` | Server/Worker 共用 |
| **`seal_trajectory` / `TrajectoryStore` 落盘** | **平台** | ❌ | ✅ `uenv-worker` | submit 同步路径 |
| **`TrajectoryUploader` / spool / gzip POST** | **平台** | ❌ | ✅ `uenv-worker` | 异步旁路 |
| **Gateway `GET /trajectories`** | **平台（过渡）** | ❌ | ✅ | ack 后以 Server 为准 |
| **Trajectory Server 进程** | **独立服务** | ❌ | ❌（独立部署） | 聚合查询，非 Worker 内嵌 |
| **是否启用轨迹、artifact 根目录** | **EnvPackage overlay** | ✅ `worker_overlay.trajectory` | 默认实现 | 如 SWE 默认 `enabled: true` |
| **upload endpoint、token** | **部署 secret** | ⚠️ 仅 `upload_endpoint_ref` | ❌ | 不进 Hub 明文 |
| **`artifact` 评测产物结构** | **per-env 逻辑** | ⚠️ grader 名在 overlay | ✅ grader 框架在平台 | SWE 的 test_results 由平台 grader 填 bundle |
| **沙箱内「轨迹脚本」** | **不应存在** | ❌ | ❌ | 非侵入 SWE 镜像 |

**结论（Worker 轨迹侧）**：

- **进 UEnv 框架（Rust，全 env 复用）**：采集机制、类型、Gateway 埋点、seal、upload 全链路。  
- **进 EnvPackage（仅配置）**：`trajectory.enabled`、`artifact_dir` 默认、是否要求 upload。  
- **不进 EnvPackage（实现代码）**：没有任何「轨迹 Python 脚本」随 SWE 包分发。  
- **math / 其他 env**：若也走 Runtime Gateway，**复用同一套** `push_step`+`seal`；若仅 native gRPC 路径，仍可调同一 `TrajectoryStore`（已实现于 executor）。

#### 4.4.3 对照总表

| | Agent 集成层 | Worker 轨迹采集 |
|--|--------------|-----------------|
| **本质** | 框架 tool → HTTP 客户端 | Gateway 边界审计 → 落盘 → 上传 |
| **平台代码（A）** | HTTP 契约、`uenv-agent-sdk` 客户端 | `push_step`、seal、upload、Gateway 路由 |
| **AgentBridgePackage（Hub）** | OpenHands Executor、patch、driver | — |
| **EnvPackage（Hub）** | 引用 bridge 版本 + `agent_defaults` | `worker_overlay.trajectory` + grader/catalog |
| **调度态（C）** | `gateway_url`、`session_id` | `run_id`（与轨迹关联） |
| **SWE 镜像内** | 无 | 无 |

---

## 5. 两类组合包：环境包与 Agent 桥接包

### 5.1 环境包（EnvPackage）— 以 `swe-bench-pro` 为例

**注册方**：SWE 环境维护者（benchmark + 镜像 + grader 配置）

**批量部署到 Worker 节点后的本地态**：

```text
/var/lib/uenv/envs/swe-bench-pro/2.1.0/
├── images.manifest.json          # digest 锁定
├── catalog.json                  # InstanceStore 数据源
├── eval_spec.yaml
├── worker.overlay.yaml           # 合并进 worker 配置
└── .synced                       # sync 完成标记（含 bundle_digest）
```

**docker store**：由内网 sync 填充，与 manifest 一致；`image_pull_policy: local_only` 时 miss 即失败（强迫预置）。

### 5.2 Agent 桥接包（AgentBridgePackage）— 以 `uenv-agent-openhands` 为例

**注册方**：UEnv 或 Agent 集成维护者

**批量部署到 Agent 池节点后的本地态**：

```text
/opt/uenv/agent-bridges/uenv-agent-openhands/1.0.2/
├── uenv_runtime/                 # client, workspace, gateway_tools, runtime
├── drivers/
│   ├── run_swebenchpro_official.py
│   └── run_pro_agent.py
├── requirements.txt              # 或 uv.lock
├── openhands_sdk_pin: 1.27.0
└── MANIFEST.json
```

**与 OpenHands 关系**：OpenHands 仍安装在 Agent 基础镜像（如 `/opt/openhands/benchmarks`）；bridge 包在运行时 `patch_openhands_tools_for_uenv()`，**不修改** OpenHands 源码树。

**一个 EnvPackage 通过 `artifacts.agent_bridge` 引用一个 AgentBridgePackage 版本**，保证「这套 SWE 评测」与「这套 Agent 集成」一起发布、一起回滚。

---

## 6. UEnv 平台（A 层）保留内容

以下内容 **不** 打入每个 EnvPackage，随 **平台 Release** 部署一次：

| 模块 | 路径/组件 | 说明 |
|------|-----------|------|
| Runtime Gateway HTTP 服务 | `uenv-worker/src/runtime_gateway/` | 固定 REST 契约 |
| SweInstancePool / SweSession | `uenv-worker/src/swe/` | docker exec/cp、grader 调用框架 |
| 轨迹 seal / upload | `trajectory.rs`、`trajectory_upload.rs` | 凡启用 Gateway 的环境均可接 |
| native DispatchEpisode | `episode/executor.rs` | gold 等无 Agent 路径 |
| Server 调度、gRPC | `uenv-server` | 与 Hub 注册分离 |
| 共享契约 | `uenv-common/trajectory.rs` | Worker / Server 共用 |

**轨迹采集对环境的通用性**：

- **机制通用**：`push_step` → `seal` → `enqueue` → upload 适用于所有走 Gateway 的 env。  
- **策略可 per-package**：是否启用、`artifact_dir`、upload endpoint 引用写在 `worker_overlay.trajectory`。  
- **artifact 内容 per-env**：SWE 的 `EpisodeArtifact` 含 test_results；math 环境结构不同，但 seal 管道相同。

---

## 7. 运行时调度态（C 层）— Server 下发

Hub **不** 固化以下字段；由 `SubmitEpisode` → 调度 → `AgentJob` / `DispatchEpisode` 注入：

| 字段 | 来源 | 说明 |
|------|------|------|
| `gateway_url` | Server 选 Worker 后 | 如 `http://10.10.20.143:28097` |
| `session_id` | Worker `create_session` 后 | 或由 Server 协调创建 |
| `run_id` / `correlation_id` | Server | 贯穿轨迹与 `ReportResult` |
| `model_endpoint` | Server / 推理层注册表 | Agent 调 LLM |
| `api_key`（Gateway） | 部署 secret | 按 Worker 配置 |

Hub 可提供 **`service_discovery` 提示**（如 `worker_pool: swe-pro`），但 **不替代** Server 的具体指派。

---

## 8. 生产批量部署流程

### 8.1 Worker 机架预置（环境包）

```text
1. 运维：uenv-worker 平台安装（A 层，一次）
2. 运维：uenv env sync swe-bench-pro@2.1.0 --target worker
      → 拉取 catalog、eval_spec、worker.overlay
      → 从内网 registry bulk pull / docker load 镜像集
      → 校验 platform.features 与 uenv_worker 版本
3. 配置：UENV_TRAJECTORY_TOKEN 等 secret 注入
4. 启动：uenv-worker（runtime_gateway.enabled 由 overlay 打开）
```

### 8.2 Agent 池预置（桥接包）

```text
1. 基础镜像：OpenHands SDK @ pin 版本（可单独维护 AgentBaseImage）
2. 运维：uenv agent-bridge sync uenv-agent-openhands@1.0.2
3. 配置：默认 LLM profile 由 Server 在 Job 中覆盖
```

### 8.3 任务运行时（与组合包解耦）

```text
Adapter → Server.SubmitEpisode(env_package: swe-bench-pro@2.1.0, instance_id)
Server  → 选已 sync 该 package 的 Worker
Server  → Worker create_session（本地镜像命中，无公网 pull）
Server  → AgentJob(gateway_url, session_id, run_id, model_endpoint,
                   agent_bridge: uenv-agent-openhands@1.0.2)
Agent   → tool loop → Gateway → submit → ReportResult
Worker  → 异步 upload trajectory（平台能力 + package 默认策略）
```

---

## 9. Hub 服务演进（相对现状）

| 现状 | 目标 |
|------|------|
| `GET .../envs/{type}/versions/latest` 返回轻量 manifest | 返回完整 **EnvPackage manifest**（含 artifacts 引用） |
| SWE catalog API 单独拉 JSON | 纳入 `artifacts.catalog`，与 package 版本绑定 |
| 镜像仅字符串引用，Worker 公网 pull | `artifacts.images.bundle` + 内网 **制品存储**（Hub 旁对象存储或 registry） |
| 无 agent bridge 概念 | 新增 `AgentBridgePackage` 注册与 `artifacts.agent_bridge` 引用 |
| 无 sync CLI | `uenv env sync` / `uenv agent-bridge sync`（P1 实施） |

**Hub 仍不参与 Episode 热路径**；仅负责 **注册、版本、制品索引与 sync 源**。

---

## 10. 与当前联调态的映射（迁移参考）

| 联调态散落物 | 迁移后归属 |
|--------------|------------|
| `config/swe/pro.json` | `artifacts.catalog` |
| Docker Hub `jefzda/sweap-images:*` | `artifacts.images`（迁入内网 registry） |
| `config/uenv-worker.deploy-7143-swe-pro.yaml` 中 swe/gateway 段 | `worker_overlay` + 节点本地 listen |
| `integrations/openhands/` | `uenv-agent-openhands@x.y.z` AgentBridgePackage |
| `208.77` rsync 脚本 | `uenv agent-bridge sync` |
| `UENV_TRAJECTORY_ENDPOINT` | `worker_overlay.trajectory` + secret |
| 硬编码 `gateway :28097` | **改为** Server `AgentJob.gateway_url` |

---

## 11. 决策摘要

| 问题 | 结论 |
|------|------|
| Hub 是否太薄弱？ | 是；应升级为 **EnvPackage + 制品 bulk 分发**，而非仅 JSON 索引 |
| 是否一个「大镜像」？ | **否**；采用 **多制品组合包 + manifest** |
| Worker 轨迹配套是否 per-env 脚本？ | **否**；机制在平台，策略在 `worker_overlay.trajectory` |
| Agent 集成是否手搓部署？ | **否**；独立 **AgentBridgePackage**，Hub 版本化 sync |
| 配套是否全进 UEnv 源码？ | **平台能力**进源码；**OpenHands 桥**进 AgentBridgePackage；**镜像/catalog** 进 EnvPackage |
| `gateway_url` 是否写 Hub？ | **否**；Hub 声明需要 Gateway；**URL 由 Server 调度下发** |

---

## 12. 待实施项（优先级）

| 优先级 | 项 | 说明 |
|--------|-----|------|
| **P0** | Hub manifest 扩展 `artifacts` + `worker_overlay` | schema 落库与 API 返回 |
| **P0** | 内网镜像 bundle + `images.manifest.json` | 7143 现有预置镜像 formalize |
| **P1** | `uenv env sync` CLI | Worker 批量预制 |
| **P1** | `uenv-agent-openhands` 独立包发布 | 从 `integrations/openhands` 抽发布物 |
| **P1** | Server `AgentJob` 字段冻结 | `gateway_url`、`agent_bridge` 版本 |
| **P2** | Agent 基础镜像流水线 | OpenHands pin + bridge sync 一层镜像 |
| **P2** | Hub 制品存储（S3/MinIO 或 registry API） | 大文件不走 SQLite |

---

## 13. 变更记录

| 版本 | 日期 | 说明 |
|------|------|------|
| v1.0 | 2026-06-29 | 首版：组合包边界、纳入/不纳入清单、SWE/Agent 双包、三层分离 |
| v1.1 | 2026-06-29 | §4.4：Agent 集成层与 Worker 轨迹采集专项拆分 |
