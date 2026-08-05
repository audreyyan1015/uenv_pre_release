from __future__ import annotations

import argparse
import json
from pathlib import Path

from .protocol import request_to_jsonable
from .verl_agent_loop import UEnvAgentLoop


def main() -> None:
    parser = argparse.ArgumentParser(prog="uenv-bridge")
    subparsers = parser.add_subparsers(dest="command", required=True)

    dry_run = subparsers.add_parser(
        "dry-run-agent-loop-json",
        help="Convert one pre-rollout VeRL sample JSON into an EpisodeRequest JSON.",
    )
    dry_run.add_argument("--input", required=True, help="Path to sample JSON.")
    dry_run.add_argument("--output", required=True, help="Path to write EpisodeRequest JSON.")

    args = parser.parse_args()
    if args.command == "dry-run-agent-loop-json":
        sample = json.loads(Path(args.input).read_text(encoding="utf-8"))
        prompt_ids = [int(item) for item in sample.get("prompt_ids", [])]
        sampling_params = sample.get("sampling_params") or {}
        raw_prompt = sample.get("raw_prompt")
        sample_kwargs = {key: value for key, value in sample.items() if key not in {"prompt_ids", "sampling_params", "raw_prompt"}}

        loop = UEnvAgentLoop(tokenizer=None)
        request = loop.build_episode_request(
            sampling_params=sampling_params,
            prompt_ids=prompt_ids,
            raw_prompt=raw_prompt,
            sample_kwargs=sample_kwargs,
        )

        output_path = Path(args.output)
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_text(
            json.dumps(request_to_jsonable(request), ensure_ascii=False, indent=2),
            encoding="utf-8",
        )


if __name__ == "__main__":
    main()
