from __future__ import annotations

import asyncio
import inspect
import json
import logging
import os
import time
import uuid
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from .agent_loop_clients import build_agent_loop_episode_client
from .clients import EpisodeClient
from .protocol import EpisodeRequest, EpisodeResult, MODE_MULTI, ResourceSpec
from .utils import prompt_text, to_jsonable

logger = logging.getLogger(__name__)

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
    client_mode: str = "rust_core"
    endpoint: str = "127.0.0.1:50051"
    timeout_seconds: float = 300.0
    startup_timeout_seconds: float = 30.0
    auto_start: bool = False
    binary: str | None = None
    fake_reward: float = 1.0
    fake_response_text: str = ""
    default_env_type: str = "math"
    default_model_endpoint: str = "https://openrouter.ai/api/v1"
    default_model_name: str = "qwen/qwen-2.5-7b-instruct"
    default_max_steps: int = 10
    default_max_turns: int = 1
    seed_base: int = 42
    result_record_path: str = ""
    request_record_path: str = ""
    batch_size: int = 0
    batch_retry_attempts: int = 3
    batch_retry_delay_seconds: float = 5.0


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
        default_model_endpoint: str = "https://openrouter.ai/api/v1",
        default_model_name: str = "qwen/qwen-2.5-7b-instruct",
        default_max_steps: int = 10,
        default_max_turns: int = 1,
        seed_base: int = 42,
        result_record_path: str = "",
        request_record_path: str = "",
        batch_size: int | None = None,
        batch_retry_attempts: int | None = None,
        batch_retry_delay_seconds: float | None = None,
        **kwargs: Any,
    ) -> None:
        super().__init__(*args, **kwargs)
        client_mode = client_mode or mode
        self.config_for_uenv = UEnvAgentLoopConfig(
            client_mode=_optional_string(client_mode) or "rust_core",
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
            result_record_path=_optional_string(result_record_path) or "",
            request_record_path=_optional_string(request_record_path) or "",
            batch_size=_int_value(batch_size, 0),
            batch_retry_attempts=_int_value(batch_retry_attempts, 3),
            batch_retry_delay_seconds=_float_value(batch_retry_delay_seconds, 5.0),
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

        return self.agent_loop_output_from_result(
            request=request,
            result=result,
            prompt_ids=prompt_ids,
            generate_seconds=float(metrics.get("generate_sequences", 0.0)),
        )

    async def run_batch(
        self,
        sampling_params_by_sample: list[dict[str, Any]],
        sample_kwargs_by_sample: list[dict[str, Any]],
        *,
        batch_id: str | None = None,
    ) -> list[AgentLoopOutput]:
        if len(sampling_params_by_sample) != len(sample_kwargs_by_sample):
            raise ValueError("sampling_params_by_sample and sample_kwargs_by_sample must have the same length")
        if not sample_kwargs_by_sample:
            return []

        common_batch_id = batch_id or f"verl-agent-loop-batch-{uuid.uuid4().hex[:8]}"
        requests: list[EpisodeRequest] = []
        prompt_ids_by_request: dict[str, list[int]] = {}
        logger.info(
            "uenv_run_batch_build_start batch_id=%s sample_count=%s micro_batch_size=%s timeout_seconds=%s",
            common_batch_id,
            len(sample_kwargs_by_sample),
            self.config_for_uenv.batch_size or len(sample_kwargs_by_sample),
            self.config_for_uenv.timeout_seconds,
        )

        for sample_index, (sampling_params, sample_kwargs) in enumerate(
            zip(sampling_params_by_sample, sample_kwargs_by_sample, strict=True)
        ):
            sample_kwargs = self._sample_kwargs_for_batch(sample_kwargs, batch_id=common_batch_id, sample_index=sample_index)
            messages = self._messages_from_raw_prompt(sample_kwargs.get("raw_prompt"))
            prompt_ids = await self._prompt_ids(messages)
            runtime_model = await self._runtime_model_endpoint(sampling_params, sample_kwargs)
            request = self.build_episode_request(
                sampling_params=sampling_params,
                prompt_ids=prompt_ids,
                raw_prompt=sample_kwargs.get("raw_prompt"),
                sample_kwargs=sample_kwargs,
                model_endpoint_override=runtime_model[0],
                model_name_override=runtime_model[1],
            )
            requests.append(request)
            prompt_ids_by_request[request.request_id] = prompt_ids

        start = time.perf_counter()
        self._record_episode_requests(requests, phase="submit_batch")
        results = await asyncio.to_thread(lambda: self._submit_episode_batch(requests))
        elapsed = time.perf_counter() - start
        logger.info(
            "uenv_run_batch_submit_done batch_id=%s sample_count=%s elapsed_s=%.3f",
            common_batch_id,
            len(requests),
            elapsed,
        )

        expected_ids = {request.request_id for request in requests}
        result_by_id = {result.request_id: result for result in results}
        if set(result_by_id) != expected_ids:
            missing = sorted(expected_ids - set(result_by_id))
            extra = sorted(set(result_by_id) - expected_ids)
            raise RuntimeError(f"UEnv batch episode result mismatch: missing={missing} extra={extra}")

        per_sample_seconds = elapsed / max(len(requests), 1)
        return [
            self.agent_loop_output_from_result(
                request=request,
                result=result_by_id[request.request_id],
                prompt_ids=prompt_ids_by_request[request.request_id],
                generate_seconds=per_sample_seconds,
            )
            for request in requests
        ]

    def agent_loop_output_from_result(
        self,
        *,
        request: EpisodeRequest,
        result: EpisodeResult,
        prompt_ids: list[int],
        generate_seconds: float,
    ) -> AgentLoopOutput:
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

        self._record_episode_result(request, result, response_ids=response_ids, response_mask=response_mask)

        agent_metrics = AgentLoopMetrics(
            generate_sequences=float(generate_seconds),
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

    def _submit_episode_batch(self, requests: list[EpisodeRequest]) -> list[EpisodeResult]:
        results_by_id: dict[str, EpisodeResult] = {}
        chunk_size = self.config_for_uenv.batch_size
        for chunk in self._request_chunks(requests, chunk_size if chunk_size > 0 else len(requests)):
            self._submit_episode_chunk_with_retry(chunk, results_by_id)
        return [results_by_id[request.request_id] for request in requests]

    def _submit_episode_chunk_with_retry(
        self,
        requests: list[EpisodeRequest],
        results_by_id: dict[str, EpisodeResult],
    ) -> None:
        attempts = max(self.config_for_uenv.batch_retry_attempts, 0)
        delay_seconds = max(self.config_for_uenv.batch_retry_delay_seconds, 0.0)
        pending = list(requests)

        for attempt in range(attempts + 1):
            chunk_start = time.perf_counter()
            logger.info(
                "uenv_submit_chunk_start sample_count=%s attempt=%s request_ids=%s",
                len(pending),
                attempt,
                ",".join(request.request_id for request in pending[:5]),
            )
            results = list(self.client.submit_episode_stream(pending))
            logger.info(
                "uenv_submit_chunk_done sample_count=%s attempt=%s elapsed_s=%.3f",
                len(pending),
                attempt,
                time.perf_counter() - chunk_start,
            )
            result_by_id = {result.request_id: result for result in results}
            missing_ids = [request.request_id for request in pending if request.request_id not in result_by_id]
            if missing_ids:
                raise RuntimeError(f"UEnv batch episode result mismatch: missing={missing_ids} extra=[]")

            retry_requests = []
            for request in pending:
                result = result_by_id[request.request_id]
                if self._is_capacity_failure(result):
                    retry_requests.append(request)
                else:
                    results_by_id[request.request_id] = result

            if not retry_requests:
                return

            if len(retry_requests) > 1:
                for chunk in self._split_requests(retry_requests):
                    self._submit_episode_chunk_with_retry(chunk, results_by_id)
                return

            if attempt < attempts and delay_seconds:
                time.sleep(delay_seconds)
            pending = retry_requests

        for request in pending:
            results_by_id[request.request_id] = result_by_id[request.request_id]

    def _is_capacity_failure(self, result: EpisodeResult) -> bool:
        if result.status not in {"failed", "error"}:
            return False
        message = (result.error_message or "").lower()
        return "no worker available" in message or "all workers at capacity" in message

    def _request_chunks(self, requests: list[EpisodeRequest], chunk_size: int) -> list[list[EpisodeRequest]]:
        if not requests:
            return []
        size = max(int(chunk_size), 1)
        return [requests[index : index + size] for index in range(0, len(requests), size)]

    def _split_requests(self, requests: list[EpisodeRequest]) -> list[list[EpisodeRequest]]:
        if len(requests) <= 1:
            return [requests]
        midpoint = max(len(requests) // 2, 1)
        return [requests[:midpoint], requests[midpoint:]]

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
        extra_info = self._jsonable(sample_kwargs.get("extra_info") or {})
        worker_question = self._worker_llm_question(raw_prompt, prompt_as_text)
        if worker_question:
            extra_info["question"] = worker_question

        metadata = {
            "batch_id": batch_id,
            "sample_index": sample_index,
            "uid": self._string_or_none(sample_kwargs.get("uid")),
            "index": self._jsonable(sample_kwargs.get("index")),
            "task_name": task_name,
            "data_source": data_source,
            "ability": self._string_or_none(sample_kwargs.get("ability")),
            "extra_info": extra_info,
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

    async def _prompt_ids(self, messages: list[dict[str, Any]]) -> list[int]:
        prompt_ids = await self.apply_chat_template(messages)
        return [int(token_id) for token_id in prompt_ids]

    def _worker_llm_question(self, raw_prompt: Any, prompt_as_text: str) -> str:
        """Full user prompt for Worker LLM (GSM8K includes #### instruction)."""
        for message in reversed(self._messages_from_raw_prompt(raw_prompt)):
            if message.get("role") == "user":
                content = message.get("content")
                if content is not None and str(content).strip():
                    return str(content).strip()
        return prompt_as_text.strip()

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
        output: list[int] = []
        for step in result.trajectory.steps:
            ids = self._ids_from_info(step.info, "response_ids")
            if ids:
                output.extend(ids)
                continue

            text = self._step_response_text(step)
            if not text:
                continue
            if output:
                text = "\n" + text
            output.extend(self._encode_response_text(text))
        return output or self._encode_response_text("")

    def _response_mask_from_result(self, result: EpisodeResult, fallback_len: int) -> list[int]:
        output: list[int] = []
        token_count = 0
        for step in result.trajectory.steps:
            ids = self._ids_from_info(step.info, "response_ids")
            if ids:
                mask = [1 if item else 0 for item in self._ids_from_info(step.info, "response_mask")]
                if len(mask) < len(ids):
                    mask.extend([1] * (len(ids) - len(mask)))
                output.extend(mask[: len(ids)])
                token_count += len(ids)
                continue

            text = self._step_response_text(step)
            if not text:
                continue
            if token_count:
                text = "\n" + text
            segment_len = len(self._encode_response_text(text))
            output.extend([1] * segment_len)
            token_count += segment_len
        return output or [1] * fallback_len

    def _record_episode_result(
        self,
        request: EpisodeRequest,
        result: EpisodeResult,
        *,
        response_ids: list[int],
        response_mask: list[int],
    ) -> None:
        path_text = self.config_for_uenv.result_record_path
        if not path_text:
            return

        payload = self._payload_from_request(request)
        metadata = payload.get("metadata") if isinstance(payload, dict) else {}
        if not isinstance(metadata, dict):
            metadata = {}
        model_endpoint = payload.get("model_endpoint") if isinstance(payload, dict) else {}
        if not isinstance(model_endpoint, dict):
            model_endpoint = {}

        record = {
            "ts": time.time(),
            "request_id": result.request_id,
            "status": result.status,
            "batch_id": metadata.get("batch_id"),
            "sample_index": metadata.get("sample_index"),
            "request_model_endpoint": request.model_endpoint,
            "request_model_name": model_endpoint.get("model_name"),
            "reward": result.summary.total_reward,
            "total_steps": result.trajectory.total_steps,
            "termination_reason": result.summary.terminate_reason,
            "response_text": self._result_response_text(result),
            "response_ids": self._result_response_ids(result),
            "verl_response_ids": response_ids,
            "verl_response_mask": response_mask,
            "trajectory": self._trajectory_to_jsonable(result),
        }

        path = Path(os.path.expandvars(path_text)).expanduser()
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(to_jsonable(record), ensure_ascii=False, separators=(",", ":")) + "\n")

    def _record_episode_requests(self, requests: list[EpisodeRequest], *, phase: str) -> None:
        if not requests:
            return

        logger.info(
            "uenv_record_request phase=%s sample_count=%s request_ids=%s",
            phase,
            len(requests),
            ",".join(request.request_id for request in requests[:5]),
        )

        path_text = self.config_for_uenv.request_record_path
        if not path_text:
            return

        path = Path(os.path.expandvars(path_text)).expanduser()
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("a", encoding="utf-8") as handle:
            for request in requests:
                payload = self._payload_from_request(request)
                metadata = payload.get("metadata") if isinstance(payload, dict) else {}
                if not isinstance(metadata, dict):
                    metadata = {}
                model_endpoint = payload.get("model_endpoint") if isinstance(payload, dict) else {}
                if not isinstance(model_endpoint, dict):
                    model_endpoint = {}

                record = {
                    "ts": time.time(),
                    "phase": phase,
                    "request_id": request.request_id,
                    "env_type": request.env_type,
                    "mode": request.mode,
                    "max_steps": request.max_steps,
                    "seed": request.seed,
                    "batch_id": metadata.get("batch_id"),
                    "sample_index": metadata.get("sample_index"),
                    "correlation_id": payload.get("correlation_id"),
                    "model_endpoint": request.model_endpoint,
                    "model_name": model_endpoint.get("model_name"),
                    "generation_config": model_endpoint.get("generation_config"),
                    "prompt_text": (
                        (payload.get("episode_config") or {})
                        .get("initial_observation", {})
                        .get("prompt_text", "")
                    ),
                    "payload": payload,
                }
                handle.write(json.dumps(to_jsonable(record), ensure_ascii=False, separators=(",", ":")) + "\n")

    def _payload_from_request(self, request: EpisodeRequest) -> dict[str, Any]:
        try:
            payload = json.loads(request.payload.decode("utf-8"))
        except Exception:
            return {}
        return payload if isinstance(payload, dict) else {}

    def _result_response_text(self, result: EpisodeResult) -> str:
        return "\n".join(text for step in result.trajectory.steps if (text := self._step_response_text(step)))

    def _result_response_ids(self, result: EpisodeResult) -> list[int]:
        output: list[int] = []
        for step in result.trajectory.steps:
            ids = self._ids_from_info(step.info, "response_ids")
            if ids:
                output.extend(ids)
        return output

    def _step_response_text(self, step: StepRecord) -> str:
        text = step.info.get("response_text")
        if text:
            return text
        return step.action.decode("utf-8", errors="replace")

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
        """Return VeRL-managed rollout endpoint and served model name.

        VeRL normally passes an LLMServerClient into AgentLoop. That client routes
        through a Ray load-balancer actor, while some test/runtime variants expose
        addresses directly on the manager. We support both shapes and fall back to
        static config when neither is available.
        """
        explicit = (
            self._value_from_extra_info(sample_kwargs, "model_endpoint", None)
            or sampling_params.get("model_endpoint")
        )
        if explicit:
            return str(explicit), self._model_name(sample_kwargs, sampling_params)

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
        explicit = (
            self._value_from_extra_info(sample_kwargs, "model_name", None)
            or sampling_params.get("model_name")
        )
        if explicit:
            return str(explicit)

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
                ray_ref = remote()
                values.extend(await self._await_ray_value(ray_ref))
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

    def _sample_kwargs_for_batch(
        self,
        sample_kwargs: dict[str, Any],
        *,
        batch_id: str,
        sample_index: int,
    ) -> dict[str, Any]:
        output = dict(sample_kwargs)
        extra_info = self._python_value(output.get("extra_info") or {})
        extra_info = dict(extra_info) if isinstance(extra_info, dict) else {}
        extra_info["batch_id"] = batch_id
        extra_info["sample_index"] = sample_index
        output["extra_info"] = extra_info
        return output
