from __future__ import annotations

import argparse
import json
from pathlib import Path

from .clients import DryRunEpisodeClient
from .protocol import request_to_jsonable
from .verl import VeRLAdapter


def main() -> None:
    parser = argparse.ArgumentParser(prog="uenv-bridge")
    subparsers = parser.add_subparsers(dest="command", required=True)

    dry_run = subparsers.add_parser("dry-run-verl-json", help="Convert a simplified VeRL batch JSON to EpisodeRequest JSON.")
    dry_run.add_argument("--input", required=True, help="Path to simplified VeRL batch JSON.")
    dry_run.add_argument("--output", required=True, help="Path to write converted EpisodeRequest JSON.")

    args = parser.parse_args()
    if args.command == "dry-run-verl-json":
        input_path = Path(args.input)
        output_path = Path(args.output)
        batch = json.loads(input_path.read_text(encoding="utf-8"))
        adapter = VeRLAdapter(client=DryRunEpisodeClient(output_path.parent))
        requests = adapter.to_episode_requests(batch)
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_text(
            json.dumps([request_to_jsonable(request) for request in requests], ensure_ascii=False, indent=2),
            encoding="utf-8",
        )


if __name__ == "__main__":
    main()
