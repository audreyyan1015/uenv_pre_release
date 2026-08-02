"""Capture provider-backed rollout token ids and logprobs for Agent training.

Supports:
  * Ark / Volcengine — chat logprobs + ``/tokenization``
  * OpenAI-compatible (vLLM / LiteLLM gateway) — chat logprobs with
    ``token_id`` fields, ``token_id:N`` strings, optional ``/tokenize``,
    or an optional HuggingFace ``tokenizer`` path in the LLM config
"""

from __future__ import annotations

import json
import re
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any


_TOKEN_ID_RE = re.compile(r"^token_id:(\d+)$")


def _get(value: Any, name: str, default: Any = None) -> Any:
    if isinstance(value, dict):
        return value.get(name, default)
    return getattr(value, name, default)


def _detect_provider(model: str, base_url: str, explicit: str = "") -> str:
    named = (explicit or "").strip().lower()
    if named in {"ark", "volcengine", "openai", "vllm", "hf"}:
        return "ark" if named in {"ark", "volcengine"} else "openai"
    blob = f"{model} {base_url}".lower()
    if "volcengine" in blob or "ark.cn" in blob or "/api/v3" in blob:
        return "ark"
    return "openai"


def _parse_token_id(token: Any, record: Any = None) -> int | None:
    if record is not None:
        for key in ("token_id", "id"):
            value = _get(record, key)
            if isinstance(value, bool):
                continue
            if isinstance(value, int):
                return value
            if isinstance(value, float) and value.is_integer():
                return int(value)
            if isinstance(value, str) and value.isdigit():
                return int(value)
    text = str(token or "")
    match = _TOKEN_ID_RE.match(text.strip())
    if match:
        return int(match.group(1))
    return None


class RolloutTraceCollector:
    """Collect chat logprobs and resolve aligned response token ids."""

    def __init__(self, config_path: str | Path) -> None:
        raw = json.loads(Path(config_path).read_text(encoding="utf-8"))
        self._api_key = str(raw.get("api_key") or "")
        self._base_url = str(raw["base_url"]).rstrip("/")
        self.model = str(raw["model"])
        self._provider_model = self.model.split("/", 1)[-1]
        self._provider = _detect_provider(
            self.model, self._base_url, str(raw.get("rollout_provider") or "")
        )
        self._tokenizer_name = str(
            raw.get("tokenizer") or raw.get("tokenizer_path") or ""
        ).strip()
        self._return_tokens_as_token_ids = bool(
            raw.get("return_tokens_as_token_ids", self._provider != "ark")
        )
        self._responses: list[dict[str, Any]] = []
        self._hf_tokenizer: Any = None

    def install(self, llm: Any, *, episode_id: str = "", dataset: str = "") -> None:
        """Request logprobs on real agent calls and capture the raw responses."""
        original_completion = llm.completion

        def add_uenv_headers(kwargs: dict[str, Any]) -> None:
            if not episode_id:
                return
            headers = dict(kwargs.get("extra_headers") or {})
            headers["X-UEnv-Episode-Id"] = episode_id
            if dataset:
                headers["X-UEnv-Dataset"] = dataset
            kwargs["extra_headers"] = headers

        def add_rollout_kwargs(kwargs: dict[str, Any]) -> None:
            # Seed / Ark reasoning models only expose ChatCompletions logprobs
            # when thinking is explicitly disabled.
            if self._provider == "ark":
                kwargs.setdefault("thinking", {"type": "disabled"})
            kwargs.setdefault("logprobs", True)
            kwargs.setdefault("top_logprobs", 1)
            if self._return_tokens_as_token_ids and self._provider != "ark":
                extra_body = dict(kwargs.get("extra_body") or {})
                extra_body.setdefault("return_tokens_as_token_ids", True)
                kwargs["extra_body"] = extra_body
            add_uenv_headers(kwargs)

        def completion(*args: Any, **kwargs: Any) -> Any:
            add_rollout_kwargs(kwargs)
            response = original_completion(*args, **kwargs)
            self.record(response)
            return response

        object.__setattr__(llm, "completion", completion)

        original_acompletion = llm.acompletion

        async def acompletion(*args: Any, **kwargs: Any) -> Any:
            add_rollout_kwargs(kwargs)
            response = await original_acompletion(*args, **kwargs)
            self.record(response)
            return response

        object.__setattr__(llm, "acompletion", acompletion)

    def record(self, response: Any) -> None:
        raw = _get(response, "raw_response")
        if raw is None:
            raw = response
        response_id = str(_get(raw, "id", "") or "")
        finish_reason = ""
        explicit_ids = _get(raw, "uenv_response_ids")
        if not isinstance(explicit_ids, list):
            explicit_ids = None

        for choice in _get(raw, "choices", ()) or ():
            finish_reason = str(_get(choice, "finish_reason", "") or finish_reason)
            choice_ids = _get(choice, "token_ids") or _get(choice, "output_token_ids")
            if isinstance(choice_ids, list) and choice_ids and explicit_ids is None:
                explicit_ids = choice_ids

            logprobs = _get(choice, "logprobs")
            records = _get(logprobs, "content", ()) or ()
            tokens: list[str] = []
            values: list[float] = []
            token_ids: list[int] = []
            ids_complete = True
            for record in records:
                token = _get(record, "token")
                logprob = _get(record, "logprob")
                if token is None or not isinstance(logprob, (int, float)):
                    continue
                tokens.append(str(token))
                values.append(float(logprob))
                parsed = _parse_token_id(token, record)
                if parsed is None:
                    ids_complete = False
                else:
                    token_ids.append(parsed)

            if not tokens and not (isinstance(explicit_ids, list) and explicit_ids):
                continue

            resolved_ids: list[int] | None = None
            if isinstance(explicit_ids, list) and explicit_ids:
                resolved_ids = [int(x) for x in explicit_ids]
            elif ids_complete and len(token_ids) == len(values) and token_ids:
                resolved_ids = token_ids

            text = "".join(
                tok if not _TOKEN_ID_RE.match(tok.strip()) else "" for tok in tokens
            )
            if not text:
                message = _get(choice, "message")
                content = _get(message, "content", "")
                if isinstance(content, str):
                    text = content
                elif isinstance(content, list):
                    parts: list[str] = []
                    for item in content:
                        part = _get(item, "text")
                        if part:
                            parts.append(str(part))
                    text = "".join(parts)

            self._responses.append(
                {
                    "response_id": response_id,
                    "text": text,
                    "tokens": tokens,
                    "logprobs": values,
                    "token_ids": resolved_ids,
                    "finish_reason": finish_reason,
                }
            )

    def _ark_tokenize(self, texts: list[str]) -> list[list[int]]:
        url = self._base_url + "/tokenization"
        request = urllib.request.Request(
            url,
            data=json.dumps({"model": self._provider_model, "text": texts}).encode(),
            method="POST",
        )
        request.add_header("Content-Type", "application/json")
        if self._api_key:
            request.add_header("Authorization", f"Bearer {self._api_key}")
        with urllib.request.urlopen(request, timeout=120) as response:
            document = json.loads(response.read().decode())
        data = document.get("data")
        if not isinstance(data, list) or len(data) != len(texts):
            raise RuntimeError("Ark tokenization response count mismatch")
        ordered = sorted(data, key=lambda item: int(item.get("index", 0)))
        return [[int(token) for token in item.get("token_ids", [])] for item in ordered]

    def _openai_tokenize(self, text: str) -> list[int] | None:
        """Best-effort OpenAI-compatible /tokenize (vLLM). Returns None if unavailable."""
        payloads = (
            {"model": self._provider_model, "prompt": text, "add_special_tokens": False},
            {"model": self._provider_model, "prompt": text},
        )
        bases = [self._base_url]
        if self._base_url.endswith("/v1"):
            bases.append(self._base_url[: -len("/v1")])
        for base in bases:
            for path in ("/tokenize", "/v1/tokenize"):
                if base.endswith("/v1") and path.startswith("/v1/"):
                    url = base[: -len("/v1")] + path
                else:
                    url = base.rstrip("/") + path
                for payload in payloads:
                    request = urllib.request.Request(
                        url,
                        data=json.dumps(payload).encode(),
                        method="POST",
                    )
                    request.add_header("Content-Type", "application/json")
                    if self._api_key:
                        request.add_header("Authorization", f"Bearer {self._api_key}")
                    try:
                        with urllib.request.urlopen(request, timeout=60) as response:
                            document = json.loads(response.read().decode())
                    except (urllib.error.HTTPError, urllib.error.URLError, TimeoutError):
                        continue
                    except Exception:  # noqa: BLE001
                        continue
                    for key in ("tokens", "token_ids", "input_ids"):
                        value = document.get(key)
                        if isinstance(value, list) and value and all(
                            isinstance(x, int) for x in value
                        ):
                            return [int(x) for x in value]
                    data = document.get("data")
                    if isinstance(data, dict):
                        for key in ("tokens", "token_ids", "input_ids"):
                            value = data.get(key)
                            if isinstance(value, list) and value and all(
                                isinstance(x, int) for x in value
                            ):
                                return [int(x) for x in value]
        return None

    def _hf_tokenize(self, text: str) -> list[int]:
        if self._hf_tokenizer is None:
            if not self._tokenizer_name:
                raise RuntimeError(
                    "OpenAI-compatible provider did not return token ids; set "
                    "return_tokens_as_token_ids on the server, expose /tokenize, "
                    "or add tokenizer=/path/or/hf-id to the LLM config"
                )
            try:
                from transformers import AutoTokenizer  # type: ignore
            except ImportError as exc:  # pragma: no cover
                raise RuntimeError(
                    "tokenizer configured but transformers is not installed"
                ) from exc
            self._hf_tokenizer = AutoTokenizer.from_pretrained(
                self._tokenizer_name, trust_remote_code=True
            )
        ids = self._hf_tokenizer.encode(text, add_special_tokens=False)
        return [int(x) for x in ids]

    def _resolve_token_ids(self, item: dict[str, Any]) -> list[int]:
        existing = item.get("token_ids")
        logprobs = item["logprobs"]
        if isinstance(existing, list) and existing:
            token_ids = [int(x) for x in existing]
            if logprobs and len(token_ids) != len(logprobs):
                raise RuntimeError(
                    f"provider token_id/logprob alignment mismatch "
                    f"ids={len(token_ids)} logprobs={len(logprobs)}"
                )
            return token_ids

        text = str(item.get("text") or "")
        if not text and item.get("tokens"):
            text = "".join(str(t) for t in item["tokens"])

        if self._provider == "ark":
            token_ids = self._ark_tokenize([text])[0]
        else:
            token_ids = self._openai_tokenize(text)
            if token_ids is None:
                token_ids = self._hf_tokenize(text)

        if logprobs and len(token_ids) != len(logprobs):
            raise RuntimeError(
                f"token/logprob alignment mismatch after tokenize "
                f"ids={len(token_ids)} logprobs={len(logprobs)} "
                f"provider={self._provider}"
            )
        return token_ids

    def finalize(self) -> dict[str, Any]:
        if not self._responses:
            raise RuntimeError("real LLM calls produced no content token logprobs")

        response_ids: list[int] = []
        rollout_log_probs: list[float] = []
        turns: list[dict[str, Any]] = []
        sources: list[str] = []

        for index, item in enumerate(self._responses):
            logprobs = list(item["logprobs"])
            if item.get("token_ids"):
                source = "provider_token_ids"
            elif self._provider == "ark":
                source = "ark_tokenization"
            else:
                source = "openai_tokenize_or_hf"
            token_ids = self._resolve_token_ids(item)
            if not logprobs:
                # Rare path: explicit ids without per-token logprobs — reject for GRPO.
                raise RuntimeError(
                    f"response={index} has token ids but no content logprobs"
                )
            if len(token_ids) != len(logprobs):
                raise RuntimeError(
                    f"token/logprob alignment mismatch response={index} "
                    f"ids={len(token_ids)} logprobs={len(logprobs)}"
                )
            response_ids.extend(token_ids)
            rollout_log_probs.extend(logprobs)
            sources.append(source)
            turns.append(
                {
                    "turn_index": index,
                    "assistant_output": item["text"],
                    "response_ids": token_ids,
                    "logprobs": logprobs,
                    "finish_reason": item.get("finish_reason") or "",
                    "provider_response_id": item["response_id"],
                }
            )

        if not response_ids:
            raise RuntimeError("rollout tokenization returned no response ids")

        source_label = (
            "ark_chat_logprobs+ark_tokenization"
            if self._provider == "ark"
            else "openai_chat_logprobs+token_ids"
        )
        return {
            "schema_version": 1,
            "corpus_kind": "openhands_real_llm_trace_rollout",
            "source_model": self.model,
            "turns": turns,
            "rollout_trace": {
                "response_ids": response_ids,
                "response_mask": [1] * len(response_ids),
            },
            "rollout_log_probs": rollout_log_probs,
            "rollout_policy_version": self.model,
            "rollout_param_version": 0,
            "rollout_trace_metadata": {
                "source": source_label,
                "provider": self._provider,
                "response_count": len(self._responses),
                "token_count": len(response_ids),
                "turn_id_sources": sources,
                "provider_response_ids_present": all(
                    bool(item["response_id"]) for item in self._responses
                ),
                "coverage": "content_tokens_returned_by_provider",
            },
        }
