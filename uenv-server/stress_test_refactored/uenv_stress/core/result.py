"""压测结果格式的稳定公开接口。

这个文件统一暴露 EpisodeObservation、结果归一化和汇总文档构造函数，避免不同场景写出互不兼容的 JSON/CSV 字段。阅读结果文件时，可以用这里的接口理解哪些字段是所有压测共享的事实记录。

实现逻辑是：从 stress_test_common 引入观测字段、schema 版本、Episode 完成状态归一化、批量观测、百分位计算以及 DSCodeBench/SWE-bench Pro/通用压测结果文档构造函数，并通过 __all__ 固定对外 API。"""

from .stress_test_common import (
    EPISODE_OBSERVATION_FIELDS,
    EPISODE_OBSERVATION_SCHEMA_VERSION,
    dscodebench_pressure_result_document,
    episode_observation_from_envelope,
    finalize_episode_observation,
    new_episode_observation,
    observe_episode_batch,
    percentile,
    sample_result_dict,
    stress_result_document,
    swebench_pro_pressure_result_document,
    validate_episode_observation,
    write_episode_observations_jsonl,
)

__all__ = [
    "EPISODE_OBSERVATION_FIELDS",
    "EPISODE_OBSERVATION_SCHEMA_VERSION",
    "dscodebench_pressure_result_document",
    "episode_observation_from_envelope",
    "finalize_episode_observation",
    "new_episode_observation",
    "observe_episode_batch",
    "percentile",
    "sample_result_dict",
    "stress_result_document",
    "swebench_pro_pressure_result_document",
    "validate_episode_observation",
    "write_episode_observations_jsonl",
]
