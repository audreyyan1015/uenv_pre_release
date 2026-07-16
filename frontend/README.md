# UEnv 可视化前端

面向 UEnv 分布式训练链路的**观测面消费端**：展示训练 run 的工作流与对象层级树，通过 REST + SSE 订阅 **`uenv-server` 内嵌 Obs 子系统**（Server 侧聚合，`:50053`）推送的 `ChainState` 增量。

设计依据：[Docs/discussions/可视化前端相关/2026-07-15-Server侧聚合与前端接入规划.md](../Docs/discussions/可视化前端相关/2026-07-15-Server侧聚合与前端接入规划.md) §4（前端推进路线）；数据模型/事件治理沿用 [260612-前端完整设计.md](../Docs/discussions/可视化前端相关/260612-前端完整设计.md) §5–§6。

> 2026-07-15 变更：观测面**不再是独立聚合层进程**，改为内嵌进 `uenv-server`；前端仍然只连观测面 HTTP（REST + SSE），不直连控制面 gRPC。

---

## 当前阶段：FE-0 / FE-1（类型 + client + store + 脱 Mock）

已完成：

| 类别 | 说明 |
|------|------|
| **类型层** | `src/lib/types/chain-state.ts`：`ChainState` / `WorkflowGraph` / `TreeGraph` / `StateDelta` / `ClientSnapshot` 等，字段名与 Server JSON snake_case 对齐 |
| **合并逻辑** | `src/lib/store/apply-delta.ts`：`applyStateDelta` 按 `entity_key`（`run` / `workflow` / `tree` / `episode:{id}` / `worker:{id}`）分派合并，只接受更高 `event_seq` |
| **本地 store** | `src/lib/store/chain-store.ts`：基于 `useSyncExternalStore` 的最小状态容器（无 zustand），管理连接态、快照列表、live/snapshot 视图切换 |
| **API 客户端** | `src/lib/api/aggregation-client.ts`：`AggregationClient.getState` / `subscribeStream`（原生 `EventSource` + `fetch`） |
| **离线演示** | `src/lib/api/fixture.ts`：未配置 `VITE_AGGREGATION_BASE_URL` 时的静态 `ChainState` + `StateDelta[]`，可离线演示状态推进 |
| **订阅 Hook** | `src/hooks/use-run-stream.ts`：`useRunStream(runId)`，fixture/真实模式自动切换，断线自动重连（带 `Last-Event-ID`） |
| **主控制台** | `src/components/training-console.tsx`：已脱离静态 Mock 数组，工作流 / 树 / 顶栏 / 快照均绑定 `useRunStream` 返回的真实（或 fixture）`ChainState` |

P0 明确不做（详见规划 §4.3）：

| 项 | 现状 |
|----|------|
| 开始 / 终止训练的真实控制 API | 顶栏按钮已禁用，`title="P0 只读观测：开始/终止训练留待 P1 接入 REST 控制"` |
| 日志 / Metrics 面板真实数据 | 底部 Tab 为占位说明，标注 P1 |
| 历史回放 | 未开始（P1） |

---

## 环境要求

| 工具 | 版本建议 | 说明 |
|------|----------|------|
| **Node.js** | ≥ 20（已在 22.x 验证） | 必需 |
| **npm** | 随 Node 自带 | 推荐；`npm install` / `npm run dev` |
| **Bun** | 可选 | 仓库含 `bun.lock`；若已安装 Bun 可用 `bun install` / `bun run dev` |

---

## 快速开始

```bash
# 进入前端目录
cd frontend

# 安装依赖（二选一）
npm install
# bun install

# 开发模式（默认 http://localhost:8080）
npm run dev
# bun run dev
```

浏览器打开 **http://localhost:8080/** 即可看到 Mock 版训练控制台。

---

## 常用命令

| 命令 | 作用 |
|------|------|
| `npm run dev` | 本地开发，HMR，默认端口 **8080** |
| `npm run build` | 生产构建，产物在 `dist/client`（客户端）与 `dist/server`（SSR） |
| `npm run preview` | 预览生产构建（需先 `build`） |
| `npm run lint` | ESLint 检查 |
| `npm run format` | Prettier 格式化 |

---

## 环境变量：Fixture 演示 vs 真实联调

**不配置任何环境变量也能跑起来**——`VITE_AGGREGATION_BASE_URL` 为空时，前端自动回落**离线 fixture 演示模式**（`src/lib/api/fixture.ts`），本地灌入一份静态 `ChainState` 并按固定间隔回放几条 `StateDelta`，无需任何后端即可看到工作流/树状态推进。

| 变量 | 作用域 | 说明 |
|------|--------|------|
| `VITE_AGGREGATION_BASE_URL` | 客户端 | Server Obs 根地址，如 `http://127.0.0.1:50053`；留空 = fixture 模式 |
| `VITE_AGGREGATION_TOKEN` | 客户端 | Bearer token；`EventSource` 无法带自定义请求头，会作为 `?token=` 查询参数附在 SSE URL 上；`getState` 走 `fetch`，用 `Authorization` 头 |
| `VITE_DEFAULT_RUN_ID` | 客户端 | 页面未带 `?run=` 时使用的默认 `training_run_id`（联调建议先用 `_orphan`，见规划 §11 拍板项 4） |
| `NODE_ENV` | 服务端 | `development` / `production` |

复制示例文件后按需修改：

```bash
cp .env.local.example .env.local
```

```env
# .env.local.example
VITE_AGGREGATION_BASE_URL=http://127.0.0.1:50053
VITE_AGGREGATION_TOKEN=
VITE_DEFAULT_RUN_ID=_orphan
```

`training_run_id` 也可以直接通过 URL 指定，优先级高于 `VITE_DEFAULT_RUN_ID`：

```
http://localhost:8080/?run=my-training-run
```

> 以 `VITE_` 开头的变量会打进客户端包，**不要**把长期密钥写进前端；生产环境应通过网关或短期 token 下发。

服务端专用配置见 `src/lib/config.server.ts`（`.server.ts` 后缀，不会进入浏览器包）。

---

## 部署

### 静态 + SSR（TanStack Start）

```bash
npm run build
```

- 客户端静态资源：`dist/client/`
- SSR 服务入口：`dist/server/server.js`

本地预览：

```bash
npm run preview
```

### 生产部署注意

1. **Server Obs CORS**：若前端与 `uenv-server` Obs（`:50053`）不同源，需在 Server 侧配置 CORS（规划 §13.1）。
2. **SSE 代理**：反向代理（Nginx 等）需关闭对 `/api/v1/runs/*/stream` 的响应缓冲，并适当拉长读超时。
3. **Nitro / Cloudflare**：`vite.config.ts` 使用 `@lovable.dev/vite-tanstack-config`；在非 Lovable 环境构建时 Nitro 部署插件默认跳过。若需 Workers 部署，在 `defineConfig` 中显式启用 `nitro: true` 并按目标平台配置。

---

## 目录结构

```
frontend/
├── src/
│   ├── components/
│   │   ├── training-console.tsx   # 主控制台：绑定 useRunStream，脱离静态 Mock
│   │   └── ui/                    # shadcn/ui 组件
│   ├── hooks/
│   │   └── use-run-stream.ts      # 订阅某 training_run_id；fixture/真实模式自动切换
│   ├── routes/
│   │   ├── __root.tsx             # 应用壳、QueryClient、全局样式
│   │   └── index.tsx              # 首页 → TrainingConsole
│   ├── lib/
│   │   ├── config.server.ts       # 服务端配置
│   │   ├── types/
│   │   │   └── chain-state.ts     # ChainState / WorkflowGraph / TreeGraph / StateDelta 等类型
│   │   ├── store/
│   │   │   ├── apply-delta.ts     # applyStateDelta / emptyChainState
│   │   │   └── chain-store.ts     # useSyncExternalStore 兼容的本地状态容器
│   │   └── api/
│   │       ├── aggregation-client.ts  # Server Obs REST + SSE 客户端
│   │       └── fixture.ts             # 离线演示用静态 ChainState + StateDelta[]
│   ├── vite-env.d.ts              # VITE_* 环境变量类型声明
│   ├── server.ts                  # SSR 入口包装
│   ├── start.ts                   # TanStack Start 实例
│   └── styles.css                 # 设计系统 / 主题变量
├── .env.local.example
├── vite.config.ts
├── package.json
└── README.md
```

路由约定见 `src/routes/README.md`（TanStack Start 文件路由，**不要**使用 Next.js 的 `pages/` 或 `app/` 结构）。

---

## 与 Server Obs 的接口约定

对接以下端点（详见规划 §4.3、§6）：

| 方法 | 路径 | 用途 |
|------|------|------|
| `GET` | `/api/v1/runs/{training_run_id}/state` | 拉取完整 `ChainState`（`AggregationClient.getState`） |
| `GET` | `/api/v1/runs/{training_run_id}/stream` | SSE：`full_state` / `state_delta` / `run_status` / `ping`（`AggregationClient.subscribeStream`） |
| `POST` | `/api/v1/runs` | 开始训练 run（P1，当前前端按钮禁用） |
| `POST` | `/api/v1/runs/{training_run_id}/stop` | 终止 run（P1，当前前端按钮禁用） |

`uenv-server` 侧的 Obs 子系统实现见规划 §6（`uenv-server/src/obs/`）。

---

## 如何运行：Fixture 演示 vs 接 Server Obs

### 方式一：Fixture 离线演示（无需任何后端）

```bash
cd frontend
npm install
npm run dev   # 不创建 .env.local，或留空 VITE_AGGREGATION_BASE_URL 即可
```

打开 `http://localhost:8080/`，顶栏会显示「Fixture 演示」标记，工作流/树状态每隔约 1.8 秒推进一次。

### 方式二：接真实 Server Obs

```bash
# 1) 确认 uenv-server 已启动且 Obs HTTP 监听 :50053（可用 seed run 联调，见规划 §4.5）
# 2) 前端配置指向该地址
cd frontend
cp .env.local.example .env.local
# 编辑 .env.local：VITE_AGGREGATION_BASE_URL=http://127.0.0.1:50053
npm install
npm run dev
# 打开 http://localhost:8080/?run=<training_run_id>
```

联调顺序建议（规划 §4.5）：**先 fixture 走通 UI → 接 Obs seed 数据 → 接真实 SubmitEpisode 链路**。

---

## 已知限制（P0 范围）

- 开始 / 终止训练：按钮已接线到禁用态，真实 REST 控制留给 P1（规划 §4.3 FE-1 备注）。
- 日志 / Metrics Tab：仍是占位说明文案，等 Server Obs 提供对应查询 API 后再接（规划 §0.4 E）。
- 历史回放：未实现（规划 §0.4 F，P1）。
- 事件流 Tab 展示的是**当前 `ChainState` 派生**的最近变化列表，不是服务端原始事件重放；完整事件日志查询是 Server 侧 P1 能力。
