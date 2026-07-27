#!/usr/bin/env python3
"""稳定性轨迹准入 manifest 生成工具。

这个文件校验五类冻结真实轨迹语料，并写出正式稳定性验收使用的统一 admission manifest。它保证验收运行前能明确知道每个数据集的轨迹路径、样本数量、哈希和校验结果。

实现逻辑是：validate_swe 对 SWE-bench Pro 额外检查冻结实例 ID；main 读取各语料路径，调用 validate_trace_file 检查 schema、样本数、turn、token、时延和 checksum，计算每个文件的 sha256，最后写出包含全部数据集条目的 manifest JSON；任一语料不满足准入条件时直接失败。"""

from __future__ import annotations

import argparse
import json
import time
from pathlib import Path
from typing import Any

from uenv_stress.core.stability_test_common import (
    PAIRED_TASK_NAMES,
    load_config,
    sha256_file,
    source_model_family,
    validate_paired_trace_order,
    validate_trace_file,
)


DATASET_NAMES = {
    "dscodebench": "dscodebench",
    "swebench_pro": "swe-bench-pro",
    "olymmath": "olymmath",
    "scitab": "scitab",
    "pubmedqa": "pubmedqa",
}


def validate_swe(path: Path, frozen_ids: set[str]) -> dict[str, Any]:
    covered: set[str] = set()
    for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if not line.strip():
            continue
        item = json.loads(line)
        if item.get("benchmark_variant") != "pro" or item.get("dataset") != "swe-bench-pro":
            raise ValueError(f"{path}:{line_number} is not tagged as SWE-bench Pro")
        driver = str(item.get("driver") or item.get("driver_entrypoint") or "")
        if not driver.endswith("run_swebenchpro_official.py"):
            raise ValueError(f"{path}:{line_number} does not use the Pro official driver")
        instance_id = str(item.get("instance_id", ""))
        if instance_id not in frozen_ids:
            raise ValueError(f"{path}:{line_number} instance is outside the frozen set")
        covered.add(instance_id)
    missing = sorted(frozen_ids - covered)
    if missing:
        raise ValueError(f"SWE-bench Pro traces miss frozen instances: {missing}")
    return {"covered_instance_count": len(covered)}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    config = load_config(args.config)
    trace_root = Path(config["traces"]["root"]).resolve()
    stats: dict[str, Any] = {}
    files = []
    source_models: dict[str, list[str]] = {}
    pairing: dict[str, Any] = {}
    for task, task_config in config["tasks"].items():
        path = Path(task_config["trace_file"]).resolve()
        try:
            relative = path.relative_to(trace_root)
        except ValueError as exc:
            raise ValueError(f"trace file must be under configured trace root: {path}") from exc
        stats[task] = validate_trace_file(
            path,
            dataset=DATASET_NAMES[task],
            minimum=int(task_config["min_valid_traces"]),
        )
        models: set[str] = set()
        traces = [
            json.loads(line)
            for line in path.read_text(encoding="utf-8").splitlines()
            if line.strip()
        ]
        for item in traces:
            models.add(f"{item['source_model']}@{item['source_version']}")
        source_models[task] = sorted(models)
        if task in PAIRED_TASK_NAMES:
            pairing[task] = validate_paired_trace_order(
                traces, expected_pairs=int(task_config["expected_pairs"])
            )
        elif any(
            source_model_family(item.get("source_model")) != "doubao"
            for item in traces
        ):
            raise ValueError("SWE-bench Pro manifest accepts Doubao traces only")
        files.append({"path": relative.as_posix(), "size_bytes": path.stat().st_size, "sha256": sha256_file(path)})
    instance_ids = set(json.loads(Path(config["tasks"]["swebench_pro"]["instance_list"]).read_text(encoding="utf-8")))
    if len(instance_ids) != 50:
        raise ValueError("SWE-bench Pro instance list must have 50 unique IDs")
    stats["swebench_pro"].update(validate_swe(Path(config["tasks"]["swebench_pro"]["trace_file"]), instance_ids))
    manifest = {
        "schema_version": 1, "created_unix": time.time(), "development_only": False,
        "selection_strategy": config["traces"]["selection_strategy"],
        "pairing_strategy": config["traces"]["pairing_strategy"],
        "datasets": stats,
        "source_models": source_models,
        "pairing": pairing,
        "swebench_pro": {"doubao_only": True, "trace_count": 50},
        "latency_replay": config["latency_replay"],
        "files": files,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    temporary = args.output.with_suffix(args.output.suffix + ".tmp")
    temporary.write_text(json.dumps(manifest, indent=2, sort_keys=True), encoding="utf-8")
    temporary.replace(args.output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
