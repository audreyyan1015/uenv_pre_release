"""规模压测场景包。

这个目录放置具体的压力场景实现，包括 DSCodeBench Code worker 压测、OlymMATH/SciTab/PubMedQA 规则任务压测、SWE-bench Pro/OpenHands 容器压测。每个场景都运行在隔离 server、隔离 worker 和隔离产物目录中。

实现逻辑是：场景脚本读取上层传入的主机、端口、数据集、并发和 replay 配置，生成 server/worker 配置文件，启动本次拥有的进程，投放 Episode，采集结果和资源证据，最后清理本次创建的进程与容器。"""
