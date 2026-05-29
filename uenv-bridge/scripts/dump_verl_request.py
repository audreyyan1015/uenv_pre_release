#!/usr/bin/env python3
"""Generate and inspect a real verl.protocol.DataProto request-like batch.

This script intentionally does not start Ray, vLLM, model loading, or GPU work.
It uses VeRL's real DataProto class to build the same kind of prompt batch that
is passed to actor_rollout_wg.generate_sequences(gen_batch), then converts each
sample to a UEnv EpisodeRequest JSON view.
"""

from __future__ import annotations

import argparse
import json
import uuid
from pathlib import Path
from typing import Any

import numpy as np
import torch
from tensordict import TensorDict
from verl.protocol import DataProto


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
        return {str(k): to_jsonable(v) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [to_jsonable(v) for v in value]
    return value


def sample_non_tensor(non_tensor_batch: dict[str, Any], idx: int) -> dict[str, Any]:
    out: dict[str, Any] = {}
    for key, value in non_tensor_batch.items():
        item: Any
        try:
            item = value[idx]
        except Exception:
            item = value
        out[key] = to_jsonable(item)
    return out


def make_real_dataproto() -> DataProto:
    # A small, CPU-only prompt batch shaped like VeRL rollout generation input.
    # Field names match the gen_batch sent to actor_rollout_wg.generate_sequences:
    # input_ids, attention_mask, position_ids in TensorDict, plus sample metadata
    # in non_tensor_batch and generation knobs in meta_info.
    input_ids = torch.tensor(
        [
            [128000, 27, 91, 882, 91, 29, 198, 9906, 11, 1917, 374, 220, 17, 10, 17, 30, 128001],
            [128000, 27, 91, 882, 91, 29, 198, 16833, 264, 4823, 369, 264, 2768, 734, 13, 128001, 0],
        ],
        dtype=torch.long,
    )
    attention_mask = (input_ids != 0).long()
    position_ids = torch.arange(input_ids.shape[1], dtype=torch.long).unsqueeze(0).repeat(input_ids.shape[0], 1)

    batch = TensorDict(
        {
            "input_ids": input_ids,
            "attention_mask": attention_mask,
            "position_ids": position_ids,
        },
        batch_size=[input_ids.shape[0]],
    )

    non_tensor_batch = {
        "sample_id": np.array(["gsm8k-train-000001", "humaneval-000042"], dtype=object),
        "prompt_id": np.array(["prompt-math-000001", "prompt-code-000042"], dtype=object),
        "task_name": np.array(["math", "code"], dtype=object),
        "raw_prompt": np.array(
            [
                "What is 2 + 2? Answer with a number.",
                "Write a Python function add(a, b) that returns their sum.",
            ],
            dtype=object,
        ),
        "data_source": np.array(["gsm8k", "humaneval"], dtype=object),
        "ability": np.array(["math", "code"], dtype=object),
        "reward_model": np.array(
            [
                {"style": "rule", "ground_truth": "4"},
                {"style": "unit_test", "entry_point": "add", "tests": ["assert add(2, 3) == 5"]},
            ],
            dtype=object,
        ),
        "extra_info": np.array(
            [
                {"split": "train", "index": 1},
                {"split": "train", "index": 42},
            ],
            dtype=object,
        ),
    }

    meta_info = {
        "batch_id": "verl-realistic-batch-0001",
        "global_steps": 0,
        "eos_token_id": 128001,
        "pad_token_id": 0,
        "do_sample": True,
        "temperature": 0.7,
        "top_p": 0.95,
        "max_new_tokens": 128,
        "validate": False,
    }

    return DataProto(batch=batch, non_tensor_batch=non_tensor_batch, meta_info=meta_info)


def dataproto_summary(dp: DataProto) -> dict[str, Any]:
    return {
        "type": f"{type(dp).__module__}.{type(dp).__name__}",
        "batch_keys": list(dp.batch.keys()) if dp.batch is not None else [],
        "batch_size": list(dp.batch.batch_size) if dp.batch is not None else [],
        "tensor_shapes": {k: list(v.shape) for k, v in dp.batch.items()} if dp.batch is not None else {},
        "tensor_dtypes": {k: str(v.dtype) for k, v in dp.batch.items()} if dp.batch is not None else {},
        "non_tensor_keys": list(dp.non_tensor_batch.keys()),
        "non_tensor_preview": {k: to_jsonable(v[:2] if hasattr(v, "__getitem__") else v) for k, v in dp.non_tensor_batch.items()},
        "meta_info": to_jsonable(dp.meta_info),
    }


def env_type_for_task(task_name: str) -> str:
    mapping = {
        "math": "math",
        "code": "code",
        "humaneval": "code",
        "mbpp": "code",
        "agent": "agent",
    }
    return mapping.get(task_name, "code")


def to_episode_requests(dp: DataProto) -> list[dict[str, Any]]:
    requests: list[dict[str, Any]] = []
    batch_size = int(dp.batch.batch_size[0])
    batch_id = str(dp.meta_info.get("batch_id") or f"verl-batch-{uuid.uuid4().hex[:8]}")

    for idx in range(batch_size):
        nt = sample_non_tensor(dp.non_tensor_batch, idx)
        task_name = str(nt.get("task_name") or nt.get("ability") or "code")
        request_id = str(uuid.uuid4())
        correlation_id = f"{batch_id}-{idx}"

        metadata = {
            "batch_id": batch_id,
            "sample_index": idx,
            "sample_id": nt.get("sample_id"),
            "prompt_id": nt.get("prompt_id"),
            "task_name": task_name,
            "data_source": nt.get("data_source"),
            "ability": nt.get("ability"),
            "global_steps": dp.meta_info.get("global_steps"),
            "extra_info": nt.get("extra_info"),
        }

        request = {
            "request_id": request_id,
            "correlation_id": correlation_id,
            "framework": "verl",
            "request_ts": 0.0,
            "env_type": env_type_for_task(task_name),
            "env_config": {
                "task_name": task_name,
                "raw_prompt": nt.get("raw_prompt"),
                "data_source": nt.get("data_source"),
            },
            "model_endpoint": {
                "endpoint_type": "http",
                "url": "http://vllm.default.svc:8000/v1",
                "model_name": "policy-model",
                "generation_config": {
                    "do_sample": dp.meta_info.get("do_sample"),
                    "temperature": dp.meta_info.get("temperature"),
                    "top_p": dp.meta_info.get("top_p"),
                    "max_new_tokens": dp.meta_info.get("max_new_tokens"),
                    "eos_token_id": dp.meta_info.get("eos_token_id"),
                    "pad_token_id": dp.meta_info.get("pad_token_id"),
                },
                "max_retries": 3,
            },
            "episode_config": {
                "max_steps": 80 if env_type_for_task(task_name) == "code" else 10,
                "seed": 42 + idx,
                "initial_observation": {
                    "input_ids": to_jsonable(dp.batch["input_ids"][idx]),
                    "attention_mask": to_jsonable(dp.batch["attention_mask"][idx]),
                    "position_ids": to_jsonable(dp.batch["position_ids"][idx]),
                    "raw_prompt": nt.get("raw_prompt"),
                },
                "system_prompt": "",
                "stop_conditions": ["done", "max_steps", "timeout"],
            },
            "reward_config": {
                "reward_type": "external" if task_name == "code" else "rubric",
                "rubric_config": nt.get("reward_model"),
            },
            "metadata": metadata,
            "timeout_seconds": 300.0,
        }
        requests.append(request)

    return requests


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--out-dir", default="/tmp/uenv-verl-request-dump")
    args = parser.parse_args()

    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    dp = make_real_dataproto()
    episode_requests = to_episode_requests(dp)

    torch.save(
        {
            "data_proto": dp,
            "batch": dp.batch.cpu() if dp.batch is not None else None,
            "non_tensor_batch": dp.non_tensor_batch,
            "meta_info": dp.meta_info,
        },
        out_dir / "verl_dataproto_request.pt",
    )
    (out_dir / "verl_dataproto_summary.json").write_text(
        json.dumps(dataproto_summary(dp), ensure_ascii=False, indent=2),
        encoding="utf-8",
    )
    (out_dir / "episode_requests.json").write_text(
        json.dumps(episode_requests, ensure_ascii=False, indent=2),
        encoding="utf-8",
    )
    (out_dir / "episode_request_0.json").write_text(
        json.dumps(episode_requests[0], ensure_ascii=False, indent=2),
        encoding="utf-8",
    )

    print(f"wrote {out_dir / 'verl_dataproto_request.pt'}")
    print(f"wrote {out_dir / 'verl_dataproto_summary.json'}")
    print(f"wrote {out_dir / 'episode_requests.json'}")
    print(f"samples={len(episode_requests)}")


if __name__ == "__main__":
    main()
