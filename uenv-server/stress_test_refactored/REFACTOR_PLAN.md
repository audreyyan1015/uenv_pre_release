# UEnv 压测代码隔离重构计划

## 1. 目标

在不修改现有 `uenv-server/stress_test` 的前提下，新建
`uenv-server/stress_test_refactored`，把代码整理为三个明确入口：

1. 规模压测：五个数据集分别验证真实 Worker、真实插件和 UEnv 调度容量。
2. 正式稳定性验收：五个数据集按配置比例混合运行 4 小时参考、72 小时长稳、容量、突发和故障阶段。
3. 轨迹采集：冻结真实模型轨迹，为规模压测和稳定性回放提供输入，不把采集结果冒充规模或稳定性证据。

五个必测数据集为：DSCodeBench、SWE-bench Pro、OlymMATH、SciTab、
PubMedQA。任何正式的整套规模压测或稳定性验收都必须五项齐全，缺一项
即整套失败。

## 2. 不变边界

- 不修改、移动或删除现有 `uenv-server/stress_test` 的文件。
- 不复制 `__pycache__`、`.bak-*`、outputs 或历史运行 artifacts。
- 本次整理不启动压测、Server、Worker、插件、OpenHands 或容器。
- 不修改生产服务、配置、数据和生产监听端口。
- Worker 只允许 `8.130.65.20` 与 `8.145.51.129`。
- `8.130.86.71` 只能出现在禁用清单或负向测试中。
- 代码和配置中不保存 SSH 密码、API Key 或其他明文凭据。

## 3. 目标结构

```text
stress_test_refactored/
├── REFACTOR_PLAN.md
├── README.md
├── CHANGELOG.md
├── SOURCE_SNAPSHOT.json
├── uenv_stress/
│   ├── cli/
│   ├── config/
│   ├── core/
│   ├── workloads/
│   ├── scale/
│   ├── stability/
│   ├── tools/
│   └── providers/ark/
└── tests/
```

## 4. 代码边界

### 4.1 公共层

- `core/stress_test_common.py`：Episode、结果和兼容统计实现。
- `core/distributed_runtime.py`：跨主机 SSH、端口检查、受控进程和生产保护。
- `core/fleet_supervisor.py`：大规模真实 Worker 子进程监管。
- `workloads/`：纯数据加载和 Episode payload 构造，可被规模与稳定性共同复用。

### 4.2 规模压测

- `cli/run_scale_suite.py`：五数据集预检、场景编排、结果汇总和覆盖完整性判定。
- `scale/dscodebench_pressure.py`：真实 Code Worker/插件规模通道。
- `scale/swebench_pro_pressure.py`：真实 SWE Worker、OpenHands、AgentControlService 和容器通道。
- `scale/rule_task_pressure.py`：同一隔离 Math Worker/插件 fleet 依次运行 OlymMATH、SciTab、PubMedQA。

每个数据集都必须：

- 使用至少 1024 个真实 UEnv Worker；
- 独立覆盖 `sync`、`one_step_off_policy`、`fully_async`；
- 每种模式提交至少 10 个 `Worker 数 × 单 Worker 容量` 波次；
- 使用真实数据输入和任务对应的冻结真实轨迹；
- 单独报告吞吐、批延迟、协议错误、Worker 调度覆盖和 replay 命中；
- 明确声明 trace replay 是系统规模证据，不是新的模型质量证据。

规则奖励三任务共享基础设施，但九个“数据集 × 并行模式”结果彼此独立。
共享 fleet 是为了减少重复启动成本，不代表把三个数据集混成一个结果。

### 4.3 正式稳定性验收

- `cli/run_formal_stability_suite.py`：五任务混合负载、阶段编排和验收指纹。
- `stability/replay_server.py`：Episode 绑定的冻结轨迹回放。
- `stability/faults.py`：只对 fleet manifest 所属资源注入故障。
- `stability/report.py`：五任务与全局指标汇总。

稳定性与规模压测复用 `workloads/` 的数据适配器和 `core/` 的 Episode/结果
契约，但保留不同的负载模型、运行时长、故障阶段和验收阈值。

### 4.4 轨迹与数据准备

- `tools/prepare_datasets.py`
- `tools/prepare_trace_manifest.py`
- `tools/collect_math_traces.py`
- `tools/freeze_existing_traces.py`
- `tools/convert_swepro_schema.py`

轨迹采集与 1024 Worker 压测分开执行。规模入口只消费已冻结、校验通过的
轨迹，不能在压测时临时调用真实模型收集轨迹。

## 5. 配置拆分

- `runtime_hosts.json`：Server、两台可用 Worker、SSH 指纹、保护端口和禁用主机。
- `scale_suite.json`：五数据集规模、波次、并行模式、真实插件和 replay 参数。
- `trace_collection.json`：五数据集真实轨迹采集数量、并发与输出位置。
- `stability_suite.json`：五任务比例、4/72 小时阶段、容量、突发、故障和阈值。

运行时 SSH 密码只通过 `UENV_PASS` 注入。

## 6. 实施顺序

1. 冻结旧目录根文件哈希和 Git 状态。
2. 在本地暂存区复制源代码与配置，不复制运行产物。
3. 建立 Python package、三类入口和统一 runtime inventory。
4. 抽出公共 workload 适配器，稳定性和规模入口共用数据到 Episode 的映射。
5. 保留 DSCodeBench 与 SWE-bench Pro 专用规模通道。
6. 增加 Math Worker 通道，覆盖 OlymMATH、SciTab、PubMedQA。
7. 在整套规模汇总中增加五数据集 fail-closed 覆盖检查。
8. 增加配置、命令构造、数据映射、host 禁用和嵌入脚本测试。
9. 仅执行编译、配置解析和单元测试。
10. 重新计算旧目录根文件哈希，证明原目录未变化。

## 7. 整理完成条件

- 新目录通过 `python3 -m compileall`。
- 新目录全部单元测试通过。
- 规模配置明确且强制覆盖五个数据集。
- 稳定性配置明确且强制覆盖五个数据集。
- `suite` 汇总只有在五个数据集场景全部通过时才通过。
- 两类套件复用 workload payload 构造，不复用错误的负载时序或验收标准。
- 新代码不含明文 SSH 密码。
- 禁用 Worker 不会被连接或调度。
- 旧 `stress_test` 根文件哈希与重构前一致。

## 8. 本次不做

- 不执行 1024 Worker 压测。
- 不执行 4 小时或 72 小时稳定性验收。
- 不执行故障注入。
- 不提交、不推送、不部署、不重启服务。
