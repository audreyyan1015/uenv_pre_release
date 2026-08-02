"""生产进程与端口保护的稳定公开接口。

这个文件给上层压测脚本提供统一的安全检查入口，确保压测使用隔离端口和隔离进程，不误伤已经运行的正式服务。它只做保护能力封装，不包含具体压测业务。

实现逻辑是：从 distributed_runtime 重新导出端口空闲检查、受保护进程快照和快照比对函数；压测开始前记录正式服务的 PID、命令行和监听端口，压测期间只允许操作本次创建的进程，压测结束后再次比对，若正式服务发生非预期变化就让压测失败。"""

from .distributed_runtime import (
    assert_port_free,
    assert_ports_free,
    assert_protected_unchanged,
    protected_snapshot,
)

__all__ = [
    "assert_port_free",
    "assert_ports_free",
    "assert_protected_unchanged",
    "protected_snapshot",
]
