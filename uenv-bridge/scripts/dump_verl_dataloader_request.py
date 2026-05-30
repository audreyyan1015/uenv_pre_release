#!/usr/bin/env python3
"""Dump a real VeRL DataProto request from the existing GSM8K parquet data.

This follows the current VeRL trainer path without starting Ray/vLLM/training:
  RLHFDataset -> collate_fn -> DataProto.from_single_dict -> _get_gen_batch
  -> repeat(rollout_n) -> convert samples to UEnv EpisodeRequest JSON.
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
        return value.decode('utf-8', errors='replace')
    if isinstance(value, dict):
        return {str(k): to_jsonable(v) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [to_jsonable(v) for v in value]
    return value


def sample_non_tensor(non_tensor_batch: dict[str, Any], idx: int) -> dict[str, Any]:
    out = {}
    for key, value in non_tensor_batch.items():
        try:
            item = value[idx]
        except Exception:
            item = value
        out[key] = to_jsonable(item)
    return out


def get_gen_batch(batch: DataProto) -> DataProto:
    # Copied from RayPPOTrainer._get_gen_batch in this checkout.
    reward_keys = set({'data_source', 'reward_model', 'extra_info', 'uid'}) & batch.non_tensor_batch.keys()
    batch_keys_to_pop = []
    non_tensor_batch_keys_to_pop = set(batch.non_tensor_batch.keys()) - reward_keys
    gen_batch = batch.pop(
        batch_keys=batch_keys_to_pop,
        non_tensor_batch_keys=list(non_tensor_batch_keys_to_pop),
    )
    gen_batch.non_tensor_batch.update(batch.non_tensor_batch)
    return gen_batch


def dataproto_summary(dp: DataProto) -> dict[str, Any]:
    batch_keys = list(dp.batch.keys()) if dp.batch is not None else []
    return {
        'type': f'{type(dp).__module__}.{type(dp).__name__}',
        'len': len(dp),
        'batch_keys': batch_keys,
        'batch_size': list(dp.batch.batch_size) if dp.batch is not None else None,
        'tensor_shapes': {k: list(v.shape) for k, v in dp.batch.items()} if dp.batch is not None else {},
        'tensor_dtypes': {k: str(v.dtype) for k, v in dp.batch.items()} if dp.batch is not None else {},
        'non_tensor_keys': list(dp.non_tensor_batch.keys()),
        'non_tensor_preview': {k: to_jsonable(v[: min(2, len(v))]) for k, v in dp.non_tensor_batch.items()},
        'meta_info': to_jsonable(dp.meta_info),
    }


def env_type_for_sample(sample: dict[str, Any]) -> str:
    data_source = str(sample.get('data_source') or '')
    ability = str(sample.get('ability') or '')
    if 'code' in data_source.lower() or 'code' in ability.lower():
        return 'code'
    if 'gsm8k' in data_source.lower() or 'math' in ability.lower() or 'math' in data_source.lower():
        return 'math'
    return 'agent'


def prompt_text(raw_prompt: Any) -> str:
    if isinstance(raw_prompt, list):
        parts = []
        for msg in raw_prompt:
            if isinstance(msg, dict):
                role = msg.get('role', '')
                content = msg.get('content', '')
                parts.append(f'{role}: {content}' if role else str(content))
            else:
                parts.append(str(msg))
        return '\n'.join(parts)
    return str(raw_prompt)


def to_episode_requests(dp: DataProto) -> list[dict[str, Any]]:
    requests = []
    batch_id = str(dp.meta_info.get('batch_id') or f'verl-dataloader-batch-{uuid.uuid4().hex[:8]}')
    for idx in range(len(dp)):
        sample = sample_non_tensor(dp.non_tensor_batch, idx)
        env_type = env_type_for_sample(sample)
        raw_prompt = sample.get('raw_prompt')
        extra_info = sample.get('extra_info') or {}
        reward_model = sample.get('reward_model')
        sample_id = sample.get('uid') or sample.get('index') or extra_info.get('index') if isinstance(extra_info, dict) else sample.get('uid')
        request_id = str(uuid.uuid4())
        correlation_id = f'{batch_id}-{idx}'

        metadata = {
            'batch_id': batch_id,
            'sample_index': idx,
            'sample_id': sample_id,
            'uid': sample.get('uid'),
            'index': sample.get('index'),
            'data_source': sample.get('data_source'),
            'global_steps': dp.meta_info.get('global_steps'),
            'extra_info': extra_info,
        }

        requests.append({
            'request_id': request_id,
            'correlation_id': correlation_id,
            'framework': 'verl',
            'request_ts': 0.0,
            'env_type': env_type,
            'env_config': {
                'data_source': sample.get('data_source'),
                'raw_prompt': prompt_text(raw_prompt),
                'index': sample.get('index'),
            },
            'model_endpoint': {
                'endpoint_type': 'http',
                'url': 'http://vllm.default.svc:8000/v1',
                'model_name': 'Qwen/Qwen2.5-0.5B-Instruct',
                'generation_config': {
                    'temperature': dp.meta_info.get('temperature'),
                    'max_new_tokens': dp.meta_info.get('max_response_length'),
                    'validate': dp.meta_info.get('validate', False),
                },
                'max_retries': 3,
            },
            'episode_config': {
                'max_steps': 10 if env_type == 'math' else 80,
                'seed': 42 + idx,
                'initial_observation': {
                    'raw_prompt': raw_prompt,
                    'prompt_text': prompt_text(raw_prompt),
                },
                'system_prompt': '',
                'stop_conditions': ['done', 'max_steps', 'timeout'],
            },
            'reward_config': {
                'reward_type': 'rubric' if env_type == 'math' else 'external',
                'rubric_config': reward_model,
            },
            'metadata': metadata,
            'timeout_seconds': 300.0,
        })
    return requests


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument('--data-file', default='/workspace/data/gsm8k/train.parquet')
    parser.add_argument('--out-dir', default='/tmp/uenv-verl-dataloader-dump')
    parser.add_argument('--batch-size', type=int, default=2)
    parser.add_argument('--rollout-n', type=int, default=1)
    args = parser.parse_args()

    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    config = OmegaConf.create({
        'use_shm': False,
        'prompt_key': 'prompt',
        'reward_fn_key': 'data_source',
        'max_prompt_length': 512,
        'return_raw_input_ids': False,
        'return_raw_chat': True,
        'return_full_prompt': False,
        'shuffle': False,
        'seed': 42,
        'image_patch_size': 14,
        'validation_shuffle': False,
        'filter_overlong_prompts': False,
        'filter_overlong_prompts_workers': 1,
        'truncation': 'error',
        'image_key': 'images',
        'video_key': 'videos',
        'audio_key': 'audios',
        'trust_remote_code': False,
        'return_multi_modal_inputs': True,
        'apply_chat_template_kwargs': {},
        'mm_processor_kwargs': {},
    })

    dataset = RLHFDataset(
        data_files=args.data_file,
        tokenizer=None,
        config=config,
        processor=None,
        max_samples=args.batch_size,
    )
    loader = DataLoader(dataset, batch_size=args.batch_size, shuffle=False, num_workers=0, collate_fn=collate_fn)
    batch_dict = next(iter(loader))
    train_batch = DataProto.from_single_dict(batch_dict)
    train_batch.meta_info['temperature'] = 1.0
    train_batch.non_tensor_batch['uid'] = np.array([str(uuid.uuid4()) for _ in range(len(train_batch))], dtype=object)

    gen_batch = get_gen_batch(train_batch)
    gen_batch.meta_info['global_steps'] = 0
    gen_batch.meta_info['batch_id'] = 'verl-gsm8k-dataloader-batch-0001'
    gen_batch.meta_info['max_response_length'] = 256
    combined_gen_batch = gen_batch.repeat(repeat_times=args.rollout_n, interleave=True)

    episode_requests = to_episode_requests(combined_gen_batch)

    torch.save({
        'train_batch': train_batch,
        'gen_batch': gen_batch,
        'combined_gen_batch': combined_gen_batch,
    }, out_dir / 'verl_real_dataloader_request.pt')
    (out_dir / 'train_batch_summary.json').write_text(json.dumps(dataproto_summary(train_batch), ensure_ascii=False, indent=2), encoding='utf-8')
    (out_dir / 'combined_gen_batch_summary.json').write_text(json.dumps(dataproto_summary(combined_gen_batch), ensure_ascii=False, indent=2), encoding='utf-8')
    (out_dir / 'episode_requests.json').write_text(json.dumps(episode_requests, ensure_ascii=False, indent=2), encoding='utf-8')
    (out_dir / 'episode_request_0.json').write_text(json.dumps(episode_requests[0], ensure_ascii=False, indent=2), encoding='utf-8')

    print(f'wrote {out_dir / "verl_real_dataloader_request.pt"}')
    print(f'wrote {out_dir / "combined_gen_batch_summary.json"}')
    print(f'wrote {out_dir / "episode_requests.json"}')
    print(f'samples={len(episode_requests)}')


if __name__ == '__main__':
    main()
