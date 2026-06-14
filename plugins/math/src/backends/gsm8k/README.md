# GSM8K backend

MathEnv 下 `payload.dataset=gsm8k` 的判分实现，对齐 VeRL `prepare_verl_gsm8k_sample.py` 中的 `extract_solution`。

**边界**：本目录属 **L2 环境制品**；Worker `episode/RewardEngine` 不包含 GSM8K 领域逻辑，只采信插件 `step.reward`。
