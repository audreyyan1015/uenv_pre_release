#!/usr/bin/env python3
"""External Runtime Gateway 端到端演示（plan §5.3）。

模拟外部 Agent（OpenHands Remote Runtime 的最小形态）经 Worker L4 HTTP 网关：
  create session → (写入并应用 gold patch) → submit 评测 → reward → delete

仅用标准库（urllib），Worker 离线可跑。

用法:
  python3 scripts/swe_gateway_demo.py --endpoint 127.0.0.1:48999 \
      --instance scikit-learn__scikit-learn-14141 --instances fixtures/swe/swe_instances.json
  # 负向对照（不打 gold patch，应 reward=0）:
  python3 scripts/swe_gateway_demo.py ... --no-gold
"""
import argparse
import json
import sys
import urllib.request


def call(base, method, path, body=None, api_key=None):
    url = f"http://{base}{path}"
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("Content-Type", "application/json")
    if api_key:
        req.add_header("X-API-Key", api_key)
    with urllib.request.urlopen(req, timeout=600) as resp:
        raw = resp.read().decode()
    return json.loads(raw) if raw.strip() else {}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--endpoint", default="127.0.0.1:48999")
    ap.add_argument("--instance", required=True)
    ap.add_argument("--instances", default="fixtures/swe/swe_instances.json")
    ap.add_argument("--command-mode", default="FullShell")
    ap.add_argument("--api-key", default=None, help="X-API-Key（网关启用鉴权时必填）")
    ap.add_argument("--gold", dest="gold", action="store_true", default=True)
    ap.add_argument("--no-gold", dest="gold", action="store_false")
    args = ap.parse_args()
    key = args.api_key

    with open(args.instances) as f:
        catalog = json.load(f)
    if args.instance not in catalog:
        print(f"instance {args.instance} not in {args.instances}", file=sys.stderr)
        sys.exit(1)
    gold_patch = catalog[args.instance].get("patch", "")

    base = args.endpoint
    print(f"[1] POST /sessions  instance={args.instance} mode={args.command_mode}")
    created = call(base, "POST", "/runtime/v1/sessions", {
        "instance_id": args.instance,
        "benchmark_variant": "verified",
        "command_mode": args.command_mode,
    }, api_key=key)
    sid = created["session_id"]
    issue = created["observation"]["issue_text"]
    print(f"    session_id={sid}")
    print(f"    observation.issue_text[:160]={issue[:160]!r}")

    try:
        if args.gold and gold_patch.strip():
            print("[2] POST /write  (gold patch -> /tmp/gold.patch) + POST /exec (git apply)")
            call(base, "POST", f"/runtime/v1/sessions/{sid}/write", {
                "path": "/tmp/gold.patch", "content": gold_patch,
            }, api_key=key)
            applied = call(base, "POST", f"/runtime/v1/sessions/{sid}/exec", {
                "command": "cd /testbed && (git apply -v /tmp/gold.patch || patch --batch --fuzz=5 -p1 < /tmp/gold.patch)",
            }, api_key=key)
            print(f"    git apply exit_code={applied['exit_code']}")
        else:
            print("[2] (skip gold patch — negative control)")

        print(f"[3] POST /sessions/{sid}/submit  (apply test_patch + run tests + grade)")
        result = call(base, "POST", f"/runtime/v1/sessions/{sid}/submit", api_key=key)
        print(f"    resolved={result['resolved']} reward={result['reward']} "
              f"tests={result['tests_passed']}/{result['tests_total']}")
        for t in result["per_test"]:
            print(f"      [{'PASS' if t['passed'] else 'FAIL'}] {t['node_id']}")
    finally:
        print(f"[4] DELETE /sessions/{sid}")
        call(base, "DELETE", f"/runtime/v1/sessions/{sid}", api_key=key)

    print(f"==== Gateway episode 完成：reward = {result['reward']} ====")


if __name__ == "__main__":
    main()
