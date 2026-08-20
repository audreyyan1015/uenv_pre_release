# Checkpoint 保存说明

本目录用于保存 VeRL 训练产生的模型 checkpoint。实际模型文件体积较大，不纳入 Git；本目录只保留说明文档和 `.gitignore`。

当前保存方式按训练 run 隔离：

```text
checkpoints/
  uenv_grpo/
    <RUN_ID>/
      metadata.json
      global_step_50/
      global_step_100/

  gsm8k_grpo/
    <RUN_ID>/
      metadata.json
      global_step_50/
      global_step_100/
```

默认规则：

- UEnv / SWE-smith 训练保存到 `checkpoints/uenv_grpo/<RUN_ID>/`。
- GSM8K native VeRL 训练保存到 `checkpoints/gsm8k_grpo/<RUN_ID>/`。
- 每次训练启动前会写入 `metadata.json`，记录 `run_id`、模型路径、数据目录、batch、rollout 数、保存频率等关键信息。
- VeRL 自身仍按 `global_step_*` 保存 checkpoint，但不同 run 会落在不同目录下，不会互相覆盖。

如需改保存位置，可以在启动训练时设置：

```bash
CHECKPOINT_ROOT=/path/to/checkpoints ./scripts/train/presets/swe_smith_grpo_train.sh
```
