# 强化学习框架支持矩阵

状态定义：

- **支持**：有发布用户入口、固定版本、完整字段契约和真实端到端训练验收。
- **实验**：有研究实现，但接口、版本或运行语义仍可能变化，不能作为生产入口。
- **规划**：只有设计或调研，没有可执行发布命令。

| 框架 | 状态 | 用户现在能做什么 | 固定点或证据 | 主要限制 |
|---|---|---|---|---|
| VeRL | 支持 | 使用 `uenv train run-task`；SWE 使用 `uenv train run-swe` | v0.7.1；接入实现 `uenv-bridge/src/uenv/bridge/verl_agent_loop.py`；映射测试 `uenv-bridge/tests/test_verl_agent_loop.py` | 普通任务当前单步；SWE 训练当前只支持 Smith |
| ROLL | 实验 | 仅可研究复现实验脚本，不作为发布训练入口 | 实验目录 `uenv-bridge/scripts/roll_step_parallel/` | 未完成正式接入契约、版本固定和四层发布验收 |
| NexRL | 规划 | 只能阅读设计材料，当前不能运行 UEnv 训练 | 架构调研 `uenv-bridge/docs/NexRL架构与VeRL-ROLL对比调研.md`；最小样例规划 `uenv-bridge/docs/NexRL调研与最小样例规划.md` | trajectory pool、权重同步与服务边界仍在设计 |

其他强化学习框架按[自定义强化学习框架接入](./03-custom-framework.md)实现。在四层验收完成并提供发布入口前，状态只能是“实验”或“规划”。

状态变化必须同时更新用户入口、固定版本、契约测试、端到端证据和本表，不能只修改宣传文字。
