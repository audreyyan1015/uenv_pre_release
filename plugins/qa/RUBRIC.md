# QaEnv Rubric 契约（插件侧）

> 与 Hub 制品元数据应对齐；Hub 侧流程见
> [`Docs/worker/260722/Hub待调整事宜-qa制品与Rubric注册.md`](../../Docs/worker/260722/Hub待调整事宜-qa制品与Rubric注册.md)。

## 生产判分

| 项 | 值 |
|----|-----|
| `env_type` | `qa` |
| 运行时插件 | `uenv-math-plugin`（`plugins/qa/run.sh` 复用） |
| 入口 | `score_action`（`plugins/math`） |
| 金标参照 | `verifiers` + `math_verify` |
| 对齐语料 | `data/alignment/qa_rubric_corpus.jsonl` |
| 对齐脚本 | `uenv-bridge/scripts/verify_qa_rubric_alignment.py` |

换机 / 发版时：**二进制 digest + 语料 digest + 对齐报告** 三者应一并记录，禁止只拷二进制。

建议在 Hub `qa` version metadata 中写入（示例）：

```yaml
rubric:
  schema_version: "1"
  production_scorer: "uenv-math-plugin/score_action"
  alignment:
    corpus_id: "qa_rubric_corpus@2026-07-25"
    agreement: 0.9655
    too_lenient: 0
    too_strict: 2
```

## 金标过严 2 条 — 产品决策（2026-07-25）

对齐率 96.55%（56/58），**过宽 = 0**（门槛已满足）。剩余 2 条均为 **过严**（UEnv 判错、参照判对）：

| 类型 | 决策 | 理由 |
|------|------|------|
| 自然语言答案、无 `####` / `\boxed{}`（gsm8k 类） | **保持过严** | 与 GSM8K 官方抽取约定一致；训练应引导模型输出约定格式，不靠放宽判分抬高分数 |
| 长左侧赋值（如 `abcd=5`）被拒 | **保持过严（有意）** | 防止把解题过程左侧噪声当答案；短赋值前缀（如 `x=`）已允许剥离 |

**禁止**在未更新语料与对齐报告的情况下 silent 放宽判分。若需放宽，走：改 scoring → 跑对齐脚本 → 新 Hub version。

## `math` 制品保留策略

| 项 | 策略 |
|----|------|
| 插件二进制 / 旧镜像 | 可保留作回滚 |
| Worker `env.types` | **不得**再含 `math` |
| 客户端 | 一律 `env_type=qa`；误发 `math` → Server `no worker supports env type`（快速失败，属预期） |
| Hub | `math` 标 deprecated 兼容别名（见 Hub 专文） |
