#!/usr/bin/env python3
"""Build a repo-complete SWE-smith EnvPackage from the HuggingFace parquet shards.

SWE-smith builds one Docker image per *repository* and expresses each task as a
bug-introducing patch applied inside it, so a repository is the smallest unit that
can be made complete without leaving dangling image references. This script takes a
list of repositories, emits every instance belonging to them (no sampling), and
writes the image manifest that pins the exact tarball each instance needs.

The full dataset is 59136 instances across 222 images (~290 GB of image bytes),
which does not fit on the Hub host; a repository-scoped package does, and stays
complete on its own terms: every instance it lists is runnable from bytes the Hub
itself serves.

Outputs (compact JSON — pretty-printing this data inflates it by ~6x):

    catalog.json           instance_id -> instance record, Worker's lookup table
    images.manifest.json   the images the catalog references, with tar digests
    eval_spec.json         grader + workspace contract
    worker.overlay.yaml    benchmark_variant / grader / image pull policy
    package.summary.json   counts used by the publish step and the report
"""

from __future__ import annotations

import argparse
import glob
import hashlib
import json
import os
import sys
from pathlib import Path

try:
    import pyarrow.parquet as pq
except ImportError:  # pragma: no cover - surfaced as a runtime hint
    sys.exit("pyarrow is required: pip install pyarrow")

COLUMNS = [
    "instance_id",
    "patch",
    "FAIL_TO_PASS",
    "PASS_TO_PASS",
    "image_name",
    "repo",
    "problem_statement",
]

OFFICIAL_SWESMITH_PREFIX = "swebench/swesmith."


def normalize_image(image: str) -> str:
    """Emit the official SWE-smith repository image reference."""
    value = str(image or "").strip()
    if "/swesmith." in value:
        value = value[value.index("swesmith.") :]
    if not value.startswith("swesmith."):
        raise ValueError(f"invalid SWE-smith image: {image!r}")
    value = f"{OFFICIAL_SWESMITH_PREFIX}{value[len('swesmith.') :]}"
    return value.removesuffix(":latest")


def sha256_file(path: Path, chunk: int = 8 << 20) -> tuple[str, int]:
    """Digest and size of a file, streamed so multi-GB tarballs stay off the heap."""
    h = hashlib.sha256()
    total = 0
    with path.open("rb") as fh:
        while True:
            block = fh.read(chunk)
            if not block:
                break
            h.update(block)
            total += len(block)
    return f"sha256:{h.hexdigest()}", total


def tar_name_for(image: str) -> str:
    """The tarball filename `docker save` output is stored under."""
    return image.replace("swebench/", "").replace(":", "_").replace("/", "_") + ".tar"


def build(args: argparse.Namespace) -> int:
    repos = [r.strip() for r in Path(args.repos).read_text().split() if r.strip()]
    if not repos:
        sys.exit(f"no repositories listed in {args.repos}")
    wanted = set(repos)

    out_dir = Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)

    catalog: dict[str, dict] = {}
    images: dict[str, str] = {}  # repo -> image
    empty_statements = 0

    shards = sorted(glob.glob(os.path.join(args.parquet, "*.parquet")))
    if not shards:
        sys.exit(f"no parquet shards under {args.parquet}")

    for shard in shards:
        table = pq.read_table(shard, columns=COLUMNS)
        cols = {name: table.column(name).to_pylist() for name in COLUMNS}
        for i, repo in enumerate(cols["repo"]):
            if repo not in wanted:
                continue
            image = normalize_image(cols["image_name"][i])
            images.setdefault(repo, image)
            statement = cols["problem_statement"][i] or ""
            if not statement.strip():
                empty_statements += 1
                if args.require_problem_statement:
                    continue
            instance_id = cols["instance_id"][i]
            # Field names and empty-string conventions mirror config/swe/smith-smoke.json
            # so the Worker's existing SWE dataset loader needs no special case.
            catalog[instance_id] = {
                "instance_id": instance_id,
                "repo": repo,
                "version": "smith",
                "base_commit": "",
                "environment_setup_commit": "",
                "problem_statement": statement,
                "patch": cols["patch"][i] or "",
                "test_patch": "",
                "FAIL_TO_PASS": list(cols["FAIL_TO_PASS"][i] or []),
                "PASS_TO_PASS": list(cols["PASS_TO_PASS"][i] or []),
                "benchmark_variant": "smith",
                "image_cache_key": f"{image}:latest",
                "test_cmd": None,
                "install_cmd": "pip install -e . -q",
            }

    missing = wanted - set(images)
    if missing:
        sys.exit(f"repositories absent from the dataset: {sorted(missing)}")

    # Image manifest: every image the catalog references, pinned to the tarball the
    # Hub will serve. A missing tar is fatal — publishing a catalog whose images
    # cannot be loaded is exactly the "complete on paper only" failure this avoids.
    tar_dir = Path(args.tars)
    image_rows = []
    total_tar_bytes = 0
    for repo in sorted(images):
        image = normalize_image(images[repo])
        tar = tar_dir / tar_name_for(image)
        if not tar.is_file():
            sys.exit(f"image tarball missing for {image}: {tar}")
        digest, size = sha256_file(tar)
        total_tar_bytes += size
        image_rows.append(
            {
                "repo": repo,
                "image": f"{image}:latest",
                "tar_name": tar.name,
                "tar_sha256": digest,
                "tar_size_bytes": size,
                "instances": sum(1 for v in catalog.values() if v["repo"] == repo),
            }
        )
        print(f"  {repo:<48} {size / 1048576:8.0f} MiB  {image_rows[-1]['instances']:>5} 条实例")

    (out_dir / "catalog.json").write_text(
        json.dumps(catalog, ensure_ascii=False, separators=(",", ":"))
    )
    (out_dir / "images.manifest.json").write_text(
        json.dumps(
            {
                "variant": "smith",
                "source": "https://huggingface.co/datasets/SWE-bench/SWE-smith",
                "image_namespace": f"{OFFICIAL_SWESMITH_PREFIX}x86_64.*",
                "hosted_by_hub": True,
                "images": image_rows,
            },
            ensure_ascii=False,
            indent=2,
        )
    )
    (out_dir / "eval_spec.json").write_text(
        json.dumps(
            {
                "grader": "swesmith",
                "workspace_dir": "/testbed",
                "log_parser": "pytest",
                "variant": "smith",
                "install_cmd": "pip install -e . -q",
                "scoring": "FAIL_TO_PASS 全部转通过且 PASS_TO_PASS 不回归，记 resolved",
            },
            ensure_ascii=False,
            indent=2,
        )
    )
    (out_dir / "worker.overlay.yaml").write_text(
        "swe:\n"
        "  benchmark_variant: smith\n"
        "  grader: swesmith\n"
        "  workspace_dir: /testbed\n"
        # Images ship with the package, so the Worker must never reach a registry.
        "  image_pull_policy: local_only\n"
        "  load_images_from_package: true\n"
    )

    summary = {
        "package_scope": "repo-complete",
        "repos": len(images),
        "instances": len(catalog),
        "instances_with_problem_statement": len(catalog) - (0 if args.require_problem_statement else empty_statements),
        "instances_without_problem_statement": 0 if args.require_problem_statement else empty_statements,
        "images": len(image_rows),
        "image_tar_bytes": total_tar_bytes,
        "catalog_bytes": (out_dir / "catalog.json").stat().st_size,
    }
    (out_dir / "package.summary.json").write_text(json.dumps(summary, ensure_ascii=False, indent=2))

    print()
    print(f"仓库 {summary['repos']} · 实例 {summary['instances']} · 镜像 {summary['images']}")
    print(f"catalog {summary['catalog_bytes'] / 1048576:.1f} MiB · 镜像 tar {total_tar_bytes / 1073741824:.2f} GiB")
    print(f"有效题面 {summary['instances_with_problem_statement']}，空题面 {summary['instances_without_problem_statement']}")
    return 0


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--parquet", required=True, help="directory of SWE-smith parquet shards")
    p.add_argument("--tars", required=True, help="directory of `docker save` tarballs")
    p.add_argument("--repos", required=True, help="file listing one repository per line")
    p.add_argument("--out", required=True, help="output package directory")
    p.add_argument(
        "--require-problem-statement",
        action="store_true",
        help="drop instances with an empty problem statement instead of keeping them",
    )
    return build(p.parse_args())


if __name__ == "__main__":
    raise SystemExit(main())
