#!/usr/bin/env python3
"""Start a ROLL pipeline from configs kept in uenv-bridge.

ROLL's example entrypoints use Hydra's relative config path and expect configs
to live under the ROLL repo. This wrapper keeps our reproduction configs in
uenv-bridge and imports the selected ROLL pipeline without modifying ROLL.
"""

from __future__ import annotations

import argparse
from pathlib import Path

from dacite import from_dict
from hydra import compose, initialize_config_dir
from omegaconf import OmegaConf

from roll.distributed.scheduler.initialize import init
from roll.pipeline.agentic.agentic_config import AgenticConfig
from roll.pipeline.agentic.agentic_pipeline import AgenticPipeline
from roll.pipeline.rlvr.rlvr_config import RLVRConfig
from roll.pipeline.rlvr.rlvr_pipeline import RLVRPipeline
from roll.pipeline.rlvr.rlvr_rollout_pipeline import RLVRRolloutPipeline


PIPELINES = {
    "rlvr": (RLVRConfig, RLVRPipeline),
    "rlvr_rollout": (RLVRConfig, RLVRRolloutPipeline),
    "agentic": (AgenticConfig, AgenticPipeline),
}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--config-dir", required=True, help="Absolute path to the Hydra config directory.")
    parser.add_argument("--config-name", required=True, help="Config file name without .yaml.")
    parser.add_argument(
        "--pipeline",
        choices=sorted(PIPELINES),
        default="rlvr",
        help="ROLL pipeline type to instantiate.",
    )
    args = parser.parse_args()

    config_dir = Path(args.config_dir).resolve()
    if not config_dir.is_dir():
        raise FileNotFoundError(f"config dir does not exist: {config_dir}")

    with initialize_config_dir(config_dir=str(config_dir), job_name="uenv-roll-repro", version_base=None):
        cfg = compose(config_name=args.config_name)

    print(OmegaConf.to_yaml(cfg, resolve=True))

    config_cls, pipeline_cls = PIPELINES[args.pipeline]
    pipeline_config = from_dict(data_class=config_cls, data=OmegaConf.to_container(cfg, resolve=True))

    init()
    pipeline = pipeline_cls(pipeline_config=pipeline_config)
    pipeline.run()


if __name__ == "__main__":
    main()
