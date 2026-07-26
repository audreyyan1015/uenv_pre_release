#!/usr/bin/env python3
"""Verify that the read-only legacy source snapshot has not changed."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def verify(snapshot_path: Path) -> list[str]:
    document = json.loads(snapshot_path.read_text(encoding="utf-8"))
    source_root = Path(document["source_root"])
    mismatches: list[str] = []
    for relative, expected in sorted(document["files_sha256"].items()):
        path = source_root / relative
        if not path.is_file():
            mismatches.append(f"missing:{relative}")
            continue
        actual = sha256_file(path)
        if actual != expected:
            mismatches.append(f"sha256:{relative}:{expected}:{actual}")
    return mismatches


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--snapshot",
        type=Path,
        default=Path(__file__).resolve().parents[2] / "SOURCE_SNAPSHOT.json",
    )
    args = parser.parse_args()
    mismatches = verify(args.snapshot)
    if mismatches:
        for mismatch in mismatches:
            print(mismatch)
        return 1
    print("LEGACY_SOURCE_SNAPSHOT=PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
