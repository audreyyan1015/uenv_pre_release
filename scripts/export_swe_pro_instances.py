#!/usr/bin/env python3
"""Export SWE-bench Pro (ScaleAI/SWE-bench_Pro) -> config/swe/pro.json for Hub/Worker.

Maps HuggingFace `dockerhub_tag` to Worker `image_cache_key`:
  jefzda/sweap-images:{dockerhub_tag}

Usage:
  python3 scripts/export_swe_pro_instances.py --limit 1 --repo-language Python --out config/swe/pro.json
  python3 scripts/export_swe_pro_instances.py --ids instance_xxx --out config/swe/pro.json
"""
from __future__ import annotations

import argparse
import ast
import json
import sys


def to_list(value) -> list[str]:
    if value is None:
        return []
    if isinstance(value, list):
        return [str(v) for v in value]
    s = str(value).strip()
    if not s:
        return []
    try:
        parsed = json.loads(s)
        if isinstance(parsed, list):
            return [str(v) for v in parsed]
    except json.JSONDecodeError:
        pass
    try:
        parsed = ast.literal_eval(s)
        if isinstance(parsed, list):
            return [str(v) for v in parsed]
    except (ValueError, SyntaxError):
        pass
    return [s]


def parse_selected(value) -> list[str]:
    if value is None:
        return []
    if isinstance(value, list):
        return [str(v) for v in value]
    s = str(value).strip()
    if not s:
        return []
    try:
        parsed = json.loads(s)
        if isinstance(parsed, list):
            return [str(v) for v in parsed]
    except json.JSONDecodeError:
        pass
    return [s]


def row_to_instance(r: dict) -> dict:
    tag = (r.get("dockerhub_tag") or "").strip()
    if not tag:
        raise ValueError(f"missing dockerhub_tag for {r.get('instance_id')}")
    image = f"jefzda/sweap-images:{tag}"
    setup_cmd = (r.get("before_repo_set_cmd") or "").strip()
    sel_files = parse_selected(r.get("selected_test_files_to_run"))
    lang = (r.get("repo_language") or "").lower()
    if lang == "python" and sel_files:
        test_cmd = f"python -m pytest {' '.join(sel_files)} -v"
    elif lang in ("javascript", "typescript", "node", "nodejs", "js"):
        test_cmd = "npm test" if not sel_files else f"npm test -- {' '.join(sel_files)}"
    elif lang == "go":
        test_cmd = "go test ./... -v"
    elif sel_files:
        test_cmd = " ".join(sel_files)
    else:
        test_cmd = "echo no test_cmd"
    row = {
        "instance_id": r["instance_id"],
        "repo": r.get("repo", ""),
        "version": "pro",
        "base_commit": r.get("base_commit", ""),
        "benchmark_variant": "pro",
        "image_cache_key": image,
        "setup_cmd": setup_cmd or None,
        "test_cmd": test_cmd,
        "problem_statement": r.get("problem_statement", ""),
        "patch": r.get("patch", ""),
        "test_patch": r.get("test_patch", ""),
        "FAIL_TO_PASS": to_list(r.get("fail_to_pass")),
        "PASS_TO_PASS": to_list(r.get("pass_to_pass")),
    }
    if lang in ("javascript", "typescript", "node", "nodejs", "js"):
        row["pre_test_cmd"] = "redis-server --daemonize yes"
    return row


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default="config/swe/pro.json")
    ap.add_argument("--ids", default="", help="comma-separated instance_id whitelist")
    ap.add_argument("--repo-language", default="", help="filter e.g. Python|Go|JavaScript")
    ap.add_argument("--limit", type=int, default=1, help="max instances (default 1 for smoke)")
    args = ap.parse_args()

    try:
        from datasets import load_dataset
    except ImportError:
        print("pip install datasets", file=sys.stderr)
        return 1

    ds = load_dataset("ScaleAI/SWE-bench_Pro", split="test")
    id_filter = {x.strip() for x in args.ids.split(",") if x.strip()}
    lang_filter = args.repo_language.strip().lower()
    out: dict = {}
    for r in ds:
        iid = r["instance_id"]
        if id_filter and iid not in id_filter:
            continue
        if lang_filter and str(r.get("repo_language", "")).lower() != lang_filter:
            continue
        try:
            out[iid] = row_to_instance(r)
        except ValueError as e:
            print(f"skip {iid}: {e}", file=sys.stderr)
            continue
        if args.limit and len(out) >= args.limit:
            break

    if not out:
        print("no instances exported", file=sys.stderr)
        return 1

    with open(args.out, "w", encoding="utf-8") as f:
        json.dump(out, f, ensure_ascii=False, indent=2)
    print(f"exported {len(out)} pro instances -> {args.out}")
    for iid, row in out.items():
        print(f"  {iid}  image={row['image_cache_key']}  F2P={len(row['FAIL_TO_PASS'])}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
