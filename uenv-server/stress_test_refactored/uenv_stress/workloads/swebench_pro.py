"""SWE-bench Pro/OpenHands workload 适配接口。

这个文件为 SWE-bench Pro 任务提供 UEnv/OpenHands 环境 payload 和 reward 配置的公开入口。它把上层压测脚本与底层字段构造细节隔离开，便于后续调整 OpenHands 参数或 reward 配置。

实现逻辑是：从 stress_test_common 引入 swe_openhands_env_payload 和 swe_reward_config，并通过 __all__ 固定对外名称；上层传入 instance_id、仓库、patch、镜像和运行参数后，由底层函数生成 Episode 所需的 env/reward 结构。"""

from uenv_stress.core.stress_test_common import (
    swe_openhands_env_payload,
    swe_reward_config,
)

__all__ = ["swe_openhands_env_payload", "swe_reward_config"]
