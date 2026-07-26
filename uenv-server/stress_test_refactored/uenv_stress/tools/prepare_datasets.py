#!/usr/bin/env python3
"""Freeze and validate official datasets used by the UEnv stability suite."""

from __future__ import annotations

import argparse
import hashlib
import json
import random
import re
import shutil
import subprocess
import tempfile
import time
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any


OFFICIAL_SOURCES = {
    "swebench_pro": "https://huggingface.co/datasets/ScaleAI/SWE-bench_Pro",
    "olymmath": "https://github.com/RUCAIBox/OlymMATH.git",
    "scitab": "https://github.com/XinyuanLu00/SciTab.git",
    "pubmedqa": "https://github.com/pubmedqa/pubmedqa.git",
}
OLYMMATH_FILES = (
    "OlymMATH-EN-EASY.jsonl", "OlymMATH-EN-HARD.jsonl",
    "OlymMATH-ZH-EASY.jsonl", "OlymMATH-ZH-HARD.jsonl",
)
DSCODEBENCH_REQUIRED_FIELDS = {
    "problem_id", "library", "code_problem", "ground_truth_code", "test_script",
}


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def read_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def read_jsonl(path: Path) -> list[dict[str, Any]]:
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]


def find_one(root: Path, name: str) -> Path:
    matches = [path for path in root.rglob(name) if path.is_file()]
    if len(matches) != 1:
        raise ValueError(f"expected exactly one {name} under {root}, found {len(matches)}")
    return matches[0]


def clone_official(url: str, target: Path) -> str:
    subprocess.run(["git", "clone", "--filter=blob:none", "--depth", "1", url, str(target)], check=True)
    return subprocess.check_output(["git", "-C", str(target), "rev-parse", "HEAD"], text=True).strip()


def acquire_online(staging: Path) -> dict[str, str]:
    revisions: dict[str, str] = {}
    try:
        from huggingface_hub import HfApi, snapshot_download  # type: ignore
    except ImportError as exc:
        raise RuntimeError("online mode requires huggingface_hub; use --source-dir for offline import") from exc
    swe = staging / "sources" / "swebench_pro"
    swe.mkdir(parents=True)
    snapshot = Path(snapshot_download(
        repo_id="ScaleAI/SWE-bench_Pro", repo_type="dataset",
        allow_patterns=["*.parquet", "**/*.parquet"], local_dir=swe
    ))
    revisions["swebench_pro"] = str(HfApi().dataset_info("ScaleAI/SWE-bench_Pro").sha)
    for name, url in OFFICIAL_SOURCES.items():
        if name == "swebench_pro":
            continue
        target = staging / "sources" / name
        revisions[name] = clone_official(url, target)
    return revisions


def sha256_tree(root: Path) -> str:
    digest = hashlib.sha256()
    for path in sorted(item for item in root.rglob("*") if item.is_file() and ".git" not in item.parts):
        digest.update(path.relative_to(root).as_posix().encode())
        digest.update(bytes.fromhex(sha256_file(path)))
    return digest.hexdigest()


def import_sources(source_dir: Path, staging: Path) -> dict[str, str]:
    revisions_path = source_dir / "source_revisions.json"
    if not revisions_path.is_file():
        raise ValueError("offline --source-dir must contain source_revisions.json with frozen commit/revision IDs")
    revisions = read_json(revisions_path)
    if set(revisions) != set(OFFICIAL_SOURCES) or not all(str(value).strip() for value in revisions.values()):
        raise ValueError(f"source_revisions.json must contain {sorted(OFFICIAL_SOURCES)}")
    for name, value in revisions.items():
        if not re.fullmatch(r"[0-9a-fA-F]{40,64}", str(value)):
            raise ValueError(f"{name} source revision must be a full commit/snapshot hash")
    target = staging / "sources"
    target.mkdir(parents=True)
    for name in OFFICIAL_SOURCES:
        source = source_dir / name
        if not source.is_dir():
            raise ValueError(f"offline source missing directory: {source}")
        shutil.copytree(source, target / name)
    return {name: str(value) for name, value in revisions.items()}


def require_fields(item: dict[str, Any], fields: tuple[str, ...], label: str) -> None:
    missing = [field for field in fields if item.get(field) in (None, "", [])]
    if missing:
        raise ValueError(f"{label} missing required fields {missing}")


def select_swe_instances(catalog: dict[str, dict[str, Any]], count: int, seed: int) -> list[str]:
    """Deterministically round-robin repository/language buckets for diversity."""
    rng = random.Random(seed)
    buckets: dict[tuple[str, str], list[dict[str, Any]]] = defaultdict(list)
    for row in catalog.values():
        buckets[(str(row["repo"]), str(row["repo_language"]))].append(row)
    for rows in buckets.values():
        rng.shuffle(rows)
    keys = sorted(buckets)
    rng.shuffle(keys)
    selected: list[str] = []
    while len(selected) < count:
        progressed = False
        for key in keys:
            rows = buckets[key]
            if not rows:
                continue
            selected.append(str(rows.pop()["instance_id"]))
            progressed = True
            if len(selected) == count:
                break
        if not progressed:
            break
    if len(selected) < count:
        raise ValueError(f"SWE-bench Pro catalog has only {len(selected)} selectable instances; needs {count}")
    return selected


def prepare_swe(source: Path, target: Path, seed: int, image_repository: str) -> dict[str, Any]:
    try:
        import pyarrow.parquet as pq  # type: ignore
    except ImportError as exc:
        raise RuntimeError("SWE-bench Pro conversion requires pyarrow") from exc
    parquet_files = sorted(source.rglob("*test*.parquet"))
    if not parquet_files:
        parquet_files = sorted(source.rglob("*.parquet"))
    if not parquet_files:
        raise ValueError(f"no SWE-bench Pro test parquet files under {source}")
    tables = [pq.read_table(path) for path in parquet_files]
    if len(tables) > 1:
        import pyarrow as pa  # type: ignore
        try:
            table = pa.concat_tables(tables, promote_options="default")
        except TypeError:
            # Compatibility with the older PyArrow used by some pressure-test hosts.
            table = pa.concat_tables(tables, promote=True)
    else:
        table = tables[0]
    rows = table.to_pylist()
    if len(rows) < 50:
        raise ValueError(f"SWE-bench Pro test split has only {len(rows)} rows")
    catalog: dict[str, dict[str, Any]] = {}
    for index, raw in enumerate(rows):
        row = {str(key): value for key, value in raw.items()}
        if not row.get("repo_language"):
            row["repo_language"] = row.get("language")
        raw_image = str(row.get("image_cache_key") or row.get("docker_image") or "").strip()
        dockerhub_tag = str(
            row.get("dockerhub_tag") or row.get("dockerhub_image_name") or row.get("image_name") or ""
        ).strip()
        if not dockerhub_tag and raw_image:
            dockerhub_tag = raw_image.rsplit(":", 1)[-1]
        row["dockerhub_tag"] = dockerhub_tag
        require_fields(
            row,
            ("instance_id", "repo", "base_commit", "problem_statement", "repo_language", "dockerhub_tag"),
            f"SWE-bench Pro row {index}",
        )
        instance_id = str(row["instance_id"])
        if instance_id in catalog:
            raise ValueError(f"duplicate SWE-bench Pro instance_id {instance_id}")
        row["benchmark_variant"] = "pro"
        row["version"] = "pro"
        row["image_cache_key"] = raw_image or f"{image_repository}:{dockerhub_tag}"
        catalog[instance_id] = row
    selected = select_swe_instances(catalog, 50, seed)
    target.mkdir(parents=True)
    pq.write_table(table, target / "test.parquet")
    (target / "catalog.json").write_text(json.dumps(catalog, indent=2, sort_keys=True), encoding="utf-8")
    (target / "swebench_pro_instances.json").write_text(json.dumps(selected, indent=2), encoding="utf-8")
    (target / "required_images.txt").write_text(
        "\n".join(str(catalog[item]["dockerhub_tag"]) for item in selected) + "\n", encoding="utf-8"
    )
    return {"sample_count": len(catalog), "selected_count": len(selected), "labels": {}}


def prepare_olymmath(source: Path, target: Path) -> dict[str, Any]:
    target.mkdir(parents=True)
    seen: set[str] = set()
    total = 0
    for name in OLYMMATH_FILES:
        path = find_one(source, name)
        rows = read_jsonl(path)
        if len(rows) != 100:
            raise ValueError(f"{name} must contain exactly 100 rows, got {len(rows)}")
        for index, row in enumerate(rows):
            require_fields(row, ("unique_id", "problem", "answer"), f"{name}:{index + 1}")
            unique_id = str(row["unique_id"])
            if unique_id in seen:
                raise ValueError(f"duplicate OlymMATH unique_id {unique_id}")
            seen.add(unique_id)
        shutil.copy2(path, target / name)
        total += len(rows)
    return {"sample_count": total, "labels": {}}


def normalize_label(value: Any) -> str:
    return str(value).strip().lower().replace("_", " ")


def prepare_scitab(source: Path, target: Path) -> dict[str, Any]:
    path = find_one(source, "sci_tab.json")
    value = read_json(path)
    rows = value if isinstance(value, list) else list(value.values())
    if len(rows) < 100:
        raise ValueError(f"SciTab must contain at least 100 rows, got {len(rows)}")
    seen: set[str] = set()
    labels: Counter[str] = Counter()
    allowed = {"supports", "refutes", "not enough info"}
    for index, row in enumerate(rows):
        require_fields(row, ("id", "claim", "table_content_values", "label"), f"SciTab row {index}")
        row_id = str(row["id"])
        if row_id in seen:
            raise ValueError(f"duplicate SciTab id {row_id}")
        label = normalize_label(row["label"])
        if label not in allowed:
            raise ValueError(f"invalid SciTab label {label!r}")
        seen.add(row_id)
        labels[label] += 1
    target.mkdir(parents=True)
    shutil.copy2(path, target / "sci_tab.json")
    return {"sample_count": len(rows), "labels": dict(labels)}


def prepare_pubmedqa(source: Path, target: Path) -> dict[str, Any]:
    path = find_one(source, "ori_pqal.json")
    document = read_json(path)
    if not isinstance(document, dict) or len(document) < 1000:
        raise ValueError(f"PubMedQA PQA-L must contain at least 1000 keyed rows")
    labels: Counter[str] = Counter()
    for pmid, row in document.items():
        require_fields(row, ("QUESTION", "CONTEXTS", "final_decision"), f"PubMedQA {pmid}")
        label = normalize_label(row["final_decision"])
        if label not in {"yes", "no", "maybe"}:
            raise ValueError(f"invalid PubMedQA answer {label!r}")
        labels[label] += 1
    target.mkdir(parents=True)
    shutil.copy2(path, target / "ori_pqal.json")
    return {"sample_count": len(document), "labels": dict(labels)}


def prepare_dscodebench(source: Path, target: Path) -> dict[str, Any]:
    """Freeze the operator-supplied DSCodeBench JSONL into the shared manifest."""
    if not source.is_file():
        raise ValueError(f"DSCodeBench JSONL does not exist: {source}")
    seen: set[str] = set()
    count = 0
    with source.open("r", encoding="utf-8") as stream:
        for line_number, line in enumerate(stream, 1):
            if not line.strip():
                continue
            row = json.loads(line)
            missing = sorted(DSCODEBENCH_REQUIRED_FIELDS - set(row))
            if missing:
                raise ValueError(f"{source}:{line_number} missing required fields {missing}")
            problem_id = str(row["problem_id"])
            if problem_id in seen:
                raise ValueError(f"{source}:{line_number} duplicate problem_id {problem_id}")
            seen.add(problem_id)
            count += 1
    if count < 100:
        raise ValueError(f"DSCodeBench must contain at least 100 rows, got {count}")
    target.mkdir(parents=True)
    destination = target / "DSCodeBench.json"
    shutil.copy2(source, destination)
    return {
        "sample_count": count,
        "labels": {},
        "source_kind": "operator_supplied_jsonl",
        "source_sha256": sha256_file(source),
    }


def build_manifest(root: Path, revisions: dict[str, str], stats: dict[str, Any]) -> dict[str, Any]:
    files = []
    for path in sorted(item for item in root.rglob("*") if item.is_file() and item.name != "dataset_manifest.json"):
        files.append({
            "path": path.relative_to(root).as_posix(),
            "size_bytes": path.stat().st_size,
            "sha256": sha256_file(path),
        })
    return {
        "schema_version": 1,
        "created_unix": time.time(),
        "official_sources": OFFICIAL_SOURCES,
        "source_revisions": revisions,
        "loader_version": "uenv-stability-v1",
        "datasets": stats,
        "files": files,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--source-dir", type=Path)
    parser.add_argument(
        "--dscodebench-jsonl",
        type=Path,
        required=True,
        help="Existing real DSCodeBench JSONL to freeze into dataset_manifest.json",
    )
    parser.add_argument("--seed", type=int, default=20260722)
    parser.add_argument("--swe-image-repository", default="jefzda/sweap-images")
    args = parser.parse_args()
    output = args.output_dir.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=f".{output.name}-", dir=output.parent) as temp:
        staging = Path(temp)
        revisions = import_sources(args.source_dir.resolve(), staging) if args.source_dir else acquire_online(staging)
        prepared = staging / "prepared"
        sources = staging / "sources"
        stats = {
            "dscodebench": prepare_dscodebench(
                args.dscodebench_jsonl.resolve(), prepared / "dscodebench"
            ),
            "swebench_pro": prepare_swe(
                sources / "swebench_pro", prepared / "swebench_pro", args.seed, args.swe_image_repository
            ),
            "olymmath": prepare_olymmath(sources / "olymmath", prepared / "olymmath"),
            "scitab": prepare_scitab(sources / "scitab", prepared / "scitab"),
            "pubmedqa": prepare_pubmedqa(sources / "pubmedqa", prepared / "pubmedqa"),
        }
        manifest = build_manifest(prepared, revisions, stats)
        (prepared / "dataset_manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True), encoding="utf-8")
        if output.exists():
            raise FileExistsError(f"refusing to replace existing dataset directory: {output}")
        prepared.replace(output)
    print(json.dumps({"output_dir": str(output), "manifest": str(output / "dataset_manifest.json")}, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
