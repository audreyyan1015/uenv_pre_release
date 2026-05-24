# uenv-bridge — UEnv 训练框架适配器

uenv-bridge 是 UEnv 分布式环境框架的**训练框架接入层**，负责将各训练框架的原生协议转换为统一的 gRPC EpisodeRequest，让同一套环境实现可在不同框架间无缝切换。

## 职责

- **协议转换**：将框架原生调用转换为 EpisodeRequest
- **结果适配**：将 EpisodeResult 转换回框架原生格式
- **错误映射**：框架错误码 ↔ UEnv 统一错误码的映射
- **通信**：通过 gRPC 与 uenv-server 交互

## 架构

```
训练框架进程
┌──────────────────────────────────────┐
│  ROLL / VeRL / NeMo-RL / TRL / ...  │
│                                      │
│  ┌──────────────────────────────┐   │
│  │  uenv-bridge (嵌入式)         │   │
│  │                              │   │
│  │  GEMAdapter  →  EpisodeReq   │   │
│  │  VeRLAdapter →  EpisodeReq   │   │
│  │  NeMoAdapter →  EpisodeReq   │   │
│  │                              │   │
│  │  EpisodeResult → 框架格式     │   │
│  └──────────┬───────────────────┘   │
└─────────────┼────────────────────────┘
              │ gRPC
              ▼
       uenv-server (port 50051)
```

## 支持的训练框架

| 框架 | Adapter | 原生协议 | 状态 |
|:-----|:--------|:---------|:-----|
| ROLL | GEMAdapter | GEM (make/step/reset/close) | ✅ 就绪 |
| VeRL | VeRLAdapter | DataProto batch | 📋 待实现 |
| NeMo-RL | NeMoAdapter | OpenAI API / gRPC | 📋 待实现 |
| TRL | TRLAdapter | MCP tool call | 📋 待实现 |
| OpenRLHF | OpenRLHFAdapter | HTTP / gRPC | 📋 待实现 |

## 部署模式

| 模式 | 说明 | 适用场景 |
|:-----|:------|:---------|
| 嵌入式（推荐） | Adapter 作为库被训练框架导入 | 生产训练 |
| 内嵌式 | Adapter 与训练框架同进程 | 调试开发 |
| Sidecar | Adapter 作为独立进程 | 框架隔离 |

## 快速使用

```bash
# 安装
pip install ./uenv-bridge

# 使用 GEMAdapter（ROLL 框架）
from uenv.bridge.gem import GEMAdapter

adapter = GEMAdapter(server_endpoint="http://127.0.0.1:50051")
result = await adapter.execute_episode(env_type="math", payload={...})
```

## 基类接口

```python
class BaseAdapter:
    def convert_request(self, framework_request):
        raise NotImplementedError

    def convert_response(self, episode_result):
        raise NotImplementedError

    async def execute_episode(self, env_type, payload):
        ...
```

## 依赖

- **通信**: grpcio, protobuf
- **Python**: >= 3.10
