#!/usr/bin/env python3
"""Bounded multi-instance SWE evaluator used by ``uenv evaluate run-swe``."""

from __future__ import annotations

import argparse
import concurrent.futures
import getpass
import grp
import json
import os
import pathlib
import pwd
import re
import stat
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request
from typing import Any


MAX_CATALOG_BYTES = 64 * 1024 * 1024
MAX_INPUT_BYTES = 16 * 1024 * 1024
MAX_RESULT_BYTES = 16 * 1024 * 1024
SAFE_ID = re.compile(r"^[A-Za-z0-9_.-]+$")
VARIANTS = ("verified", "lite", "pro", "smith")
CATALOG_COMPARE_FIELDS = (
    "instance_id",
    "repo",
    "base_commit",
    "problem_statement",
    "benchmark_variant",
    "image_cache_key",
)


class UserError(RuntimeError):
    pass


def parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description="Run a bounded batch of SWE instances")
    p.add_argument("provider", choices=("local", "volcengine"))
    p.add_argument("--model", required=True)
    p.add_argument("--base-url")
    p.add_argument("--api-key-file")
    p.add_argument("--gateway", required=True)
    p.add_argument("--catalog", required=True)
    p.add_argument("--benchmark-variant", required=True, choices=VARIANTS)
    p.add_argument("--input", required=True)
    p.add_argument("--output", required=True)
    p.add_argument("--artifacts-dir", required=True)
    p.add_argument("--max-iterations", required=True, type=int)
    p.add_argument("--batch-size", required=True, type=int)
    p.add_argument("--offline", action="store_true")
    return p


def _regular_file(path: pathlib.Path, max_bytes: int, label: str) -> bytes:
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    try:
        fd = os.open(path, flags)
    except OSError as exc:
        raise UserError(f"无法读取{label}：{path}: {exc}") from exc
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode):
            raise UserError(f"{label}必须是普通文件：{path}")
        if info.st_size > max_bytes:
            raise UserError(f"{label}过大：{path}")
        data = b""
        while len(data) <= max_bytes:
            chunk = os.read(fd, min(1024 * 1024, max_bytes + 1 - len(data)))
            if not chunk:
                break
            data += chunk
        if len(data) > max_bytes:
            raise UserError(f"{label}过大：{path}")
        return data
    finally:
        os.close(fd)


def _load_json(path: pathlib.Path, label: str, max_bytes: int) -> Any:
    try:
        return json.loads(_regular_file(path, max_bytes, label).decode("utf-8"))
    except UnicodeDecodeError as exc:
        raise UserError(f"{label}不是 UTF-8：{path}") from exc
    except json.JSONDecodeError as exc:
        raise UserError(f"{label}不是有效 JSON：{path}: {exc}") from exc


def _catalog(path: pathlib.Path) -> dict[str, dict[str, Any]]:
    value = _load_json(path, "catalog", MAX_CATALOG_BYTES)
    if not isinstance(value, dict) or not value:
        raise UserError("catalog 必须是非空 JSON 对象")
    result: dict[str, dict[str, Any]] = {}
    for key, row in value.items():
        if not isinstance(key, str) or not SAFE_ID.fullmatch(key):
            raise UserError(f"catalog 包含无效 instance_id：{key!r}")
        if not isinstance(row, dict):
            raise UserError(f"catalog 行必须是 JSON 对象：{key}")
        row_id = row.get("instance_id", key)
        if row_id != key:
            raise UserError(f"catalog key 与 row.instance_id 不一致：{key!r} / {row_id!r}")
        normalized = dict(row)
        normalized["instance_id"] = key
        result[key] = normalized
    return result


def _cases(path: pathlib.Path, catalog: dict[str, dict[str, Any]], variant: str) -> list[dict[str, str]]:
    raw = _regular_file(path, MAX_INPUT_BYTES, "输入 JSONL").decode("utf-8")
    rows: list[dict[str, str]] = []
    ids: set[str] = set()
    for line_number, line in enumerate(raw.splitlines(), 1):
        if not line.strip():
            continue
        try:
            value = json.loads(line)
        except json.JSONDecodeError as exc:
            raise UserError(f"输入第 {line_number} 行不是有效 JSON：{exc}") from exc
        if not isinstance(value, dict):
            raise UserError(f"输入第 {line_number} 行必须是 JSON 对象")
        instance = value.get("instance_id")
        case_id = value.get("id", instance)
        if not isinstance(instance, str) or not SAFE_ID.fullmatch(instance):
            raise UserError(f"输入第 {line_number} 行的 instance_id 无效")
        if not isinstance(case_id, str) or not SAFE_ID.fullmatch(case_id):
            raise UserError(f"输入第 {line_number} 行的 id 无效")
        if case_id in ids:
            raise UserError(f"输入包含重复 id：{case_id}")
        if instance not in catalog:
            raise UserError(f"输入实例不在 catalog：{instance}")
        row_variant = str(catalog[instance].get("benchmark_variant") or "verified").lower()
        if row_variant != variant:
            raise UserError(
                f"实例 {instance} 的 catalog variant 是 {row_variant}，命令指定的是 {variant}"
            )
        ids.add(case_id)
        rows.append({"id": case_id, "instance_id": instance})
    if not rows:
        raise UserError("输入 JSONL 没有任务")
    return rows


def _url(value: str, label: str) -> str:
    parsed = urllib.parse.urlsplit(value)
    if parsed.scheme not in ("http", "https") or not parsed.netloc:
        raise UserError(f"{label}必须是 http:// 或 https:// URL")
    if parsed.username or parsed.password or parsed.query or parsed.fragment:
        raise UserError(f"{label}不能包含账号、查询参数或 fragment")
    return urllib.parse.urlunsplit((parsed.scheme, parsed.netloc, parsed.path.rstrip("/"), "", ""))


def _secret(args: argparse.Namespace) -> tuple[str, str]:
    from_file = ""
    if args.api_key_file:
        path = pathlib.Path(args.api_key_file)
        info = path.lstat()
        if stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode):
            raise UserError("API Key 文件必须是普通文件")
        if info.st_mode & 0o077:
            raise UserError("API Key 文件权限必须为 0600")
        from_file = _regular_file(path, 64 * 1024, "API Key 文件").decode("utf-8")
        if from_file.endswith("\r\n"):
            from_file = from_file[:-2]
        elif from_file.endswith(("\n", "\r")):
            from_file = from_file[:-1]
        if not from_file or "\n" in from_file or "\r" in from_file:
            raise UserError("API Key 文件只能包含一行")
    if args.provider == "local":
        if not args.base_url:
            raise UserError("local provider 需要 --base-url")
        return _url(args.base_url, "--base-url"), (from_file or os.environ.get("LOCAL_MODEL_API_KEY") or "EMPTY")
    base = args.base_url or os.environ.get("ARK_BASE_URL") or "https://ark.cn-beijing.volces.com/api/v3"
    key = from_file or os.environ.get("ARK_API_KEY", "")
    if not key and sys.stdin.isatty():
        key = getpass.getpass("火山方舟 API Key（输入不显示）: ")
    if not key:
        raise UserError("volcengine provider 需要 ARK_API_KEY 或 --api-key-file")
    return _url(base, "--base-url"), key


def _gateway_key() -> str:
    key = os.environ.get("UENV_GATEWAY_API_KEY", "")
    if key:
        return key
    path = pathlib.Path("/etc/uenv/secrets/swe.env")
    if not path.exists():
        return ""
    text = _regular_file(path, 64 * 1024, "Gateway 配置").decode("utf-8")
    values: dict[str, str] = {}
    for line in text.splitlines():
        if "=" in line:
            name, value = line.split("=", 1)
            values[name] = value
    return values.get("UENV_SWE_GATEWAY_API_KEY") or values.get("UENV_RUNTIME_GATEWAY_API_KEY") or ""


def _gateway_rows(
    gateway: str,
    key: str,
    cases: list[dict[str, str]],
    catalog: dict[str, dict[str, Any]],
) -> None:
    checked: set[str] = set()
    for case in cases:
        instance = case["instance_id"]
        if instance in checked:
            continue
        checked.add(instance)
        url = f"{gateway}/runtime/v1/instances/{urllib.parse.quote(instance, safe='')}"
        request = urllib.request.Request(url)
        if key:
            request.add_header("X-API-Key", key)
        try:
            with urllib.request.urlopen(request, timeout=10) as response:
                body = response.read(MAX_RESULT_BYTES + 1)
                if len(body) > MAX_RESULT_BYTES:
                    raise UserError(f"Gateway catalog 响应过大：{instance}")
                remote = json.loads(body.decode("utf-8"))
        except (urllib.error.URLError, urllib.error.HTTPError, TimeoutError, json.JSONDecodeError) as exc:
            raise UserError(f"无法从 Gateway 读取实例 {instance}：{exc}") from exc
        if not isinstance(remote, dict):
            raise UserError(f"Gateway 返回的实例不是 JSON 对象：{instance}")
        local = catalog[instance]
        mismatched: list[str] = []
        for field in CATALOG_COMPARE_FIELDS:
            left = local.get(field)
            right = remote.get(field)
            if field == "instance_id":
                left = left or instance
                right = right or instance
            elif field == "benchmark_variant":
                left = left or "verified"
                right = right or "verified"
            if left != right:
                mismatched.append(field)
        if mismatched:
            raise UserError(
                f"本地 catalog 与 Worker catalog 不一致：{instance}（字段：{', '.join(mismatched)}）"
            )


def _secure_artifact_root(path: pathlib.Path) -> None:
    if path.exists() or path.is_symlink():
        raise UserError(f"--artifacts-dir 必须是尚未存在的新目录：{path}")
    parent = path.parent.resolve(strict=True)
    current = parent
    while True:
        info = current.stat()
        if info.st_uid != 0 or info.st_mode & 0o022:
            raise UserError(f"制品目录父路径必须由 root 管理且不可被组/其他用户写入：{current}")
        if current == current.parent:
            break
        current = current.parent
    path.mkdir(mode=0o750)
    try:
        os.chown(path, 0, grp.getgrnam("uenv").gr_gid)
    except KeyError as exc:
        raise UserError("系统缺少 uenv 组，请先运行 prepare-swe") from exc


def _safe_submit(path: pathlib.Path, expected_uid: int, instance: str) -> dict[str, Any]:
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    try:
        fd = os.open(path, flags)
    except OSError as exc:
        raise UserError(f"未生成 submit_result.json：{exc}") from exc
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != expected_uid or info.st_size > MAX_RESULT_BYTES:
            raise UserError("submit_result.json 的类型、属主或大小无效")
        raw = b""
        while len(raw) <= MAX_RESULT_BYTES:
            chunk = os.read(fd, min(1024 * 1024, MAX_RESULT_BYTES + 1 - len(raw)))
            if not chunk:
                break
            raw += chunk
        value = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise UserError(f"submit_result.json 无效：{exc}") from exc
    finally:
        os.close(fd)
    if not isinstance(value, dict):
        raise UserError("submit_result.json 必须是 JSON 对象")
    result_instance = value.get("instance_id")
    if result_instance and result_instance != instance:
        raise UserError("submit_result.json 的 instance_id 与当前任务不一致")
    return value


def _tail(handle: Any, limit: int = 8000) -> str:
    handle.flush()
    handle.seek(0, os.SEEK_END)
    size = handle.tell()
    handle.seek(max(0, size - limit))
    return handle.read().decode("utf-8", errors="replace").strip()


def _run_case(
    index: int,
    case: dict[str, str],
    args: argparse.Namespace,
    base_url: str,
    model_key: str,
    gateway_key: str,
    artifact_root: pathlib.Path,
) -> dict[str, Any]:
    case_dir = artifact_root / f"{index:05d}-{case['id']}"
    case_dir.mkdir(mode=0o700)
    stdout_fd = os.open(case_dir / "runner.stdout.log", os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
    stderr_fd = os.open(case_dir / "runner.stderr.log", os.O_RDWR | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
    command = [
        str(pathlib.Path(__file__).with_name("evaluate_one.sh")),
        args.provider,
        "--model", args.model,
        "--base-url", base_url,
        "--gateway", args.gateway,
        "--catalog", str(pathlib.Path(args.catalog).resolve()),
        "--benchmark-variant", args.benchmark_variant,
        "--instance", case["instance_id"],
        "--output-dir", str(case_dir),
        "--max-iterations", str(args.max_iterations),
    ]
    if args.offline:
        command.append("--offline")
    env = {
        "PATH": "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        "LANG": "C.UTF-8",
        "LC_ALL": "C.UTF-8",
        "UENV_EVAL_MODEL_API_KEY": model_key,
        "UENV_GATEWAY_API_KEY": gateway_key,
    }
    try:
        with os.fdopen(stdout_fd, "wb", closefd=True) as stdout_handle, os.fdopen(stderr_fd, "w+b", closefd=True) as stderr_handle:
            try:
                completed = subprocess.run(command, env=env, stdout=stdout_handle, stderr=stderr_handle, check=False)
                exit_code = completed.returncode
            except OSError as exc:
                exit_code = 127
                stderr_handle.write(str(exc).encode("utf-8", errors="replace"))
            error_tail = _tail(stderr_handle)
        submit: dict[str, Any] = {}
        error = ""
        if exit_code == 0:
            try:
                submit = _safe_submit(case_dir / "submit_result.json", pwd.getpwnam("uenv-agent").pw_uid, case["instance_id"])
            except (UserError, KeyError) as exc:
                exit_code = 1
                error = str(exc)
        else:
            error = error_tail or f"single-case runner exited with {exit_code}"
        result: dict[str, Any] = {
            "case_id": case["id"],
            "instance_id": case["instance_id"],
            "status": "completed" if exit_code == 0 else "failed",
            "exit_code": exit_code,
            "error": error,
            "artifact_dir": str(case_dir),
        }
        for key in ("resolved", "reward", "tests_passed", "tests_total", "trajectory_id"):
            if key in submit:
                result[key] = submit[key]
        return result
    except Exception as exc:  # keep one infrastructure error from cancelling the batch
        return {
            "case_id": case["id"],
            "instance_id": case["instance_id"],
            "status": "failed",
            "exit_code": 1,
            "error": str(exc),
            "artifact_dir": str(case_dir),
        }


def _write_results(path: pathlib.Path, results: list[dict[str, Any]]) -> None:
    if path.exists() or path.is_symlink():
        raise UserError(f"--output 已存在：{path}")
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.parent / f".{path.name}.tmp.{os.getpid()}"
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0)
    fd = os.open(temporary, flags, 0o600)
    try:
        with os.fdopen(fd, "w", encoding="utf-8", closefd=True) as handle:
            for result in results:
                handle.write(json.dumps(result, ensure_ascii=False, sort_keys=True) + "\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.link(temporary, path)
    finally:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        if args.max_iterations < 1 or args.batch_size < 1:
            raise UserError("--max-iterations 和 --batch-size 必须是正整数")
        args.gateway = _url(args.gateway, "--gateway")
        base_url, model_key = _secret(args)  # validate credentials before any network or writes
        gateway_key = _gateway_key()
        catalog = _catalog(pathlib.Path(args.catalog))
        cases = _cases(pathlib.Path(args.input), catalog, args.benchmark_variant)
        output = pathlib.Path(args.output).absolute()
        artifacts = pathlib.Path(args.artifacts_dir).absolute()
        if output.exists() or output.is_symlink():
            raise UserError(f"--output 已存在：{output}")
        _gateway_rows(args.gateway, gateway_key, cases, catalog)
        _secure_artifact_root(artifacts)
        with concurrent.futures.ThreadPoolExecutor(max_workers=args.batch_size) as executor:
            futures = [
                executor.submit(_run_case, index, case, args, base_url, model_key, gateway_key, artifacts)
                for index, case in enumerate(cases)
            ]
            results = [future.result() for future in futures]
        _write_results(output, results)
        failed = sum(result["status"] != "completed" for result in results)
        print(f"SWE batch completed: total={len(results)} failed={failed}")
        print(f"results: {output}")
        print(f"artifacts: {artifacts}")
        return 1 if failed else 0
    except (UserError, OSError, ValueError) as exc:
        print(f"错误：{exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
