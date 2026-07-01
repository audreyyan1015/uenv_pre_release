# UEnv LLM Gateway（7142 DeepSeek-V3-0324-AWQ）

在 A100 **7142** 上部署 **DeepSeek-V3-0324-AWQ**（vLLM TP=8）+ **uenv-llm-gateway**（`:18888` 对外 OpenAI API，`:18777` 健康检查）。

方案详见 [`Docs/260626-deepseek-v3-fp8-7142-deployment-plan.md`](../../Docs/260626-deepseek-v3-fp8-7142-deployment-plan.md)；SSH/端口见 [`secrets/README.md`](../../secrets/README.md) §1.2。

## 架构

```text
客户端 (OpenHands / curl)
    → uenv-llm-gateway :18888  (鉴权 + 就绪探测)
        → vLLM :8000 (127.0.0.1 only, 8×A100 TP=8)
```

| 端口 | 用途 |
|------|------|
| `18888` | OpenAI-compatible `/v1/*` |
| `18777` | `GET /health` |
| `8000` | vLLM 内网后端 |

公网映射：`219.147.100.43:18888` / `:18777`（7142）。

## 目录结构

```text
scripts/uenv-llm-gateway/
├── uenv_llm_gateway.py          # 网关主进程
├── requirements.txt
├── deploy-uenv-llm-7142.sh      # 同步 + 安装 + 启动
├── stop-uenv-llm-7142.sh        # 停止并释放 GPU
├── smoke-test-7142.sh           # 本机冒烟
├── start-vllm-when-ready-7142.sh
├── resume-download-7142.sh          # 7142 断点续传 + 断线自动重试
├── download-awq-resumable.py        # snapshot_download（禁用 xet CAS）
├── remote-start-resume-download-7142.sh  # 开发机同步并启动续传
├── monitor-download-7142.py
└── README.md

config/uenv-llm-gateway-7142.yaml.example   # 网关配置模板（仓库根）
```

## 前置条件（7142）

1. **8× A100-80GB 空闲**（与 VeRL 互斥）
2. **`/data` 可用空间 ≥ 400GB**（AWQ 权重 ~360GB）
3. 模型已下载至 **`/data/models/DeepSeek-V3-0324-AWQ`**

```bash
export HF_HOME=/data/huggingface
pip install -U "huggingface_hub[cli]"
huggingface-cli download cognitivecomputations/DeepSeek-V3-0324-AWQ \
  --local-dir /data/models/DeepSeek-V3-0324-AWQ \
  --local-dir-use-symlinks False
```

## 从开发机一键部署

```powershell
# Windows PowerShell（在 UEnv 仓库根目录）
$env:UENV_SSH_KEY = "secrets\2a9f778a35e7d08c738c79493ba643ef_65c3b455afbe3c81a8a757c01b0faae8_8.142"
bash scripts/uenv-llm-gateway/deploy-uenv-llm-7142.sh
```

```bash
# Linux / Git Bash
export UENV_SSH_KEY=secrets/2a9f778a35e7d08c738c79493ba643ef_65c3b455afbe3c81a8a757c01b0faae8_8.142
bash scripts/uenv-llm-gateway/deploy-uenv-llm-7142.sh
```

子命令：

| 命令 | 说明 |
|------|------|
| `deploy`（默认） | 安装依赖、写 systemd、启动 vLLM + 网关、冒烟 |
| `smoke` | 仅同步代码并跑冒烟（服务需已运行） |
| `gateway-only` | 只重启网关（vLLM 已在跑） |

### 模型下载与 vLLM 启动

7142 无法直连 HuggingFace，使用 **hf-mirror**；下载脚本带**断点续传 + 断线自动重试**（关闭 `hf_transfer`，避免 xet CAS 403）。

```bash
# 开发机：同步脚本并启动续传（推荐）
bash scripts/uenv-llm-gateway/remote-start-resume-download-7142.sh

# 开发机：持续监控（可选 --auto-restart 在 supervisor 挂掉时自动拉起）
python scripts/uenv-llm-gateway/monitor-download-7142.py --auto-restart

# 7142 上手动
bash /root/UEnv/scripts/uenv-llm-gateway/resume-download-7142.sh start
bash /root/UEnv/scripts/uenv-llm-gateway/resume-download-7142.sh status
tail -f /var/log/uenv/model-download.log

# 查看体积
du -sh /data/models/DeepSeek-V3-0324-AWQ
```

# 下载完成后（≥300GB）一键启动 vLLM 并等待网关就绪
bash /root/UEnv/scripts/uenv-llm-gateway/start-vllm-when-ready-7142.sh
bash /root/UEnv/scripts/uenv-llm-gateway/smoke-test-7142.sh
```

## 7142 本机手动操作

```bash
# 启动整栈
systemctl start uenv-llm.target
# 或
systemctl start vllm-dsv3-awq && systemctl start uenv-llm-gateway

# 日志
journalctl -u vllm-dsv3-awq -f
journalctl -u uenv-llm-gateway -f

# 健康
curl -s http://127.0.0.1:18777/health
curl -s http://127.0.0.1:8000/v1/models

# 冒烟
source /root/.uenv-llm-gateway.env
bash /root/UEnv/scripts/uenv-llm-gateway/smoke-test-7142.sh

# 停止（还 GPU）
bash /root/UEnv/scripts/uenv-llm-gateway/stop-uenv-llm-7142.sh --local
```

## 对外调用

```bash
source /root/.uenv-llm-gateway.env

# 本机
curl -s http://127.0.0.1:18888/v1/models \
  -H "Authorization: Bearer $UENV_LLM_GATEWAY_API_KEY"

# 公网
curl -s http://219.147.100.43:18888/v1/chat/completions \
  -H "Authorization: Bearer $UENV_LLM_GATEWAY_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"model":"deepseek-v3-0324-awq","messages":[{"role":"user","content":"Say OK"}],"max_tokens":16}'
```

## OpenHands 对接

编辑 **`config/uenv-worker-llm.env`**（勿提交）：

```env
UENV_LLM_ENDPOINT=http://127.0.0.1:18888/v1
UENV_LLM_MODEL_NAME=deepseek-v3-0324-awq
UENV_LLM_API_KEY=<与 UENV_LLM_GATEWAY_API_KEY 相同>
UENV_LLM_MAX_TOKENS=8192
UENV_LLM_TEMPERATURE=0.2
UENV_LLM_HTTP_TIMEOUT_SECS=600
```

```bash
python3 scripts/gen-openhands-llm-config.py config/uenv-worker-llm.env config/openhands-llm-20877.json
# 208.77 上执行 OpenHands llm 评测（LLM endpoint 可指向 7142 :18888）
MAX_ITERATIONS=50 bash scripts/run-openhands-pro-20877.sh llm
```

## 网关行为

| 场景 | HTTP |
|------|------|
| 无 API Key | `401` |
| `model.enabled=false` | `503` `model_disabled` |
| vLLM 未就绪 | `503` `backend_starting` |
| 正常 | 透明转发 `/v1/*`（含 `stream` SSE） |
| `GET /health` | `200 ok` 或 `503 starting/backend_down` |

## 故障排查

| 现象 | 处理 |
|------|------|
| vLLM OOM | 降 `--max-model-len` 至 16384；`gpu-memory-utilization` 0.88 |
| tool_calls 为空 | 确认 `--chat-template` 与 `deepseek_v3` parser |
| 503 backend_starting | AWQ 冷启动 10–15min；`journalctl -u vllm-dsv3-awq` |
| 401 | 检查 `/root/.uenv-llm-gateway.env` 与客户端 Key |

## 环境变量（部署脚本）

| 变量 | 默认 |
|------|------|
| `UENV_SSH_KEY` | 7142 私钥路径 |
| `UENV_MODEL_DIR` | `/data/models/DeepSeek-V3-0324-AWQ` |
| `UENV_VLLM_VENV` | `/opt/vllm-dsv3-awq` |
| `UENV_GATEWAY_VENV` | `/opt/uenv-llm-gateway` |
