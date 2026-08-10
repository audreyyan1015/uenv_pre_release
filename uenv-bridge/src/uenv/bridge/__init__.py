import os as _os

# gen/ 下的 pb2 由旧版 protoc 生成，与 protobuf>=5 默认的 upb 运行时不兼容
# （import 时报 Descriptors cannot be created directly）。在导入任何子模块之前
# 默认退回纯 Python 解析；已显式设置该变量的环境（如 VeRL 训练容器）不受影响。
_os.environ.setdefault("PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION", "python")

from .clients import (

    EpisodeClient,
    RustCoreClientConfig,
    RustCoreEpisodeClient,
)
from .protocol import EpisodeRequest, EpisodeResult

# 训练侧 AgentLoop 只在带 verl / 完整 bridge 依赖的部署里可用。评测机与 Agent 机
# 只需 gRPC 客户端，因此这里不硬依赖，缺失时留 None，导入即失败会阻断轻量部署。
try:
    from .verl_agent_loop import UEnvAgentLoop, UEnvAgentLoopConfig
except ImportError:  # pragma: no cover - 轻量部署（无 verl / 无 agent-loop 模块）
    UEnvAgentLoop = None  # type: ignore[assignment]
    UEnvAgentLoopConfig = None  # type: ignore[assignment]

__all__ = [
    "EpisodeClient",
    "EpisodeRequest",
    "EpisodeResult",
    "RustCoreEpisodeClient",
    "RustCoreClientConfig",
    "UEnvAgentLoop",
    "UEnvAgentLoopConfig",
]
