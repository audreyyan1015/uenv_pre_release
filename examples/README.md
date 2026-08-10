# UEnv 任务样本

`examples/` 保存可复制、可修改的任务样本，并说明 JSONL 字段的填写方法。用户通过 `uenv evaluate`、`uenv train` 和 `uenv env plugin` 命令执行评测、训练和 process plugin（进程插件）管理。`libexec/uenv/` 保存这些命令调用的内部脚本。

- `cases/evaluation/`：评测任务样本 JSONL 和字段说明。
- `cases/training/`：VeRL 训练任务样本 JSONL、VeRL 配置示例和字段说明。

增加其他数据集或自定义环境时，复制交互和判分方式最接近的 JSONL，替换任务样本字段，并在命令行填写环境类型（`--env-type`）、数据集 ID（`--dataset`）、任务样本文件（`--input`）与其他运行参数。详细方法见 [UEnv 评测指南第 6 节](../Docs/deployment/UEnv评测指南.md#6-接入新任务)。
