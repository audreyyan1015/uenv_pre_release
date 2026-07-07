#!/usr/bin/env python3
"""Resumable snapshot_download wrapper; prefers classic HF CDN over xet CAS."""
from __future__ import annotations

import os
import sys

os.environ.setdefault("HF_HUB_ENABLE_HF_TRANSFER", "0")
os.environ.setdefault("HF_HUB_DISABLE_XET", "1")
os.environ.setdefault("HF_ENDPOINT", "https://hf-mirror.com")
os.environ.setdefault("HF_HOME", "/data/huggingface")

from huggingface_hub import snapshot_download  # noqa: E402


def main() -> int:
    repo = os.environ.get("UENV_MODEL_REPO", "cognitivecomputations/DeepSeek-V3-0324-AWQ")
    local_dir = os.environ.get("UENV_MODEL_DIR", "/data/models/DeepSeek-V3-0324-AWQ")
    endpoint = os.environ.get("HF_ENDPOINT", "https://hf-mirror.com")
    token = os.environ.get("HF_TOKEN") or os.environ.get("HUGGING_FACE_HUB_TOKEN")

    print(
        f"snapshot_download repo={repo} local_dir={local_dir} endpoint={endpoint} "
        f"hf_transfer={os.environ.get('HF_HUB_ENABLE_HF_TRANSFER')} "
        f"disable_xet={os.environ.get('HF_HUB_DISABLE_XET')}",
        flush=True,
    )
    snapshot_download(
        repo_id=repo,
        local_dir=local_dir,
        endpoint=endpoint,
        token=token,
        local_dir_use_symlinks=False,
        resume_download=True,
        max_workers=int(os.environ.get("HF_HUB_DOWNLOAD_MAX_WORKERS", "4")),
    )
    print("snapshot_download finished", flush=True)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:  # noqa: BLE001
        print(f"snapshot_download error: {exc}", file=sys.stderr, flush=True)
        raise
