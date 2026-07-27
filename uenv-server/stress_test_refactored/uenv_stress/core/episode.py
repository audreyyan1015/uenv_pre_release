"""Episode 构造的稳定公开接口。

这个文件只重新导出构造 UEnv Episode envelope 所需的核心函数，让上层代码不必直接依赖 stress_test_common 中较多的历史工具函数。阅读者可以把它看成 Episode payload 的入口文件。

实现逻辑是：从 stress_test_common 引入 json_bytes 和 make_sample_envelope，并通过 __all__ 固定对外暴露的名称；后续如果底层字段或实现位置调整，上层只需要继续依赖这个稳定接口。"""

from .stress_test_common import json_bytes, make_sample_envelope

__all__ = ["json_bytes", "make_sample_envelope"]
