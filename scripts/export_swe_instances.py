#!/usr/bin/env python3
"""Export SWE-bench_Verified parquet -> swe_instances.json for the Worker.

Worker 7143 无外网且无 datasets/pyarrow；本脚本在本地（有 pyarrow）将数据集行
导出为 Worker 可直接读取的 JSON，作为 InstanceSpec/TaskSpec + 评测真值的来源。

用法:
  python3 export_swe_instances.py <parquet> <out.json> [--ids id1,id2 | --repo-substr astropy --limit 5]
"""
import argparse
import json
import sys

import pyarrow.parquet as pq


def to_list(value):
    """FAIL_TO_PASS / PASS_TO_PASS 在数据集里是 JSON 字符串或 list。"""
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


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("parquet")
    ap.add_argument("out")
    ap.add_argument("--ids", default="", help="逗号分隔 instance_id 白名单")
    ap.add_argument("--repo-substr", default="", help="按 repo 子串筛选")
    ap.add_argument("--limit", type=int, default=0, help="最多导出条数 (0=全部)")
    ap.add_argument("--schema-only", action="store_true")
    args = ap.parse_args()

    table = pq.read_table(args.parquet)
    cols = table.column_names
    if args.schema_only:
        print("columns:", cols)
        print("rows:", table.num_rows)
        return

    id_filter = {x for x in args.ids.split(",") if x.strip()}
    rows = table.to_pylist()
    out = {}
    for r in rows:
        iid = r["instance_id"]
        if id_filter and iid not in id_filter:
            continue
        if args.repo_substr and args.repo_substr not in str(r.get("repo", "")):
            continue
        out[iid] = {
            "instance_id": iid,
            "repo": r.get("repo", ""),
            "version": str(r.get("version", "")),
            "base_commit": r.get("base_commit", ""),
            "environment_setup_commit": r.get("environment_setup_commit", ""),
            "problem_statement": r.get("problem_statement", ""),
            "patch": r.get("patch", ""),
            "test_patch": r.get("test_patch", ""),
            "FAIL_TO_PASS": to_list(r.get("FAIL_TO_PASS")),
            "PASS_TO_PASS": to_list(r.get("PASS_TO_PASS")),
        }
        if args.limit and len(out) >= args.limit:
            break

    with open(args.out, "w") as f:
        json.dump(out, f, ensure_ascii=False, indent=2)
    print(f"exported {len(out)} instances -> {args.out}")
    for iid in list(out)[:10]:
        e = out[iid]
        print(f"  {iid}  repo={e['repo']} ver={e['version']} "
              f"F2P={len(e['FAIL_TO_PASS'])} P2P={len(e['PASS_TO_PASS'])} "
              f"patch={len(e['patch'])}B test_patch={len(e['test_patch'])}B")


if __name__ == "__main__":
    main()
