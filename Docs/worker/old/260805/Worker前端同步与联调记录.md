# Worker 前端同步与联调记录（2026-08-05）

> **机器**：阿里云 Server `8.130.75.157`  
> **范围**：仅可视化前端（`:8888`）；**未**重启 `uenv-adapter-core` / Worker / Hub / Agent

---

## 1. 部署动作

| 项 | 内容 |
|----|------|
| 落点 | `/home/uenv-frontend-add/frontend` |
| 备份 | `/home/uenv-frontend-add/backups/<timestamp>/` |
| 进程 | 仅重启 `vite dev --port 8888`；`uenv-adapter-core` pid **60402** 全程保持 |
| 路由修正 | `server.worker.tsx` → `server_.worker.tsx`，使 `/server/worker` 与 `/server` 平级（避免嵌套进无 Outlet 的 Episode 页） |

同步文件：

- `src/components/worker-detail.tsx`（新）
- `src/components/worker-status-overview.tsx`
- `src/lib/worker-status.ts` / `worker-tree.ts`（新）
- `src/routes/server_.worker.tsx`（新）
- `Docs/worker/260805/面向用户的Worker前端设计.md`

---

## 2. 服务隔离验收

| 端口 / 进程 | 联调后 | 说明 |
|-------------|--------|------|
| `:8088` gRPC | 仍为 pid 60402 | 控制面未动 |
| `:50053` Obs | health `ok` | 内嵌 Obs 未动 |
| `:50052` admin | `worker_count=1` `accepting=true` | Worker 注册正常 |
| `:8077` trajectory | LISTEN | 未动 |
| `:8888` 前端 | 新 vite pid | 仅前端热替换 |

---

## 3. 真实数据联调

**Run**：`verl_swesmith_grpo_train_20260805_102624`（RUNNING，256 Episode）  
**Worker**：`worker-7143-pro`（admin：`ready`，load/capacity 与 Obs 一致）

| 步骤 | 结果 |
|------|------|
| 打开 `http://8.130.75.157:8888/server?run=…` | 已连接；Worker 总数 1；负载条可见 |
| 点击列表中的 `worker-7143-pro` | 跳转 `/server/worker?run=…&worker=worker-7143-pro&status=busy` |
| 详情页 | 执行中；本任务 Episode 253 / 已完成 244；负载 1/4；活跃 Episode 可见；`supported_env_types=qa,code,swe`；endpoint `219.147.100.43:28888` |
| 返回链接 | 回到 `/server?run=…` 保留 run |

说明：Obs 中该 Worker 的 `env_instances[]` 为空，详情页按设计展示空态「尚未上报环境实例」。

---

## 4. 访问入口

```text
任务台：http://8.130.75.157:8888/server?run=verl_swesmith_grpo_train_20260805_102624
Worker：http://8.130.75.157:8888/server/worker?run=verl_swesmith_grpo_train_20260805_102624&worker=worker-7143-pro&status=busy
运维台：http://8.130.75.157:8888/?run=…
```

---

## 11. 实时性修复与单条 Episode 验证（2026-08-05 晚）

### 根因
- Worker 详情**不是**静态快照页，而是 Obs `poll` + 舰队实时通道。
- 旧实现把「本 run 历史 ACTIVE Episode」当成当前执行，且心跳依赖易丢弃的 `WORKER_HEARTBEAT` 事件时间戳，出现「已连接但仍显示数小时前心跳 / step 卡住」的观感。

### 修复
- 新增 `/fleet` → admin `:50052` 只读代理；详情页每 3s 拉舰队名册。
- 活跃任务改为 **Server 当前 Episode 名册**；累计完成数单独标注「本训练运行」。
- 心跳优先显示 `last_heartbeat_secs`；Obs 侧另补了 snapshot 字段（需下次重启 `uenv-adapter-core` 生效，本期未重启核心进程）。

### 验证
| 项 | 结果 |
|----|------|
| 心跳 | UI 显示 `0~4 秒前心跳`，随刷新跳动 |
| 当前执行 | Episode ID / `已运行 Xs` 随舰队更新；GRPO 任务切换时 ID 变化 |
| 单条 QA smoke | `qa-live-1785935862` / gsm8k → `status=completed` `reward=1`（7142→Server:8088） |
| 核心服务 | `uenv-adapter-core` pid 未重启 |
