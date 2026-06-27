#!/usr/bin/env python3
"""Entry point shim — see scripts/uenv-llm-gateway/uenv_llm_gateway.py."""
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT / "uenv-llm-gateway"))

from uenv_llm_gateway import main  # noqa: E402

if __name__ == "__main__":
    main()
