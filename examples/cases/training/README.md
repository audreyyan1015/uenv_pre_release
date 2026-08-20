# 强化学习训练示例输入

本目录保存 `uenv train` 使用的示例 JSONL 与当前发布实现的 Hydra 覆盖文件。QA 与 Code 是自拟输入，不是 benchmark 原始样本；process plugin 文件是接口模板，未安装对应环境时不能执行。

| 文件 | 数据性质 | 案例文档 |
|---|---|---|
| `qa-gsm8k.jsonl` | 两条自拟数学问答，使用 `qa/gsm8k` 路由 | [数学问答训练](../../../Docs/guide/cases/training-gsm8k-verl.md) |
| `code-dscodebench.jsonl` | 一个自包含 `add(a,b)` 测试 | [代码生成训练](../../../Docs/guide/cases/training-code-verl.md) |
| `process-plugin.jsonl` | `my-environment/my-dataset` 字段模板 | [自定义环境训练](../../../Docs/guide/cases/training-process-plugin.md) |
| `verl-grpo-overrides.conf` | 当前发布 runner 的 Hydra 覆盖示例 | 所有训练案例均可引用 |

软件工程训练从 `config/swe/smith-sample-catalog.json` 选择实例，见[软件工程修复训练](../../../Docs/guide/cases/training-swe-smith-verl.md)。

## 数据约定

普通训练 JSONL 每行必须显式声明：

- 唯一 `id`；
- `env_type`、`dataset` 和当前公共入口要求的 `max_steps=1`；
- prompt 与足够的 `target` / `reward_config`；
- 环境需要的 `env_config`。

Bridge 不从文件名或默认值猜测任务。`process-plugin.jsonl` 只有部署了实现 `expected_action` 和 plugin reward 契约的环境后才可执行；只替换命令中的名称不会创建环境。

安装包路径：

```text
/opt/uenv/current/examples/cases/training/
```
