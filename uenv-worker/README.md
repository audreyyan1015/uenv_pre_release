# uenv-worker — UEnv 环境执行引擎

Worker 是 UEnv 分布式环境框架的**执行层**，负责执行完整 Episode 周期。Worker 内部完成环境交互循环：模型回调获取 action → 环境 step → Reward 计算，直至 Episode 完成。

## 职责

- **Episode 执行**：管理 reset() → N × step() → close() 完整周期
- **推理端点调用**：直连 vLLM / SGLang 等推理服务获取 action
- **预热池**：提前创建环境实例，消除冷启动延迟
- **环境生命周期**：管理 Environment 实例的创建、复用和销毁

## 架构

```
┌────────────────────────────────────────────┐
│  uenv-worker                               │
│                                              │
│  ┌──────────────────────────────────────┐   │
│  │ EpisodeExecutor                       │   │
│  │                                      │   │
│  │  ┌──────┐  ┌──────┐  ┌──────────┐  │   │
│  │  │Reset  │→│Model │→│Env.step  │  │   │
│  │  │ env   │  │Call  │  │+ Reward  │  │   │
│  │  └──────┘  └──────┘  └──────────┘  │   │
│  │       │        ↻ (until done)        │   │
│  │       └──────────────────────────────┘   │
│  └──────────────────────────────────────┘   │
│                                              │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐   │
│  │ 推理客户端│ │ 预热池   │ │ 状态机   │   │
│  │ HTTP/gRPC │ │ LRU 缓存 │ │ Worker   │   │
│  │ Ray Actor │ │ 环境实例 │ │ 生命周期 │   │
│  └──────────┘ └──────────┘ └──────────┘   │
│                                              │
│  ┌──────────────────────────────────────┐   │
│  │ gRPC 客户端 (连接 uenv-server)        │   │
│  │ Register / Heartbeat / ReportResult  │   │
│  └──────────────────────────────────────┘   │
└────────────────────────────────────────────┘
```

## 执行模式

| 模式 | 说明 | 适用场景 |
|:-----|:------|:---------|
| 单轮 | 一步完成，action = 完整输出 | 数学、问答、代码生成 |
| 多轮 | 多次 step，每步调用模型 | 工具使用、多步推理 |
| 模型回调 | Worker 内部循环调用推理服务 | Agent 训练、ReAct 循环 |
| 可定制 | 自定义执行循环 | 复杂环境逻辑 |

## 环境系统

### Environment ABC

Worker 内嵌 Python 环境，支持标准 Environment 泛型基类：

| 方法 | 功能 |
|:-----|:------|
| reset(seed) | 重置环境到初始状态 |
| step(action) | 执行一步，返回 (obs, reward, terminated, truncated, info) |
| close() | 释放环境资源 |

### MCPEnvironment 中间层

Agent 训练场景的自动工具路由层。当 action type 为 call_tool 时自动分发到注册的 MCPTool，否则委派给 _step_impl()。

### Reward 系统（v4.0 兼容）

四层可组合 Reward 架构（类 nn.Module）：

| 层 | 组件 | 说明 |
|:----|:------|:------|
| 信号源 | RuleReward / NeuralRM / LLMJudge | 产生原始奖励信号 |
| 容器 | Sequential / Gate / WeightedSum | 组合多个奖励 |
| 信用分配 | ExponentialDiscounting / Uniform | Episode 级→步级分配 |
| 归一化 | GroupNorm / SumThenNormalize | 标准化奖励分布 |

## 快速使用

```bash
# 启动 Worker（连接 Server）
cargo run -- start --server-addr http://127.0.0.1:50051

# 查看状态
cargo run -- status
```

## 配置

参考 ../config/worker.example.toml：

```toml
server_addr = "http://127.0.0.1:50051"
worker_id = "worker-1"
supported_env_types = ["math", "code", "agent"]
max_concurrent_episodes = 8
```

## 后端引擎

| 后端 | 启动时间 | 隔离 | 适用 |
|:-----|:---------|:-----|:------|
| ProcessBackend | <10ms | 进程级 | 开发调试 |
| PodmanBackend | ~2s | Rootless 容器 | 生产部署 |

## 依赖

- **通信**: tonic (gRPC), reqwest (HTTP)
- **运行时**: tokio
- **环境**: Python 3.10+（内嵌环境执行）
