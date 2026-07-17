# OlymMATH failed requests for Server/Worker 排查

## Run 信息

| 项 | 值 |
|---|---|
| 结果目录 | `temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_long_full_noresume_20260716_150507/` |
| batch_id | `olymmath-uenv-20260716_000508` |
| AdapterCore endpoint | `8.130.75.157:8088` |
| Model endpoint | `http://10.10.20.142:18094/v1` |
| 样本数 | 400 |
| failed | 149 |
| 共同错误码 | `5001` |
| 本地错误信息 | `episode ... exceeded max attempts (3)` |
| 远端日志观察到的失败原因 | `dispatch_failed / execute_episode_failed / h2 protocol error / CANCEL` |
| Gateway 对账 | 当前运行窗口内 698 次 `/v1/chat/completions` 均为 HTTP 200，gateway error 为 0 |

## 分布

| 维度 | 分布 |
|---|---|
| language | {'EN': 2, 'ZH': 147} |
| source_file | {'OlymMATH-EN-EASY.jsonl': 1, 'OlymMATH-EN-HARD.jsonl': 1, 'OlymMATH-ZH-EASY.jsonl': 81, 'OlymMATH-ZH-HARD.jsonl': 66} |
| difficulty | {'EASY': 82, 'HARD': 67} |
| subject | {'Combinatorics': 1, 'Algebra': 1, '组合': 41, '代数': 33, '几何': 45, '数论': 28} |

## 完整 TSV

完整清单见：`/data/ronghao/uenv/uenv-bridge/temp/benchmarks/olymmath/qwen3_6_35b_a3b_uenv_long_full_noresume_20260716_150507/failed_requests_for_server_worker.tsv`