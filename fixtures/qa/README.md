# QaEnv fixtures (`env_type=qa`)

L1 调度键为 **`qa`**（单轮问答/分类验证环境，原 `math` 更名而来）；各 benchmark 通过 `payload.dataset` 区分，判分按 `dataset` 路由，与 `env_type` 无关。

`fixtures/math/` 保留为兼容期镜像，字段除 `env_type` 外与本目录一致。

## 支持的 dataset

| dataset | 说明 | target 示例 |
|---------|------|-------------|
| `gsm8k` | 小学数学应用题 | `"20"` |
| `pubmedqa` | PubMed 摘要阅读理解 | `"yes"` / `"no"` / `"maybe"` |
| `scitab` | 科学表格 claim 验证 | `"supports"` / `"refutes"` / `"not enough info"` |
| `olymmath-easy` | OlymMATH 奥赛数学（Easy） | `"42"`、`\sqrt{33}` 等 |
| `olymmath-hard` | OlymMATH 奥赛数学（Hard） | 同上 |

## 文件

| 文件 | 说明 |
|------|------|
| `episode_001.textproto` | GSM8K 可读 EpisodeRequest 样例 |
| `samples/pubmedqa_smoke.json` | PubMedQA smoke payload |
| `samples/scitab_smoke.json` | SciTab smoke payload |
| `samples/olymmath_easy_smoke.json` | OlymMATH-Easy smoke payload |

二进制 fixture（`episode_001.pb` / `expected_result_001.pb`）由生成器产出，需要时执行 `scripts/gen-math-fixture.sh` 后按 `env_type` 复制；本目录只维护可读版本，避免手工改二进制导致 protobuf 长度前缀错位。

## Payload 示例

### PubMedQA

```json
{
  "question": "Context: ... abstract ...\nQuestion: Does X cause Y?",
  "dataset": "pubmedqa"
}
```

```json
{"type": "rule_reward", "target": "yes"}
```

### SciTab

```json
{
  "question": "Table: ...\nClaim: Group A outperformed Group B.",
  "dataset": "scitab"
}
```

```json
{"type": "rule_reward", "target": "supports"}
```

## 免-LLM smoke

`uenv-bridge/scripts/smoke_qa_datasets_grpcurl.py` 直接对 Adapter Core 发 `envType=qa` 的四数据集请求；payload 不带 `question` 时走 Worker `model_client` 的 `rule_reward` 短路，用于验证链路而非判分逻辑。
