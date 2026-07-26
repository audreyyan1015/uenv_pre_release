# DSCodeBench Agent 轨道评测（Verifiers 风格 ToolEnv）

> 日期：2026-07-25
> 轨道：`agentic`　指标：`agentic_pass@1`
> 官方单轮轨道：[DSCodeBench 代码生成基线评测](./DSCodeBench代码生成基线评测.md)（指标 `pass@1`）
> 选型与规划：[验证型环境改造与DSCode-Agent评测-实施规划](../../../Docs/worker/260722/验证型环境改造与DSCode-Agent评测-实施规划.md)

---

## 1. 为什么要分轨

DSCodeBench 官方口径是**一次生成即判分**：模型收到题目描述，输出一份 Python 代码，官方 harness 生成测试用例并比对输出，得到 `pass@1`。这是与外部结果可比的基线，不能改动。

Agent 轨道回答的是另一个问题：**给模型工具和多轮修错机会后，同一套题能做到什么程度**。Agent 可以先在沙箱里跑自己的代码、看报错、改一版，最后再定稿提交。这更接近训练时的 rollout 形态，也更能反映环境作为 RL 环境的可用性，但它天然会高于单轮口径。

因此两条轨道**指标不可直接比较**，只能并列展示：

| | 官方单轮轨道 | Agent 轨道（ToolEnv） |
|---|---|---|
| 指标 | `pass@1` | `agentic_pass@1` |
| 生成方式 | Worker code env 向 Model Gateway 取一次 completion | Agent 多轮：`run_python` 自测/自修 → `submit_code` 定稿 |
| 轮次预算 | 1 | `--max-turns`（默认 4） |
| 判分入口 | 官方 harness（`inline_harness`） | **同一** 官方 harness、同一 `--num-tests`、同一 seed 规则 |
| 入口脚本 | `run_dscodebench_uenv_baseline.sh` | `run_dscodebench_agent_toolenv.sh` |
| 输出根目录 | `temp/benchmarks/dscodebench/` | `temp/benchmarks/dscodebench-agentic/` |
| 对外汇报 | 任务书基线数字 | 环境能力/Agent 能力补充证据，需标注轨道 |

判分侧完全一致是这套设计的关键：Agent 轨只改变“代码怎么产生”，不改变“代码怎么被判对错”。

## 2. 工具与链路

Agent 提供两个工具，语义对齐 PrimeIntellect `verifiers` 的 `ToolEnv`：

- `run_python(code, stdin, timeout)`：在 Agent 机的临时目录用受限子进程执行代码，返回 `stdout/stderr/exit_code/timed_out`。解释器由 `--python-bin` 指定，需含 numpy/pandas 等依赖（建议指向 DSCodeBench venv）。
- `submit_code(code)`：定稿，结束 episode，交给官方 harness 判分。

判分回传通道不需要改控制面：Worker 判分时会向 `EpisodeRequest.model_endpoint` 拉候选代码，Agent 侧因此起一个 OpenAI 兼容 shim，把定稿代码作为 completion 返回，并把 `model_endpoint` 指向该 shim。

```
Agent(208.77)
  ├─ run_python  ──► 本地沙箱（迭代自测，不计分）
  ├─ submit_code ──► shim(:18899 /v1)  ◄── Worker 拉候选代码
  └─ EpisodeRequest ──► Adapter Core(8.130.75.157:8088) ──► Worker(7143) code env
                                                              └─ 官方 inline_harness ──► reward / tests_passed
```

## 3. 用法

```bash
# Agent 机（208.77）或 7143 本机
export UENV_ADAPTER_CORE_ENDPOINT=8.130.75.157:8088
LLM_ENDPOINT=http://10.10.20.142:18099/v1 LLM_MODEL=Qwen3-14B \
LIMIT=5 MAX_TURNS=4 NUM_TESTS=50 \
PYTHON_BIN=/var/lib/uenv/envs/dscodebench/0.2.0/venv/bin/python \
  uenv-bridge/scripts/benchmark/run_dscodebench_agent_toolenv.sh
```

关键环境变量：

| 变量 | 默认 | 说明 |
|---|---|---|
| `POLICY` | `llm` | `llm` 走真实模型；`mock` 用 ground truth 做免-LLM 链路验证 |
| `LLM_ENDPOINT` / `LLM_MODEL` | — | OpenAI 兼容端点与模型名，`POLICY=llm` 必填 |
| `OUTPUT_ROOT` | `temp/benchmarks/dscodebench-agentic` | Agent 轨固定输出根目录 |
| `RUN_NAME` | `toolenv_<model>_<ts>` | 子目录名与报告标识 |
| `LIMIT` / `LIBRARY` / `MAX_PER_LIBRARY` | 全量 | 题目筛选 |
| `MAX_TURNS` | 4 | ToolEnv 轮次上限 |
| `NUM_TESTS` | 200 | 官方 harness 用例数，需与对照的官方轨一致 |
| `PYTHON_BIN` | DSCodeBench venv | `run_python` 沙箱解释器 |
| `SHIM_HOST` / `SHIM_PORT` / `SHIM_PUBLIC_URL` | `127.0.0.1:18899` | 判分回传 shim；跨机需 `0.0.0.0` + 公网 URL |
| `RESUME` | `0` | `1` 跳过 `results.jsonl` 中已完成题目 |

## 4. 产物布局

固定目录，便于归档与二次分析：

```text
temp/benchmarks/dscodebench-agentic/<RUN_NAME>/
  run_config.json            # 运行参数快照（不含 api key）
  results.jsonl              # 每题一行，边跑边落盘，支持 --resume
  metrics.json               # 聚合指标；track=agentic，comparable_with_official_pass_at_1=false
  report.md                  # 报告（总体 / 分库 / 未通过明细 / 可选官方轨并列）
  codes/<problem_id>.py      # Agent 定稿代码
  traces/<problem_id>.history.json  # ToolEnv 多轮轨迹（工具调用与观测）
  run.log
```

`metrics.json` 自带轨道标识字段，防止下游误当成官方 `pass@1` 使用：

```json
{ "track": "agentic", "metric": "agentic_pass@1", "comparable_with_official_pass_at_1": false }
```

报告可单独重生成，也可并列官方轨数字（仅展示，不做差值结论）：

```bash
python3 uenv-bridge/scripts/benchmark/report_dscode_agentic.py \
  --output-dir temp/benchmarks/dscodebench-agentic/<RUN_NAME> \
  --baseline-metrics temp/benchmarks/dscodebench/<official_run>/metrics.json
```

## 5. 已验证结果（小样本）

| 场景 | 配置 | 结果 |
|---|---|---|
| 免-LLM 链路 | `POLICY=mock`，`numpy_0` | 2 轮 → 官方 harness `20/20`，`reward=1.0` |
| 真实模型 · 难例 | Qwen3-14B，`numpy_0/1/2`，`NUM_TESTS=50` | 全部 `completed`；`numpy_2` 28/50；`agentic_pass@1=0` |
| 真实模型 · 短题 | Qwen3-14B，`numpy_3/10/11/17/25` | 4/5 通过（50/50），`agentic_pass@1=0.8` |

小样本用于验证链路与口径，不作为对外能力数字；正式数字需固定模型、`NUM_TESTS=200`、全量或分层抽样后重跑。

## 6. 汇报注意事项

1. 任何 Agent 轨数字必须带轨道标注（`agentic_pass@1`、轮次上限、模型），不得与官方 `pass@1` 混列在同一“基线”表里。
2. 与官方轨对照时，`NUM_TESTS`、`prompt-style`、数据子集必须一致，否则连并列都不成立。
3. `run_python` 在 Agent 机执行，属于 Agent 侧沙箱，不参与判分；判分只认 `submit_code` 的定稿代码。
