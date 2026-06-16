from __future__ import annotations

import asyncio
import inspect
import json
import time
import uuid
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from .agent_loop_clients import build_agent_loop_episode_client
from .clients import EpisodeClient
from .protocol import EpisodeRequest, EpisodeResult, MODE_MULTI, ResourceSpec, request_to_jsonable
from .utils import prompt_text, to_jsonable

try:
    from verl.experimental.agent_loop.agent_loop import AgentLoopBase, AgentLoopMetrics, AgentLoopOutput, register
    from verl.utils.profiler import simple_timer
    from verl.utils.rollout_trace import rollout_trace_op
except Exception:

    class AgentLoopBase:
        def __init__(
            self,
            trainer_config: Any = None,
            server_manager: Any = None,
            tokenizer: Any = None,
            processor: Any = None,
            dataset_cls: Any = None,
            data_config: Any = None,
            **_kwargs: Any,
        ) -> None:
            self.config = getattr(trainer_config, "config", trainer_config)
            self.server_manager = server_manager
            self.tokenizer = tokenizer
            self.processor = processor
            self.dataset_cls = dataset_cls
            self.data_config = getattr(data_config, "config", data_config) or {}
            self.rollout_config = getattr(getattr(self.config, "actor_rollout_ref", None), "rollout", None)

        async def apply_chat_template(self, messages: list[dict[str, Any]], **_kwargs: Any) -> list[int]:
            if self.tokenizer is None:
                return []
            if hasattr(self.tokenizer, "apply_chat_template"):
                return list(
                    self.tokenizer.apply_chat_template(
                        messages,
                        tokenize=True,
                        add_generation_prompt=True,
                    )
                )
            return list(self.tokenizer.encode(prompt_text(messages), add_special_tokens=False))

    @dataclass(slots=True)
    class AgentLoopMetrics:
        generate_sequences: float = 0.0
        tool_calls: float = 0.0
        compute_score: float = 0.0
        num_preempted: int = -1

        def model_dump(self) -> dict[str, Any]:
            return {
                "generate_sequences": self.generate_sequences,
                "tool_calls": self.tool_calls,
                "compute_score": self.compute_score,
                "num_preempted": self.num_preempted,
            }

    @dataclass(slots=True)
    class AgentLoopOutput:
        prompt_ids: list[int]
        response_ids: list[int]
        response_mask: list[int]
        metrics: AgentLoopMetrics
        reward_score: float | None = None
        num_turns: int = 0
        response_logprobs: list[float] | None = None
        routed_experts: Any = None
        multi_modal_data: dict[str, Any] | None = None
        extra_fields: dict[str, Any] = field(default_factory=dict)
        mm_processor_kwargs: dict[str, Any] | None = None

    def register(_agent_name: str):
        def decorator(cls):
            return cls

        return decorator

    def simple_timer(name: str, metrics: dict[str, float]):
        class Timer:
            def __enter__(self):
                self.start = time.perf_counter()
                return self

            def __exit__(self, _exc_type, _exc, _traceback):
                metrics[name] = time.perf_counter() - self.start

        return Timer()

    def rollout_trace_op(func):
        return func


def _optional_string(value: Any) -> str | None:
    if value is None:
        return None
    text = str(value)
    if text in {"", "None", "none", "null", "NULL"}:
        return None
    return text


def _float_value(value: Any, default: float) -> float:
    if value is None:
        return default
    return float(value)


def _int_value(value: Any, default: int) -> int:
    if value is None:
        return default
    return int(value)


def _bool_value(value: Any, default: bool = False) -> bool:
    if value is None:
        return default
    if isinstance(value, bool):
        return value
    return str(value).strip().lower() not in {"", "0", "false", "no", "off"}


@dataclass(slots=True)
class UEnvAgentLoopConfig:
    client_mode: str = "fake"
    endpoint: str = "127.0.0.1:50051"
    timeout_seconds: float = 300.0
    startup_timeout_seconds: float = 30.0
    auto_start: bool = False
    binary: str | None = None
    fake_reward: float = 1.0
    fake_response_text: str = ""
    default_env_type: str = "math"
    default_model_endpoint: str = "http://uenv-rollout.default.svc:8000/v1"
    default_model_name: str = "policy-model"
    default_max_steps: int = 10
    default_max_turns: int = 1
    seed_base: int = 42
    request_record_path: str = ""


@register("uenv_agent")
class UEnvAgentLoop(AgentLoopBase):
    """VeRL AgentLoop bridge for pre-rollout handoff to UEnv.

    This class is the Route A hook. It runs inside VeRL's rollout worker before
    local vLLM generation would normally happen, builds one PRD-style
    EpisodeRequest, then expects UEnv Server/Worker to return the complete
    rollout result: response tokens/text, trajectory and reward.
    """

    def __init__(
        self,
        *args: Any,
        client: EpisodeClient | None = None,
        mode: str | None = None,
        client_mode: str | None = None,
        endpoint: str | None = None,
        timeout_seconds: float | None = None,
        startup_timeout_seconds: float | None = None,
        auto_start: bool | None = None,
        binary: str | None = None,
        fake_reward: float | None = None,
        fake_response_text: str | None = None,
        default_env_type: str = "math",
        default_model_endpoint: str = "http://uenv-rollout.default.svc:8000/v1",
        default_model_name: str = "policy-model",
        default_max_steps: int = 10,
        default_max_turns: int = 1,
        seed_base: int = 42,
        request_record_path: str = "",
        **kwargs: Any,
    ) -> None:
        super().__init__(*args, **kwargs)
        client_mode = client_mode or mode
        self.config_for_uenv = UEnvAgentLoopConfig(
            client_mode=_optional_string(client_mode) or "fake",
            endpoint=_optional_string(endpoint) or "127.0.0.1:50051",
            timeout_seconds=_float_value(timeout_seconds, 300.0),
            startup_timeout_seconds=_float_value(startup_timeout_seconds, 30.0),
            auto_start=_bool_value(auto_start, False),
            binary=_optional_string(binary),
            fake_reward=_float_value(fake_reward, 1.0),
            fake_response_text=_optional_string(fake_response_text) or "",
            default_env_type=default_env_type,
            default_model_endpoint=default_model_endpoint,
            default_model_name=default_model_name,
            default_max_steps=_int_value(default_max_steps, 10),
            default_max_turns=_int_value(default_max_turns, 1),
            seed_base=_int_value(seed_base, 42),
            request_record_path=_optional_string(request_record_path) or "",
        )
        self.client = client or build_agent_loop_episode_client(
            mode=self.config_for_uenv.client_mode,
            endpoint=self.config_for_uenv.endpoint,
            timeout_seconds=self.config_for_uenv.timeout_seconds,
            startup_timeout_seconds=self.config_for_uenv.startup_timeout_seconds,
            auto_start=self.config_for_uenv.auto_start,
            binary=self.config_for_uenv.binary,
            fake_reward=self.config_for_uenv.fake_reward,
            fake_response_text=self.config_for_uenv.fake_response_text,
        )

    @rollout_trace_op
    async def run(self, sampling_params: dict[str, Any], **kwargs: Any) -> AgentLoopOutput:
        messages = self._messages_from_raw_prompt(kwargs.get("raw_prompt"))
        prompt_ids = await self._prompt_ids(messages)
        runtime_model = await self._runtime_model_endpoint(sampling_params, kwargs)

        request = self.build_episode_request(
            sampling_params=sampling_params,
            prompt_ids=prompt_ids,
            raw_prompt=kwargs.get("raw_prompt"),
            sample_kwargs=kwargs,
            model_endpoint_override=runtime_model[0],
            model_name_override=runtime_model[1],
        )

        metrics: dict[str, float] = {}
        with simple_timer("generate_sequences", metrics):
            self._record_episode_requests([request], phase="submit_single")
            result = await asyncio.to_thread(self.client.submit_episode, request)

        if result.status not in {"completed", "recorded"}:
            raise RuntimeError(
                f"UEnv pre-rollout episode failed: request_id={result.request_id} "
                f"status={result.status} error={result.error_message}"
            )

        response_ids = self._response_ids_from_result(result)
        max_response_length = self._rollout_response_length()
        response_ids = response_ids[:max_response_length] if max_response_length else response_ids
        if not response_ids:
            response_ids = [self._pad_token_id()]

        response_mask = self._response_mask_from_result(result, len(response_ids))
        response_mask = response_mask[: len(response_ids)]
        if len(response_mask) < len(response_ids):
            response_mask.extend([1] * (len(response_ids) - len(response_mask)))

        agent_metrics = AgentLoopMetrics(
            generate_sequences=float(metrics.get("generate_sequences", 0.0)),
            tool_calls=0.0,
            compute_score=0.0,
            num_preempted=-1,
        )
        return AgentLoopOutput(
            prompt_ids=prompt_ids,
            response_ids=response_ids,
            response_mask=response_mask,
            reward_score=float(result.summary.total_reward),
            num_turns=max(result.trajectory.total_steps + 1, 2),
            metrics=agent_metrics,
            extra_fields={
                "uenv_request_id": result.request_id,
                "uenv_status": result.status,
                "uenv_termination_reason": result.summary.terminate_reason or result.status,
                "uenv_trajectory": self._trajectory_to_jsonable(result),
                "turn_scores": [],
                "tool_rewards": [],
            },
        )

    def build_episode_request(
        self,
        *,
        sampling_params: dict[str, Any],
        prompt_ids: list[int],
        raw_prompt: Any,
        sample_kwargs: dict[str, Any],
        model_endpoint_override: str | None = None,
        model_name_override: str | None = None,
    ) -> EpisodeRequest:
        request_id = str(uuid.uuid4())
        env_type = self._env_type(sample_kwargs)
        max_steps = int(self._value_from_extra_info(sample_kwargs, "max_steps", self.config_for_uenv.default_max_steps))
        sample_index = self._sample_index(sample_kwargs)
        seed = int(self._value_from_extra_info(sample_kwargs, "seed", self.config_for_uenv.seed_base + sample_index))
        batch_id = str(self._value_from_extra_info(sample_kwargs, "batch_id", f"verl-agent-loop-{uuid.uuid4().hex[:8]}"))
        reward_model = sample_kwargs.get("reward_model")
        data_source = self._string_or_none(sample_kwargs.get("data_source"))
        task_name = self._task_name(sample_kwargs, env_type)
        prompt_as_text = prompt_text(raw_prompt)
        model_endpoint = model_endpoint_override or self._model_endpoint(sample_kwargs, sampling_params)
        model_name = model_name_override or self._model_name(sample_kwargs, sampling_params)

        metadata = {
            "batch_id": batch_id,
            "sample_index": sample_index,
            "uid": self._string_or_none(sample_kwargs.get("uid")),
            "index": self._jsonable(sample_kwargs.get("index")),
            "task_name": task_name,
            "data_source": data_source,
            "ability": self._string_or_none(sample_kwargs.get("ability")),
            "extra_info": self._jsonable(sample_kwargs.get("extra_info") or {}),
            "rollout_n": self._value_from_extra_info(sample_kwargs, "rollout_n", None),
            "global_steps": self._value_from_extra_info(sample_kwargs, "global_steps", None),
            "required_result_fields": [
                "response_ids",
                "response_mask",
                "response_text",
                "reward",
                "trajectory",
                "finish_reason",
            ],
        }
        generation_config = {
            "temperature": sampling_params.get("temperature"),
            "top_p": sampling_params.get("top_p"),
            "top_k": sampling_params.get("top_k"),
            "logprobs": sampling_params.get("logprobs"),
            "max_new_tokens": self._rollout_response_length(),
        }

        payload = {
            "protocol_version": "1.0",
            "framework": "verl",
            "correlation_id": f"{batch_id}-{sample_index}",
            "request_ts": time.time(),
            "env_config": {
                "task_name": task_name,
                "data_source": data_source,
                "raw_prompt": prompt_as_text,
            },
            "model_endpoint": {
                "endpoint_type": "http",
                "url": model_endpoint,
                "model_name": model_name,
                "generation_config": {key: value for key, value in generation_config.items() if value is not None},
                "max_retries": 3,
            },
            "episode_config": {
                "max_steps": max_steps,
                "max_turns": int(self.config_for_uenv.default_max_turns),
                "seed": seed,
                "initial_observation": {
                    "raw_prompt": self._jsonable(raw_prompt),
                    "prompt_text": prompt_as_text,
                    "prompt_ids": prompt_ids,
                    "token_source": "verl_agent_loop",
                },
                "stop_conditions": ["done", "max_steps", "timeout"],
            },
            "reward_config": {
                "reward_type": "rubric" if env_type == "math" else "external",
                "rubric_config": self._jsonable(reward_model),
            },
            "metadata": metadata,
            "timeout_seconds": self.config_for_uenv.timeout_seconds,
        }

        return EpisodeRequest(
            request_id=request_id,
            env_type=env_type,
            payload=json.dumps(to_jsonable(payload), ensure_ascii=False, separators=(",", ":")).encode("utf-8"),
            mode=MODE_MULTI,
            max_steps=max_steps,
            resource_spec=ResourceSpec(),
            model_endpoint=model_endpoint,
            seed=seed,
        )

    def _record_episode_requests(self, requests: list[EpisodeRequest], *, phase: str) -> None:
        if not self.config_for_uenv.request_record_path:
            return

        path = Path(self.config_for_uenv.request_record_path)
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("a", encoding="utf-8") as file:
            for request in requests:
                payload = self._payload_dict(request)
                metadata = payload.get("metadata") if isinstance(payload.get("metadata"), dict) else {}
                model_endpoint = payload.get("model_endpoint") if isinstance(payload.get("model_endpoint"), dict) else {}
                initial_observation = {}
                episode_config = payload.get("episode_config")
                if isinstance(episode_config, dict) and isinstance(episode_config.get("initial_observation"), dict):
                    initial_observation = episode_config["initial_observation"]
                record = {
                    "ts": time.time(),
                    "phase": phase,
                    "request_id": request.request_id,
                    "batch_id": metadata.get("batch_id"),
                    "sample_index": metadata.get("sample_index"),
                    "env_type": request.env_type,
                    "mode": request.mode,
                    "max_steps": request.max_steps,
                    "seed": request.seed,
                    "model_endpoint": request.model_endpoint,
                    "payload_model_endpoint": model_endpoint,
                    "generation_config": model_endpoint.get("generation_config", {}),
                    "prompt_text": initial_observation.get("prompt_text") or payload.get("env_config", {}).get("raw_prompt"),
                    "request": request_to_jsonable(request),
                    "payload": payload,
                }
                file.write(json.dumps(to_jsonable(record), ensure_ascii=False, separators=(",", ":")) + "\n")

    def _payload_dict(self, request: EpisodeRequest) -> dict[str, Any]:
        try:
            value = json.loads(request.payload.decode("utf-8", errors="replace"))
        except Exception:
            return {}
        return value if isinstance(value, dict) else {}

    async def _prompt_ids(self, messages: list[dict[str, Any]]) -> list[int]:
        prompt_ids = await self.apply_chat_template(messages)
        return [int(token_id) for token_id in prompt_ids]

    def _messages_from_raw_prompt(self, raw_prompt: Any) -> list[dict[str, Any]]:
        value = self._python_value(raw_prompt)
        if isinstance(value, list):
            messages = []
            for item in value:
                if isinstance(item, dict):
                    messages.append(item)
                else:
                    messages.append({"role": "user", "content": str(item)})
            return messages
        if isinstance(value, dict):
            return [value]
        return [{"role": "user", "content": "" if value is None else str(value)}]

    def _response_ids_from_result(self, result: EpisodeResult) -> list[int]:
        for step in reversed(result.trajectory.steps):
            ids = self._ids_from_info(step.info, "response_ids")
            if ids:
                return ids
            text = step.info.get("response_text") or step.action.decode("utf-8", errors="replace")
            ids = self._encode_response_text(text)
            if ids:
                return ids
        return self._encode_response_text("")

    def _response_mask_from_result(self, result: EpisodeResult, fallback_len: int) -> list[int]:
        for step in reversed(result.trajectory.steps):
            mask = self._ids_from_info(step.info, "response_mask")
            if mask:
                return [1 if item else 0 for item in mask]
        return [1] * fallback_len

    def _ids_from_info(self, info: dict[str, str], key: str) -> list[int]:
        raw = info.get(key)
        if raw is None:
            return []
        try:
            value = json.loads(raw)
        except Exception:
            value = raw
        if not isinstance(value, list):
            return []
        ids = []
        for item in value:
            try:
                ids.append(int(item))
            except Exception:
                return []
        return ids

    def _encode_response_text(self, text: str) -> list[int]:
        if self.tokenizer is None:
            return [ord(char) for char in text] or [0]
        if hasattr(self.tokenizer, "encode"):
            try:
                return [int(token_id) for token_id in self.tokenizer.encode(text, add_special_tokens=False)]
            except TypeError:
                return [int(token_id) for token_id in self.tokenizer.encode(text)]
        return [ord(char) for char in text] or [0]

    def _rollout_response_length(self) -> int:
        for name in ("response_length", "max_response_length"):
            value = getattr(getattr(self, "rollout_config", None), name, None)
            if value is not None:
                return int(value)
        return 0

    def _pad_token_id(self) -> int:
        value = getattr(self.tokenizer, "pad_token_id", None)
        return int(value) if value is not None else 0

    def _env_type(self, sample_kwargs: dict[str, Any]) -> str:
        candidates = [
            sample_kwargs.get("task_name"),
            sample_kwargs.get("ability"),
            sample_kwargs.get("data_source"),
        ]
        lowered = " ".join(str(self._python_value(item) or "").lower() for item in candidates)
        if "gsm8k" in lowered or "math" in lowered:
            return "math"
        if "humaneval" in lowered or "mbpp" in lowered or "code" in lowered:
            return "code"
        if "agent" in lowered:
            return "agent"
        return self.config_for_uenv.default_env_type

    def _task_name(self, sample_kwargs: dict[str, Any], env_type: str) -> str:
        for key in ("task_name", "ability", "data_source"):
            value = self._string_or_none(sample_kwargs.get(key))
            if value:
                return value
        return env_type

    def _model_endpoint(self, sample_kwargs: dict[str, Any], sampling_params: dict[str, Any]) -> str:
        endpoint = (
            self._value_from_extra_info(sample_kwargs, "model_endpoint", None)
            or sampling_params.get("model_endpoint")
            or self.config_for_uenv.default_model_endpoint
        )
        return str(endpoint)

    def _model_name(self, sample_kwargs: dict[str, Any], sampling_params: dict[str, Any]) -> str:
        model_name = (
            self._value_from_extra_info(sample_kwargs, "model_name", None)
            or sampling_params.get("model_name")
            or self.config_for_uenv.default_model_name
        )
        return str(model_name)

    async def _runtime_model_endpoint(
        self,
        sampling_params: dict[str, Any],
        sample_kwargs: dict[str, Any],
    ) -> tuple[str | None, str | None]:
        explicit_endpoint = (
            self._value_from_extra_info(sample_kwargs, "model_endpoint", None)
            or sampling_params.get("model_endpoint")
        )
        if explicit_endpoint:
            return str(explicit_endpoint), self._model_name(sample_kwargs, sampling_params)

        for candidate in await self._runtime_model_endpoint_candidates():
            endpoint = self._normalize_openai_endpoint(candidate)
            if endpoint:
                return endpoint, self._runtime_model_name(sample_kwargs, sampling_params)
        return None, None

    def _runtime_model_name(
        self,
        sample_kwargs: dict[str, Any],
        sampling_params: dict[str, Any],
    ) -> str:
        explicit_name = self._value_from_extra_info(sample_kwargs, "model_name", None) or sampling_params.get("model_name")
        if explicit_name:
            return str(explicit_name)

        candidates = [
            self._nested_value(self.config, ("actor_rollout_ref", "rollout", "prometheus", "served_model_name")),
            self._nested_value(self.rollout_config, ("prometheus", "served_model_name")),
            self._nested_value(self.config, ("actor_rollout_ref", "model", "path")),
            getattr(self.tokenizer, "name_or_path", None),
        ]
        for candidate in candidates:
            text = str(candidate or "").strip()
            if text:
                return text
        return self.config_for_uenv.default_model_name

    def _nested_value(self, value: Any, path: tuple[str, ...]) -> Any:
        current = value
        for key in path:
            if current is None:
                return None
            if isinstance(current, dict):
                current = current.get(key)
                continue
            getter = getattr(current, "get", None)
            if callable(getter):
                try:
                    current = getter(key)
                    continue
                except Exception:
                    pass
            current = getattr(current, key, None)
        return current

    async def _runtime_model_endpoint_candidates(self) -> list[Any]:
        manager = getattr(self, "server_manager", None)
        if manager is None:
            return []

        values: list[Any] = []
        for attr in ("server_addresses", "addresses"):
            value = getattr(manager, attr, None)
            if value:
                values.extend(value if isinstance(value, (list, tuple, set)) else [value])

        for method_name in ("get_addresses", "get_server_addresses"):
            method = getattr(manager, method_name, None)
            if callable(method):
                try:
                    value = method()
                    if inspect.isawaitable(value):
                        value = await value
                    if value:
                        values.extend(value if isinstance(value, (list, tuple, set)) else [value])
                except Exception:
                    pass

        load_balancer = getattr(manager, "_load_balancer", None)
        get_all_servers = getattr(load_balancer, "get_all_servers", None)
        remote = getattr(get_all_servers, "remote", None)
        if callable(remote):
            try:
                values.extend(await self._await_ray_value(remote()))
            except Exception:
                pass

        return values

    async def _await_ray_value(self, value: Any) -> list[Any]:
        if inspect.isawaitable(value):
            try:
                resolved = await value
            except Exception:
                return []
            if isinstance(resolved, (list, tuple, set)):
                return list(resolved)
            return [resolved]
        try:
            import ray  # type: ignore

            resolved = await asyncio.to_thread(ray.get, value)
        except Exception:
            return []
        if isinstance(resolved, (list, tuple, set)):
            return list(resolved)
        return [resolved]

    def _normalize_openai_endpoint(self, value: Any) -> str | None:
        text = str(value or "").strip()
        if not text:
            return None
        if text.startswith(("http://", "https://")):
            base = text.rstrip("/")
        else:
            base = f"http://{text.rstrip('/')}"
        if base.endswith("/v1"):
            return base
        return f"{base}/v1"

    def _sample_index(self, sample_kwargs: dict[str, Any]) -> int:
        value = self._value_from_extra_info(
            sample_kwargs,
            "sample_index",
            self._value_from_extra_info(sample_kwargs, "index", sample_kwargs.get("index", 0)),
        )
        try:
            return int(self._python_value(value))
        except Exception:
            return 0

    def _value_from_extra_info(self, sample_kwargs: dict[str, Any], key: str, default: Any) -> Any:
        extra_info = self._python_value(sample_kwargs.get("extra_info") or {})
        if isinstance(extra_info, dict) and key in extra_info:
            return extra_info[key]
        if key in sample_kwargs:
            return sample_kwargs[key]
        return default

    def _trajectory_to_jsonable(self, result: EpisodeResult) -> list[dict[str, Any]]:
        output = []
        for step in result.trajectory.steps:
            output.append(
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
        return output

    def _jsonable(self, value: Any) -> Any:
        return to_jsonable(self._python_value(value))

    def _string_or_none(self, value: Any) -> str | None:
        value = self._python_value(value)
        if value is None:
            return None
        return str(value)

    def _python_value(self, value: Any) -> Any:
        if hasattr(value, "item"):
            try:
                return value.item()
            except Exception:
                pass
        if hasattr(value, "tolist"):
            try:
                return value.tolist()
            except Exception:
                pass
        return value
