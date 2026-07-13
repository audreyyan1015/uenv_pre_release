# MathEnv fixtures (`env_type=math`)

L1 调度键为 **`math`**；各 benchmark 通过 `payload.dataset` 区分。

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
| `episode_001.pb` | 二进制 fixture（由生成器产出） |
| `expected_result_001.pb` | 期望 EpisodeResult |
| `samples/pubmedqa_smoke.json` | PubMedQA smoke payload |
| `samples/scitab_smoke.json` | SciTab smoke payload |
| `samples/olymmath_easy_smoke.json` | OlymMATH-Easy smoke payload |

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
  "question": "Table: ...\nClaim: The treatment improved outcomes.",
  "dataset": "scitab"
}
```

```json
{"type": "rule_reward", "target": "supports"}
```

### OlymMATH

```json
{
  "question": "Let a,b be positive integers ...",
  "dataset": "olymmath-easy"
}
```

```json
{"type": "rule_reward", "target": "16"}
```

## 再生

```bash
cargo run -p uenv-mock-scheduler --example gen_math_fixture
```
