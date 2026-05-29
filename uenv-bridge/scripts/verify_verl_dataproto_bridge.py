#!/usr/bin/env python3
"""Verify uenv-bridge with real verl.protocol.DataProto objects.

This script runs inside a VeRL runtime. It builds a real DataProto via
RLHFDataset -> collate_fn -> DataProto.from_single_dict, submits it through
VeRLAdapter with a fake EpisodeClient, converts the fake server result back to
VeRL DataProto reward output, and verifies VeRL's extract_reward can consume it.
"""

from __future__ import annotations

import argparse
import json
import uuid
from pathlib import Path
from typing import Any

import numpy as np
import torch
from omegaconf import OmegaConf
from torch.utils.data import DataLoader

from uenv.bridge import FakeEpisodeClient, VeRLAdapter
from verl.protocol import DataProto
from verl.trainer.ppo.core_algos import AdvantageEstimator
from verl.trainer.ppo.ray_trainer import compute_advantage
from verl.trainer.ppo.reward import extract_reward
from verl.utils.dataset.rl_dataset import RLHFDataset, collate_fn


def to_jsonable(value: Any) -> Any:
    if isinstance(value, torch.Tensor):
        return value.detach().cpu().tolist()
    if isinstance(value, np.ndarray):
        return value.tolist()
    if isinstance(value, np.generic):
        return value.item()
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace")
    if isinstance(value, dict):
        return {str(key): to_jsonable(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [to_jsonable(item) for item in value]
    return value


def get_gen_batch(batch: DataProto) -> DataProto:
    reward_keys = set({"data_source", "reward_model", "extra_info", "uid"}) & batch.non_tensor_batch.keys()
    non_tensor_batch_keys_to_pop = set(batch.non_tensor_batch.keys()) - reward_keys
    gen_batch = batch.pop(batch_keys=[], non_tensor_batch_keys=list(non_tensor_batch_keys_to_pop))
    gen_batch.non_tensor_batch.update(batch.non_tensor_batch)
    return gen_batch


def make_dataset_config() -> Any:
    return OmegaConf.create(
        {
            "use_shm": False,
            "prompt_key": "prompt",
            "reward_fn_key": "data_source",
            "max_prompt_length": 512,
            "return_raw_input_ids": False,
            "return_raw_chat": True,
            "return_full_prompt": False,
            "shuffle": False,
            "seed": 42,
            "image_patch_size": 14,
            "validation_shuffle": False,
            "filter_overlong_prompts": False,
            "filter_overlong_prompts_workers": 1,
            "truncation": "error",
            "image_key": "images",
            "video_key": "videos",
            "audio_key": "audios",
            "trust_remote_code": False,
            "return_multi_modal_inputs": True,
            "apply_chat_template_kwargs": {},
            "mm_processor_kwargs": {},
        }
    )


def make_real_dataproto(data_file: str, batch_size: int, rollout_n: int) -> DataProto:
    dataset = RLHFDataset(
        data_files=data_file,
        tokenizer=None,
        config=make_dataset_config(),
        processor=None,
        max_samples=batch_size,
    )
    loader = DataLoader(dataset, batch_size=batch_size, shuffle=False, num_workers=0, collate_fn=collate_fn)
    batch_dict = next(iter(loader))
    train_batch = DataProto.from_single_dict(batch_dict)
    train_batch.meta_info["temperature"] = 1.0
    train_batch.non_tensor_batch["uid"] = np.array([str(uuid.uuid4()) for _ in range(len(train_batch))], dtype=object)

    gen_batch = get_gen_batch(train_batch)
    gen_batch.meta_info["global_steps"] = 0
    gen_batch.meta_info["batch_id"] = "uenv-bridge-real-verl-batch-0001"
    gen_batch.meta_info["max_response_length"] = 256
    combined = gen_batch.repeat(repeat_times=rollout_n, interleave=True)

    response_len = 8
    rollout_tensors = {
        "responses": torch.ones((len(combined), response_len), dtype=torch.long),
        "response_mask": torch.ones((len(combined), response_len), dtype=torch.long),
    }
    return DataProto.from_dict(
        tensors=rollout_tensors,
        non_tensors=combined.non_tensor_batch,
        meta_info=combined.meta_info,
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--data-file", required=True)
    parser.add_argument("--out-dir", default="/tmp/uenv-verl-bridge-real-check")
    parser.add_argument("--batch-size", type=int, default=2)
    parser.add_argument("--rollout-n", type=int, default=1)
    parser.add_argument("--fake-reward", type=float, default=1.0)
    args = parser.parse_args()

    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    batch = make_real_dataproto(args.data_file, args.batch_size, args.rollout_n)
    adapter = VeRLAdapter(client=FakeEpisodeClient(reward=args.fake_reward))
    requests = adapter.to_episode_requests(batch)
    bridge_result = adapter.execute_batch(batch)
    reward_dp = adapter.results_to_dataproto(batch, bridge_result["results"])
    merged = batch.union(reward_dp)
    reward_tensor, reward_extra_infos = extract_reward(merged)
    merged.batch["token_level_scores"] = reward_tensor
    merged.batch["token_level_rewards"] = reward_tensor
    adv_batch = compute_advantage(
        merged,
        adv_estimator=AdvantageEstimator.GRPO,
        num_repeat=args.rollout_n,
        norm_adv_by_std_in_grpo=False,
    )

    summary = {
        "input_type": f"{type(batch).__module__}.{type(batch).__name__}",
        "batch_len": len(batch),
        "request_count": len(requests),
        "request_env_types": [request.env_type for request in requests],
        "request_payload_0": json.loads(requests[0].payload.decode("utf-8")) if requests else None,
        "bridge_result": to_jsonable(bridge_result),
        "reward_dp_batch_keys": list(reward_dp.batch.keys()),
        "reward_dp_non_tensor_keys": list(reward_dp.non_tensor_batch.keys()),
        "merged_batch_keys": list(merged.batch.keys()),
        "merged_non_tensor_keys": list(merged.non_tensor_batch.keys()),
        "reward_tensor_shape": list(reward_tensor.shape),
        "reward_tensor": to_jsonable(reward_tensor),
        "reward_extra_info_keys": list(reward_extra_infos.keys()),
        "reward_extra_infos": to_jsonable(reward_extra_infos),
        "sequence_scores": to_jsonable(reward_tensor.sum(-1)),
        "advantage_keys": [key for key in ["token_level_scores", "token_level_rewards", "advantages", "returns"] if key in adv_batch.batch.keys()],
        "advantages_shape": list(adv_batch.batch["advantages"].shape),
        "returns_shape": list(adv_batch.batch["returns"].shape),
        "advantages": to_jsonable(adv_batch.batch["advantages"]),
        "returns": to_jsonable(adv_batch.batch["returns"]),
    }
    (out_dir / "summary.json").write_text(json.dumps(summary, ensure_ascii=False, indent=2), encoding="utf-8")
    (out_dir / "episode_requests.json").write_text(
        json.dumps([json.loads(request.payload.decode("utf-8")) for request in requests], ensure_ascii=False, indent=2),
        encoding="utf-8",
    )

    print(f"wrote {out_dir / 'summary.json'}")
    print(f"wrote {out_dir / 'episode_requests.json'}")
    print(f"batch_len={summary['batch_len']} request_count={summary['request_count']}")
    print(f"reward_tensor_shape={summary['reward_tensor_shape']} sequence_scores={summary['sequence_scores']}")
    print(f"advantages_shape={summary['advantages_shape']} returns_shape={summary['returns_shape']}")


if __name__ == "__main__":
    main()
