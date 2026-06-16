# UEnv 可视化前端

面向 UEnv 分布式训练链路的**观测面消费端**：展示训练 run 的工作流与对象层级树，后续通过 SSE 订阅聚合层推送的 `ChainState` 增量。

设计依据：[Docs/UEnv可视化实现规划v1.0.md](../Docs/UEnv可视化实现规划v1.0.md)

---

## 当前阶段：UI 骨架（Mock）

本目录已完成 **S0-7 / B-M1** 级别的工程与界面骨架，使用静态 Mock 数据驱动 `TrainingConsole`，**尚未**接入聚合层 REST/SSE 与 `ChainState` 状态层。

### 与规划文档的对齐情况

| 类别 | 规划要求 | 骨架状态 |
|------|----------|----------|
| **工程** | 路由、布局、主题（§3.3） | ✅ TanStack Start + React 19 + Tailwind 4 + shadcn/ui |
| **布局** | 工作流主区 + 侧栏树 + 顶栏控制 + 底部 Tab（§3.3） | ✅ 已实现 |
| **工作流视图** | submit → dispatch → execute → report 等阶段（§0.4 B） | ✅ UI + Mock；未绑定 `WorkflowGraph` |
| **树状详情** | run → worker → env_instance → episode → step（§5.5.3） | ✅ UI + Mock；未绑定 `TreeGraph` |
| **节点详情** | 工作流/树选中联动摘要（§0.4 B 建议） | ⚠️ 部分：工作流选中驱动详情，树选中未联动 |
| **顶栏 Run 控制** | 开始 / 终止 + 状态展示（§0.4 A） | ⚠️ 仅有「停止」按钮 UI，缺「开始训练」与真实 API |
| **实时 ↔ 快照** | 抓拍深拷贝 `ChainState`，切换视图（§3.1–3.2） | ⚠️ 有模式切换 UI；抓拍按钮当前仅切换模式，未实现深拷贝与快照列表写入 |
| **SSE 连接指示** | 连接中 / 重连中 / 已断开（§0.4 G） | ⚠️ 静态展示「SSE 已连接」 |
| **日志 / Metrics Tab** | P1 独立 Tab（§0.4 E） | ✅ Mock 占位（超前于 P0，便于后续接线） |
| **历史回放** | P1（§0.4 F） | ❌ 未开始 |
| **搜索 / 索引跳转** | 建议（§0.4 D） | ⚠️ Tab 空态占位 |
| **API 客户端** | REST + SSE 封装（§4） | ❌ 未实现 |
| **ChainState 层** | 本地副本 + `StateDelta` 合并（§5.5、§6.3） | ❌ 未实现 |
| **鉴权** | Bearer token（§13.1） | ❌ 未实现 |

**结论**：**UI 骨架与 §3.3 布局已对齐**，可作为 P0 开发基线；**功能层面距 P0 MVP（§10）仍有 B-M2～B-M27 等待办**，需等聚合层 stub/正式 API 就绪后逐项接线。

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

## 环境变量

当前骨架**不依赖**环境变量即可运行。接入聚合层后建议新增（命名可在联调时与聚合层对齐）：

| 变量 | 作用域 | 说明 |
|------|--------|------|
| `VITE_AGGREGATION_BASE_URL` | 客户端 | 聚合层 REST/SSE 根地址，如 `http://127.0.0.1:8090` |
| `VITE_AGGREGATION_TOKEN` | 客户端 | start/stop 等控制 API 的 Bearer token（§13.1） |
| `NODE_ENV` | 服务端 | `development` / `production` |

在项目根目录创建 `.env.local`（勿提交密钥）：

```env
VITE_AGGREGATION_BASE_URL=http://127.0.0.1:8090
VITE_AGGREGATION_TOKEN=your-operator-token
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

1. **聚合层 CORS**：若前端与聚合层不同源，需在聚合层配置 CORS（§13.1）。
2. **SSE 代理**：反向代理（Nginx 等）需关闭对 `/api/v1/runs/*/stream` 的响应缓冲，并适当拉长读超时。
3. **Nitro / Cloudflare**：`vite.config.ts` 使用 `@lovable.dev/vite-tanstack-config`；在非 Lovable 环境构建时 Nitro 部署插件默认跳过。若需 Workers 部署，在 `defineConfig` 中显式启用 `nitro: true` 并按目标平台配置。

---

## 目录结构

```
frontend/
├── src/
│   ├── components/
│   │   ├── training-console.tsx   # 主控制台（当前为 Mock 数据）
│   │   └── ui/                    # shadcn/ui 组件
│   ├── routes/
│   │   ├── __root.tsx             # 应用壳、QueryClient、全局样式
│   │   └── index.tsx              # 首页 → TrainingConsole
│   ├── lib/
│   │   ├── config.server.ts       # 服务端配置
│   │   └── api/                   # 预留：聚合层 API 客户端（待实现）
│   ├── server.ts                  # SSR 入口包装
│   ├── start.ts                   # TanStack Start 实例
│   └── styles.css                 # 设计系统 / 主题变量
├── vite.config.ts
├── package.json
└── README.md
```

路由约定见 `src/routes/README.md`（TanStack Start 文件路由，**不要**使用 Next.js 的 `pages/` 或 `app/` 结构）。

---

## 与聚合层的接口约定（待接入）

实现 P0 时将对接以下端点（详见规划 §4.3）：

| 方法 | 路径 | 用途 |
|------|------|------|
| `POST` | `/api/v1/runs` | 开始训练 run |
| `POST` | `/api/v1/runs/{training_run_id}/stop` | 终止 run |
| `GET` | `/api/v1/runs/{training_run_id}/stream` | SSE：`full_state` / `state_delta` / `run_status` / `ping` |
| `GET` | `/api/v1/runs/{training_run_id}/state` | 拉取完整 `ChainState` |

聚合层服务尚在规划中；开发时可先用 A 侧提供的 SSE stub（实现清单 S0-6）联调。

---

## 后续开发顺序（P0）

1. 在 `src/lib/types/` 定义 `ChainState`、`StateDelta`、`EventCursor`（对齐 §5.4–5.5）
2. 实现 `src/lib/api/aggregation-client.ts`（REST + EventSource/SSE）
3. 实现 `ChainState` store 与 `StateDelta` 合并 hook
4. 将 `training-console.tsx` 从 Mock 改为消费 store
5. 补全「开始训练」、真实快照深拷贝、连接状态与空态/异常态

任务明细见 [Docs/discussions/可视化前端相关/260612-实现清单.md](../Docs/discussions/可视化前端相关/260612-实现清单.md) §5.1。

---

## 联调前置

1. 聚合层 HTTP 服务已启动并可访问
2. （可选）至少一条 Adapter → Server → Worker 事件链路向聚合层上报
3. 前端 `VITE_AGGREGATION_BASE_URL` 指向聚合层
4. 控制 API token 已配置且具备 operator 权限

完整联调步骤见实现清单 §6（人员 C）与 [全链路联调文档](../Docs/全链路联调-各层接口与参数字段.md)。
