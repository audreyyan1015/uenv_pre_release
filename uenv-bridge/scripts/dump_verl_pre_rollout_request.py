#!/usr/bin/env python3
"""Dump VeRL data before vLLM rollout and shape an external rollout request.

This is a spike script. It does not patch VeRL or start Ray/vLLM. It follows the
trainer-side data path up to the prompt batch that would be sent to
actor_rollout_wg.generate_sequences(gen_batch), then emits a JSON request shape
that a future UEnv external rollout path could send to Serve/Worker.
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

from verl.protocol import DataProto
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
        return {str(k): to_jsonable(v) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [to_jsonable(v) for v in value]
    return value


def sample_non_tensor(non_tensor_batch: dict[str, Any], idx: int) -> dict[str, Any]:
    sample: dict[str, Any] = {}
    for key, value in non_tensor_batch.items():
        try:
            item = value[idx]
        except Exception:
            item = value
        sample[key] = to_jsonable(item)
    return sample


def prompt_text(raw_prompt: Any) -> str:
    if isinstance(raw_prompt, list):
        parts = []
        for message in raw_prompt:
            if isinstance(message, dict):
                role = message.get("role", "")
                content = message.get("content", "")
                parts.append(f"{role}: {content}" if role else str(content))
            else:
                parts.append(str(message))
        return "\n".join(parts)
    return "" if raw_prompt is None else str(raw_prompt)


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


def get_gen_batch(batch: DataProto) -> DataProto:
    """Mirror the current VeRL trainer's prompt-batch extraction shape."""
    reward_keys = {"data_source", "reward_model", "extra_info", "uid"} & batch.non_tensor_batch.keys()
    non_tensor_batch_keys_to_pop = set(batch.non_tensor_batch.keys()) - reward_keys
    gen_batch = batch.pop(batch_keys=[], non_tensor_batch_keys=list(non_tensor_batch_keys_to_pop))
    gen_batch.non_tensor_batch.update(batch.non_tensor_batch)
    return gen_batch


def dataproto_summary(dp: DataProto) -> dict[str, Any]:
    batch_keys = list(dp.batch.keys()) if dp.batch is not None else []
    return {
        "type": f"{type(dp).__module__}.{type(dp).__name__}",
        "len": len(dp),
        "batch_keys": batch_keys,
        "batch_size": list(dp.batch.batch_size) if dp.batch is not None else None,
        "tensor_shapes": {k: list(v.shape) for k, v in dp.batch.items()} if dp.batch is not None else {},
        "tensor_dtypes": {k: str(v.dtype) for k, v in dp.batch.items()} if dp.batch is not None else {},
        "non_tensor_keys": list(dp.non_tensor_batch.keys()),
        "non_tensor_preview": {k: to_jsonable(v[: min(2, len(v))]) for k, v in dp.non_tensor_batch.items()},
        "meta_info": to_jsonable(dp.meta_info),
    }


def env_type_for_sample(sample: dict[str, Any]) -> str:
    data_source = str(sample.get("data_source") or "").lower()
    ability = str(sample.get("ability") or "").lower()
    if "gsm8k" in data_source or "math" in data_source or "math" in ability:
        return "math"
    if "code" in data_source or "code" in ability:
        return "code"
    return "agent"


def tensor_item(dp: DataProto, key: str, idx: int) -> Any:
    if dp.batch is None or key not in dp.batch.keys():
        return None
    return to_jsonable(dp.batch[key][idx])


def generation_config(meta_info: dict[str, Any]) -> dict[str, Any]:
    keys = [
        "do_sample",
        "temperature",
        "top_p",
        "top_k",
        "max_new_tokens",
        "max_response_length",
        "eos_token_id",
        "pad_token_id",
        "validate",
    ]
    return {key: to_jsonable(meta_info.get(key)) for key in keys if meta_info.get(key) is not None}


def sample_id_for(sample: dict[str, Any]) -> Any:
    extra_info = sample.get("extra_info") or {}
    if sample.get("uid") is not None:
        return sample.get("uid")
    if sample.get("sample_id") is not None:
        return sample.get("sample_id")
    if sample.get("index") is not None:
        return sample.get("index")
    if isinstance(extra_info, dict):
        return extra_info.get("index")
    return None


def local_prompt_tokens(raw_prompt: Any, tokenizer: Any, max_prompt_length: int) -> dict[str, Any] | None:
    if tokenizer is None:
        return None
    try:
        if isinstance(raw_prompt, list) and hasattr(tokenizer, "apply_chat_template"):
            token_ids = tokenizer.apply_chat_template(raw_prompt, tokenize=True, add_generation_prompt=True)
        else:
            token_ids = tokenizer.encode(prompt_text(raw_prompt), add_special_tokens=False)
    except Exception:
        return None

    if token_ids and isinstance(token_ids[0], list):
        token_ids = token_ids[0]
    token_ids = list(token_ids)[-max_prompt_length:]
    return {
        "input_ids": token_ids,
        "attention_mask": [1] * len(token_ids),
        "position_ids": list(range(len(token_ids))),
    }


def build_external_rollout_request(
    gen_batch: DataProto,
    *,
    rollout_n: int,
    model_endpoint: str,
    model_name: str,
    tokenizer: Any = None,
    max_prompt_length: int = 512,
) -> dict[str, Any]:
    batch_id = str(gen_batch.meta_info.get("batch_id") or f"verl-pre-rollout-{uuid.uuid4().hex[:8]}")
    request = {
        "protocol_version": "0.1-spike",
        "request_type": "external_rollout_batch",
        "request_id": str(uuid.uuid4()),
        "batch_id": batch_id,
        "framework": "verl",
        "rollout_n": rollout_n,
        "model_endpoint": {
            "endpoint_type": "http",
            "url": model_endpoint,
            "model_name": model_name,
            "generation_config": generation_config(gen_batch.meta_info),
        },
        "required_result_fields": [
            "sample_index",
            "response_text",
            "response_token_ids",
            "response_mask",
            "finish_reason",
            "trajectory",
            "reward",
        ],
        "samples": [],
    }

    for idx in range(len(gen_batch)):
        sample = sample_non_tensor(gen_batch.non_tensor_batch, idx)
        raw_prompt = sample.get("raw_prompt")
        env_type = env_type_for_sample(sample)
        reward_model = sample.get("reward_model")
        tensor_input_ids = tensor_item(gen_batch, "input_ids", idx)
        tensor_attention_mask = tensor_item(gen_batch, "attention_mask", idx)
        tensor_position_ids = tensor_item(gen_batch, "position_ids", idx)
        local_tokens = None
        token_source = "dataproto"
        if tensor_input_ids is None:
            local_tokens = local_prompt_tokens(raw_prompt, tokenizer, max_prompt_length)
            token_source = "local_tokenizer" if local_tokens is not None else "none"
        request["samples"].append(
            {
                "sample_index": idx,
                "sample_id": sample_id_for(sample),
                "uid": sample.get("uid"),
                "env_type": env_type,
                "prompt": {
                    "raw_prompt": raw_prompt,
                    "prompt_text": prompt_text(raw_prompt),
                    "input_ids": tensor_input_ids if tensor_input_ids is not None else (local_tokens or {}).get("input_ids"),
                    "attention_mask": tensor_attention_mask
                    if tensor_attention_mask is not None
                    else (local_tokens or {}).get("attention_mask"),
                    "position_ids": tensor_position_ids
                    if tensor_position_ids is not None
                    else (local_tokens or {}).get("position_ids"),
                    "token_source": token_source,
                },
                "env_config": {
                    "task_name": "math" if env_type == "math" else env_type,
                    "data_source": sample.get("data_source"),
                    "raw_prompt": prompt_text(raw_prompt),
                },
                "episode_config": {
                    "max_steps": 10 if env_type == "math" else 80,
                    "seed": 42 + idx,
                    "stop_conditions": ["done", "max_steps", "timeout"],
                },
                "reward_config": {
                    "reward_type": "rubric" if env_type == "math" else "external",
                    "rubric_config": reward_model,
                },
                "metadata": {
                    "batch_id": batch_id,
                    "sample_index": idx,
                    "data_source": sample.get("data_source"),
                    "extra_info": sample.get("extra_info"),
                    "global_steps": gen_batch.meta_info.get("global_steps"),
                },
            }
        )

    return request


def mock_response_text(sample: dict[str, Any]) -> str:
    rubric = (sample.get("reward_config") or {}).get("rubric_config")
    if isinstance(rubric, dict) and rubric.get("ground_truth") is not None:
        return str(rubric["ground_truth"])
    return "mock external rollout response"


def build_mock_rollout_result(request: dict[str, Any]) -> dict[str, Any]:
    results = []
    for sample in request["samples"]:
        response_text = mock_response_text(sample)
        results.append(
            {
                "sample_index": sample["sample_index"],
                "sample_id": sample.get("sample_id"),
                "status": "completed",
                "response_text": response_text,
                "finish_reason": "mock_external_rollout",
                "reward": None,
                "trajectory": [
                    {
                        "step_index": 1,
                        "observation": sample["prompt"]["prompt_text"],
                        "action": response_text,
                        "reward": None,
                        "terminated": True,
                    }
                ],
            }
        )
    return {
        "request_id": request["request_id"],
        "batch_id": request["batch_id"],
        "status": "completed",
        "results": results,
    }


def build_post_rollout_dataproto(
    gen_batch: DataProto,
    mock_result: dict[str, Any],
    *,
    tokenizer_path: str,
    max_response_length: int,
) -> DataProto:
    from transformers import AutoTokenizer

    tokenizer = AutoTokenizer.from_pretrained(tokenizer_path, trust_remote_code=True)
    pad_token_id = tokenizer.pad_token_id
    if pad_token_id is None:
        pad_token_id = tokenizer.eos_token_id if tokenizer.eos_token_id is not None else 0

    responses = torch.full((len(gen_batch), max_response_length), int(pad_token_id), dtype=torch.long)
    response_mask = torch.zeros((len(gen_batch), max_response_length), dtype=torch.long)
    for result in mock_result["results"]:
        idx = int(result["sample_index"])
        token_ids = tokenizer.encode(str(result["response_text"]), add_special_tokens=False)
        token_ids = token_ids[:max_response_length]
        if token_ids:
            responses[idx, : len(token_ids)] = torch.tensor(token_ids, dtype=torch.long)
            response_mask[idx, : len(token_ids)] = 1

    non_tensors = dict(gen_batch.non_tensor_batch)
    non_tensors["uenv_external_rollout_text"] = np.array(
        [str(item["response_text"]) for item in mock_result["results"]],
        dtype=object,
    )
    non_tensors["uenv_external_rollout_status"] = np.array(
        [str(item["status"]) for item in mock_result["results"]],
        dtype=object,
    )
    return DataProto.from_dict(
        tensors={"responses": responses, "response_mask": response_mask},
        non_tensors=non_tensors,
        meta_info=dict(gen_batch.meta_info),
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--data-file",
        default="/tmp/uenv-bridge/tmp/verl_real_dataloader_dump/gsm8k_verl_format_sample.parquet",
        help="VeRL-format parquet containing a prompt column. Convert raw GSM8K with scripts/prepare_verl_gsm8k_sample.py.",
    )
    parser.add_argument("--out-dir", default="/tmp/uenv-verl-pre-rollout-dump")
    parser.add_argument("--batch-size", type=int, default=2)
    parser.add_argument("--rollout-n", type=int, default=1)
    parser.add_argument("--model-endpoint", default="http://vllm.default.svc:8000/v1")
    parser.add_argument("--model-name", default="policy-model")
    parser.add_argument("--max-prompt-length", type=int, default=512)
    parser.add_argument("--max-response-length", type=int, default=64)
    parser.add_argument("--tokenizer-path", default="")
    args = parser.parse_args()

    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    dataset_tokenizer = None
    if args.tokenizer_path:
        from transformers import AutoTokenizer

        dataset_tokenizer = AutoTokenizer.from_pretrained(args.tokenizer_path, trust_remote_code=True)

    dataset = RLHFDataset(
        data_files=args.data_file,
        tokenizer=dataset_tokenizer,
        config=make_dataset_config(),
        processor=None,
        max_samples=args.batch_size,
    )
    loader = DataLoader(dataset, batch_size=args.batch_size, shuffle=False, num_workers=0, collate_fn=collate_fn)
    try:
        batch_dict = next(iter(loader))
    except KeyError as exc:
        raise RuntimeError(
            "RLHFDataset could not read the prompt column. Use a VeRL-format "
            "parquet file, or convert raw GSM8K with scripts/prepare_verl_gsm8k_sample.py."
        ) from exc
    train_batch = DataProto.from_single_dict(batch_dict)
    train_batch.meta_info["temperature"] = 1.0
    train_batch.meta_info["global_steps"] = 0
    train_batch.meta_info["batch_id"] = "verl-pre-rollout-batch-0001"
    train_batch.meta_info["max_response_length"] = args.max_response_length
    if "uid" not in train_batch.non_tensor_batch:
        train_batch.non_tensor_batch["uid"] = np.array([str(uuid.uuid4()) for _ in range(len(train_batch))], dtype=object)

    gen_batch = get_gen_batch(train_batch)
    gen_batch.meta_info["batch_id"] = train_batch.meta_info["batch_id"]
    combined_gen_batch = gen_batch.repeat(repeat_times=args.rollout_n, interleave=True)
    combined_gen_batch.meta_info["batch_id"] = train_batch.meta_info["batch_id"]

    rollout_request = build_external_rollout_request(
        combined_gen_batch,
        rollout_n=args.rollout_n,
        model_endpoint=args.model_endpoint,
        model_name=args.model_name,
        tokenizer=dataset_tokenizer,
        max_prompt_length=args.max_prompt_length,
    )
    mock_result = build_mock_rollout_result(rollout_request)

    torch.save(
        {
            "train_batch": train_batch,
            "gen_batch": gen_batch,
            "combined_gen_batch": combined_gen_batch,
        },
        out_dir / "verl_pre_rollout_batches.pt",
    )
    (out_dir / "train_batch_summary.json").write_text(
        json.dumps(dataproto_summary(train_batch), ensure_ascii=False, indent=2),
        encoding="utf-8",
    )
    (out_dir / "combined_gen_batch_summary.json").write_text(
        json.dumps(dataproto_summary(combined_gen_batch), ensure_ascii=False, indent=2),
        encoding="utf-8",
    )
    (out_dir / "external_rollout_request.json").write_text(
        json.dumps(rollout_request, ensure_ascii=False, indent=2),
        encoding="utf-8",
    )
    (out_dir / "external_rollout_request_0.json").write_text(
        json.dumps(rollout_request["samples"][0], ensure_ascii=False, indent=2),
        encoding="utf-8",
    )
    (out_dir / "mock_external_rollout_result.json").write_text(
        json.dumps(mock_result, ensure_ascii=False, indent=2),
        encoding="utf-8",
    )

    post_rollout_status: dict[str, Any]
    if args.tokenizer_path:
        post_rollout = build_post_rollout_dataproto(
            combined_gen_batch,
            mock_result,
            tokenizer_path=args.tokenizer_path,
            max_response_length=args.max_response_length,
        )
        torch.save(post_rollout, out_dir / "mock_post_rollout_dataproto.pt")
        post_rollout_status = {
            "built": True,
            "path": str(out_dir / "mock_post_rollout_dataproto.pt"),
            "summary": dataproto_summary(post_rollout),
        }
    else:
        post_rollout_status = {
            "built": False,
            "reason": "pass --tokenizer-path to tokenize response_text into VeRL responses/response_mask tensors",
        }
    (out_dir / "mock_post_rollout_status.json").write_text(
        json.dumps(post_rollout_status, ensure_ascii=False, indent=2),
        encoding="utf-8",
    )

    print(f"wrote {out_dir / 'verl_pre_rollout_batches.pt'}")
    print(f"wrote {out_dir / 'combined_gen_batch_summary.json'}")
    print(f"wrote {out_dir / 'external_rollout_request.json'}")
    print(f"wrote {out_dir / 'mock_external_rollout_result.json'}")
    print(f"samples={len(rollout_request['samples'])} rollout_n={args.rollout_n}")
    print(f"post_rollout_dataproto_built={post_rollout_status['built']}")


if __name__ == "__main__":
    main()
