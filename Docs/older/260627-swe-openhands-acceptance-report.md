# SWE-bench Pro + OpenHands 验收报告（最终版）

> **文档版本**：v2.0（整合归档）  
> **日期**：2026-06-27  
> **取代**：`260624-swe-bench-pro-7143-联调报告.md`、`260625-openhands-7142-acceptance.md`、`260627-openhands-20877-acceptance.md`  
> **方案**：`260627-swe-openhands-integration-plan.md`

---

## 0. 结论

| 验收项 | 状态 |
|--------|------|
| Hub Pro catalog 拉取 | **通过** |
| Worker docker pull `jefzda/sweap-images` | **通过**（mirror 回退见 §3） |
| 7143 Runtime Gateway + Pro grader | **通过** |
| duck-type gold（NodeBB JS / qutebrowser Python） | **reward=1.0** |
| 轨迹捕获（TrajectoryRef + GET bundle） | **通过** |
| OpenHands 官方 SDK gold（208.77） | **reward=1.0，56/56** |
| OpenHands 官方 SDK llm（历史 7142 / flash） | loop 跑通；**reward=0**（Agent 未改源码） |
| OpenHands runner 公网 **8777/8888** | **通过** |

**核心链路**：Hub 元数据 → 7143 拉镜像 → Gateway 会话 → 容器 `/app` → gold/Agent → grader → 轨迹落盘。

---

## 1. 拓扑（当前）

| 组件 | 地址 | 说明 |
|------|------|------|
| OpenHands | **8.130.208.77** | runner **8777/8888**；SSH `root` / `dev@BDW2026` |
| Worker | **7143** `10.10.20.143` | Gateway **:28097**；gRPC **:28888** |
| LLM（可选） | 7142 **:18888** | DeepSeek-V3 AWQ 网关 |
| Hub | `8.130.95.176:8088` | Pro catalog |

**测试实例**：

- Python：`instance_qutebrowser__qutebrowser-f91ace96223cac8161c16dd061907e138fe85111-v059c6fdc75567943479b23ebca7c07b5e9a7f34c`
- JS：`instance_NodeBB__NodeBB-04998908ba6721d64eba79ae3b65a351dcfbc5b5-vnan`

---

## 2. 测试结果汇总

### 2.1 7143 duck-type（`run_swebench.py`）

| 实例 | 模式 | reward | tests |
|------|------|--------|-------|
| NodeBB (JS) | gold | **1.0** | 291/291 |
| qutebrowser (Python) | gold | **1.0** | 56/56 |
| qutebrowser | gold + 轨迹 | **1.0** | 56/56；`trj-worker-7143-pro-1782321085352-00001` |
| qutebrowser | llm 单轮 | 0.0 | 52/56；幻觉 patch 路径 |

### 2.2 OpenHands 官方 SDK

| 阶段 | 节点 | 模式 | reward | tests | 备注 |
|------|------|------|--------|-------|------|
| v1.3 | 7142 | gold | **1.0** | 56/56 | `trj-…-00003` |
| v1.3 | 7142 | llm (flash, 15 iter) | 0.0 | 52/56 | `trj-…-00004`；多轮 tool use 正常 |
| **v2.0** | **208.77** | **gold** | **1.0** | **56/56** | `pro-official-gold-20260627-160516/` |

**LLM 失败共性**：Agent 在 `/home` 搜索，未改 `/app` 源码 → 4 条 F2P 失败（`hide_qt_warning`）；**非 Gateway/grader 回归**。

---

## 3. 主要问题与解决（精选）

| # | 现象 | 解决 |
|---|------|------|
| 1 | Windows 部署无 rsync | tar + scp；7143 `sed` 去 CRLF |
| 2 | HF 连不通 | `HF_ENDPOINT=https://hf-mirror.com` |
| 3 | Pro 容器 exit 126 | `--entrypoint tail -f /dev/null` |
| 4 | 工作区路径错误 | Pro 用 `/app` 非 `/testbed` |
| 5 | Docker Hub 429 | `pull-pro-image-7143.sh` 多 mirror |
| 6 | 208.77 无法访问内网 Gateway | `uenv-gateway-tunnel.service` |
| 7 | runner 端口未在安全组 | 改用 **8777/8888**（阿里云统一口） |
| 8 | 208.77 `uv sync` lxml 哈希失败 | 自 7142 复制 SDK `.venv` |

完整问题表见归档 `260624-swe-bench-pro-7143-联调报告.md` §4。

---

## 4. 复现命令

```bash
# 7143 Python Pro duck-type 一键
bash /root/UEnv/scripts/deploy-pro-python-openhands-7143.sh

# 208.77 OpenHands 官方 gold/llm
bash /root/UEnv/scripts/run-openhands-pro-20877.sh gold
MAX_ITERATIONS=50 bash /root/UEnv/scripts/run-openhands-pro-20877.sh llm

# 7143 轨迹查询
curl -sS -H 'X-API-Key: swe-pro-secret' \
  http://127.0.0.1:28097/runtime/v1/trajectories/{trajectory_id}

# 208.77 runner
curl http://8.130.208.77:8777/health
curl -X POST http://8.130.208.77:8888/v1/runs \
  -H 'Content-Type: application/json' -d '{"mode":"gold","max_iterations":30}'
```

---

## 5. 遗留项

| 项 | 说明 |
|----|------|
| LLM resolve 率 | flash 15 iter reward=0；待更强模型 / 更高 iter / prompt |
| Server Register | `8.130.75.157:8088` 偶发 degraded 启动 |
| Hub SWE manifest | `GET /envs/swe/versions/latest` → 404 |
| A100 NAT :28097 | 未开通；当前用 SSH 隧道 |

---

## 6. 变更记录

| 版本 | 日期 | 说明 |
|------|------|------|
| v1.0–1.4 | 2026-06-24–25 | 7143 联调、7142 OpenHands 试点 |
| v2.0 | 2026-06-27 | 208.77 迁移验收；文档整合 |
