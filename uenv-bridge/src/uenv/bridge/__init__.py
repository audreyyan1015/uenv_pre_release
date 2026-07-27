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
