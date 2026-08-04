# SWE-smith 全量 catalog 修复后仍命中旧 fixture 诊断说明

> 日期：2026-08-04
> 面向对象：Worker / Server / Adapter
> 关联 run：`verl_swesmith_grpo_train_20260804_134850`
> 关联 Worker 报告：[`../worker/260802/SWE-smith全量catalog补齐与Worker重启报告.md`](../worker/260802/SWE-smith全量catalog补齐与Worker重启报告.md)

## 1. 背景

Worker 侧报告显示，7143 已将 SWE-smith EnvPackage 从 smoke catalog 切换为全量 catalog：

| 项 | 数值 |
|---|---:|
| SWE-smith catalog | 59136 |
| SWE-bench-Pro catalog | 731 |
| Gateway 合并 catalog | 59867 |
| SWE-smith unique images | 222 |
| Docker 镜像缺口 | 0 |

因此 Adapter 侧重新启动 SWE-smith 训练，期望正式训练数据不再大量触发 `not in catalog`。

本轮训练日志：

```text
/data/ronghao/uenv/uenv-bridge/temp/logs/verl_layer4_agent_loop/verl_swesmith_grpo_train_20260804_134850.log
/data/ronghao/uenv/uenv-bridge/temp/logs/layer4_distributed/verl_swesmith_grpo_train_20260804_134850/
```

## 2. 当前现象

检查时该训练进程仍在运行，但第一轮 rollout 已经返回 8 条结果，全部失败：

```text
agent-loop-requests.jsonl: 16 条
agent-loop-results.jsonl: 8 条
model-gateway.jsonl: 未生成
```

本轮训练参数中：

```text
TRAIN_BATCH_SIZE=2
ROLLOUT_N=4
```

因此每个 GRPO step 会产生 `2 * 4 = 8` 条 episode。当前 16 条 request 表明前两个 step 的 rollout 已经发出；8 条 result 对应第一个 step。

第一轮 8 条 result 全部是 catalog 查找失败，没有进入模型调用阶段，因此没有产生 `model-gateway.jsonl`。

## 3. 失败证据

`agent-loop-results.jsonl` 中的典型错误如下：

```text
instance 'pytest-dev__iniconfig.16793ead.combine_module__lxshiekf'
not in /root/UEnv/fixtures/swe/smith_catalog.json

instance 'Cog-Creators__Red-DiscordBot.33e0eac7.combine_module__bjf0rr5u'
not in /root/UEnv/fixtures/swe/smith_catalog.json
```

但 Adapter 发送给 Worker 的字段是正确的：

```text
env_package_id=swe-bench-smith
benchmark_variant=smith
driver_entrypoint=run_swesmith_official.py
workspace_dir=/testbed
llm_config_path=/root/UEnv/config/openhands-llm-qwen3-thinking-max-token-8192.json
```

也就是说，训练侧 episode 已经明确声明使用 SWE-smith EnvPackage，并没有误发为 SWE-bench-Pro。

## 4. 初步判断

本轮失败发生在 Worker/OpenHands driver 的早期 catalog 查找阶段，而不是模型推理、gateway、vLLM 或 VeRL update 阶段。

当前实际执行路径仍然读取：

```text
/root/UEnv/fixtures/swe/smith_catalog.json
```

没有读取 Worker 报告中补齐后的全量 catalog：

```text
/var/lib/uenv/envs/swe-bench-smith/0.1.0-local/catalog.json
```

因此问题可以概括为：

```text
Worker runtime catalog 可能已经加载全量 EnvPackage，
但 OpenHands driver 子进程仍然命中旧 smoke fixture。
```

从当前代码看，OpenHands driver 中的 catalog 候选路径包含旧 fixture。如果没有显式传入 `instances_catalog`、`UENV_SWE_INSTANCES` 或 `UENV_SWE_ENV_PACKAGE`，且旧 fixture 文件存在，driver 仍可能优先选择：

```text
/root/UEnv/fixtures/swe/smith_catalog.json
```

这会导致全量 SWE-smith 样本在 driver 侧继续 `not in catalog`。

## 5. 需要 Worker 侧核验的内容

建议 Worker 侧在 7143 上核验 OpenHands 执行进程实际环境：

```bash
echo "$UENV_SWE_ENV_PACKAGE"
echo "$UENV_SWE_INSTANCES"
ls -lh /var/lib/uenv/envs/swe-bench-smith/0.1.0-local/catalog.json
ls -lh /root/UEnv/fixtures/swe/smith_catalog.json
```

同时确认 OpenHands driver 启动时是否显式传入了全量 catalog 路径。

短期可通过以下任一方式强制命中全量 catalog：

```bash
export UENV_SWE_ENV_PACKAGE=/var/lib/uenv/envs/swe-bench-smith/0.1.0-local
```

或：

```bash
export UENV_SWE_INSTANCES=/var/lib/uenv/envs/swe-bench-smith/0.1.0-local/catalog.json
```

更直接的方式是 Server/Worker 在生成 AgentJob 时填充：

```text
instances_catalog=/var/lib/uenv/envs/swe-bench-smith/0.1.0-local/catalog.json
```

这样 OpenHands driver 不再依赖默认 fixture 候选顺序。

## 6. 后续风险

即使显式指向全量 catalog，当前 OpenHands driver 的 `_load_catalog` 逻辑也是整文件读取：

```text
json.loads(path.read_text())
```

而全量 SWE-smith catalog 约 4.84 GiB。如果每条 episode 都读取一次全量 JSON，会带来明显的 I/O、内存和启动耗时压力。

长期更合理的方案是：

1. Worker / Server 为每个 AgentJob 生成单样本小 catalog，并通过 `instances_catalog` 传给 OpenHands driver。
2. 或者 OpenHands driver 直接从 AgentJob / task payload 读取当前 instance 所需字段，跳过全量 catalog 查找。
3. 或者将全量 catalog 改造成可按 `instance_id` 随机访问的索引格式，避免每条 episode 整文件加载。

## 7. Adapter 侧结论

Adapter 当前发送字段没有发现明显错误。本轮失败不应归因于训练参数、模型推理或 gateway。

在 Worker/OpenHands driver 侧确认 catalog 路径前，不建议继续使用全量 SWE-smith 数据正式训练；否则失败 episode 会被 Adapter 的 `zero_reward` 容错策略转换为 0 奖励，训练不会立刻崩溃，但得到的是 catalog 基础设施失败样本，而不是模型真实修复能力反馈。
