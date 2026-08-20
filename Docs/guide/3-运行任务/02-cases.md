# 案例库

案例库按使用阶段分为评测与强化学习训练。先依次理解[通用评测流程](./03-evaluation.md)、[强化学习训练指南](./07-post-training.md)和[轨迹采集指南](./12-trajectory.md)，再在具体阶段选择与任务类型匹配的案例。

案例中的 QA 与 Code JSONL 是仓库自拟的最小示例输入，仅用于展示字段和端到端执行；benchmark 得分以官方评测为准。SWE 案例从安装包固定 catalog 选择实际实例。自定义环境（process plugin）单独成节，不绑定评测或训练阶段；自定义环境案例是接口模板，完成 plugin 实现、测试与安装后才能执行。

## 当前支持的环境与数据集

环境类型（`env_type`）选择环境实现，`dataset` 选择该环境内的数据格式和判分方式：

| `env_type` | `dataset` | 对应官方数据集 | 执行内容 |
|---|---|---|---|
| `qa` | `gsm8k` | [GSM8K](https://huggingface.co/datasets/openai/gsm8k) | 数学问答与结果匹配 |
| `qa` | `pubmedqa` | [PubMedQA](https://github.com/pubmedqa/pubmedqa) | 生物医学问答与分类 |
| `qa` | `scitab` | [SCITAB](https://github.com/XinyuanLu00/SciTab) | 科学表格声明验证 |
| `qa` | `olymmath`、`olymmath-easy`、`olymmath-hard` | [OlymMATH](https://huggingface.co/datasets/RUC-AIBOX/OlymMATH) | 奥数问题与答案匹配 |
| `code` | `dscodebench` | [DSCodeBench](https://github.com/ShuyinOuyang/DSCodeBench) | 生成代码并运行测试 |

SWE 任务通过变体（`--benchmark-variant`）和 catalog 选择实例：

| variant | 官方数据集 | 发布包中的内容 |
|---|---|---|
| `verified` | [SWE-bench Verified](https://huggingface.co/datasets/SWE-bench/SWE-bench_Verified) | 10 条用于检查安装的样例 |
| `lite` | [SWE-bench Lite](https://huggingface.co/datasets/SWE-bench/SWE-bench_Lite) | 按需生成 catalog |
| `pro` | [SWE-bench Pro](https://huggingface.co/datasets/ScaleAI/SWE-bench_Pro) | 按需生成 catalog |
| `smith` | [SWE-smith](https://huggingface.co/datasets/SWE-bench/SWE-smith) | 5 条训练样例 |

发布包自带的 catalog 是冒烟样例；四个 variant 的完整 catalog 都可以用官方导出自行生成，命令见[代码修复](./06-evaluation-swe-verified.md#输入与-catalog)。训练入口 `run-swe` 只支持 `smith`（见[强化学习训练指南](./07-post-training.md#选择任务入口)）。

表中没有的任务用 process plugin 自建环境，见[自定义环境](./11-process-plugin.md)。

## 评测

下表是本阶段的演示案例，“本案例输入”列为仓库自拟示例或发布包样例；系统当前支持的全部环境与数据集见上文[当前支持的环境与数据集](#当前支持的环境与数据集)。

| 任务类型 | 环境 / 本案例输入（示例） | 命令 |
|---|---|---|
| [数学问答](./04-evaluation-gsm8k.md) | `qa` / 自拟 GSM8K 风格输入 | `uenv evaluate run-task` |
| [代码生成](./05-evaluation-code.md) | `code` / 自包含函数测试 | `uenv evaluate run-task` |
| [代码修复](./06-evaluation-swe-verified.md) | SWE / Verified catalog 实例 | `sudo uenv evaluate run-swe` |

## 强化学习训练

下表是本阶段的演示案例，“本案例输入”列为仓库自拟示例或发布包样例：

| 任务类型 | 环境 / 本案例输入（示例） | 命令 |
|---|---|---|
| [数学问答](./08-training-gsm8k-verl.md) | `qa` / 自拟 GSM8K 风格输入 | `uenv train run-task` |
| [代码生成](./09-training-code-verl.md) | `code` / 自包含函数测试 | `uenv train run-task` |
| [代码修复](./10-training-swe-smith-verl.md) | SWE / Smith catalog 实例 | `uenv train run-swe` |

## 自定义环境

自定义环境（process plugin）不属于某个使用阶段：同一套创建、测试、安装流程之后，评测用 `uenv evaluate run-task`，训练用 `uenv train run-task`。

| 内容 | 说明 | 入口 |
|---|---|---|
| [自定义环境](./11-process-plugin.md) | process plugin 接口模板：创建、测试、安装与两个使用入口 | 评测 `uenv evaluate run-task`；训练 `uenv train run-task` |

每个评测或训练案例产生结果后都会生成轨迹并自动进入集中存储，统一按[轨迹采集指南](./12-trajectory.md)用 `uenv trajectory` 查询。

## 文件来源与安装路径

文档不复制维护另一份输入；示例文件统一从下列位置读取：

```text
examples/cases/evaluation/   # 评测示例输入（JSONL）
examples/cases/training/     # 训练示例输入（JSONL）
config/swe/                  # SWE catalog（实例元数据）
```

为什么 SWE catalog 不在 `examples/cases/`：两类文件性质不同。`examples/cases/` 是逐条提交给 UEnv Server 的样本输入；`config/swe/` 是 UEnv Worker 运行时加载的 SWE 实例元数据（仓库、commit、镜像索引），不属于任何一次提交的样本——SWE 输入 JSONL 只是按 `instance_id` 从 catalog 中选择实例。因此 catalog 与 Worker 侧的运行配置放在一起，安装布局中位于 `share/swe/`。

发布安装默认根目录为 `/opt/uenv/current`。执行案例前确认相应文件存在：

```bash
export UENV_RELEASE_ROOT='/opt/uenv/current'
test -d "$UENV_RELEASE_ROOT/examples/cases"
```

从源码运行时，普通 JSONL 位于仓库根目录下的 `examples/cases/`，SWE catalog 位于 `config/swe/`；在安装布局中两者分别对应 `examples/cases/` 与 `share/swe/`。具体 SWE 页面分别给出需要替换的变量。

