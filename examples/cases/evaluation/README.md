# 评测示例输入

本目录保存案例页使用的输入 JSONL。QA 与 Code 文件是为了说明字段和执行链路而编写的自拟示例，不是 GSM8K 或 DSCodeBench 的原始样本；SWE 文件只保存 catalog 实例选择，任务正文和测试来自安装包 catalog。

| 文件 | 数据性质 | 案例文档 |
|---|---|---|
| `qa-gsm8k.jsonl` | 两条自拟数学问答，使用 `qa/gsm8k` 判分路由 | [数学问答评测](../../../Docs/guide/3-运行任务/04-evaluation-gsm8k.md) |
| `code-custom.jsonl` | 一个自包含 `add(a,b)` 函数测试 | [代码生成评测](../../../Docs/guide/3-运行任务/05-evaluation-code.md) |
| `swe-verified.jsonl` | 两个 Verified catalog 实例 ID | [代码修复评测](../../../Docs/guide/3-运行任务/06-evaluation-swe-verified.md) |

## 数据约定

`run-task` 输入每行必须显式声明唯一 `id`、`env_type`、`dataset` 和 `max_steps`，并用 `target` 或 `reward_config` 提供判分信息；任务特有字段放在 `env_config`。

`run-swe` 输入每行用 `instance_id` 选择 catalog 实例。问题正文、仓库、commit、测试和镜像信息只保存在 catalog，避免两份数据漂移。

安装包路径：

```text
/opt/uenv/current/examples/cases/evaluation/
```

完整前置、变量、命令、预期结果与验收位于 `Docs/guide/3-运行任务/`。
