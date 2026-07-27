#!/usr/bin/env python3
"""SWE-bench Pro 离线轨迹采集入口。

这个文件提供一个语义清晰的启动命令，用于采集后续稳定性 replay 所需的 SWE-bench Pro 轨迹。它本身不重新实现压测流程，而是复用规模压测套件中已有的安全校验和子任务编排。

实现逻辑是：把当前命令转换为 run_scale_suite 的 trace-collection 模式参数，保留原有生产保护、端口隔离、输入校验和产物组织方式，只改变任务目标为轨迹采集。"""

from __future__ import annotations

import sys

from uenv_stress.cli import run_scale_suite


def arguments(argv: list[str]) -> list[str]:
    return [
        "swebench-pro-trace-collection",
        *argv,
    ]


def main() -> int:
    return run_scale_suite.main(
        arguments(sys.argv[1:]),
        prog="run_trace_collection.py",
    )


if __name__ == "__main__":
    raise SystemExit(main())
