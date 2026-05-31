# MathEnv fixtures (`env_type=math`)

L1 调度键为 **`math`**；GSM8K benchmark 通过 `payload.dataset=gsm8k` 区分。

## 文件

| 文件 | 说明 |
|------|------|
| `episode_001.textproto` | 可读 EpisodeRequest 样例 |
| `episode_001.pb` | 二进制 fixture（由生成器产出） |
| `expected_result_001.pb` | 期望 EpisodeResult |

## 再生

```bash
cargo run -p uenv-mock-scheduler --example gen_math_fixture
```
