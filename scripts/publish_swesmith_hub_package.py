#!/usr/bin/env python3
"""Publish a repo-complete SWE-smith EnvPackage from Hub-local staging paths.

The catalog (~0.5 GiB) and image tarballs (~14 GiB) are already on the Hub host.
This script builds a PublishPackageRequest that stages them via `file_artifacts`
(streamed sha256, no RAM buffering of the whole blob) and posts it to
`POST /api/v1/packages/{package_id}/versions`.

Environment:
  UENV_HUB_ENDPOINT   default http://127.0.0.1:8088
  UENV_HUB_TOKEN      publisher-or-admin bearer token (required)
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.error
import urllib.request
from pathlib import Path


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--staging", required=True, help="directory with package/ + tars/")
    p.add_argument("--package-id", default="swe-bench-smith")
    p.add_argument("--version", default="0.2.0")
    p.add_argument(
        "--endpoint",
        default=os.environ.get("UENV_HUB_ENDPOINT", "http://127.0.0.1:8088"),
    )
    p.add_argument("--token", default=os.environ.get("UENV_HUB_TOKEN", ""))
    args = p.parse_args()
    if not args.token:
        sys.exit("UENV_HUB_TOKEN / --token required")

    staging = Path(args.staging)
    pkg = staging / "package" if (staging / "package").is_dir() else staging
    tars = staging / "tars" if (staging / "tars").is_dir() else staging / "tars"
    # Allow either layout: staging/{catalog,…,tars/} or staging/package + staging/tars
    if not (pkg / "catalog.json").is_file():
        pkg = staging
    if not tars.is_dir():
        tars = staging / "tars"
    summary = json.loads((pkg / "package.summary.json").read_text())
    images_manifest = json.loads((pkg / "images.manifest.json").read_text())

    file_artifacts = [
        {
            "name": "catalog.json",
            "kind": "catalog",
            "sync_mode": "inline",
            "media_type": "application/json",
            "target_rel_path": "catalog.json",
            "local_path": str((pkg / "catalog.json").resolve()),
        }
    ]
    for im in images_manifest["images"]:
        tar = tars / im["tar_name"]
        if not tar.is_file():
            sys.exit(f"missing tar: {tar}")
        file_artifacts.append(
            {
                "name": im["tar_name"],
                "kind": "image_tar",
                "sync_mode": "inline",
                "media_type": "application/x-tar",
                "target_rel_path": f"images/{im['tar_name']}",
                "local_path": str(tar.resolve()),
            }
        )

    overlay = {
        "swe": {
            "benchmark_variant": "smith",
            "command_mode": "FullShell",
            "grader": "swesmith",
            "image_pull_policy": "local_only",
            "load_images_from_package": True,
            "workspace_dir": "/testbed",
            "instance_count": summary["instances"],
            "repo_count": summary["repos"],
            "image_count": summary["images"],
            "package_scope": "repo-complete",
        },
        "instance_count": summary["instances"],
        "runtime_gateway": {"enabled": True},
        "trajectory": {"enabled": True, "artifact_dir": "/var/lib/uenv/trajectories"},
    }
    eval_spec = json.loads((pkg / "eval_spec.json").read_text())

    body = {
        "version": args.version,
        "publisher": "org-uenv-swe",
        "description": (
            f"SWE-smith repo-complete EnvPackage — {summary['repos']} repositories, "
            f"{summary['instances']} instances, {summary['images']} Hub-hosted image tars "
            f"({summary['image_tar_bytes'] / (1 << 30):.1f} GiB). "
            "Every listed instance is runnable from bytes this Hub serves (zero egress)."
        ),
        "changelog": (
            "Promote smith from smoke (5 instances, no image tar) to a repo-complete "
            "benchmark: 10 Python repositories, full instance set, docker-save tarballs "
            "staged on the Hub for Worker docker load."
        ),
        "platform": {
            "uenv_worker_min": "0.1.0",
            "features": [
                "runtime_gateway",
                "swe_instance_pool",
                "trajectory_v2_2",
                "hub_hosted_image_tar",
            ],
            "consumers": ["worker"],
        },
        "worker_overlay": overlay,
        "agent_defaults": {
            "driver_entrypoint": "run_swesmith_official.py",
            "workspace_dir": "/testbed",
            "tools": ["terminal", "file_editor"],
            "max_iterations_default": 30,
            "agent_bridge_id": "uenv-agent-openhands",
            "agent_bridge_version": "1.0.0",
        },
        "contracts": {
            "runtime_gateway_api": "runtime/v1",
            "trajectory_bundle_schema": "v2.2",
            "tool_bridge_schema": "openhands-uenv-v1",
        },
        "artifacts": [
            {
                "name": "images.manifest.json",
                "kind": "images",
                "sync_mode": "inline",
                "media_type": "application/json",
                "target_rel_path": "images.manifest.json",
                "content": json.dumps(images_manifest, ensure_ascii=False, indent=2),
            },
            {
                "name": "eval_spec.json",
                "kind": "eval_spec",
                "sync_mode": "inline",
                "media_type": "application/json",
                "target_rel_path": "eval_spec.json",
                "content": json.dumps(eval_spec, ensure_ascii=False, indent=2),
            },
            {
                "name": "worker.overlay.yaml",
                "kind": "overlay",
                "sync_mode": "inline",
                "media_type": "application/yaml",
                "target_rel_path": "worker.overlay.yaml",
                "content": json.dumps(overlay, ensure_ascii=False, indent=2),
            },
        ],
        "file_artifacts": file_artifacts,
    }

    url = f"{args.endpoint.rstrip('/')}/api/v1/packages/{args.package_id}/versions"
    data = json.dumps(body).encode("utf-8")
    print(
        f"POST {url}  package={args.package_id}@{args.version}  "
        f"file_artifacts={len(file_artifacts)}  body={len(data)/1048576:.1f} MiB"
    )
    req = urllib.request.Request(
        url,
        data=data,
        method="POST",
        headers={
            "Authorization": f"Bearer {args.token}",
            "Content-Type": "application/json",
            "Accept": "application/json",
        },
    )
    try:
        with urllib.request.urlopen(req, timeout=7200) as resp:
            payload = json.load(resp)
            print("status", resp.status)
    except urllib.error.HTTPError as e:
        err = e.read().decode("utf-8", "replace")
        print("HTTP", e.code, err[:2000], file=sys.stderr)
        return 1

    arts = payload.get("manifest", payload).get("artifacts", [])
    total = sum(a.get("size_bytes") or 0 for a in arts)
    print(
        f"published {args.package_id}@{payload.get('manifest', payload).get('version', args.version)} "
        f"artifacts={len(arts)} bytes={total / (1 << 30):.2f} GiB"
    )
    tars_n = sum(1 for a in arts if a.get("kind") == "image_tar")
    print(f"image_tar count={tars_n}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
