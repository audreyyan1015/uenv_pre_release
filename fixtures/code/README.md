# CodeEnv fixtures (`env_type=code`)

L1 调度键为 **`code`**；DSCodeBench benchmark 通过 `payload.dataset=dscodebench` 区分。

## 文件

| 文件 | 说明 |
|------|------|
| `samples/ds_smoke_001.json` | 最小 smoke 样本（inline `test_code`） |
| `samples/ds_001.json` | 官方风格 harness 样本（`test_script_path` + `ground_truth_path`） |
| `benchmark/stdlib/ds_001_*.py` | golden 用最小 stdlib 题（ground truth + test generator） |
| `episode_001.textproto` | 可读 EpisodeRequest 样例 |

## Smoke 样本

`ds_smoke_001.json` 使用 inline `test_code`，无需安装完整 DSCodeBench 数据即可联调。

`ds_001.json` + `benchmark/stdlib/` 覆盖官方 harness 路径（固定 `random_seed=42`），供 `dscodebench_golden` 与本地评测使用。全量官方题库仍经 Hub `dscodebench@0.2.0` sync，不要把完整 `benchmark/` 树提交进仓库。
