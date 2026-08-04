"""UEnv 压测与正式稳定性验收代码包。

这个包把压测代码按职责拆成入口、公共运行时、规模压测场景、稳定性验收、数据准备工具、模型服务适配和 workload 构造几个部分。阅读者可以先看 cli 目录了解怎么启动，再看 scale 和 stability 目录了解不同压测目标，最后看 core 目录了解这些脚本复用的协议、结果、机器编排和生产保护逻辑。

实现逻辑是：入口脚本读取配置和命令行参数，调用 core 中的安全检查与远程运行工具，在隔离端口和隔离工作目录中启动 server、worker、replay 或代理进程；workloads 负责把数据集样本转换成 UEnv Episode 需要的 env/reward payload；结果统一写成 JSON/CSV/SQLite 证据，供报告和指标聚合使用。"""
