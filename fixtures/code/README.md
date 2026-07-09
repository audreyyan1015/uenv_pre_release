# CodeEnv fixtures (`env_type=code`)

L1 调度键为 **`code`**；DSCodeBench benchmark 通过 `payload.dataset=dscodebench` 区分。

## 文件

| 文件 | 说明 |
|------|------|
| `samples/ds_smoke_001.json` | 最小 smoke 样本（inline `test_code`） |
| `episode_001.textproto` | 可读 EpisodeRequest 样例 |

## Smoke 样本

`ds_smoke_001.json` 使用 inline `test_code`，无需安装完整 DSCodeBench 数据即可联调。

实机部署 DSCodeBench 后，将 `test_script_path` 指向官方 test case script，并设置 `UENV_DSCODEBENCH_ROOT`。
