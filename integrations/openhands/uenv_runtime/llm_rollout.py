"""Capture provider-backed rollout token ids and logprobs for Agent training."""

from __future__ import annotations

import json
import urllib.request
from pathlib import Path
from typing import Any


def _get(value: Any, name: str, default: Any = None) -> Any:
    if isinstance(value, dict):
        return value.get(name, default)
    return getattr(value, name, default)


class RolloutTraceCollector:
    """Collect Ark chat logprobs, then resolve exact ids via Ark tokenization."""

    def __init__(self, config_path: str | Path) -> None:
        raw = json.loads(Path(config_path).read_text(encoding="utf-8"))
        self._api_key = str(raw["api_key"])
        self._base_url = str(raw["base_url"]).rstrip("/")
        self.model = str(raw["model"])
        self._provider_model = self.model.removeprefix("volcengine/")
        self._responses: list[dict[str, Any]] = []

    def install(self, llm: Any) -> None:
        """Request logprobs on real agent calls and capture the raw responses."""
        original_completion = llm.completion

        def completion(*args: Any, **kwargs: Any) -> Any:
            # Seed 2.1 is a reasoning model by default. Ark only exposes
            # ChatCompletions logprobs when thinking is explicitly disabled.
            kwargs.setdefault("thinking", {"type": "disabled"})
            kwargs.setdefault("logprobs", True)
            kwargs.setdefault("top_logprobs", 1)
            response = original_completion(*args, **kwargs)
            self.record(response)
            return response

        object.__setattr__(llm, "completion", completion)

        original_acompletion = llm.acompletion

        async def acompletion(*args: Any, **kwargs: Any) -> Any:
            kwargs.setdefault("thinking", {"type": "disabled"})
            kwargs.setdefault("logprobs", True)
            kwargs.setdefault("top_logprobs", 1)
            response = await original_acompletion(*args, **kwargs)
            self.record(response)
            return response

        object.__setattr__(llm, "acompletion", acompletion)

    def record(self, response: Any) -> None:
        raw = _get(response, "raw_response")
        response_id = str(_get(raw, "id", "") or "")
        for choice in _get(raw, "choices", ()) or ():
            logprobs = _get(choice, "logprobs")
            records = _get(logprobs, "content", ()) or ()
            tokens: list[str] = []
            values: list[float] = []
            for record in records:
                token = _get(record, "token")
                logprob = _get(record, "logprob")
                if token is None or not isinstance(logprob, (int, float)):
                    continue
                tokens.append(str(token))
                values.append(float(logprob))
            if tokens:
                self._responses.append(
                    {
                        "response_id": response_id,
                        "text": "".join(tokens),
                        "logprobs": values,
                    }
                )

    def _tokenize(self, texts: list[str]) -> list[list[int]]:
        url = self._base_url + "/tokenization"
        request = urllib.request.Request(
            url,
            data=json.dumps({"model": self._provider_model, "text": texts}).encode(),
            method="POST",
        )
        request.add_header("Content-Type", "application/json")
        request.add_header("Authorization", f"Bearer {self._api_key}")
        with urllib.request.urlopen(request, timeout=120) as response:
            document = json.loads(response.read().decode())
        data = document.get("data")
        if not isinstance(data, list) or len(data) != len(texts):
            raise RuntimeError("Ark tokenization response count mismatch")
        ordered = sorted(data, key=lambda item: int(item.get("index", 0)))
        return [[int(token) for token in item.get("token_ids", [])] for item in ordered]

    def finalize(self) -> dict[str, Any]:
        if not self._responses:
            raise RuntimeError("real LLM calls produced no content token logprobs")
        token_groups = self._tokenize([item["text"] for item in self._responses])
        response_ids: list[int] = []
        rollout_log_probs: list[float] = []
        for index, (item, token_ids) in enumerate(zip(self._responses, token_groups, strict=True)):
            logprobs = item["logprobs"]
            if len(token_ids) != len(logprobs):
                raise RuntimeError(
                    f"Ark token/logprob alignment mismatch response={index} "
                    f"ids={len(token_ids)} logprobs={len(logprobs)}"
                )
            response_ids.extend(token_ids)
            rollout_log_probs.extend(logprobs)
        if not response_ids:
            raise RuntimeError("Ark tokenization returned no response ids")
        return {
            "rollout_trace": {
                "response_ids": response_ids,
                "response_mask": [1] * len(response_ids),
            },
            "rollout_log_probs": rollout_log_probs,
            "rollout_policy_version": self.model,
            "rollout_param_version": 0,
            "rollout_trace_metadata": {
                "source": "ark_chat_logprobs+ark_tokenization",
                "response_count": len(self._responses),
                "token_count": len(response_ids),
                "provider_response_ids_present": all(
                    bool(item["response_id"]) for item in self._responses
                ),
                "coverage": "content_tokens_returned_by_provider",
            },
        }
