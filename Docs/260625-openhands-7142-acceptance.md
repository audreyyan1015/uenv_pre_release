# OpenHands 官方接入 — 7142 实机验收记录

> **日期**：2026-06-25  
> **关联**：`260625-openhands-official-integration-plan.md`

## 部署

| 项 | 值 |
|----|-----|
| OpenHands SDK | **v1.27.0**（`/opt/openhands/benchmarks/vendor/software-agent-sdk`） |
| Benchmarks SHA | `82687c83dfcc193989336f41d235612c02f2c044` |
| SDK SHA | `43376f1868ffd702746080714a59c16d3f69ec12` |
| 7142 → 7143 Gateway | `http://10.10.20.143:28999` |
| 驱动 | `integrations/openhands/run_swebenchpro_official.py` |
| 运行脚本 | `scripts/run-openhands-pro-7142.sh` |

**说明**：7142 无法直连 GitHub；benchmarks 通过本机 tarball 上传，SDK 用 `uv sync` 在 `vendor/software-agent-sdk` 安装（跳过 benchmarks 根目录 `swt-bench` 依赖）。

## 验收结果（qutebrowser Pro smoke）

| 模式 | reward | tests | trajectory_id |
|------|--------|-------|---------------|
| **gold** | **1.0** | 56/56 | `trj-worker-7143-pro-1782324281766-00003` |
| **llm**（deepseek-v4-flash，15 iter） | 0.0 | 52/56 | `trj-worker-7143-pro-1782324511090-00004` |

### LLM 行为摘要

- OpenHands SDK **Agent + Conversation + terminal/file_editor/finish** 已跑通多轮 loop。
- Gateway 工具 shim 生效（命令在 **7143 容器 `/app`** 执行，非 7142 本地）。
- Agent 在 15 步内未定位仓库文件（大量 `find /home`），未产生有效 patch → 4 条 F2P 仍失败（与此前单轮 LLM 类似，属 **Agent 探索策略** 问题，非 Gateway/grader 回归）。
- 产物目录（7142）：`/var/log/uenv/openhands-runs/pro-official-llm-20260625-020613/`

## 复现

```bash
# 7142 上
bash /root/UEnv/scripts/run-openhands-pro-7142.sh gold
MAX_ITERATIONS=30 bash /root/UEnv/scripts/run-openhands-pro-7142.sh llm
```

## 代码增量

- `integrations/openhands/uenv_runtime/workspace.py` — `UEnvWorkspace(LocalWorkspace)`
- `integrations/openhands/uenv_runtime/gateway_tools.py` — Gateway terminal/file_editor shim
- `integrations/openhands/run_swebenchpro_official.py` — 官方 SDK 驱动
- `integrations/openhands/PIN.md`
- `scripts/deploy-openhands-7142.sh` / `scripts/run-openhands-pro-7142.sh` / `scripts/gen-openhands-llm-config.py`
