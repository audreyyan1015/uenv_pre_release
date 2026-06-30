# DeepSeek-V3-0324-AWQ 本地推理 — 7142 部署方案

> **文档版本**：v2.1（**采用方案** + 模型网关）  
> **日期**：2026-06-26  
> **状态**：**方案冻结**，待 7142 实装  
> **决策**：OpenHands SWE-bench Pro Agent 的 LLM 从 DashScope `deepseek-v4-flash` 切换为 **7142 本地 `DeepSeek-V3-0324-AWQ`（INT4）**；对外经 **`uenv-llm-gateway` `:18888`** 提供 OpenAI-compatible 模型服务  
> **关联**：`260627-swe-openhands-integration-plan.md`、`260627-swe-openhands-acceptance-report.md`、`secrets/README.md` §1.2（7142 端口）  
> **OpenHands**：已迁至 **208.77**；7142 仅 LLM 网关 `:18888`。联调见 `scripts/run-openhands-pro-20877.sh`（208.77）。  
> **文件名说明**：历史文件名含 `fp8`；v2.0 起 **主路径为 AWQ**；v2.1 起增加 **18888 模型网关层**，FP8 见 §13 备选。

---

## 0. 决策摘要（TL;DR）

| 项 | 冻结值 |
|----|--------|
| **采用模型** | **`cognitivecomputations/DeepSeek-V3-0324-AWQ`**（社区 AWQ 4-bit，基座 `deepseek-ai/DeepSeek-V3-0324`） |
| 架构 | **671B MoE / 37B active**（与官方 V3-0324 同结构，非 70B 稠密） |
| 推理框架 | **vLLM** ≥ **0.8.3**（`deepseek_v3` tool parser）；建议 **0.8.x–0.19.x** stable |
| 部署节点 | **A100 7142**（`10.10.20.142` / SSH `:7142`） |
| GPU | **8× A100-SXM4-80GB**（实施前 `nvidia-smi` 确认 8 卡空闲） |
| 并行策略 | **`tensor-parallel-size=8`**（AWQ 权重 ~352GB → ~44GB/卡；**TP=4 不可行**，见 §3.2） |
| **推理后端（内网）** | vLLM **`127.0.0.1:8000/v1`** — 仅本机，不对外 |
| **模型网关（对外）** | **`uenv-llm-gateway`** 监听 **`0.0.0.0:18888`** → 公网 **`219.147.100.43:18888`** |
| 网关探活 | **`0.0.0.0:18777/health`** → 公网 **`219.147.100.43:18777`** |
| 消费者（本机） | OpenHands / 脚本 → Gateway **`127.0.0.1:18888/v1`**（或内网 `10.10.20.142:18888`） |
| 消费者（外网/7143） | 可选 **`http://10.10.20.142:18888/v1`**（内网）或公网 `:18888` |
| 验收实例 | qutebrowser Pro smoke（与联调报告 §3.9.3 同实例） |

**目标**：在 Gateway/grader 路径不变的前提下，用 **V3-0324 级 coding/agent 能力** 替换 flash API，提高 Pro smoke **resolve** 概率；**工程风险低于 FP8 / GLM-5 在 A100 上的部署**。

---

## 1. 背景与动机

### 1.1 当前 LLM 失败模式（已验证）

| 路径 | reward | 主因 |
|------|--------|------|
| duck-type 单轮 LLM | 0.0 / 52/56 | 幻觉 patch 路径、apply 失败 |
| OpenHands 官方 SDK + flash（15 iter） | 0.0 / 52/56 | 多轮 tool use 正常，但未在 `/app` 改源码 |

**结论**：非 Gateway/grader 回归；需更强模型 + 更高 iter + prompt 调优。

### 1.2 方案选型过程（冻结结论）

| 候选 | 7142 结论 |
|------|-----------|
| DashScope `deepseek-v4-flash` | 现状；Agent 探索与 patch 质量不足 |
| **DeepSeek-V3-0324-AWQ** | **✅ 采用** — 完整 MoE 能力、A100 成熟、TP=8 有余量 |
| DeepSeek-V3 FP8 官方权重 | ❌ 主路径 — ~671GB 权重 TP=8 每卡 ~84GB 极紧 + A100 sm80 FP8 kernel 麻烦 |
| GLM-5/5.1 FP8 | ❌ — 需 8×H200/H20；A100 需 AWQ + DSA workaround |
| GLM-5 AWQ | ⚠️ 备选 — 工程复杂度高于 V3 AWQ 生态 |
| DeepSeek-R1-Distill-Llama-70B INT4 | ❌ — 部署最省卡，但 **SWE 多轮 Agent 档次不够** |

### 1.3 为何选 DeepSeek-V3-0324-AWQ

- **0324** 为 DeepSeek 刷新版，coding / agent benchmark 优于旧 V3
- **AWQ ~352GB**，8×80GB 上 **TP=8 每卡 ~44GB 权重**，余 **~200GB+** 整机 KV cache（对应原「留 KV 余量」意图）
- vLLM **Marlin AWQ + MLA** 在 **8×A100** 有社区 benchmark（cognitivecomputations 维护）
- **Tool calling**：vLLM `--tool-call-parser deepseek_v3` + 专用 chat template（OpenHands 必需）
- OpenHands（208.77）LLM 配置：`config/openhands-llm-20877.json`（endpoint 可指向 7142 `:18888`）

### 1.4 模型仓库说明

| 项目 | 值 |
|------|-----|
| **采用 checkpoint** | [`cognitivecomputations/DeepSeek-V3-0324-AWQ`](https://huggingface.co/cognitivecomputations/DeepSeek-V3-0324-AWQ) |
| 基座 | `deepseek-ai/DeepSeek-V3-0324` |
| 量化 | AWQ 4-bit（Activation-aware Weight Quantization） |
| 官方 AWQ | **无** — DeepSeek 官方仅发布 FP8；AWQ 为 **社区量化**（Eric Hartford / cognitivecomputations） |
| 镜像仓库 | `QuixiAI/DeepSeek-V3-0324-AWQ` 等 — **以 cognitivecomputations 为准** |
| 磁盘体积 | **~360GB**（36 分片）；HF cache 建议 **≥ 400GB** 可用 |
| License | MIT（随基座） |

> **勿混淆**：不存在「DeepSeek V3 70B」— V3 为 **671B MoE（37B active）**；70B 指 **R1-Distill-Llama-70B**，非本方案。

---

## 2. 目标架构

### 2.1 双层服务（推理后端 + 模型网关）

vLLM 占满 8 卡且启动慢，**不直接对外暴露**；由 **`uenv-llm-gateway`** 统一管理「模型已启用 / 可接受请求」状态，并在 **7142 业务口 `:18888`** 提供 OpenAI-compatible API（与 7143 Worker `:28888` 端口命名对齐，见 `secrets/README.md` §1.2）。

```text
                    外部 / 7143 内网                    7142 本机
              ┌──────────────────────────────────────────────────────────┐
              │                                                          │
  公网/内网   │   uenv-llm-gateway :18888  ──proxy──►  vLLM :8000       │
  OpenHands   │   (OpenAI /v1/*, API Key)            (127.0.0.1 only)   │
  VeRL/Bridge │        │                                 8×A100 TP=8     │
              │   :18777/health ◄── readiness ──► vLLM /v1/models       │
              │                                                          │
              │   OpenHands run_swebenchpro_official.py                    │
              │        │                                                 │
              └────────┼─────────────────────────────────────────────────┘
                       │  http://10.10.20.143:28999
              ┌────────▼─────────────────────────────────────────────────┐
              │  7143 runtime_gateway :28999  →  Pro grader / trajectory │
              └──────────────────────────────────────────────────────────┘
```

### 2.2 7142 端口约定（冻结）

与 A100 公网映射 **18xxx** 对齐（7143 Worker 为 **28xxx**）：

| 用途 | 7142 bind | 公网探活 / 调用 | 说明 |
|------|-----------|-----------------|------|
| **LLM OpenAI API** | `0.0.0.0:18888` | **`219.147.100.43:18888`** | `uenv-llm-gateway` 业务口 |
| health / 就绪 | `0.0.0.0:18777` | `219.147.100.43:18777` | 聚合网关 + vLLM 后端状态 |
| vLLM 推理后端 | `127.0.0.1:8000` | **不映射公网** | 仅网关反代 |

> **安全**：vLLM 绑定 `127.0.0.1`；公网仅暴露网关，由 **`UENV_LLM_GATEWAY_API_KEY`** 鉴权（见 §5.6）。

### 2.3 数据流

1. **资源分配**：下载 AWQ 权重 → systemd 启动 `vllm-dsv3-awq`（8 卡 TP=8）  
2. **启用模型**：`uenv-llm-gateway` 轮询 vLLM `/v1/models` 就绪后，在 `:18888` 接受流量（`model.enabled=true`）  
3. **OpenHands Agent** 调用 **`http://127.0.0.1:18888/v1/chat/completions`**（带 API Key）→ 网关反代 vLLM → tool_calls  
4. Agent 经 Gateway shim 在 **7143 容器 `/app`** 执行工具 → `submit` → grader / trajectory  

---

## 3. 硬件与并行策略

### 3.1 7142 资源（实机）

| 资源 | 值 | 备注 |
|------|-----|------|
| GPU | 8× NVIDIA A100-SXM4-80GB | 宿主机共享；VM 内见整机 8 卡 |
| 内存 | 1 TiB | 权重下载 / HF cache |
| 系统盘 | 850 GB | **不够**单独存 AWQ；权重放 **`/data`** |
| 内网 → 7143 | `10.10.20.143:28999` | OpenHands 调 Gateway |

### 3.2 为何 TP=8（非 TP=4）

| 配置 | 权重/GPU | 8×A100-80GB |
|------|----------|-------------|
| TP=4 | ~352÷4 ≈ **88GB/卡** | ❌ 超过单卡 80GB |
| **TP=8** | ~352÷8 ≈ **44GB/卡** | ✅ 社区推荐；余量给 KV |

vLLM 标准部署下 **KV cache 与权重同卡**；无法「4 卡权重 + 4 卡纯 KV」（PD 分离 Out of Scope）。**8 卡全用于 TP=8**；与 VeRL 训练 **互斥**。

### 3.3 7142 冻结 vLLM 参数

| 参数 | 冻结值 | 说明 |
|------|--------|------|
| `tensor-parallel-size` | **8** | AWQ 在 8×80GB 上的 validated 配置 |
| `gpu-memory-utilization` | **0.90** | 留 ~10% 给 KV / 碎片 |
| `max-model-len` | **32768**（首版） | 50 step Agent + 长 tool 输出；OOM → **16384** |
| `max-num-seqs` | **4** | 单 Agent 为主 |
| `enable-chunked-prefill` | **true** | 长 context 预填充 |
| `enable-prefix-caching` | **true** | 多轮重复 system prompt 省 KV |

### 3.4 显存粗算（TP=8 + AWQ）

| 组件 | 估算 |
|------|------|
| AWQ 权重 | ~352 GB ÷ 8 ≈ **44 GB/卡** |
| 可用（util=0.90） | 80 × 0.90 = **72 GB/卡** |
| KV 余量（粗算） | ~**28 GB/卡** → 整机 **~200GB+** KV 预算 |
| 能力损失 | 较 FP8 约 **1–3%** coding 回退（可接受） |

---

## 4. 软件栈与版本 Pin

| 组件 | 版本 / 来源 |
|------|-------------|
| 模型 | **`cognitivecomputations/DeepSeek-V3-0324-AWQ`** |
| vLLM | **≥ 0.8.3**（tool calling）；推荐 **0.8.2+** 或 **0.19.x** stable |
| transformers | **≥ 4.48**（模型 config 要求） |
| Chat template | vLLM 自带 `examples/tool_chat_template_deepseekv3.jinja` |
| OpenHands SDK | `integrations/openhands/PIN.md`（v1.27.0） |
| **模型网关** | **`uenv-llm-gateway`**（`scripts/uenv-llm-gateway/`） |
| Python | 3.11+；网关与 vLLM 可共用 venv 或独立 venv |

**环境变量（AWQ / A100 推荐）**：

```bash
export VLLM_USE_V1=0
export VLLM_WORKER_MULTIPROC_METHOD=spawn
export VLLM_MARLIN_USE_ATOMIC_ADD=1
```

---

## 5. 部署步骤

### 5.1 前置检查（7142）

```bash
nvidia-smi                                    # 8×A100 空闲
df -h /data                                   # ≥ 400GB 可用
free -h
curl -sS http://10.10.20.143:28999/health     # 7143 Gateway
```

停止 VeRL / 其他 vLLM；`export CUDA_VISIBLE_DEVICES=0,1,2,3,4,5,6,7`。

### 5.2 权重下载

```bash
export HF_ENDPOINT=https://hf-mirror.com      # 7142 无法直连 HF 时
export HF_HOME=/data/huggingface

pip install -U "huggingface_hub[cli]"
huggingface-cli download cognitivecomputations/DeepSeek-V3-0324-AWQ \
  --local-dir /data/models/DeepSeek-V3-0324-AWQ \
  --local-dir-use-symlinks False
```

> 7142 无外网：本机下载后 `rsync` / `scp` 至 `/data/models/DeepSeek-V3-0324-AWQ`。

### 5.3 安装 vLLM

```bash
python3 -m venv /opt/vllm-dsv3-awq
source /opt/vllm-dsv3-awq/bin/activate
pip install -U "vllm>=0.8.3" --torch-backend auto
pip install -U "transformers>=4.48"

# 定位 chat template（随 vLLM 安装路径）
VLLM_EXAMPLES=$(python -c "import vllm, pathlib; print(pathlib.Path(vllm.__file__).parent / 'examples')")
ls "$VLLM_EXAMPLES/tool_chat_template_deepseekv3.jinja"
```

### 5.4 启动 vLLM（7142 冻结命令）

```bash
export CUDA_VISIBLE_DEVICES=0,1,2,3,4,5,6,7
export HF_HOME=/data/huggingface
export VLLM_USE_V1=0
export VLLM_WORKER_MULTIPROC_METHOD=spawn
export VLLM_MARLIN_USE_ATOMIC_ADD=1

VLLM_CHAT_TEMPLATE=$(python -c "import vllm, pathlib; print(pathlib.Path(vllm.__file__).parent / 'examples/tool_chat_template_deepseekv3.jinja')")

vllm serve /data/models/DeepSeek-V3-0324-AWQ \
  --served-model-name deepseek-v3-0324-awq \
  --host 127.0.0.1 \
  --port 8000 \
  --trust-remote-code \
  --tensor-parallel-size 8 \
  --gpu-memory-utilization 0.90 \
  --max-model-len 32768 \
  --max-num-seqs 4 \
  --enable-chunked-prefill \
  --enable-prefix-caching \
  --enable-auto-tool-choice \
  --tool-call-parser deepseek_v3 \
  --chat-template "$VLLM_CHAT_TEMPLATE"
```

**健康检查 — Chat**：

```bash
curl -sS http://127.0.0.1:8000/v1/models
curl -sS http://127.0.0.1:8000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"deepseek-v3-0324-awq","messages":[{"role":"user","content":"Say OK"}],"max_tokens":16}'
```

**健康检查 — Tool call（OpenHands 依赖）**：

```bash
curl -sS http://127.0.0.1:8000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "deepseek-v3-0324-awq",
    "messages": [{"role": "user", "content": "What is 2+2?"}],
    "tools": [{
      "type": "function",
      "function": {
        "name": "bash",
        "description": "Run a command",
        "parameters": {"type": "object", "properties": {"command": {"type": "string"}}, "required": ["command"]}
      }
    }],
    "tool_choice": "auto"
  }'
# 预期：choices[0].message.tool_calls 非空
```

### 5.5 systemd 单元（可选）

路径：`/etc/systemd/system/vllm-dsv3-awq.service`

```ini
[Unit]
Description=vLLM DeepSeek-V3-0324-AWQ (TP8, 8xA100)
After=network.target

[Service]
Type=simple
User=root
Environment=CUDA_VISIBLE_DEVICES=0,1,2,3,4,5,6,7
Environment=HF_HOME=/data/huggingface
Environment=VLLM_USE_V1=0
Environment=VLLM_WORKER_MULTIPROC_METHOD=spawn
Environment=VLLM_MARLIN_USE_ATOMIC_ADD=1
ExecStart=/opt/vllm-dsv3-awq/bin/vllm serve /data/models/DeepSeek-V3-0324-AWQ \
  --served-model-name deepseek-v3-0324-awq \
  --host 127.0.0.1 --port 8000 --trust-remote-code \
  --tensor-parallel-size 8 --gpu-memory-utilization 0.90 \
  --max-model-len 32768 --max-num-seqs 4 \
  --enable-chunked-prefill --enable-prefix-caching \
  --enable-auto-tool-choice --tool-call-parser deepseek_v3 \
  --chat-template /opt/vllm-dsv3-awq/lib/python3.12/site-packages/vllm/examples/tool_chat_template_deepseekv3.jinja
Restart=on-failure
RestartSec=30

[Install]
WantedBy=multi-user.target
```

> `--chat-template` 路径随 Python 版本调整；可用 `python -c "import vllm, pathlib; ..."` 写入启动脚本。

```bash
systemctl daemon-reload
systemctl enable --now vllm-dsv3-awq
journalctl -u vllm-dsv3-awq -f
```

### 5.6 部署 uenv-llm-gateway（模型网关，冻结）

网关职责：

| 职责 | 说明 |
|------|------|
| **启停编排** | 依赖 vLLM 就绪后再对外 `LISTEN`（或启动后返回 503 直至 ready） |
| **对外 API** | 反代 OpenAI **`/v1/models`**、**`/v1/chat/completions`**（含 **tool_calls**） |
| **鉴权** | Header **`Authorization: Bearer <key>`** 或 **`X-API-Key`**（与 7143 Gateway 风格一致） |
| **模型开关** | 配置 `model.enabled`；关闭时 `503` + 明确错误体 |
| **可观测** | **`GET /health`**（18777）：`ok` / `starting` / `backend_down` |

**配置文件**（建议路径 `config/uenv-llm-gateway-7142.yaml`）：

```yaml
# 7142 模型网关 — DeepSeek-V3-0324-AWQ
listen: "0.0.0.0:18888"
advertise_endpoint: "219.147.100.43:18888"

observability:
  health_listen: "0.0.0.0:18777"

backend:
  # vLLM OpenAI 根路径（内网-only）
  base_url: "http://127.0.0.1:8000/v1"
  readiness_path: "/models"
  readiness_interval_sec: 5
  readiness_timeout_sec: 900   # AWQ 冷启动可达 10–15min

auth:
  # 从环境变量注入，勿提交仓库：UENV_LLM_GATEWAY_API_KEY
  api_key_env: "UENV_LLM_GATEWAY_API_KEY"

model:
  id: "deepseek-v3-0324-awq"
  enabled: true

proxy:
  timeout_sec: 600
  max_body_bytes: 8388608
```

**环境变量**（7142 `/root/.uenv-llm-gateway.env`，权限 `600`）：

```bash
UENV_LLM_GATEWAY_API_KEY=llm-gateway-secret-change-me
```

**启动命令（实现参考 — 待实装为 systemd）**：

```bash
source /root/.uenv-llm-gateway.env
python3 /root/UEnv/scripts/uenv-llm-gateway.py \
  --config /root/UEnv/config/uenv-llm-gateway-7142.yaml
```

**网关行为契约（实现必须满足）**：

1. 收到请求时校验 API Key；失败 → `401`  
2. `model.enabled=false` → `503` JSON `{"error":"model_disabled"}`  
3. vLLM 未就绪 → `503` JSON `{"error":"backend_starting"}`  
4. 就绪后 **透明转发** 请求体/响应体（保留 `tools` / `tool_choice` / `stream`）  
5. **`GET /health`**（18777）：`backend_ready && model.enabled` → 200 `ok`，否则 503  

**对外调用示例**：

```bash
# 公网（需防火墙已开通 18888）
curl -sS http://219.147.100.43:18888/v1/models \
  -H "Authorization: Bearer $UENV_LLM_GATEWAY_API_KEY"

curl -sS http://219.147.100.43:18888/v1/chat/completions \
  -H "Authorization: Bearer $UENV_LLM_GATEWAY_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"model":"deepseek-v3-0324-awq","messages":[{"role":"user","content":"Say OK"}],"max_tokens":16}'

# 7143 内网调用 7142 模型服务
curl -sS http://10.10.20.142:18888/v1/models \
  -H "Authorization: Bearer $UENV_LLM_GATEWAY_API_KEY"
```

### 5.7 systemd 单元 — 模型网关

路径：`/etc/systemd/system/uenv-llm-gateway.service`

```ini
[Unit]
Description=UEnv LLM Gateway (7142 :18888 -> vLLM :8000)
After=network.target vllm-dsv3-awq.service
Wants=vllm-dsv3-awq.service

[Service]
Type=simple
User=root
EnvironmentFile=-/root/.uenv-llm-gateway.env
ExecStart=/opt/vllm-dsv3-awq/bin/python /root/UEnv/scripts/uenv-llm-gateway.py \
  --config /root/UEnv/config/uenv-llm-gateway-7142.yaml
Restart=on-failure
RestartSec=10

[Install]
WantedBy=multi-user.target
```

**一键目标（推荐）** — `uenv-llm.target` 同时拉起推理栈：

```ini
# /etc/systemd/system/uenv-llm.target
[Unit]
Description=UEnv 7142 Local LLM Stack (vLLM + Gateway)
Requires=vllm-dsv3-awq.service uenv-llm-gateway.service
After=vllm-dsv3-awq.service uenv-llm-gateway.service
```

```bash
systemctl daemon-reload
systemctl enable vllm-dsv3-awq uenv-llm-gateway
systemctl start uenv-llm.target
# 或：systemctl start vllm-dsv3-awq && systemctl start uenv-llm-gateway
```

### 5.8 资源分配与启停顺序（冻结）

```text
[1] GPU 释放     停止 VeRL / 其他占卡进程；nvidia-smi 确认 8 卡空闲
[2] 权重就绪     /data/models/DeepSeek-V3-0324-AWQ 已下载
[3] 推理后端     systemctl start vllm-dsv3-awq
                 等待 curl -sS http://127.0.0.1:8000/v1/models 成功
[4] 模型网关     systemctl start uenv-llm-gateway
                 等待 curl -sS http://127.0.0.1:18777/health → ok
[5] 启用对外     确认 :18888 公网可达；OpenHands / 7143 改 endpoint
[6] 联调         208.77: run-openhands-pro-20877.sh llm（LLM endpoint → 7142 :18888）
```

**停止（还 GPU 给 VeRL）**：

```bash
systemctl stop uenv-llm-gateway
systemctl stop vllm-dsv3-awq
# 可选：systemctl stop uenv-llm.target
```

---

## 6. OpenHands / UEnv 对接

### 6.1 LLM 配置（7142）

**统一经模型网关访问**（本机 OpenHands、后续 7143 Worker `llm.env` 同源）：

编辑 **`config/uenv-worker-llm.env`**（**勿提交 secrets**）：

```env
# 7142 本机 OpenHands：走网关（与对外 :18888 同一入口）
UENV_LLM_ENDPOINT=http://127.0.0.1:18888/v1
UENV_LLM_MODEL_NAME=deepseek-v3-0324-awq
UENV_LLM_API_KEY=${UENV_LLM_GATEWAY_API_KEY}
UENV_LLM_MAX_TOKENS=8192
UENV_LLM_TEMPERATURE=0.2
UENV_LLM_HTTP_TIMEOUT_SECS=600
UENV_LLM_MAX_RETRIES=3
```

> 7142 上先 `source /root/.uenv-llm-gateway.env`，或把 gateway key **显式写入** `uenv-worker-llm.env`（勿提交）。  
> **7143 Worker** 若需共用 7142 模型：  
> `UENV_LLM_ENDPOINT=http://10.10.20.142:18888/v1` + 同一 API Key。

生成 OpenHands JSON：

```bash
python3 /root/UEnv/scripts/gen-openhands-llm-config.py \
  /root/UEnv/config/uenv-worker-llm.env \
  /root/UEnv/config/openhands-llm-20877.json
```

预期：

```json
{
  "model": "openai/deepseek-v3-0324-awq",
  "base_url": "http://127.0.0.1:18888/v1",
  "api_key": "<UENV_LLM_GATEWAY_API_KEY>",
  "temperature": 0.2,
  "max_output_tokens": 8192
}
```

### 6.2 Agent 运行参数（冻结）

| 参数 | 值 | 说明 |
|------|-----|------|
| `MAX_ITERATIONS` | **50** | 原 15 iter 不足 |
| `--mode` | `llm` | gold 仍走 catalog patch 回归 |
| Gateway | `http://10.10.20.143:28999` | 内网 |
| 实例 | qutebrowser Pro smoke | `config/swe/pro-python-smoke.json` |

```bash
MAX_ITERATIONS=50 bash /root/UEnv/scripts/run-openhands-pro-20877.sh llm   # 208.77
```

### 6.3 Prompt 增强（建议，非阻塞）

`run_swebenchpro_official.py` 已含 `/app` 引导；若仍搜 `/home`，追加：

- 「**禁止**在 `/home` 搜索；仓库根目录 **仅** `/app`」
- 「修改前先 `ls -la /app` 与 `grep -r hide_qt_warning /app`」

---

## 7. 验收标准

### 7.1 阶段 A — vLLM 推理后端（内网）

| 检查项 | 通过条件 |
|--------|----------|
| 服务启动 | 无 OOM，8 卡有利用率 |
| Chat | `http://127.0.0.1:8000/v1/chat/completions` 正常 |
| **Tool call** | 直连 vLLM 时 `tool_calls` 非空（§5.4） |

### 7.2 阶段 A′ — uenv-llm-gateway（对外 :18888）

| 检查项 | 通过条件 |
|--------|----------|
| 就绪 | `curl http://127.0.0.1:18777/health` → **`ok`** |
| 鉴权 | 无 Key → `401`；有 Key → `/v1/models` 200 |
| 反代 Chat | 公网或 `127.0.0.1:18888/v1/chat/completions` 正常 |
| **Tool call** | 经 **:18888** 转发后 `tool_calls` 非空 |
| 公网 | `219.147.100.43:18888` 可达（防火墙已开通） |

### 7.3 阶段 B — OpenHands + Gateway

| 检查项 | 通过条件 |
|--------|----------|
| 会话 | `create_session` 成功 |
| 多轮 loop | trajectory `step_count` ≥ 10 |
| 探索 | 至少一步在 **`/app`** 下 `ls` / `grep` |
| 评分 | **primary**：`reward=1.0` / 56/56；**minimal**：有效 patch 或 F2P 失败数下降 |

### 7.4 产物路径

| 产物 | 路径 |
|------|------|
| OpenHands run | `/var/log/uenv/openhands-runs/pro-official-llm-<stamp>/` |
| 7143 轨迹 | `GET http://10.10.20.143:28999/runtime/v1/trajectories/{id}` |
| vLLM 日志 | `journalctl -u vllm-dsv3-awq` |
| 网关日志 | `journalctl -u uenv-llm-gateway` |

---

## 8. 故障排查与降级

| 现象 | 可能原因 | 处理 |
|------|----------|------|
| 启动 OOM | KV 过大 | 降 `max-model-len` → **16384**；`gpu-memory-utilization` → **0.88** |
| Marlin / AWQ 报错 | vLLM 版本 | 确认 `VLLM_MARLIN_USE_ATOMIC_ADD=1`；升级 vLLM |
| Tool call 为空 | 缺 chat template | 必须 `--chat-template ...deepseekv3.jinja` |
| Tool call 解析失败 | parser 版本 | 对齐 vLLM ≥ 0.8.3；查 [vLLM #17784](https://github.com/vllm-project/vllm/pull/17784) |
| 推理极慢 | A100 无 FlashMLA | 单 Agent 可接受；勿并发压测 |
| Agent 超时 | 首 token 慢 | `UENV_LLM_HTTP_TIMEOUT_SECS=600` |
| **18888 401** | 未配 gateway key | 设置 `UENV_LLM_GATEWAY_API_KEY`；OpenHands env 同步 |
| **18888 503 backend_starting** | vLLM 未就绪 | 等 vLLM；查 `journalctl -u vllm-dsv3-awq` |
| 公网不通 | 防火墙 / 映射 | 确认 `219.147.100.43:18888` 与 §2.2 一致 |
| 仍 reward=0 | prompt / iter | 查 trajectory；对照 flash 基线 |
| 输出质量异常 | 社区量化 | 对比官方 API；必要时换 GPTQ 变体 |

**降级顺序**：

1. 调 vLLM 参数（上表）  
2. **DeepSeek 官方 API**（`deepseek-ai/DeepSeek-V3-0324` 云端）  
3. DashScope flash（仅保链路，已知 reward=0）  

---

## 9. 与现有组件关系

| 组件 | 是否变更 |
|------|----------|
| 7143 Worker / Gateway | **否** |
| Hub Pro catalog | **否** |
| `run_swebenchpro_official.py` | **否**（可选 prompt PR） |
| `gen-openhands-llm-config.py` | **否**（endpoint 改 :18888） |
| **`uenv-llm-gateway`** | **新增** — 7142 `:18888` 模型服务入口 |
| VeRL 训练 | **互斥** — AWQ 占满 8 卡 |

**OpenHands 官方方案** §1 原写「7142 云端 API」→ 本方案：**7142 本地 AWQ + `uenv-llm-gateway`（:18888）为主**；云端 API 为降级。

**与 7143 对称**：7143 `runtime_gateway :28999`（环境/沙箱）；7142 `uenv-llm-gateway :18888`（LLM）。二者职责分离。

---

## 10. 实施清单（Checklist）

- [ ] 7142 确认 8 卡空闲 + `/data` 磁盘 **≥ 400GB**  
- [ ] 下载 `cognitivecomputations/DeepSeek-V3-0324-AWQ`  
- [ ] 安装 vLLM ≥ 0.8.3，启动 `vllm-dsv3-awq`，通过 §7.1  
- [ ] 编写 `config/uenv-llm-gateway-7142.yaml` + `/root/.uenv-llm-gateway.env`  
- [ ] 实装并启动 `uenv-llm-gateway`，通过 §7.2（:18888 + :18777/health）  
- [ ] 更新 `config/uenv-worker-llm.env`（endpoint **`127.0.0.1:18888/v1`**）→ `openhands-llm-20877.json`（208.77）  
- [ ] `MAX_ITERATIONS=50 bash scripts/run-openhands-pro-20877.sh llm`（208.77）
- [ ] 更新 `secrets/README.md` §1.2 Adapter 行：18888 = LLM Gateway  
- [ ] （可选）`scripts/deploy-uenv-llm-7142.sh` 一键 §5.8  

---

## 11. 后续脚本与代码（待实装）

| 路径 | 作用 |
|------|------|
| `scripts/uenv-llm-gateway.py` | 网关主进程：鉴权、就绪探测、反代 `/v1/*` |
| `config/uenv-llm-gateway-7142.yaml` | 7142 冻结配置（§5.6） |
| `config/uenv-llm-gateway-7142.yaml.example` | 仓库内示例（key 用 env 占位） |
| `scripts/deploy-uenv-llm-7142.sh` | 安装 vLLM + 网关 systemd + §5.8 冒烟 |
| `scripts/stop-uenv-llm-7142.sh` | 停止 gateway + vLLM，释放 GPU |
| `config/uenv-worker-llm.env.example` | 增加「7142 gateway :18888」注释块 |

**网关实现要点**（Python 参考栈：`httpx` 异步反代 + `uvicorn`；或 `nginx` + `auth_request` 简化版）：

- 不修改 vLLM；仅 HTTP 转发  
- 支持 **`stream=true`** SSE 透传（OpenHands 若启用流式）  
- 日志：`/var/log/uenv/uenv-llm-gateway.log`（请求 id、latency、502/503 计数）

---

## 12. 备选方案（不采用为主路径）

### 12.1 DeepSeek-V3 FP8（H200 / 未来节点）

| 项 | 值 |
|----|-----|
| 模型 | `deepseek-ai/DeepSeek-V3` 或 `DeepSeek-V3-0324` FP8 |
| 硬件 | **8×H200/H20（141GB×8）** — 官方 recipe |
| 并行 | TP=8 + `--enable-expert-parallel` |
| 7142 A100 | **不推荐** — 权重 ~84GB/卡 + sm80 FP8 限制 |

### 12.2 GLM-5.1 AWQ（A100）

需 DSA/MLA fallback；工程风险高于 V3 AWQ。仅作对照实验。

### 12.3 官方 DeepSeek API

零 GPU；能力完整；联调初期可作 AWQ 部署前的 **能力上界对照**。

---

## 13. 参考链接

- [cognitivecomputations/DeepSeek-V3-0324-AWQ](https://huggingface.co/cognitivecomputations/DeepSeek-V3-0324-AWQ)
- [deepseek-ai/DeepSeek-V3-0324](https://huggingface.co/deepseek-ai/DeepSeek-V3-0324)
- [vLLM Tool Calling — deepseek_v3](https://docs.vllm.ai/en/latest/features/tool_calling/)
- [vLLM DeepSeek-V3 Recipe（FP8 / H200）](https://docs.vllm.ai/projects/recipes/en/stable/DeepSeek/DeepSeek-V3.html)
- [CROZ — V3-0324 AWQ on 8×A100 benchmark](https://croz.net/deepseek-v3-0324-heavy-load-benchmark/)
- 本仓 `260627-swe-openhands-acceptance-report.md`（flash 基线见 §2.2 历史 7142 行）
