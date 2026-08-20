# 数学问答

## 任务与数据性质

本案例验证模型能否按指定格式回答数学问答，并由 `qa` 环境计算规则 reward。输入文件含两道仓库自拟题，使用 `gsm8k` 判分路由，仅用于演示字段与执行链路；benchmark 得分以官方评测为准。

| 项目 | 本案例取值 |
|---|---|
| 环境 / 路由 | `qa` / `gsm8k` |
| 输入真源 | `examples/cases/evaluation/qa-gsm8k.jsonl` |
| 样本数 | 2 |
| 目标答案 | 8、8 |
| 最大步数 | 1 |

## 执行主机

在任意安装了 `uenv` 且能访问 UEnv Server 50051/TCP 的客户端主机执行。模型 API 由可能接单的 UEnv Worker 访问。

## 前置检查

先完成[通用评测流程](./03-evaluation.md)中的 UEnv Server、UEnv Worker 和模型检查。然后设置本轮变量：

```bash
export UENV_SERVER_ENDPOINT='127.0.0.1:50051'
export UENV_RELEASE_ROOT='/opt/uenv/current'
export INPUT="$UENV_RELEASE_ROOT/examples/cases/evaluation/qa-gsm8k.jsonl"
export RUN_ID="math-eval-$(date +%Y%m%d-%H%M%S)"
export OUTPUT="$PWD/results/$RUN_ID/results.jsonl"

test -r "$INPUT"
jq -e -c . "$INPUT" >/dev/null
mkdir -p "$(dirname "$OUTPUT")"
```

多机部署必须替换 `UENV_SERVER_ENDPOINT`；源码运行则把 `UENV_RELEASE_ROOT` 设为仓库根目录绝对路径。

## 执行

```bash
uenv evaluate run-task \
  --endpoint "$UENV_SERVER_ENDPOINT" \
  --env-type qa \
  --dataset gsm8k \
  --input "$INPUT" \
  --output "$OUTPUT" \
  --max-steps 1 \
  --batch-size 2 \
  --streaming
```

预期终端汇总含 `cases=2`、`completed=2`、`failed=0`。reward 取决于模型回答，允许为 0 到 1 之间的任意值。

## 结果与验收

查看每条结果：

```bash
jq -c '{case_id,status,reward,answer:(.steps[-1].action // ""),error_message}' "$OUTPUT"
```

机器验收基础设施完成和 ID 对齐：

```bash
jq -e -s '
  length == 2 and
  (map(.case_id) | sort) == ["gsm8k-1","gsm8k-2"] and
  all(.[]; .status == "completed" and (.steps | length) >= 1 and (.reward | type) == "number")
' "$OUTPUT" >/dev/null && echo 'math evaluation completed'
```

满分回答的最终答案应符合输入要求的 `#### 8` 格式。格式或答案错误是任务低分；模型不可达、无 UEnv Worker 或 Episode 失败是基础设施问题。

## 替换为自己的数据

| 要替换的内容 | 修改位置 |
|---|---|
| UEnv Server | `UENV_SERVER_ENDPOINT` |
| 自有数学问答 | 新 JSONL 的 `id`、`question`、`target` |
| 判分路由 | 命令与每行 `dataset` 同时修改 |
| 结果目录 | `RUN_ID` / `OUTPUT` |
| 并发 | `--batch-size`，不超过 UEnv Worker 与模型容量 |

使用真实 GSM8K 时，应从有许可的数据源获取并转换为同一 JSONL 契约，记录数据版本和 split；不要把本例两道题称为 GSM8K 样本。

## 失败定位

| 现象 | 处理 |
|---|---|
| UEnv Worker 报默认模型未配置 | 在每个可能接单的 UEnv Worker 上运行 `configure-model` |
| 环境或路由不匹配 | 核对命令和 JSONL 中的 `qa/gsm8k` |
| completed 但 reward 为 0 | 检查最终答案格式和 `target`，不要重装服务 |
| 输出顺序变化 | 按 `case_id` 对齐；streaming 不保证完成顺序 |
