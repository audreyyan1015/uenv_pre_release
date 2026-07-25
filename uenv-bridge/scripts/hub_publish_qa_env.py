#!/usr/bin/env python3
"""在 Hub 注册/发布 qa 环境（镜像 math 的 manifest）。

在能访问 Hub 且已 export UENV_HUB_TOKEN 的机器上运行（如 7143）：
    set -a; source /root/.uenv-worker.env; set +a
    python3 hub_publish_qa_env.py [hub_base]
默认 hub_base = http://8.130.95.176:8088。幂等：env 已存在则跳过 create；version 已存在返回 409 时视为已发布。
"""

from __future__ import annotations

import json
import os
import sys
import urllib.request
import urllib.error

HUB = sys.argv[1] if len(sys.argv) > 1 else "http://8.130.95.176:8088"
TOKEN = os.environ.get("UENV_HUB_TOKEN", "")
SRC_ENV = "math"
DST_ENV = "qa"

if not TOKEN:
    print("ERROR: UENV_HUB_TOKEN not set", file=sys.stderr)
    raise SystemExit(2)


def req(method: str, path: str, body: dict | None = None) -> tuple[int, dict | str]:
    url = f"{HUB}{path}"
    data = json.dumps(body).encode() if body is not None else None
    r = urllib.request.Request(url, data=data, method=method)
    r.add_header("Authorization", f"Bearer {TOKEN}")
    if data is not None:
        r.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(r, timeout=15) as resp:
            raw = resp.read().decode()
            try:
                return resp.status, json.loads(raw)
            except Exception:
                return resp.status, raw
    except urllib.error.HTTPError as e:
        raw = e.read().decode()
        try:
            return e.code, json.loads(raw)
        except Exception:
            return e.code, raw


def main() -> int:
    # 1) fetch math manifest as template
    st, math_manifest = req("GET", f"/api/v1/envs/{SRC_ENV}/versions/latest")
    if st != 200 or not isinstance(math_manifest, dict):
        print(f"ERROR fetch {SRC_ENV} manifest: {st} {math_manifest}", file=sys.stderr)
        return 1

    # 2) create qa env (idempotent)
    create_body = {
        "env_type": DST_ENV,
        "namespace": "default",
        "description": "Single-turn QA / classification verification environment (reused math scoring)",
        "author": "uenv-team",
        "license": "Apache-2.0",
        "tags": ["qa", "reasoning", "validation", "single-turn"],
    }
    st, resp = req("POST", "/api/v1/envs", create_body)
    if st in (200, 201):
        print(f"create_env qa: OK ({st})")
    elif st == 409:
        print("create_env qa: already exists (409), continue")
    else:
        print(f"ERROR create_env qa: {st} {resp}", file=sys.stderr)
        return 1

    # 3) publish qa version, mirroring math manifest fields
    publish_body = {
        "version": math_manifest.get("version", "0.2.0"),
        "changelog": "qa v{}: 由 math 更名而来的单轮问答/分类验证环境；plugins/qa 复用 math 判分（按 dataset 路由）。".format(
            math_manifest.get("version", "0.2.0")
        ),
        "base_image": None,
        "health_check_path": math_manifest.get("health_check_path", "/health"),
        "entrypoint": math_manifest.get("entrypoint", "./run.sh"),
        "supported_backends": math_manifest.get("supported_backends", ["process"]),
        "config_schema": math_manifest.get("config_schema"),
        "default_config": math_manifest.get("default_config"),
        "resources": math_manifest.get("resources", {}),
        "interface": math_manifest.get("interface", {}),
        "examples": math_manifest.get("examples", []),
        "min_uenv_version": math_manifest.get("min_uenv_version"),
    }
    st, resp = req("POST", f"/api/v1/envs/{DST_ENV}/versions", publish_body)
    if st in (200, 201):
        print(f"publish_version qa: OK ({st})")
    elif st == 409:
        print("publish_version qa: version already exists (409), continue")
    else:
        print(f"ERROR publish_version qa: {st} {resp}", file=sys.stderr)
        return 1

    # 4) verify
    st, resp = req("GET", f"/api/v1/envs/{DST_ENV}/versions/latest")
    ok = st == 200 and isinstance(resp, dict) and resp.get("env_type") == DST_ENV
    print(json.dumps({"verify_status": st, "env_type": resp.get("env_type") if isinstance(resp, dict) else None, "version": resp.get("version") if isinstance(resp, dict) else None, "ok": ok}, ensure_ascii=False))
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
