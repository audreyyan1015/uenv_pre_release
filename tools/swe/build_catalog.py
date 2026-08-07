#!/usr/bin/env python3
"""Convert official SWE benchmark exports into the catalog schema used by UEnv."""

from __future__ import annotations

import argparse
import ast
import json
import os
import pathlib
import shlex
import sys
from typing import Any, Iterable


VARIANTS = ("verified", "lite", "pro", "smith")
VERIFIED_PREFIX = "swebench/sweb.eval."


def _list(value: Any) -> list[str]:
    if value is None:
        return []
    if isinstance(value, (list, tuple)):
        return [str(item) for item in value]
    text = str(value).strip()
    if not text:
        return []
    for loader in (json.loads, ast.literal_eval):
        try:
            parsed = loader(text)
        except (ValueError, SyntaxError, json.JSONDecodeError):
            continue
        if isinstance(parsed, (list, tuple)):
            return [str(item) for item in parsed]
    return [text]


def _rows(path: pathlib.Path) -> list[dict[str, Any]]:
    suffix = path.suffix.lower()
    if suffix == ".parquet":
        try:
            import pyarrow.parquet as parquet  # type: ignore[import-not-found]
        except ImportError as exc:
            raise ValueError("读取 Parquet 需要安装 pyarrow") from exc
        values = parquet.read_table(path).to_pylist()
    elif suffix == ".jsonl":
        values = []
        for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if not line.strip():
                continue
            try:
                values.append(json.loads(line))
            except json.JSONDecodeError as exc:
                raise ValueError(f"第 {number} 行不是有效 JSON：{exc}") from exc
    else:
        loaded = json.loads(path.read_text(encoding="utf-8"))
        if isinstance(loaded, dict):
            values = []
            for key, value in loaded.items():
                if not isinstance(value, dict):
                    raise ValueError(f"catalog 行必须是 JSON 对象：{key}")
                row = dict(value)
                row.setdefault("instance_id", key)
                if row["instance_id"] != key:
                    raise ValueError(f"catalog key 与 row.instance_id 不一致：{key}")
                values.append(row)
        elif isinstance(loaded, list):
            values = loaded
        else:
            raise ValueError("JSON 输入必须是对象或数组")
    if not all(isinstance(value, dict) for value in values):
        raise ValueError("每条输入记录都必须是 JSON 对象")
    return [dict(value) for value in values]


def _official_image(instance_id: str) -> str:
    return VERIFIED_PREFIX + "x86_64." + instance_id.replace("__", "_1776_") + ":latest"


def _pro_image(row: dict[str, Any]) -> str:
    image = str(row.get("image_cache_key") or "").strip()
    if image:
        return image
    tag = str(row.get("dockerhub_tag") or "").strip()
    if not tag:
        raise ValueError("Pro 记录缺少 image_cache_key 或 dockerhub_tag")
    if "/" in tag:
        return tag
    return f"jefzda/sweap-images:{tag}"


def _selected_tests(value: Any, language: str) -> list[str]:
    selected = _list(value)
    if language in {"javascript", "typescript", "node", "nodejs", "js", "ts"}:
        selected = [item.split(" | ", 1)[0].strip() for item in selected]
    result: list[str] = []
    seen: set[str] = set()
    for item in selected:
        if item and item not in seen:
            seen.add(item)
            result.append(item)
    return result


def _pro_test_command(row: dict[str, Any]) -> tuple[str, str | None]:
    explicit = str(row.get("test_cmd") or "").strip()
    language = str(row.get("repo_language") or "").strip().lower()
    selected = _selected_tests(row.get("selected_test_files_to_run"), language)
    quoted = " ".join(shlex.quote(item) for item in selected)
    if explicit:
        command = explicit
    elif language == "python":
        command = "python -m pytest -v" + (f" {quoted}" if quoted else "")
    elif language in {"javascript", "typescript", "node", "nodejs", "js", "ts"}:
        command = "npm test" + (f" -- {quoted}" if quoted else "")
    elif language == "go":
        command = "go test ./... -v"
    elif selected:
        raise ValueError("Pro 的未知 repo_language 需要显式 test_cmd")
    else:
        raise ValueError("Pro 记录缺少可生成 test_cmd 的字段")
    pre_test = str(row.get("pre_test_cmd") or "").strip() or None
    if not pre_test and language in {"javascript", "typescript", "node", "nodejs", "js", "ts"}:
        pre_test = "redis-server --daemonize yes"
    return command, pre_test


def _normalize(row: dict[str, Any], variant: str) -> dict[str, Any]:
    instance_id = str(row.get("instance_id") or "").strip()
    if not instance_id:
        raise ValueError("记录缺少 instance_id")
    repo = str(row.get("repo") or "").strip()
    if not repo:
        raise ValueError(f"{instance_id} 缺少 repo")

    image = str(row.get("image_cache_key") or "").strip()
    if variant in {"verified", "lite"}:
        image = image or _official_image(instance_id)
    elif variant == "pro":
        image = _pro_image(row)
    else:
        image = image or str(row.get("image_name") or "").strip()
        if not image:
            raise ValueError(f"{instance_id} 缺少 image_cache_key 或 image_name")

    if variant in {"verified", "lite"} and not image.startswith(VERIFIED_PREFIX):
        raise ValueError(f"{instance_id} 的 {variant} 镜像必须位于 {VERIFIED_PREFIX} 命名空间")
    if variant == "pro" and image.startswith(VERIFIED_PREFIX):
        raise ValueError(f"{instance_id} 的 Pro 镜像不能使用 Verified 命名空间")
    if variant == "smith" and (image.startswith(VERIFIED_PREFIX) or "swesmith" not in image.lower()):
        raise ValueError(f"{instance_id} 的 Smith 镜像必须使用 swesmith 命名空间")

    normalized: dict[str, Any] = {
        "instance_id": instance_id,
        "repo": repo,
        "version": str(row.get("version") or ("pro" if variant == "pro" else "")),
        "base_commit": str(row.get("base_commit") or ""),
        "environment_setup_commit": str(row.get("environment_setup_commit") or ""),
        "problem_statement": str(row.get("problem_statement") or ""),
        "patch": str(row.get("patch") or ""),
        "test_patch": str(row.get("test_patch") or ""),
        "FAIL_TO_PASS": _list(row.get("FAIL_TO_PASS", row.get("fail_to_pass"))),
        "PASS_TO_PASS": _list(row.get("PASS_TO_PASS", row.get("pass_to_pass"))),
        "benchmark_variant": variant,
        "image_cache_key": image,
    }
    for source, target in (
        ("install_cmd", "install_cmd"),
        ("setup_cmd", "setup_cmd"),
        ("before_repo_set_cmd", "setup_cmd"),
        ("pre_test_cmd", "pre_test_cmd"),
        ("test_cmd", "test_cmd"),
    ):
        value = row.get(source)
        if value not in (None, "") and target not in normalized:
            normalized[target] = str(value).strip()
    if variant == "pro":
        test_cmd, pre_test = _pro_test_command(row)
        normalized["test_cmd"] = test_cmd
        if pre_test:
            normalized["pre_test_cmd"] = pre_test
        setup = str(row.get("setup_cmd") or row.get("before_repo_set_cmd") or "").strip()
        if setup:
            normalized["setup_cmd"] = setup
    return normalized


def _select(rows: Iterable[dict[str, Any]], ids: set[str], limit: int) -> list[dict[str, Any]]:
    result: list[dict[str, Any]] = []
    for row in rows:
        instance_id = str(row.get("instance_id") or "")
        if ids and instance_id not in ids:
            continue
        result.append(row)
        if limit and len(result) >= limit:
            break
    return result


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--variant", required=True, choices=VARIANTS)
    ap.add_argument("--input", required=True)
    ap.add_argument("--output", required=True)
    ap.add_argument("--instance", action="append", default=[], help="只转换指定 instance_id，可重复")
    ap.add_argument("--limit", type=int, default=0, help="最多转换 N 条；0 表示全部")
    args = ap.parse_args(argv)
    try:
        if args.limit < 0:
            raise ValueError("--limit 不能为负数")
        source = pathlib.Path(args.input)
        if not source.is_file():
            raise ValueError(f"输入文件不存在：{source}")
        selected = _select(_rows(source), set(args.instance), args.limit)
        if not selected:
            raise ValueError("没有匹配的记录")
        output: dict[str, dict[str, Any]] = {}
        for row in selected:
            normalized = _normalize(row, args.variant)
            instance_id = normalized["instance_id"]
            if instance_id in output:
                raise ValueError(f"重复 instance_id：{instance_id}")
            output[instance_id] = normalized
        destination = pathlib.Path(args.output)
        if destination.exists() or destination.is_symlink():
            raise ValueError(f"输出文件已存在：{destination}")
        destination.parent.mkdir(parents=True, exist_ok=True)
        temporary = destination.parent / f".{destination.name}.tmp.{os.getpid()}"
        with temporary.open("x", encoding="utf-8") as handle:
            json.dump(output, handle, ensure_ascii=False, indent=2, sort_keys=True)
            handle.write("\n")
        os.replace(temporary, destination)
        print(f"catalog: {destination} ({len(output)} instances, variant={args.variant})")
        return 0
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        print(f"错误：{exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
