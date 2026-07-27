"""数据集 workload 构造包。

这个目录把不同数据集样本转换成 UEnv Episode 需要的 env payload 和 reward 配置。它不负责启动 server 或 worker，只负责回答“给定一条数据集记录，应该发给 UEnv 什么任务参数”。

实现逻辑是：DSCodeBench、规则任务和 SWE-bench Pro 各有独立适配文件；上层压测脚本读取样本后调用这些适配函数，得到统一的 payload，再交给 Episode 构造和执行逻辑。"""
