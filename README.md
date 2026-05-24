# UEnv — 训练框架无关的分布式环境执行框架

[![Rust](https://img.shields.io/badge/language-Rust-orange)](https://www.rust-lang.org)
[![Python](https://img.shields.io/badge/language-Python-blue)](https://www.python.org)
[![gRPC](https://img.shields.io/badge/communication-gRPC-brightgreen)](https://grpc.io)

UEnv 是一个**训练框架无关的分布式环境执行框架**，为 LLM 后训练（Post-Training）提供统一的 Environment 接口。它支持 ROLL、VeRL、NeMo-RL、TRL、OpenRLHF 五大训练框架，通过 gRPC 双向流实现 Episode 级粒度的高效通信，让同一套环境实现可以在不同训练框架间无缝切换。

---

## 架构总览

```
训练集群 (ROLL / VeRL / NeMo-RL / TRL / OpenRLHF)
       │
       │ Episode Task (框架原生协议)
       ▼
┌──────────────────────────┐
│  uenv-bridge              │  ← 训练框架适配器层（协议转换）
│  GEMAdapter / VeRLAdapter │
│  NeMoAdapter / TRL / ...  │
└──────────┬───────────────┘
           │ gRPC EpisodeRequest / EpisodeResult
           ▼
┌──────────────────────────┐
│  uenv-server              │  ← 调度服务层（控制平面）
│  环境注册表 · 调度器      │
│  实例池 · 后端管理器      │
└──────────┬───────────────┘
           │ gRPC DispatchEpisode / ReportResult
           ▼
┌──────────────────────────┐
│  uenv-worker              │  ← 环境执行层
│  Episode 执行引擎         │
│  推理端点 · 预热池        │
└──────────┬───────────────┘
           │ pull 环境定义
           ▼
┌──────────────────────────┐
│  uenv-hub                 │  ← 环境注册层（离线目录）
│  环境元数据 · 版本管理    │
└──────────────────────────┘
```

### 核心设计原则

| 原则 | 说明 |
|------|------|
| **训练框架无关** | 通过 Adapter 适配 5 大训练框架，环境实现只需一次开发 |
| **控制/数据面分离** | Server 仅做调度编排，step 级数据流由 Worker 直连推理服务 |
| **三层解耦** | 训练框架↔环境、调度↔执行、定义↔分发，各层独立演进 |
| **渐进部署** | Process → Podman 渐进路径，从本地开发到生产部署 |

---

## 四大部分

| 目录 | 语言 | 职责 | 独立运行 |
|:-----|:-----|:-----|:---------|
| [`uenv-bridge`](./uenv-bridge/) | Python | 协议转换：框架原生格式 ↔ gRPC EpisodeRequest | 嵌入训练框架进程 |
| [`uenv-server`](./uenv-server/) | Rust | 调度服务：注册表 + 调度器 + 实例池 + 后端管理 | `uenv-server start` |
| [`uenv-worker`](./uenv-worker/) | Rust + Python | 执行服务：Episode 循环 + 推理调用 + 预热池 | `uenv-worker start` |
| [`uenv-hub`](./uenv-hub/) | Rust | 注册服务：环境元数据 + 版本管理 + 镜像索引 | `uenv-hub start` |

### 组件协作

```
Training       uenv-server        uenv-worker       uenv-hub
Adapter         (port 50051)                        (port 50053)
 ─────────      ──────────         ──────────        ─────────
     │               │                 │                 │
     │ ① EpisodeReq  │                 │                 │
     │──────────────►│                 │                 │
     │               │ ② 查注册表      │                 │
     │               │    调度决策     │                 │
     │               │ ③ DispatchEp   │                 │
     │               │────────────────►│                 │
     │               │                 │ ④ Episode 循环 │
     │               │                 │    reset →      │
     │               │                 │    model →      │
     │               │                 │    step →       │
     │               │                 │    reward →     │
     │               │                 │    done         │
     │               │ ⑤ Result       │                 │
     │               │◄────────────────│                 │
     │ ⑥ Response    │                 │                 │
     │◄──────────────│                 │ ⑦ pull env def  │
     │               │                 │────────────────►│
     └── 返回训练框架                  │                 │
```

---

## 快速开始

```bash
# 1. 启动环境注册中心
cd uenv-hub && cargo run -- start

# 2. 启动调度服务
cd uenv-server && cargo run -- start

# 3. 启动 Worker（连接 Server）
cd uenv-worker && cargo run -- start --server-addr http://127.0.0.1:50051

# 4. 训练框架侧安装适配器
pip install ./uenv-bridge
# 在训练脚本中使用: from uenv.bridge.gem import GEMAdapter
```

---

## 通信协议

全链路 **gRPC 双向流 + Protobuf 序列化**，无消息中间件。

| 链路 | 协议 | 说明 |
|:-----|:-----|:------|
| Bridge ↔ Server | `UEnvService` (gRPC) | 提交 Episode，返回结果 |
| Server ↔ Worker | `WorkerService` (gRPC) | 调度分发 + 心跳 + 上报 |
| Worker ↔ Hub | `HubService` (gRPC) | 拉取环境定义 |
| Worker ↔ 推理服务 | HTTP/gRPC（直连） | 模型回调，不经过 Server |

---

## 构建

```bash
# 各 part 独立编译，target 在各目录内
make build-server    # uenv-server/target/
make build-worker    # uenv-worker/target/
make build-hub      # uenv-hub/target/
make build          # 全部

# Proto 代码生成
make proto

# 测试
make test
```

---

## 许可

Apache-2.0
