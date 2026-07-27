"""Worker fleet 描述与分配的稳定公开接口。

这个文件把 worker 主机解析、连接和任务分配能力集中暴露给上层压测脚本。它不启动具体业务任务，只负责让压测入口以统一方式理解“有哪些 worker 主机、每个 worker 应该跑在哪台机器上”。

实现逻辑是：从 distributed_runtime 重新导出 WorkerNode、parse_worker_nodes、connect_worker_nodes 和 worker_assignments；这些函数会解析命令行中的 worker 节点配置，建立 SSH 连接，并按索引把 worker 进程分配到不同主机。"""

from .distributed_runtime import (
    WorkerNode,
    connect_worker_nodes,
    parse_worker_nodes,
    worker_assignments,
)

__all__ = [
    "WorkerNode",
    "connect_worker_nodes",
    "parse_worker_nodes",
    "worker_assignments",
]
