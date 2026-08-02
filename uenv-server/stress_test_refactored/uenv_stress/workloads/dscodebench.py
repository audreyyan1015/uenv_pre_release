"""DSCodeBench workload 适配接口。

这个文件为 DSCodeBench 代码任务提供数据读取、prompt 构造、内联测试代码、环境 payload 和 reward 配置的公开入口。它本身只做导出，具体实现集中在 stress_test_common 中，保证规模压测和其他入口使用同一套字段。

实现逻辑是：从 stress_test_common 引入 load_dscodebench_jsonl、dscodebench_prompt、dscodebench_inline_test_code、dscodebench_env_payload 和 dscodebench_reward_config，并通过 __all__ 固定对外名称；上层只依赖 workloads.dscodebench，不直接依赖底层公共文件。"""

from uenv_stress.core.stress_test_common import (
    dscodebench_env_payload,
    dscodebench_inline_test_code,
    dscodebench_prompt,
    dscodebench_reward_config,
    load_dscodebench_jsonl,
)

__all__ = [
    "dscodebench_env_payload",
    "dscodebench_inline_test_code",
    "dscodebench_prompt",
    "dscodebench_reward_config",
    "load_dscodebench_jsonl",
]
