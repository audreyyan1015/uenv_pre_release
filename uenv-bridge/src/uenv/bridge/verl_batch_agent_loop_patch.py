from __future__ import annotations

import os
import uuid
import logging
from typing import Any

logger = logging.getLogger(__name__)


def apply_verl_agent_loop_batch_patch() -> None:
    """Batch UEnv AgentLoop requests at VeRL's AgentLoopWorker boundary."""

    from verl.experimental.agent_loop import agent_loop as verl_agent_loop

    cls = verl_agent_loop.AgentLoopWorker
    if getattr(cls, "_uenv_batch_patch_applied", False):
        return

    original_generate_sequences = cls.generate_sequences

    async def generate_sequences(self, batch):
        if not _batch_enabled():
            return await original_generate_sequences(self, batch)

        config = self.rollout_config
        if "agent_name" not in batch.non_tensor_batch:
            default_agent_loop = config.agent.default_agent_loop
            batch.non_tensor_batch["agent_name"] = verl_agent_loop.np.array([default_agent_loop] * len(batch), dtype=object)

        agent_names = [_python_value(item) for item in batch.non_tensor_batch["agent_name"]]
        if not agent_names or any(name != "uenv_agent" for name in agent_names):
            return await original_generate_sequences(self, batch)

        if "uenv_agent" not in verl_agent_loop._agent_loop_registry:
            return await original_generate_sequences(self, batch)

        validate = batch.meta_info.get("validate", False)
        sampling_params = _sampling_params(config, validate)
        index = batch.non_tensor_batch["index"] if "index" in batch.non_tensor_batch else verl_agent_loop.np.arange(len(batch))
        trajectory_info = await verl_agent_loop.get_trajectory_info(
            batch.meta_info.get("global_steps", -1),
            index.tolist() if hasattr(index, "tolist") else list(index),
            validate,
        )

        per_sample_do_sample = batch.non_tensor_batch.get("__do_sample__")
        sampling_params_by_sample = []
        sample_kwargs_by_sample = []
        for i in range(len(batch)):
            sample_sampling_params = dict(sampling_params)
            if not validate and per_sample_do_sample is not None and not bool(per_sample_do_sample[i]):
                _apply_greedy_sampling_params(sample_sampling_params)

            kwargs = {key: value[i] for key, value in batch.non_tensor_batch.items() if key != "__do_sample__"}
            kwargs = _with_batch_extra_info(
                kwargs,
                global_steps=batch.meta_info.get("global_steps", -1),
                rollout_n=trajectory_info[i]["rollout_n"],
            )
            sampling_params_by_sample.append(sample_sampling_params)
            sample_kwargs_by_sample.append(kwargs)

        agent_loop = verl_agent_loop.hydra.utils.instantiate(
            config=verl_agent_loop._agent_loop_registry["uenv_agent"],
            trainer_config=verl_agent_loop.DictConfigWrap(config=self.config),
            server_manager=self.llm_client,
            tokenizer=self.tokenizer,
            processor=self.processor,
            dataset_cls=self.dataset_cls,
            data_config=verl_agent_loop.DictConfigWrap(self.config.data),
            tools=verl_agent_loop.ToolListWrap(self.tools),
        )
        if not hasattr(agent_loop, "run_batch"):
            return await original_generate_sequences(self, batch)

        batch_id = _batch_id(batch.meta_info.get("global_steps", -1))
        print(
            f"uenv_agent_loop_batch_start batch_id={batch_id} sample_count={len(batch)} validate={validate}",
            flush=True,
        )
        logger.info(
            "uenv_agent_loop_batch_start batch_id=%s sample_count=%s validate=%s",
            batch_id,
            len(batch),
            validate,
        )
        try:
            outputs = await agent_loop.run_batch(
                sampling_params_by_sample,
                sample_kwargs_by_sample,
                batch_id=batch_id,
            )
        finally:
            close = getattr(agent_loop, "close", None)
            if callable(close):
                close()
        internal_outputs = [
            await self._agent_loop_postprocess(output, validate, **kwargs)
            for output, kwargs in zip(outputs, sample_kwargs_by_sample, strict=True)
        ]
        return self._postprocess(
            internal_outputs,
            input_non_tensor_batch=batch.non_tensor_batch,
            validate=validate,
        )

    cls.generate_sequences = generate_sequences
    cls._uenv_batch_patch_applied = True


def _batch_enabled() -> bool:
    return os.environ.get("UENV_AGENT_LOOP_BATCH", "0").strip().lower() in {"1", "true", "yes", "on"}


def _sampling_params(config: Any, validate: bool) -> dict[str, Any]:
    params = {
        "temperature": config.temperature,
        "top_p": config.top_p,
        "top_k": config.top_k,
        "repetition_penalty": 1.0,
        "logprobs": config.calculate_log_probs,
    }
    if validate:
        params["top_p"] = config.val_kwargs.top_p
        params["top_k"] = config.val_kwargs.top_k
        params["temperature"] = config.val_kwargs.temperature
    return params


def _apply_greedy_sampling_params(params: dict[str, Any]) -> None:
    params["top_p"] = 1.0
    params["top_k"] = -1
    params["temperature"] = 0


def _with_batch_extra_info(
    kwargs: dict[str, Any],
    *,
    global_steps: Any,
    rollout_n: Any,
) -> dict[str, Any]:
    output = dict(kwargs)
    extra_info = _python_value(output.get("extra_info") or {})
    extra_info = dict(extra_info) if isinstance(extra_info, dict) else {}
    extra_info["global_steps"] = _python_value(global_steps)
    extra_info["rollout_n"] = _python_value(rollout_n)
    output["extra_info"] = extra_info
    return output


def _batch_id(global_steps: Any) -> str:
    step = _python_value(global_steps)
    if step is None or str(step) == "-1":
        return f"verl-agent-loop-batch-{uuid.uuid4().hex[:8]}"
    return f"verl-agent-loop-step-{step}-{uuid.uuid4().hex[:8]}"


def _python_value(value: Any) -> Any:
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
