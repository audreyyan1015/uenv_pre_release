# A100 测试机联调指南

本目录存放两台 A100 测试机的 SSH 私钥，以及四端联调的主机/端口/账号/硬件说明。**请勿将私钥或账号密码提交到公开仓库或分享给无关人员。**

---

## 0. 机器硬件资源

> **最后实机采集**：2026-06-16（SSH 登录各机执行 `nproc` / `free -h` / `df -h` / `nvidia-smi`）。占用率为采集时刻快照，随训练任务波动。

### 0.1 总览

| 机器 | 角色 | CPU | 内存 | 系统盘 | GPU |
|------|------|-----|------|--------|-----|
| **7142**（`10.10.20.142`） | VeRL / Python Adapter | 128 逻辑核 | **1 TiB** | **850 GB** | 8× A100-SXM4-80GB |
| **7143**（`10.10.20.143`） | uenv-worker | 128 逻辑核 | **1 TiB** | **850 GB** | 8× A100-SXM4-80GB |
| **`8.130.86.71`** | uenv-adapter-core + Server | **4 vCPU** | **16 GiB** | **99 GB** NVMe | 无 |
| **`8.130.95.176`** | uenv-hub | **4 vCPU** | **16 GiB** | **99 GB** NVMe | 无 |

### 0.2 A100 7142（Adapter / VeRL 训练侧）

| 项 | 规格 |
|----|------|
| 公网 SSH | `219.147.100.43:7142` |
| 内网 IP | `10.10.20.142` |
| CPU | AMD EPYC 7742 × 2 Socket，**128 逻辑核**（`nproc=128`） |
| 内存 | **1.0 TiB** 总量；Swap **39 GiB** |
| 磁盘 | `/dev/sda4` **850 GB**（采集时已用约 **78%**，剩余约 **188 GB**） |
| GPU | **8× NVIDIA A100-SXM4-80GB**（每卡 81920 MiB） |

**实机注意**：7142 与 7143 为同一物理 A100 宿主机上的两台 VM，共用公网 IP `219.147.100.43`；`lscpu` / `nvidia-smi` 可见**整台宿主机**资源，实际可用 GPU/内存受宿主机调度与其他任务影响，**并非每台 VM 独占 128 核 + 8 卡**。VeRL 容器内 vLLM 默认占 GPU 0 易 OOM，联调建议 `CUDA_VISIBLE_DEVICES_IN_CONTAINER=4`（按 `nvidia-smi` 选空闲卡，见 §1.6）。

### 0.3 A100 7143（Worker）

| 项 | 规格 |
|----|------|
| 公网 SSH | `219.147.100.43:7143` |
| 内网 IP | `10.10.20.143` |
| CPU | 与 7142 相同（EPYC 7742，**128 逻辑核**） |
| 内存 | **1.0 TiB** 总量；Swap **39 GiB** |
| 磁盘 | `/dev/sda4` **850 GB**（采集时已用约 **66%**，剩余约 **294 GB**） |
| GPU | **8× NVIDIA A100-SXM4-80GB** |

**软件层资源配额**（`config/uenv-worker.deploy-7143.yaml`）：

| 配置项 | 值 | 说明 |
|--------|-----|------|
| `worker.max_concurrent` | 4 | 最多 4 个并发 Episode |
| `pool.warmup_size` | 2 | math 预热池实例数 |
| LLM 推理 | OpenRouter 云端 | AgentLoop 路径下 Worker **不占用本地 GPU** 做 completion |

**Register 上报（可选 env 覆盖）**：`UENV_WORKER_GPU_COUNT=1`、`UENV_WORKER_GPU_TYPE=A100`；未设置时 `detect_resource_spec()` 默认内存 **8192 MB**、GPU **0**（仅影响 Register 字段，不影响物理资源）。

### 0.4 阿里云 8.130.86.71（Server / adapter-core）

| 项 | 规格 |
|----|------|
| CPU | **4 vCPU**（Intel Xeon 6982P-C，2 线程/核） |
| 内存 | **16 GiB**（无 Swap） |
| 磁盘 | `/dev/nvme0n1p3` **99 GB** NVMe（采集时已用约 **12%**，剩余约 **83 GB**） |
| GPU | 无 |
| 部署进程 | 仅 `uenv-adapter-core`（见 §1.4、§6.1） |

### 0.5 阿里云 8.130.95.176（Hub）

| 项 | 规格 |
|----|------|
| CPU | **4 vCPU**（与 Server 同规格） |
| 内存 | **16 GiB**（无 Swap） |
| 磁盘 | `/dev/nvme0n1p3` **99 GB** NVMe（采集时已用约 **7%**，剩余约 **88 GB**） |
| GPU | 无 |
| 内网 IP（同 VPC） | `192.168.0.133`（可选用 `http://192.168.0.133:8088`） |
| 持久化 | SQLite `/root/uenv/uenv-hub/data/hub.db` |

### 0.6 资源自检命令

```bash
# A100 7142 / 7143（私钥见 §2）
ssh -i secrets/<key> -p 7142 root@219.147.100.43 'nproc; free -h; df -h /; nvidia-smi -L'
ssh -i secrets/<key> -p 7143 root@219.147.100.43 'nproc; free -h; df -h /; nvidia-smi -L'

# 阿里云 Server / Hub（密码见 §3.1）
ssh root@8.130.86.71 'nproc; free -h; df -h /'
ssh root@8.130.95.176 'nproc; free -h; df -h /'
```

---

## 1. 四端联调部署分配

| 组件 | 部署位置 | 登录 / 可达地址 | 说明 |
|------|----------|-----------------|------|
| **uenv-adapter** | A100 **7142** | `ssh -p 7142 root@219.147.100.43`（私钥见 §2） | 内网 `10.10.20.142` |
| **uenv-worker** | A100 **7143** | `ssh -p 7143 root@219.147.100.43`（私钥见 §2） | 内网 `10.10.20.143`；配置见 `config/uenv-worker.deploy-7143.yaml` |
| **uenv-adapter + uenv-server** | 阿里云 **`8.130.86.71`** | `ssh root@8.130.86.71` | 密码：`dev@BDW2026`；**同一进程** `uenv-adapter-core`，公网入口 **`8.130.86.71:8088`** |
| **uenv-hub** | 阿里云 **`8.130.95.176`** | `ssh root@8.130.95.176`（密码 `pku@345`）；普通用户 `pku` / `pku@123` | 公网 **`http://8.130.95.176:8088`**；Token 见 §1.5 |

```
                    ┌──────────────────────────────────────────┐
  VeRL / 训练侧      │  A100 7142 — uenv-adapter                │
                    └──────────────────┬───────────────────────┘
                                       │
                    ┌──────────────────▼───────────────────────┐
                    │  阿里云 8.130.86.71                       │
                    │  uenv-adapter-core :8088（Adapter+Server）│
                    │  阿里云 8.130.95.176 — uenv-hub :8088        │
                    └──────────────────┬───────────────────────┘
                                       │ DispatchEpisode（回连）
                    ┌──────────────────▼───────────────────────┐
                    │  A100 7143 — uenv-worker                  │
                    │  公网 219.147.100.43:28888（gRPC 业务）    │
                    │  公网 219.147.100.43:28777（health）       │
                    └──────────────────────────────────────────┘
```

### 1.1 Worker 业务地址（当前，告知 Server 侧）

> **Server 同学需配置回连地址；Worker 启动前须先能连上 Server 完成 Register。**

| 项 | 地址 | 说明 |
|----|------|------|
| **Worker gRPC 业务 endpoint** | **`219.147.100.43:28888`** | Register 上报地址；Server `DispatchEpisode` **必须**能公网 TCP 连通 |
| Worker 本机 bind | `0.0.0.0:28888` | 7143 VM 内监听 |
| Worker health / metrics | **`http://219.147.100.43:28777/health`** | 探活期望返回 `ok`；本机 bind `0.0.0.0:28777` |
| Worker → Server（出站） | **`8.130.86.71:8088`** | Register / Heartbeat / ReportResult |
| 支持 env | `math` | `supported_env_types: ["math"]` |

**启动顺序**：① Server 在 `8.130.86.71:8088` 就绪 → ② Worker 在 7143 启动并完成 Register → ③ Server 回连 `219.147.100.43:28888` 派发。

临时保活（Server 未就绪、仅本机联调）：`config/uenv-worker.deploy-7143.standby.yaml`（连 `127.0.0.1:50051`）。

### 1.2 A100 公网端口映射（`219.147.100.43`）

两台 VM 共用公网 IP，**SSH 与业务口按实例前缀区分**。

| SSH 端口 | 内网 IP | 角色 | 已开通公网 TCP 口 |
|----------|---------|------|-------------------|
| **7142** | `10.10.20.142` | Adapter | **18000**，**18077**，**18088**，**18099**，**18777**，**18888** |
| **7143** | `10.10.20.143` | Worker | **28000**，**28077**，**28088**，**28099**，**28777**，**28888** |

**Worker（7143）端口约定**（与 `config/uenv-worker.deploy-7143.yaml` 一致）：

| 用途 | 本机 bind | 注册给 Server 的 endpoint | 公网探活 |
|------|-----------|---------------------------|----------|
| gRPC 业务（原 50052） | `0.0.0.0:28888` | **`219.147.100.43:28888`** | Server 回连 |
| health / metrics（原 19090） | `0.0.0.0:28777` | — | `curl http://219.147.100.43:28777/health` |

**Adapter（7142）**：业务口按实际 bind 与上表 **18xxx** 映射对齐（部署 Adapter 时再定具体口）。

### 1.3 阿里云公网端口（Server / Hub）

两台阿里云主机均开放：**8000**，**8077**，**8088**，**8099**。

| 组件 | 主机 | 配置中应使用的 endpoint |
|------|------|-------------------------|
| **uenv-adapter-core**（Adapter+Server） | **`8.130.86.71`** | Worker `server.endpoint`、7142 Adapter 均指向 **`8.130.86.71:8088`** |
| **uenv-hub** REST | **`8.130.95.176`** | Worker `hub.endpoint`：**`http://8.130.95.176:8088`**（Hub 实际用 **8088**，非 8080） |

> Worker **出站**连 Server/Hub 时使用上表公网地址；Server **入站回连** Worker 时使用 **`219.147.100.43:28888`**，不要填内网 `10.10.20.143` 或 `0.0.0.0`。

### 1.4 ⚠️ Adapter 与 Server 共用同一进程（重要）

在阿里云 **`8.130.86.71`** 上，**`uenv-adapter`（VeRL Bridge Core）与 `uenv-server`（ControlPlane）不是两个独立服务**，而是合并在 **`uenv-adapter-core`** 一个进程中，**共用同一个 gRPC 入口**（当前公网 **`8.130.86.71:8088`**）。

该进程同时注册三类 gRPC Service（同端口、同进程）：

| Service | 用途 | 谁连接 |
|---------|------|--------|
| `AdapterCoreService` | VeRL 训练侧提交 batch | A100 **7142** Python Adapter |
| `ControlPlaneService` | Register / Heartbeat / ReportResult | A100 **7143** Worker |
| `AdminService` | 运维查询 | 运维 / 调试 |

**正确做法**

```bash
# 仅启动 uenv-adapter-core，监听与公网映射一致的地址
export UENV_ADDR=0.0.0.0:8088
/home/uenv/target/release/uenv-adapter-core
# 或：nohup .../uenv-adapter-core >> /var/log/uenv/adapter-core.log 2>&1 &
```

**错误做法（勿重复）**

- ❌ 认为 8088 上是「纯 Adapter」，再单独起 `uenv-server` 抢端口
- ❌ `pkill uenv-adapter-core` 后改起 `uenv-server -b 0.0.0.0:8088`（二者 ControlPlane 能力重复，会破坏 Adapter 入口）
- ❌ 在同一主机上对 **8088** 启动两个进程

A100 **7142** 部署的是 **Python VeRL Adapter 客户端**，通过配置中的 `server.endpoint` / `core.endpoint` 指向 **`8.130.86.71:8088`**（即上述统一入口），**不是**在 7142 上再跑一份 `uenv-adapter-core`（除非架构另有约定）。

### 1.5 Worker ↔ Hub 对接（`8.130.95.176:8088`）

Hub 为 **HTTP REST** 元数据服务，Worker 启动时拉取 `env` manifest（失败降级本地 `plugins/`）。详见 **`Docs/hub/uenv-hub服务指南.md`**。

| 项 | 值 |
|----|-----|
| **Hub Base URL** | **`http://8.130.95.176:8088`** |
| 探活（无需 token） | `GET /healthz` → `{"status":"ok","db":"up"}` |
| Worker 主路径 | `GET /api/v1/envs/math/versions/latest`（**需 Bearer Token**） |
| Worker 配置 | `hub.enabled: true`，`hub.endpoint: "http://8.130.95.176:8088"` |
| Token | **`UENV_HUB_TOKEN`**（与 Hub 主机 `data/.admin_token` 一致，**勿提交仓库**） |
| Token 读取（Hub SSH） | `ssh root@8.130.95.176` → `cat /root/uenv/uenv-hub/data/.admin_token` |

**7143 启动 Worker 时：**

```bash
export UENV_HUB_TOKEN=uenvh_xxxxxxxx   # 勿提交仓库；Hub 上 cat /root/uenv/uenv-hub/data/.admin_token
export UENV_MATH_PLUGIN_BIN=/root/UEnv/target/release/uenv-math-plugin
./target/release/uenv-worker --config config/uenv-worker.deploy-7143.yaml serve
# 或持久化：7143 上 source /root/.uenv-worker.env && bash /root/UEnv/scripts/start-worker-7143-hub.sh
```

**连通性自检（7143 上）：**

```bash
curl -s http://8.130.95.176:8088/healthz
curl -s -H "Authorization: Bearer $UENV_HUB_TOKEN" \
  http://8.130.95.176:8088/api/v1/envs/math/versions/latest
```

成功时 Worker 日志出现 `hub_manifest_pulled`；无 token 或网络不通则 `hub_pull_failed_using_local_manifest`（不阻塞 Register/Episode，但版本元数据来自本地）。

> **Token 不会自动获取**：`UENV_HUB_TOKEN` 为部署期注入的共享 API 密钥（与 Hub `data/.admin_token` 一致），Worker 不会 SSH 登录 Hub 拉取。7143 上可写入 **`/root/.uenv-worker.env`**（权限 600，**勿提交仓库**），启动前 `source` 即可。

### 1.6 全链路（VeRL AgentLoop → Worker）注意事项

**发起位置**：VeRL 训练在 **7142**，通过 **`UEnvAgentLoop`（`default_agent_loop=uenv_agent`）** 将 GSM8K rollout 交给 UEnv；**不要**在 7142 再起 `uenv-adapter-core`（Core+Server 已在 **`8.130.86.71:8088`**）。

**7142 VeRL / AgentLoop 连远端 Core（必配）**：

```bash
export UENV_AGENT_LOOP_CLIENT=rust_core
export UENV_ADAPTER_CORE_ENDPOINT=8.130.86.71:8088
export UENV_ADAPTER_CORE_AUTO_START=0
export UENV_ADAPTER_CORE_BACKEND=server
# VeRL 容器内需：grpcio、uenv-bridge 代码与 adapter_core proto stub
```

**7143 Worker 跑 Episode 必配环境变量**（缺任一则易出现 `plugin math-1 not ready` 或 LLM 调用失败）：

| 变量 | 值（7143 示例） | 说明 |
|------|-----------------|------|
| `UENV_MATH_PLUGIN_BIN` | `/root/UEnv/target/release/uenv-math-plugin` | `plugins/math/run.sh` 依赖 |
| `UENV_PLUGIN_DIR` | `/root/UEnv/plugins` | 与 yaml `plugin_dir` 一致 |
| `UENV_HUB_TOKEN` | Hub `.admin_token` | Hub manifest 拉取（可选降级本地） |
| `UENV_PREWARM_ON_STARTUP` | 建议 `true`（VeRL 联调） | 启动即预热 math 插件，避免首条 Dispatch 超时 |
| **LLM（OpenRouter）** | 见 **§1.7** | AgentLoop 全栈时 Worker 负责调 LLM 生成答案 |

**推荐启动（7143）**：

```bash
source /root/.uenv-worker.env    # 含 Hub Token、OpenRouter API Key 等（勿提交仓库）
bash /root/UEnv/scripts/start-worker-7143-hub.sh
# 日志期望：hub_manifest_pulled、warmup_pool_prewarmed_on_startup、register、heartbeat
```

**链路自检（不跑完整 VeRL 时）**：

```bash
PYTHONPATH=uenv-bridge/src python uenv-bridge/scripts/verify_pre_rollout_rust_core_loop.py
# Worker 日志应出现 dispatch_received → model_callback → step_complete → report_result
```

---

### 1.7 Worker LLM 配置（OpenRouter，AgentLoop 全栈必配）

AgentLoop 路径下，VeRL **不在本地生成** GSM8K completion，而是由 **7143 Worker** 的 `ModelClient` 调 LLM，再将生成文本交给 math 插件判分。默认提供商为 **[OpenRouter](https://openrouter.ai)**（OpenAI 兼容 `POST /chat/completions`）。

#### 1.7.1 配置文件位置

| 文件 | 说明 |
|------|------|
| `config/uenv-worker-llm.env.example` | 仓库内模板（**可提交**） |
| `config/uenv-worker-llm.env` | 实机配置（`**/*.env` 已被 gitignore，勿提交**） |
| `/root/.uenv-worker.env` | 7143 推荐：与 `UENV_HUB_TOKEN` 等同文件 `source` |

Worker 启动时自动加载 `config/uenv-worker-llm.env`；可用 `UENV_WORKER_LLM_ENV` 覆盖路径。`deploy-7143.yaml` 中 `llm.env_file` 可改默认路径。

#### 1.7.2 首次部署（7143）

```bash
cd /root/UEnv
cp config/uenv-worker-llm.env.example config/uenv-worker-llm.env
chmod 600 config/uenv-worker-llm.env

# 编辑：填入 OpenRouter API Key（见 1.7.3）
vi config/uenv-worker-llm.env
```

或合并进 `/root/.uenv-worker.env`（权限 **600**）：

```bash
# --- OpenRouter（Worker LLM）---
export UENV_LLM_PROVIDER=openrouter
export UENV_LLM_ENDPOINT=https://openrouter.ai/api/v1
export UENV_LLM_MODEL_NAME=qwen/qwen-2.5-7b-instruct
export UENV_LLM_API_KEY=sk-or-v1-xxxxxxxx    # 勿提交仓库
export UENV_LLM_HTTP_REFERER=https://github.com/your-org/UEnv
export UENV_LLM_APP_TITLE=UEnv
export UENV_LLM_MAX_TOKENS=512
export UENV_LLM_TEMPERATURE=1.0
```

#### 1.7.3 获取 OpenRouter API Key

1. 登录 [openrouter.ai](https://openrouter.ai) 注册账号。
2. 进入 **Keys** 页面创建 API Key（形如 `sk-or-v1-...`）。
3. 确保账户有足够 credits；GSM8K smoke 建议先用较小模型（默认 `qwen/qwen-2.5-7b-instruct`）。
4. 将 Key 写入 `UENV_LLM_API_KEY`，**不要**写入仓库、不要贴在群聊。

#### 1.7.4 环境变量说明

| 变量 | 默认值 | 必填 | 说明 |
|------|--------|------|------|
| `UENV_LLM_PROVIDER` | `openrouter` | 否 | 当前仅实现 OpenRouter 兼容调用 |
| `UENV_LLM_ENDPOINT` | `https://openrouter.ai/api/v1` | 否 | OpenRouter API 根路径 |
| `UENV_LLM_MODEL_NAME` | `qwen/qwen-2.5-7b-instruct` | 否 | OpenRouter 模型 slug，见 [模型列表](https://openrouter.ai/models) |
| `UENV_LLM_API_KEY` | — | **是** | `Authorization: Bearer <key>` |
| `UENV_LLM_HTTP_REFERER` | 空 | 建议 | OpenRouter 可选排行/归因头 `HTTP-Referer` |
| `UENV_LLM_APP_TITLE` | `UEnv` | 否 | OpenRouter 可选头 `X-Title` |
| `UENV_LLM_MAX_TOKENS` | `512` | 否 | 单次 completion 上限 |
| `UENV_LLM_TEMPERATURE` | `1.0` | 否 | 采样温度 |
| `UENV_WORKER_LLM_ENV` | `config/uenv-worker-llm.env` | 否 | 覆盖 env 文件路径 |

**权威来源**：Worker `ModelClient` **优先使用 Episode 传入的** `model_endpoint` / `model_name` / `generation_config`；API Key 与 OpenRouter 归因头仍由 `uenv-worker-llm.env` 提供。Episode 未带 endpoint 时回退 `UENV_LLM_ENDPOINT`。

#### 1.7.5 连通性自检（7143 上）

```bash
# 需已 export UENV_LLM_API_KEY（或 source uenv-worker-llm.env）
curl -s https://openrouter.ai/api/v1/models \
  -H "Authorization: Bearer $UENV_LLM_API_KEY" | head -c 200

curl -s https://openrouter.ai/api/v1/chat/completions \
  -H "Authorization: Bearer $UENV_LLM_API_KEY" \
  -H "Content-Type: application/json" \
  -H "HTTP-Referer: https://github.com/your-org/UEnv" \
  -H "X-Title: UEnv" \
  -d '{
    "model": "qwen/qwen-2.5-7b-instruct",
    "messages": [{"role":"user","content":"What is 2+2? Answer with #### 4"}],
    "max_tokens": 32
  }'
```

期望：HTTP 200，响应 JSON 中 `choices[0].message.content` 含答案文本。

#### 1.7.6 注意事项

| 项 | 说明 |
|----|------|
| **出站网络** | 7143 Worker 需能访问 **`https://openrouter.ai:443`**（见 §7 防火墙） |
| **勿用 rule_reward 捷径** | 已配置 LLM 时 Worker **不会**把 `ground_truth` 当 action；必须真实调 OpenRouter |
| **API Key 安全** | 仅放 `uenv-worker-llm.env` 或 `/root/.uenv-worker.env`；权限 `600`；勿提交 git |
| **模型选择** | OpenRouter 模型名带厂商前缀，如 `qwen/qwen-2.5-7b-instruct`、`google/gemma-2-9b-it:free` |
| **与 7142 vLLM 关系** | GRPO 训练侧容器内仍有 vLLM；**GSM8K rollout 生成**在 AgentLoop 路径由 Worker+OpenRouter 完成 |
| **错误排查** | `OpenRouter requires UENV_LLM_API_KEY` → 未配 Key；HTTP 401 → Key 无效；HTTP 402 → 余额不足 |

#### 1.7.7 与 VeRL 脚本默认值对齐（7142，可选）

`run_verl_grpo_1step_with_uenv_agent_loop.sh` 中 envelope 默认与 Worker 一致：

```bash
export UENV_ROLLOUT_MODEL_ENDPOINT=https://openrouter.ai/api/v1
export UENV_ROLLOUT_MODEL_NAME=qwen/qwen-2.5-7b-instruct
```

实际 HTTP 调用与鉴权仅在 **7143 Worker** 发生；7142 **不需要**配置 `UENV_LLM_API_KEY`。

---

## 2. 机器与密钥对照（A100）

| 角色 | 主机 | SSH 端口 | 用户 | 私钥文件 |
|------|------|----------|------|----------|
| **Adapter（7142）** | `219.147.100.43` | **7142** | `root` | `2a9f778a35e7d08c738c79493ba643ef_65c3b455afbe3c81a8a757c01b0faae8_8.142` |
| **Worker（7143）** | `219.147.100.43` | **7143** | `root` | `9aa460dab6678381f86a1022b8a54c9f_32e42d1c7902ce68ba6719d551645e02_8.143` |

---

## 3. SSH 登录

### Linux / macOS / WSL / Git Bash

```bash
cd /path/to/UEnv

chmod 600 secrets/9aa460dab6678381f86a1022b8a54c9f_32e42d1c7902ce68ba6719d551645e02_8.143
chmod 600 secrets/2a9f778a35e7d08c738c79493ba643ef_65c3b455afbe3c81a8a757c01b0faae8_8.142

# Adapter — 7142
ssh -i secrets/2a9f778a35e7d08c738c79493ba643ef_65c3b455afbe3c81a8a757c01b0faae8_8.142 \
    -p 7142 root@219.147.100.43

# Worker — 7143
ssh -i secrets/9aa460dab6678381f86a1022b8a54c9f_32e42d1c7902ce68ba6719d551645e02_8.143 \
    -p 7143 root@219.147.100.43

# Server（密码登录）
ssh root@8.130.86.71
# 密码：dev@BDW2026

# Hub（密码登录）
ssh root@8.130.95.176
# root 密码：pku@345
# 普通用户：pku / pku@123
# 读取 Token：cat /root/uenv/uenv-hub/data/.admin_token
```

### Windows PowerShell

```powershell
ssh -i secrets\2a9f778a35e7d08c738c79493ba643ef_65c3b455afbe3c81a8a757c01b0faae8_8.142 `
    -p 7142 root@219.147.100.43

ssh -i secrets\9aa460dab6678381f86a1022b8a54c9f_32e42d1c7902ce68ba6719d551645e02_8.143 `
    -p 7143 root@219.147.100.43

ssh root@8.130.86.71

ssh root@8.130.95.176
```

### 可选：写入 `~/.ssh/config` 简化登录

```
Host uenv-a100-7142
    HostName 219.147.100.43
    Port 7142
    User root
    IdentityFile /path/to/UEnv/secrets/2a9f778a35e7d08c738c79493ba643ef_65c3b455afbe3c81a8a757c01b0faae8_8.142

Host uenv-a100-7143
    HostName 219.147.100.43
    Port 7143
    User root
    IdentityFile /path/to/UEnv/secrets/9aa460dab6678381f86a1022b8a54c9f_32e42d1c7902ce68ba6719d551645e02_8.143

Host uenv-server
    HostName 8.130.86.71
    User root

Host uenv-hub
    HostName 8.130.95.176
    User root
    # 密码：pku@345（普通用户 pku / pku@123）
```

---

## 3.1 阿里云 Hub / Server 账号速查

| 主机 | 用户 | 密码 | 用途 |
|------|------|------|------|
| **`8.130.86.71`**（Server） | `root` | `dev@BDW2026` | `uenv-adapter-core` |
| **`8.130.95.176`**（Hub） | `root` | `pku@345` | 运维、`data/.admin_token` |
| **`8.130.95.176`**（Hub） | `pku` | `pku@123` | 普通登录 |

---

## 4. 推荐联调拓扑（四端 / MathEnv）

| 组件 | 部署位置 | 端口 / endpoint | 说明 |
|------|----------|-----------------|------|
| `uenv-adapter`（Python） | A100 **7142** | 出站 → **`8.130.86.71:8088`** | VeRL 训练侧；连统一 Core 入口 |
| `uenv-adapter-core`（Adapter+Server） | **`8.130.86.71`** | 公网 **`:8088`** | **唯一** gRPC 入口；含 ControlPlane |
| `uenv-hub` | **`8.130.95.176`** | **`http://8.130.95.176:8088`** | Worker 启动拉 manifest；需 `UENV_HUB_TOKEN` |
| `uenv-worker` | A100 **7143** | 公网 **`:28888`**（gRPC）、**`:28777`**（health） | `config/uenv-worker.deploy-7143.yaml` |

---

## 5. 环境准备（A100 7142 / 7143）

```bash
apt-get update && apt-get install -y build-essential pkg-config libssl-dev protobuf-compiler git curl
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"

git clone <repo-url> UEnv && cd UEnv
make proto
cargo build -p uenv-worker -p uenv-adapter-core --release

sudo mkdir -p /var/log/uenv /tmp/uenv/wal
sudo chown -R "$USER" /var/log/uenv /tmp/uenv
```

---

## 6. 分步启动与验证

### 6.1 阿里云：启动 uenv-adapter-core（Adapter + Server 合一）

在 **`8.130.86.71`** 上**只启动 `uenv-adapter-core`**，不要单独再起 `uenv-server`：

```bash
export UENV_ADDR=0.0.0.0:8088
nohup /home/uenv/target/release/uenv-adapter-core >> /var/log/uenv/adapter-core.log 2>&1 &

# 确认：进程名应为 uenv-adapter-core，8088 仅一份监听
pgrep -af uenv-adapter-core
ss -tlnp | grep 8088
```

Worker 的 `server.endpoint` 与 7142 Python Adapter 的 Core/Server 地址均指向 **`8.130.86.71:8088`**。

### 6.2 A100 7143：启动 uenv-worker

```bash
cd /root/UEnv
source /root/.uenv-worker.env   # 含 UENV_MATH_PLUGIN_BIN、UENV_HUB_TOKEN 等（勿提交仓库）
bash scripts/start-worker-7143-hub.sh
# 或手动：
export UENV_MATH_PLUGIN_BIN=/root/UEnv/target/release/uenv-math-plugin
export UENV_PLUGIN_DIR=/root/UEnv/plugins
export UENV_HUB_TOKEN=...        # 见 §1.5
export UENV_PREWARM_ON_STARTUP=true   # VeRL 联调建议开启
# OpenRouter：cp config/uenv-worker-llm.env.example config/uenv-worker-llm.env 并填入 UENV_LLM_API_KEY（§1.7）

./target/release/uenv-worker --config config/uenv-worker.deploy-7143.yaml serve
```

确认监听与注册：

```bash
ss -tlnp | grep -E '28888|28777'
curl -s http://127.0.0.1:28777/health
# 日志中应出现 register_endpoint=219.147.100.43:28888、server_endpoint=8.130.86.71:8088
# VeRL 联调另查：warmup_pool_prewarmed_on_startup、hub_manifest_pulled
tail -f /var/log/uenv/worker.log
```

### 6.3 连通性检查

```bash
# 外网探活 Worker
curl -s http://219.147.100.43:28777/health
nc -zv 219.147.100.43 28888

# Worker 侧：能否连上 Server
nc -zv 8.130.86.71 8088
```

---

## 7. 防火墙与端口放行

| 方向 | 地址:端口 | 用途 |
|------|-----------|------|
| Worker → Server | **`8.130.86.71:8088`** | Register / Heartbeat / ReportResult |
| Server → Worker | **`219.147.100.43:28888`** | DispatchEpisode |
| 运维 | **`219.147.100.43:28777`** | Worker `/health`、`/metrics` |
| Worker → Hub | **`http://8.130.95.176:8088`** | 可选 manifest pull（`/api/v1/**` 需 token） |
| Worker → OpenRouter | **`https://openrouter.ai:443`** | AgentLoop 全栈时 LLM 生成（见 §1.7） |
| Adapter（7142） | `219.147.100.43:18xxx` | 按 Adapter 实际 bind 配置 |

---

## 8. 常见问题

| 现象 | 排查 |
|------|------|
| Worker 注册失败 | `server.endpoint` 是否为 **`8.130.86.71:8088`**；**`uenv-adapter-core`** 是否在监听（勿误起独立 `uenv-server`） |
| Dispatch 超时 | Server 能否访问 **`219.147.100.43:28888`** |
| **`plugin math-1 not ready`** | Worker 进程是否带 **`UENV_MATH_PLUGIN_BIN`**；`plugins/math/run.sh` 是否可执行；手工 `bash run.sh --uds-path /tmp/t.sock` 能否创建 sock |
| Hub 401 / 无 manifest | 是否 `export UENV_HUB_TOKEN=...`（见 §1.5） |
| 7142 AgentLoop 报 stub 错误 | 容器内是否安装 **`grpcio`**；`UENV_ADAPTER_CORE_ENDPOINT` 是否 **`8.130.86.71:8088`** 且 **`AUTO_START=0`** |
| 8088 端口冲突 | 确认只有 **`uenv-adapter-core`** 占用 8088，不要 Adapter/Server 各起一个进程 |
| 7142 上误跑 Worker | Worker 应只在 **7143**；7142 为 Python Adapter 客户端 |
| 插件启动失败 | Linux + `UENV_MATH_PLUGIN_BIN` + `plugins/math/run.sh` 可执行 |
| **`OpenRouter requires UENV_LLM_API_KEY`** | 7143 上配置 `config/uenv-worker-llm.env` 或 `/root/.uenv-worker.env`（§1.7） |
| **model client HTTP 401/402** | OpenRouter Key 无效或余额不足；`curl` 自检 §1.7.5 |
| **GSM8K reward 恒为 1.0 但答案明显错** | 检查是否未配 LLM 却走了旧 stub；确认日志有 `model_callback` 且非 rule_reward 短路 |

---

## 9. 联调记录模板

```
日期：
分支/提交：
Adapter：7142 / 10.10.20.142
Worker 业务地址：219.147.100.43:28888
Worker health：219.147.100.43:28777
Server（uenv-adapter-core @ 8.130.86.71:8088）：
Hub：http://8.130.95.176:8088
Episode ID：
结果：success / fail
异常与处置：
```

---

## 参考文档

- [全链路联调-各层接口与参数字段.md](../Docs/全链路联调-各层接口与参数字段.md)
- [uenv-worker/README.md](../uenv-worker/README.md)
- [Docs/hub/uenv-hub服务指南.md](../Docs/hub/uenv-hub服务指南.md)
