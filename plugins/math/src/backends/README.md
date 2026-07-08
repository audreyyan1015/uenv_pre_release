# MathEnv backends

MathEnv（`env_type=math`）下的 benchmark 判分实现，由 `uenv-math-plugin` 在 step 阶段调用。

## 已实现 dataset

| 目录 | dataset 键 | 判分方式 |
|------|-----------|----------|
| `gsm8k/` | `gsm8k` | `####` 答案提取 + 归一化精确匹配 |
| `pubmedqa/` | `pubmedqa` | 从自由文本提取 yes / no / maybe |
| `scitab/` | `scitab` | 从自由文本提取 supports / refutes / not enough info |
| `olymmath/` | `olymmath`, `olymmath-easy`, `olymmath-hard` | `\boxed{}` 或 `####` 提取 + LaTeX 归一化 |

统一路由见 `src/score.rs`。

## 扩展新 benchmark

1. 在 `src/backends/<name>/scoring.rs` 实现 `answers_match(action, target) -> bool`
2. 在 `src/backends/mod.rs` 与 `src/score.rs` 注册
3. 更新 `manifest.yaml` 的 `datasets` 列表
4. 在 `uenv-worker/src/episode/payload.rs` 的 `normalize_dataset()` 增加别名映射
