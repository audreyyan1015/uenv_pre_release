"""Process-wide Python startup hooks for uenv-bridge smoke tests.

This module is imported automatically by Python when it is present on
PYTHONPATH. Keep behavior behind explicit environment flags.
"""

from __future__ import annotations

import os


def _run_when_module_imported(module_name: str, callback) -> None:
    """Run ``callback`` after ``module_name`` is imported, without importing it now."""

    import builtins
    import sys

    if module_name in sys.modules:
        callback()
        return

    import_attr = "_uenv_import_hook_callbacks"
    callbacks = getattr(builtins, import_attr, None)
    if callbacks is None:
        callbacks = {}
        setattr(builtins, import_attr, callbacks)

        original_import = builtins.__import__

        def hooked_import(name, globals=None, locals=None, fromlist=(), level=0):
            module = original_import(name, globals, locals, fromlist, level)
            for target_name, target_callbacks in list(callbacks.items()):
                if target_name in sys.modules:
                    callbacks.pop(target_name, None)
                    pending_callbacks = []
                    for target_callback in target_callbacks:
                        try:
                            target_callback()
                        except AttributeError:
                            pending_callbacks.append(target_callback)
                    if pending_callbacks:
                        callbacks[target_name] = pending_callbacks
            if not callbacks:
                builtins.__import__ = original_import
            return module

        builtins.__import__ = hooked_import

    callbacks.setdefault(module_name, []).append(callback)


def _patch_resource_tracker_duplicate_unregister() -> None:
    """Tolerate duplicate shared-memory UNREGISTER messages in Python 3.12.

    vLLM's multiprocessing workers can emit duplicate resource-tracker
    unregister events during shutdown. CPython 3.12's resource tracker uses
    set.remove(), so the second unregister prints a KeyError traceback even
    after training has completed successfully. The tracker process is launched
    as a fresh Python interpreter that imports this sitecustomize module before
    running ``from multiprocessing.resource_tracker import main``.
    """

    import signal
    import sys
    import warnings
    from multiprocessing import resource_tracker as rt

    def main(fd: int) -> None:
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        if rt._HAVE_SIGMASK:
            signal.pthread_sigmask(signal.SIG_UNBLOCK, rt._IGNORED_SIGNALS)

        for stream in (sys.stdin, sys.stdout):
            try:
                stream.close()
            except Exception:
                pass

        cache = {rtype: set() for rtype in rt._CLEANUP_FUNCS.keys()}
        try:
            with open(fd, "rb") as file:
                for line in file:
                    try:
                        cmd, name, rtype = line.strip().decode("ascii").split(":")
                        cleanup_func = rt._CLEANUP_FUNCS.get(rtype)
                        if cleanup_func is None:
                            raise ValueError(
                                f"Cannot register {name} for automatic cleanup: "
                                f"unknown resource type {rtype}"
                            )

                        if cmd == "REGISTER":
                            cache[rtype].add(name)
                        elif cmd == "UNREGISTER":
                            cache[rtype].discard(name)
                        elif cmd == "PROBE":
                            pass
                        else:
                            raise RuntimeError(f"unrecognized command {cmd!r}")
                    except Exception:
                        try:
                            sys.excepthook(*sys.exc_info())
                        except Exception:
                            pass
        finally:
            for rtype, rtype_cache in cache.items():
                if rtype_cache:
                    try:
                        warnings.warn(
                            "resource_tracker: There appear to be %d leaked %s "
                            "objects to clean up at shutdown"
                            % (len(rtype_cache), rtype)
                        )
                    except Exception:
                        pass
                for name in rtype_cache:
                    try:
                        rt._CLEANUP_FUNCS[rtype](name)
                    except Exception as exc:
                        warnings.warn(f"resource_tracker: {name!r}: {exc}")

    rt.main = main


if os.environ.get("UENV_PATCH_RESOURCE_TRACKER") == "1":
    _patch_resource_tracker_duplicate_unregister()


def _patch_transformers_pad_return_tensors() -> None:
    """Make tokenizer.pad(return_tensors="pt") robust for VeRL agent loops.

    Some VeRL experimental fully-async agent-loop paths call
    ``tokenizer.pad(..., return_tensors="pt")`` and then immediately use
    ``.dim()`` on ``input_ids``. With the local transformers stack, that call
    can still return Python lists for a single sample. Keep the patch narrow:
    only convert common pad outputs when PyTorch tensors were explicitly
    requested.
    """

    import torch
    from transformers.tokenization_utils_base import PreTrainedTokenizerBase

    cls = PreTrainedTokenizerBase
    if getattr(cls, "_uenv_pad_return_tensors_patch_applied", False):
        return

    original_pad = cls.pad

    def pad(self, *args, **kwargs):
        output = original_pad(self, *args, **kwargs)
        if kwargs.get("return_tensors") != "pt":
            return output

        for key in ("input_ids", "attention_mask", "token_type_ids"):
            value = output.get(key) if hasattr(output, "get") else None
            if isinstance(value, list):
                output[key] = torch.tensor(value, dtype=torch.long)
        return output

    cls.pad = pad
    cls._uenv_pad_return_tensors_patch_applied = True


if os.environ.get("UENV_PATCH_TRANSFORMERS_PAD_RETURN_TENSORS") == "1":
    _run_when_module_imported("transformers.tokenization_utils_base", _patch_transformers_pad_return_tensors)


def _patch_verl_agent_loop_empty_response() -> None:
    """Prevent empty fully-async agent-loop responses from breaking batching.

    VeRL's experimental fully-async rollouter can abort in
    ``AgentLoopWorker._postprocess`` when one generated sample has zero
    response tokens or missing rollout logprobs while other samples are padded
    to ``response_length``. Keep the semantic effect minimal: add one tokenizer
    pad/eos token and mark it as non-generated in ``response_mask``; for
    missing logprobs, add a zero tensor with the padded response width.
    """

    import torch
    from verl.experimental.agent_loop import agent_loop as verl_agent_loop

    cls = verl_agent_loop.AgentLoopWorker
    if getattr(cls, "_uenv_empty_response_patch_applied", False):
        return

    original_postprocess = cls._agent_loop_postprocess

    async def _agent_loop_postprocess(self, output, validate, **kwargs):
        if not getattr(output, "response_ids", None):
            token_id = getattr(self.tokenizer, "pad_token_id", None)
            if token_id is None:
                token_id = getattr(self.tokenizer, "eos_token_id", None)
            if token_id is None:
                token_id = 0
            output.response_ids = [int(token_id)]
            output.response_mask = [0]
            if getattr(output, "response_logprobs", None) is not None:
                output.response_logprobs = [0.0]
        result = await original_postprocess(self, output, validate, **kwargs)
        if result.response_logprobs is None:
            result.response_logprobs = torch.zeros_like(result.response_mask, dtype=torch.float32)
        return result

    cls._agent_loop_postprocess = _agent_loop_postprocess
    cls._uenv_empty_response_patch_applied = True


if os.environ.get("UENV_PATCH_VERL_AGENT_LOOP_EMPTY_RESPONSE") == "1":
    _run_when_module_imported("verl.experimental.agent_loop.agent_loop", _patch_verl_agent_loop_empty_response)


def _patch_torch_cuda_is_available_no_devices() -> None:
    """Treat Ray CPU actors with zero visible GPUs as CUDA-unavailable.

    In VeRL's one-step off-policy path, Ray creates temporary CPU actors that
    import transformer_engine before any training worker is placed. On this
    host those actors can report ``torch.cuda.is_available() == True`` while
    ``torch.cuda.device_count() == 0`` because Ray hides GPUs for the actor.
    transformer_engine then calls ``get_device_properties(0)`` and aborts with
    ``AssertionError: Invalid device id``. Returning False only in the zero
    visible-device case keeps normal GPU workers unchanged.
    """

    import torch

    cuda = torch.cuda
    if getattr(cuda, "_uenv_is_available_no_devices_patch_applied", False):
        return

    original_is_available = cuda.is_available

    def is_available() -> bool:
        if not original_is_available():
            return False
        try:
            return cuda.device_count() > 0
        except Exception:
            return False

    cuda.is_available = is_available
    cuda._uenv_is_available_no_devices_patch_applied = True


if os.environ.get("UENV_PATCH_TORCH_CUDA_IS_AVAILABLE_NO_DEVICES") == "1":
    _run_when_module_imported("torch", _patch_torch_cuda_is_available_no_devices)


def _patch_verl_device_capability_fallback() -> None:
    """Let CPU-only Ray actors import VeRL CUDA constants safely.

    VeRL's experimental fully-async rollouter is a CPU Ray actor, but during
    startup it imports ``verl.trainer.constants_ppo``. That module queries CUDA
    capability at import time. Ray normally hides GPUs from CPU actors by
    setting ``CUDA_VISIBLE_DEVICES`` to an empty value, so the query can fail
    before the actual GPU actors are created. Return the local A100 capability
    only for that failed query instead of changing Ray's GPU isolation.
    """

    from verl.utils import device as verl_device

    if getattr(verl_device, "_uenv_device_capability_fallback_patch_applied", False):
        return

    raw_capability = os.environ.get("UENV_VERL_DEVICE_CAPABILITY_FALLBACK", "8,0")
    try:
        major_text, minor_text = raw_capability.split(",", 1)
        fallback = (int(major_text), int(minor_text))
    except Exception:
        fallback = (8, 0)

    original_get_device_capability = verl_device.get_device_capability

    def get_device_capability(device_id: int = 0):
        try:
            return original_get_device_capability(device_id)
        except (AssertionError, RuntimeError) as exc:
            message = str(exc).lower()
            cuda_unavailable = (
                "no cuda gpus are available" in message
                or "cuda error" in message
                or "invalid device id" in message
                or "invalid device ordinal" in message
            )
            if not cuda_unavailable:
                raise
            return fallback

    verl_device.get_device_capability = get_device_capability
    verl_device._uenv_device_capability_fallback_patch_applied = True


if os.environ.get("UENV_PATCH_VERL_DEVICE_CAPABILITY_FALLBACK") == "1":
    _run_when_module_imported("verl.utils.device", _patch_verl_device_capability_fallback)


def _patch_verl_agent_loop_batch() -> None:
    from uenv.bridge.verl_batch_agent_loop_patch import apply_verl_agent_loop_batch_patch

    apply_verl_agent_loop_batch_patch()


if os.environ.get("UENV_AGENT_LOOP_BATCH", "0").strip().lower() in {"1", "true", "yes", "on"}:
    _run_when_module_imported("verl.experimental.agent_loop.agent_loop", _patch_verl_agent_loop_batch)
