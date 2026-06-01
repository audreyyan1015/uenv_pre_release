from __future__ import annotations

import json
import uuid
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Iterable

from .base import BaseAdapter
from .clients import EpisodeClient, FakeEpisodeClient
from .protocol import EpisodeRequest, EpisodeResult, MODE_MULTI, ResourceSpec


# VeRL batches contain tensors, numpy arrays, numpy scalars, and bytes;
# normalize them before embedding fields in JSON payloads or dry-run output.
def to_jsonable(value: Any) -> Any:
    if value is None or isinstance(value, (str, int, float, bool)):
        return value
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace")
    if isinstance(value, dict):
        return {str(key): to_jsonable(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [to_jsonable(item) for item in value]
    if hasattr(value, "tolist"):
        return to_jsonable(value.tolist())
    if hasattr(value, "item"):
        try:
            return value.item()
        except Exception:
            pass
    return value


# non_tensor_batch is batch-major in real DataProto objects, so each
# sample envelope needs the idx-th element from every available metadata field.
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


@dataclass(slots=True)
class VeRLAdapterConfig:
    default_env_type: str = "code"
    task_to_env_type: dict[str, str] = field(
        default_factory=lambda: {
            "code": "code",
            "humaneval": "code",
            "mbpp": "code",
            "math": "math",
            "gsm8k": "math",
            "agent": "agent",
        }
    )
    default_model_endpoint: str = "http://vllm.default.svc:8000/v1"
    default_model_name: str = "policy-model"
    default_timeout_seconds: float = 300.0
    default_max_steps: int = 80
    math_max_steps: int = 10
    seed_base: int = 42

    @classmethod
    def from_file(cls, path: str | Path) -> "VeRLAdapterConfig":
        config_path = Path(path)
        text = config_path.read_text(encoding="utf-8")
        if config_path.suffix.lower() == ".json":
            data = json.loads(text)
        else:
            data = _load_yaml_mapping(text)
        return cls.from_mapping(data)

    @classmethod
    def from_mapping(cls, data: dict[str, Any]) -> "VeRLAdapterConfig":
        # Keep the parser intentionally narrow: the full YAML may contain
        # server/logging/batch sections, but this config only controls fields
        # needed while converting VeRL samples into EpisodeRequest payloads.
        mapping = data.get("mapping") or {}
        model_endpoint = data.get("model_endpoint") or {}
        grpc = (data.get("server") or {}).get("grpc") or {}

        default_config = cls()
        task_to_env_type = mapping.get("task_to_env_type")
        return cls(
            default_env_type=str(mapping.get("default_env_type", default_config.default_env_type)),
            task_to_env_type=dict(task_to_env_type or default_config.task_to_env_type),
            default_model_endpoint=str(model_endpoint.get("url", default_config.default_model_endpoint)),
            default_model_name=str(model_endpoint.get("model_name", default_config.default_model_name)),
            default_timeout_seconds=float(
                grpc.get("timeout_seconds", mapping.get("default_timeout_seconds", default_config.default_timeout_seconds))
            ),
            default_max_steps=int(mapping.get("default_max_steps", default_config.default_max_steps)),
            math_max_steps=int(mapping.get("math_max_steps", default_config.math_max_steps)),
            seed_base=int(mapping.get("seed_base", default_config.seed_base)),
        )


def _load_yaml_mapping(text: str) -> dict[str, Any]:
    try:
        import yaml
    except ImportError as exc:
        raise RuntimeError("Loading YAML config requires PyYAML; use JSON config or install pyyaml") from exc
    data = yaml.safe_load(text) or {}
    if not isinstance(data, dict):
        raise ValueError("VeRL adapter config must be a YAML mapping")
    return data


class VeRLAdapter(BaseAdapter):
    def __init__(self, client: EpisodeClient | None = None, config: VeRLAdapterConfig | None = None) -> None:
        self.client = client or FakeEpisodeClient()
        self.config = config or VeRLAdapterConfig()

    def convert_request(self, request: Any) -> EpisodeRequest:
        # Public single-sample adapter entrypoint from BaseAdapter. Callers may
        # pass either a bare sample dict or the internal envelope shape produced
        # by _iter_sample_envelopes().
        if isinstance(request, dict) and "sample" in request:
            return self._convert_sample_request(request)
        if isinstance(request, dict):
            return self._convert_sample_request({"sample": request, "sample_index": 0, "meta_info": {}})
        raise TypeError("VeRLAdapter.convert_request expects a sample dict or an envelope with sample metadata")

    def convert_response(self, response: EpisodeResult) -> dict[str, Any]:
        # Convert one UEnv/Serve EpisodeResult into the bridge's plain Python
        # result dict. Batch-level VeRL tensor backfill is handled separately by
        # results_to_dataproto().
        done = response.status == "completed"
        return {
            "uenv_request_id": response.request_id,
            "trajectory": self._trajectory_to_jsonable(response),
            "reward": response.summary.total_reward,
            "scores": response.summary.total_reward,
            "done": done,
            "termination_reason": response.summary.terminate_reason or response.status,
            "uenv_error": None
            if done
            else {
                "code": response.error_code,
                "message": response.error_message,
                "status": response.status,
            },
        }

    def to_episode_requests(self, batch: Any) -> list[EpisodeRequest]:
        # Batch-level request conversion: split a dict fixture or real VeRL
        # DataProto into sample envelopes, then convert each envelope into an
        # EpisodeRequest. This does not submit requests to any client.
        return [self._convert_sample_request(envelope) for envelope in self._iter_sample_envelopes(batch)]

    def execute_episode(self, sample: dict[str, Any]) -> dict[str, Any]:
        request = self.convert_request(sample)
        return self.convert_response(self.client.submit_episode(request))

    def execute_batch(self, batch: Any) -> dict[str, Any]:
        # Submit all samples through the client boundary. Streamed Serve
        # responses may arrive out of order, so results are restored by request_id.
        envelopes = list(self._iter_sample_envelopes(batch))
        requests = [self._convert_sample_request(envelope) for envelope in envelopes]
        request_to_index = {request.request_id: envelope["sample_index"] for request, envelope in zip(requests, envelopes)}

        results: list[dict[str, Any] | None] = [None] * len(requests)
        for response in self.client.submit_episode_stream(requests):
            sample_index = request_to_index.get(response.request_id)
            if sample_index is None:
                continue
            results[sample_index] = self.convert_response(response)

        # Missing responses are converted into per-sample failures instead of
        # raising, so one bad episode does not abort the entire trainer step.
        return {
            "batch_id": self._batch_id(batch),
            "results": [
                result
                if result is not None
                else {
                    "uenv_request_id": None,
                    "trajectory": [],
                    "reward": 0.0,
                    "scores": 0.0,
                    "done": False,
                    "termination_reason": "missing_result",
                    "uenv_error": {"code": None, "message": "missing result", "status": "missing"},
                }
                for result in results
            ],
        }

    def execute_batch_dataproto(self, batch: Any) -> Any:
        output = self.execute_batch(batch)
        return self.results_to_dataproto(batch, output["results"])

    def results_to_dataproto(self, batch: Any, results: list[dict[str, Any]]) -> Any:
        # Batch-level response conversion for VeRL: take bridge result dicts
        # from convert_response()/execute_batch() and build a DataProto carrying
        # rm_scores plus UEnv extra fields.
        data_proto_cls = type(batch)
        torch, np = self._import_torch_numpy()

        # VeRL reward managers expose scalar episode rewards as rm_scores.
        # The scalar is placed on the last valid response token; VeRL will later
        # compute token_level_scores/rewards and GRPO/PPO advantages itself.
        response_shape = self._response_shape(batch)
        rewards = [float(result.get("reward") or 0.0) for result in results]
        rm_scores = torch.zeros(response_shape, dtype=torch.float32)
        for idx, reward in enumerate(rewards):
            last_index = self._last_response_index(batch, idx)
            rm_scores[idx, last_index] = reward

        # Extra UEnv fields ride in non_tensor_batch and are advertised via
        # reward_extra_keys so verl.trainer.ppo.reward.extract_reward can find them.
        non_tensors = {
            "uenv_done": np.array([bool(result.get("done")) for result in results], dtype=object),
            "uenv_termination_reason": np.array(
                [str(result.get("termination_reason") or "") for result in results],
                dtype=object,
            ),
            "uenv_request_id": np.array(
                [result.get("uenv_request_id") for result in results],
                dtype=object,
            ),
            "uenv_error": np.array(
                [result.get("uenv_error") for result in results],
                dtype=object,
            ),
            "uenv_trajectory": np.array(
                [result.get("trajectory") or [] for result in results],
                dtype=object,
            ),
        }
        meta_info = {"reward_extra_keys": list(non_tensors.keys())}
        return data_proto_cls.from_dict(tensors={"rm_scores": rm_scores}, non_tensors=non_tensors, meta_info=meta_info)

    def _iter_sample_envelopes(self, batch: Any) -> Iterable[dict[str, Any]]:
        batch_size = self._batch_size(batch)
        batch_id = self._batch_id(batch)
        meta_info = self._meta_info(batch)
        non_tensor_batch = self._non_tensor_batch(batch)
        tensor_batch = self._tensor_batch(batch)

        # Build a framework-neutral envelope before protocol conversion. Here
        # "envelope" means "one sample plus its batch context": the actual
        # sample data, its sample_index, the batch_id, and batch-level meta_info.
        # This keeps dict fixtures and real DataProto objects on the same path.
        for idx in range(batch_size):
            sample = sample_non_tensor(non_tensor_batch, idx)
            sample["initial_observation"] = self._initial_observation(tensor_batch, sample, idx)
            yield {
                "batch_id": batch_id,
                "sample_index": idx,
                "sample": sample,
                "meta_info": meta_info,
            }

    def _convert_sample_request(self, envelope: dict[str, Any]) -> EpisodeRequest:
        # Internal core converter. It expects the normalized envelope shape from
        # _iter_sample_envelopes()/convert_request() and performs the actual
        # EpisodeRequest construction.
        sample = dict(envelope["sample"])
        sample_index = int(envelope.get("sample_index", 0))
        meta_info = dict(envelope.get("meta_info") or {})
        batch_id = str(envelope.get("batch_id") or meta_info.get("batch_id") or f"verl-batch-{uuid.uuid4().hex[:8]}")
        request_id = str(envelope.get("request_id") or uuid.uuid4())

        task_name = self._task_name(sample)
        env_type = self.env_type_for_sample(sample)
        max_steps = self.config.math_max_steps if env_type == "math" else self.config.default_max_steps
        seed = int(meta_info.get("seed", self.config.seed_base + sample_index))
        raw_prompt = sample.get("raw_prompt")
        reward_model = sample.get("reward_model")
        extra_info = sample.get("extra_info") or {}
        sample_id = self._sample_id(sample, extra_info)

        # These fields are required to correlate streamed EpisodeResult values
        # back to the original VeRL sample and to debug failed samples later.
        metadata = {
            "batch_id": batch_id,
            "sample_index": sample_index,
            "sample_id": sample_id,
            "uid": sample.get("uid"),
            "prompt_id": sample.get("prompt_id"),
            "index": sample.get("index"),
            "task_name": task_name,
            "data_source": sample.get("data_source"),
            "ability": sample.get("ability"),
            "global_steps": meta_info.get("global_steps"),
            "extra_info": extra_info,
        }

        generation_config = {
            "do_sample": meta_info.get("do_sample"),
            "temperature": meta_info.get("temperature"),
            "top_p": meta_info.get("top_p"),
            "max_new_tokens": meta_info.get("max_new_tokens", meta_info.get("max_response_length")),
            "eos_token_id": meta_info.get("eos_token_id"),
            "pad_token_id": meta_info.get("pad_token_id"),
            "validate": meta_info.get("validate", False),
        }
        env_config = {
            "task_name": task_name,
            "data_source": sample.get("data_source"),
            "raw_prompt": prompt_text(raw_prompt),
            "index": sample.get("index"),
        }
        if sample.get("uenv_response_text") is not None:
            env_config["response_text"] = str(sample.get("uenv_response_text"))

        # The current bridge dataclass does not yet expose every proto field
        # directly, so Phase 0 stores versioned UEnv fields inside payload JSON.
        payload = {
            "protocol_version": "1.0",
            "framework": "verl",
            "correlation_id": f"{batch_id}-{sample_index}",
            "env_config": env_config,
            "model_endpoint": {
                "endpoint_type": "http",
                "url": meta_info.get("model_endpoint", self.config.default_model_endpoint),
                "model_name": meta_info.get("model_name", self.config.default_model_name),
                "generation_config": {key: value for key, value in generation_config.items() if value is not None},
                "max_retries": int(meta_info.get("max_retries", 3)),
            },
            "episode_config": {
                "max_steps": max_steps,
                "seed": seed,
                "initial_observation": sample.get("initial_observation") or {"raw_prompt": raw_prompt, "prompt_text": prompt_text(raw_prompt)},
                "system_prompt": meta_info.get("system_prompt", ""),
                "stop_conditions": ["done", "max_steps", "timeout"],
            },
            "reward_config": {
                "reward_type": "rubric" if env_type == "math" else "external",
                "rubric_config": reward_model,
            },
            "metadata": metadata,
            "timeout_seconds": float(meta_info.get("timeout_seconds", self.config.default_timeout_seconds)),
        }

        return EpisodeRequest(
            request_id=request_id,
            env_type=env_type,
            payload=json.dumps(to_jsonable(payload), ensure_ascii=False, separators=(",", ":")).encode("utf-8"),
            mode=MODE_MULTI,
            max_steps=max_steps,
            resource_spec=ResourceSpec(),
            model_endpoint=str(meta_info.get("model_endpoint", self.config.default_model_endpoint)),
            seed=seed,
        )

    def env_type_for_sample(self, sample: dict[str, Any]) -> str:
        fields = [
            str(sample.get("task_name") or ""),
            str(sample.get("ability") or ""),
            str(sample.get("data_source") or ""),
        ]
        lowered = [field.lower() for field in fields if field]

        # Prefer exact task/source matches, then fall back to substring matches
        # for dataset names like openai/gsm8k.
        for field in lowered:
            if field in self.config.task_to_env_type:
                return self.config.task_to_env_type[field]
        for field in lowered:
            for key, env_type in self.config.task_to_env_type.items():
                if key in field:
                    return env_type
        return self.config.default_env_type

    def _task_name(self, sample: dict[str, Any]) -> str:
        return str(sample.get("task_name") or sample.get("ability") or sample.get("data_source") or self.config.default_env_type)

    def _sample_id(self, sample: dict[str, Any], extra_info: Any) -> Any:
        if sample.get("sample_id") is not None:
            return sample.get("sample_id")
        if sample.get("uid") is not None:
            return sample.get("uid")
        if sample.get("index") is not None:
            return sample.get("index")
        if isinstance(extra_info, dict):
            return extra_info.get("index")
        return None

    def _batch_id(self, batch: Any) -> str:
        meta_info = self._meta_info(batch)
        return str(meta_info.get("batch_id") or f"verl-batch-{uuid.uuid4().hex[:8]}")

    def _batch_size(self, batch: Any) -> int:
        if isinstance(batch, dict):
            explicit_len = batch.get("len")
            if explicit_len is not None:
                return int(explicit_len)
            batch_size = batch.get("batch_size")
            if isinstance(batch_size, (list, tuple)) and batch_size:
                return int(batch_size[0])
            if batch_size is not None:
                return int(batch_size)
            return self._batch_size_from_fields(batch)

        if hasattr(batch, "__len__"):
            try:
                return len(batch)
            except TypeError:
                pass
        return self._batch_size_from_fields(batch)

    def _batch_size_from_fields(self, batch: Any) -> int:
        tensor_batch = self._tensor_batch(batch)
        batch_size = getattr(tensor_batch, "batch_size", None)
        if batch_size is not None:
            return int(batch_size[0])
        non_tensor_batch = self._non_tensor_batch(batch)
        for value in non_tensor_batch.values():
            try:
                return len(value)
            except TypeError:
                continue
        return 0

    def _meta_info(self, batch: Any) -> dict[str, Any]:
        if isinstance(batch, dict):
            return dict(batch.get("meta_info") or {})
        return dict(getattr(batch, "meta_info", {}) or {})

    def _non_tensor_batch(self, batch: Any) -> dict[str, Any]:
        if isinstance(batch, dict):
            return dict(batch.get("non_tensor_batch") or {})
        return dict(getattr(batch, "non_tensor_batch", {}) or {})

    def _tensor_batch(self, batch: Any) -> Any:
        if isinstance(batch, dict):
            tensor_batch = batch.get("batch")
            return tensor_batch if tensor_batch is not None else {}
        tensor_batch = getattr(batch, "batch", None)
        return tensor_batch if tensor_batch is not None else {}

    def _initial_observation(self, tensor_batch: Any, sample: dict[str, Any], idx: int) -> dict[str, Any]:
        observation: dict[str, Any] = {}
        keys = ["input_ids", "prompts", "responses", "attention_mask", "response_mask", "position_ids"]
        for key in keys:
            value = None
            try:
                value = tensor_batch[key][idx]
            except Exception:
                pass
            if value is not None:
                observation[key] = to_jsonable(value)

        raw_prompt = sample.get("raw_prompt")
        if raw_prompt is not None:
            observation["raw_prompt"] = raw_prompt
            observation["prompt_text"] = prompt_text(raw_prompt)
        if sample.get("uenv_response_text") is not None:
            observation["response_text"] = str(sample.get("uenv_response_text"))
        return observation

    def _response_shape(self, batch: Any) -> tuple[int, int]:
        tensor_batch = self._tensor_batch(batch)
        responses = None
        try:
            responses = tensor_batch["responses"]
        except Exception:
            pass
        if responses is not None:
            return tuple(responses.shape)
        # Pre-rollout dry-run batches do not have response tokens yet; use a
        # single-token placeholder so result conversion still has a stable shape.
        return (self._batch_size(batch), 1)

    def _last_response_index(self, batch: Any, idx: int) -> int:
        tensor_batch = self._tensor_batch(batch)
        # In real rollout batches response_mask identifies generated tokens;
        # older fixtures may only have responses, so fall back to the final column.
        try:
            response_mask = tensor_batch["response_mask"][idx]
        except Exception:
            response_shape = self._response_shape(batch)
            return max(response_shape[1] - 1, 0)

        try:
            valid_length = int(response_mask.sum().item())
        except Exception:
            valid_length = len(response_mask)
        return max(valid_length - 1, 0)

    def _import_torch_numpy(self) -> tuple[Any, Any]:
        try:
            import numpy as np
            import torch
        except Exception as exc:
            raise RuntimeError("results_to_dataproto requires torch and numpy in the VeRL runtime") from exc
        return torch, np

    def _trajectory_to_jsonable(self, response: EpisodeResult) -> list[dict[str, Any]]:
        trajectory = []
        for step in response.trajectory.steps:
            trajectory.append(
                {
                    "step_index": step.step_index,
                    "observation": step.observation.decode("utf-8", errors="replace"),
                    "action": step.action.decode("utf-8", errors="replace"),
                    "reward": step.reward,
                    "terminated": step.terminated,
                    "truncated": step.truncated,
                    "info": dict(step.info),
                    "duration_ms": step.duration_ms,
                }
            )
        return trajectory
