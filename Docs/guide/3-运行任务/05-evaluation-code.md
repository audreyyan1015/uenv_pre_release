# 代码生成

## 任务与数据性质

本案例要求模型只返回定义 `add(a, b)` 的 Python 代码，并在 UEnv Worker 隔离环境中执行断言。它使用 `code/dscodebench` 路由，输入是仓库自包含示例，仅用于演示字段与执行链路；benchmark 得分以官方评测为准。

| 项目 | 本案例取值 |
|---|---|
| 环境 / 路由 | `code` / `dscodebench` |
| 输入真源 | `examples/cases/evaluation/code-custom.jsonl` |
| 入口函数 | `add` |
| 测试 | `assert add(2, 3) == 5` |
| 执行超时 | 30 秒 |

主要判分证据位于 `env_config.test_code`、`entry_point` 与测试结果；样本中的 `target` 是兼容字段，不应被理解成期望生成的源代码。

## 执行主机

在安装了 `uenv` 且能访问 UEnv Server 的客户端主机执行。模型调用和不可信代码实际运行在 UEnv Worker；客户端不执行模型生成代码。

## 前置检查

先完成[通用评测流程](./03-evaluation.md)的 UEnv Server、UEnv Worker 和模型检查。代码 UEnv Worker 必须启用容器/进程隔离、执行超时和资源上限；若无法确认这些条件，不要在生产主机运行不可信代码。

```bash
export UENV_SERVER_ENDPOINT='127.0.0.1:50051'
export UENV_RELEASE_ROOT='/opt/uenv/current'
export INPUT="$UENV_RELEASE_ROOT/examples/cases/evaluation/code-custom.jsonl"
export RUN_ID="code-eval-$(date +%Y%m%d-%H%M%S)"
export OUTPUT="$PWD/results/$RUN_ID/results.jsonl"

test -r "$INPUT"
jq -e -c . "$INPUT" >/dev/null
mkdir -p "$(dirname "$OUTPUT")"
uenv workers
```

`uenv workers` 中必须有 `ready` 且支持 `code` 的 UEnv Worker。多机或源码运行时替换相应变量。

## 执行

```bash
uenv evaluate run-task \
  --endpoint "$UENV_SERVER_ENDPOINT" \
  --env-type code \
  --dataset dscodebench \
  --input "$INPUT" \
  --output "$OUTPUT" \
  --max-steps 1 \
  --batch-size 1 \
  --streaming
```

预期终端汇总含 `cases=1`、`completed=1`、`failed=0`。模型代码可能通过或不通过测试，因此 reward 不作为基础设施完成条件。

## 结果与验收

```bash
jq -c '{case_id,status,reward,action:.steps[-1].action,info:.steps[-1].info,error_message}' \
  "$OUTPUT"
```

验收唯一结果、环境 step 与数值 reward：

```bash
jq -e -s '
  length == 1 and
  .[0].case_id == "custom-code-1" and
  .[0].status == "completed" and
  (.[0].steps | length) == 1 and
  (.[0].reward | type) == "number"
' "$OUTPUT" >/dev/null && echo 'code evaluation completed'
```

模型成功时，action 定义 `add`，step 的 `info` / UEnv Worker 测试日志显示断言通过并给出满额 reward。生成代码错误但环境正常执行应是 completed 加业务低分；沙箱、模型或 UEnv Worker 故障应是基础设施失败。

## 替换为自己的任务

| 目标 | 修改位置 |
|---|---|
| 新函数 | `question`、`entry_point`、`ground_truth_code`、`test_code` |
| 更多测试 | `test_code`、测试文件配置和 `num_tests` |
| 执行预算 | `env_config.timeout_secs` 与 UEnv Worker 资源上限 |
| 批量任务 | 添加唯一 `id` 的行并调整 `--batch-size` |
| UEnv Server / 结果 | `UENV_SERVER_ENDPOINT` / `RUN_ID` |

使用真实 DSCodeBench 时，按数据许可取得任务并转换为 code 环境契约，记录数据版本、split 和测试依赖；不能只把本例的 `dataset` 字符串改名。

## 失败定位

| 现象 | 处理 |
|---|---|
| action 含 Markdown 围栏 | 当前环境会自动剥离常见围栏后执行；仍失败时在 prompt 中要求只返回代码，或按任务规则显式提取 |
| 找不到入口函数 | 核对 `entry_point` 与生成函数名 |
| Episode 超时 | 区分模型 API 超时、执行超时与死循环 |
| UEnv Worker 主机受到生成代码影响 | 隔离未正确部署；停止任务并修复容器、网络和文件权限 |
