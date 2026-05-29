from __future__ import annotations

import hashlib
import json
import os
import threading
from pathlib import Path
from typing import Iterable

import numpy as np

from uenv.bridge.clients import EpisodeClient
from uenv.bridge.protocol import EpisodeRequest, EpisodeResult, EpisodeSummary, StepRecord, Trajectory, request_to_jsonable
from uenv.bridge.verl import VeRLAdapter
from verl import DataProto
from verl.experimental.reward_loop.reward_manager.base import RewardManagerBase


class MathProxyEpisodeClient:
    """Local stand-in for Serve while the real gRPC Serve side is unavailable."""

    def __init__(
        self,
        default_reward: float = 0.0,
        format_reward: float = 0.1,
        nonempty_reward: float = 0.05,
        shaping: bool = True,
    ) -> None:
        self.default_reward = default_reward
        self.format_reward = format_reward
        self.nonempty_reward = nonempty_reward
        self.shaping = shaping

    def submit_episode(self, request: EpisodeRequest) -> EpisodeResult:
        reward, reason = self._score_request(request)
        step = StepRecord(
            step_index=0,
            reward=reward,
            terminated=True,
            info={"source": "math_proxy", "reason": reason},
        )
        return EpisodeResult(
            request_id=request.request_id,
            status="completed",
            trajectory=Trajectory(steps=[step], total_reward=reward, total_steps=1),
            summary=EpisodeSummary(total_reward=reward, total_steps=1, terminate_reason=reason),
        )

    def submit_episode_stream(self, requests: Iterable[EpisodeRequest]) -> Iterable[EpisodeResult]:
        for request in requests:
            yield self.submit_episode(request)

    def _score_request(self, request: EpisodeRequest) -> tuple[float, str]:
        try:
            payload = json.loads(request.payload.decode("utf-8"))
        except Exception:
            return self.default_reward, "invalid_payload"

        reward_config = payload.get("reward_config") or {}
        rubric = reward_config.get("rubric_config") or {}
        ground_truth = str(rubric.get("ground_truth") or "").strip()
        env_config = payload.get("env_config") or {}
        observation = (payload.get("episode_config") or {}).get("initial_observation") or {}
        response_text = str(env_config.get("response_text") or observation.get("response_text") or "").strip()

        if ground_truth and self._normalize_answer(ground_truth) in self._normalize_answer(response_text):
            return 1.0, "exact_match"
        if any(ch.isdigit() for ch in response_text):
            return self._with_debug_shaping(self.format_reward, response_text), "format_digit"
        if response_text:
            return self._with_debug_shaping(self.nonempty_reward, response_text), "nonempty_response"
        return self.default_reward, "empty_response"

    def _with_debug_shaping(self, base_reward: float, response_text: str) -> float:
        if not self.shaping:
            return base_reward
        digest = hashlib.sha1(response_text.encode("utf-8", errors="replace")).digest()
        return min(base_reward + (digest[0] % 5) * 0.01, 0.99)

    def _normalize_answer(self, value: str) -> str:
        return "".join(ch for ch in value.lower() if ch.isalnum() or ch in ".-")


class RecordingEpisodeClientWrapper:
    def __init__(self, delegate: EpisodeClient, output_dir: str | None) -> None:
        self.delegate = delegate
        self.output_dir = Path(output_dir) if output_dir else None
        self._lock = threading.Lock()
        if self.output_dir is not None:
            self.output_dir.mkdir(parents=True, exist_ok=True)

    def submit_episode(self, request: EpisodeRequest) -> EpisodeResult:
        response = self.delegate.submit_episode(request)
        self._record(request, response)
        return response

    def submit_episode_stream(self, requests: Iterable[EpisodeRequest]) -> Iterable[EpisodeResult]:
        for request in requests:
            yield self.submit_episode(request)

    def _record(self, request: EpisodeRequest, response: EpisodeResult) -> None:
        if self.output_dir is None:
            return
        request_payload = request_to_jsonable(request)
        try:
            request_payload["payload_json"] = json.loads(request.payload.decode("utf-8"))
        except Exception:
            request_payload["payload_json"] = None
        result_payload = {
            "request_id": response.request_id,
            "status": response.status,
            "reward": response.summary.total_reward,
            "termination_reason": response.summary.terminate_reason,
        }
        with self._lock:
            with (self.output_dir / "episode_requests.jsonl").open("a", encoding="utf-8") as f:
                f.write(json.dumps(request_payload, ensure_ascii=False) + "\n")
            with (self.output_dir / "episode_results.jsonl").open("a", encoding="utf-8") as f:
                f.write(json.dumps(result_payload, ensure_ascii=False) + "\n")


class UEnvBridgeRewardManager(RewardManagerBase):
    def __init__(self, config, tokenizer, compute_score, reward_router_address=None, reward_model_tokenizer=None):
        super().__init__(config, tokenizer, compute_score)
        client = MathProxyEpisodeClient(
            default_reward=float(os.getenv("UENV_BRIDGE_FAKE_DEFAULT_REWARD", "0.0")),
            format_reward=float(os.getenv("UENV_BRIDGE_FAKE_FORMAT_REWARD", "0.1")),
            nonempty_reward=float(os.getenv("UENV_BRIDGE_FAKE_NONEMPTY_REWARD", "0.05")),
            shaping=os.getenv("UENV_BRIDGE_FAKE_SHAPING", "1") not in {"0", "false", "False"},
        )
        self.adapter = VeRLAdapter(client=RecordingEpisodeClientWrapper(client, os.getenv("UENV_BRIDGE_RECORD_DIR")))
        self.verbose = os.getenv("UENV_BRIDGE_VERBOSE", "1") not in {"0", "false", "False"}

    async def run_single(self, data: DataProto) -> dict:
        # In VeRL naming, batch["responses"] is the policy model's rollout
        # completion for the prompt. Decode it before sending the sample to UEnv
        # so the environment/reward side can score the model's answer text.
        response_text = await self._decode_response(data)
        data.non_tensor_batch["uenv_response_text"] = np.array([response_text], dtype=object)

        bridge_result = await self.loop.run_in_executor(None, lambda: self.adapter.execute_batch(data))
        result = bridge_result["results"][0]
        reward = float(result.get("reward") or 0.0)
        extra_info = {
            "uenv_reward": reward,
            "uenv_request_id": str(result.get("uenv_request_id") or ""),
            "uenv_done": bool(result.get("done")),
            "uenv_termination_reason": str(result.get("termination_reason") or ""),
            "uenv_response_preview": response_text[:256],
            "acc": 1.0 if reward >= 1.0 else 0.0,
        }
        if self.verbose:
            print(
                "UEnvBridgeRewardManager "
                f"request_id={extra_info['uenv_request_id']} "
                f"reward={reward} reason={extra_info['uenv_termination_reason']}",
                flush=True,
            )
        return {"reward_score": reward, "reward_extra_info": extra_info}

    async def _decode_response(self, data: DataProto) -> str:
        data_item = data[0]
        # This "response" is not a gRPC response or EpisodeResult. It is the
        # generated token ids stored by VeRL after rollout: prompt -> model
        # response/completion. Padding is removed with response_mask when
        # available, then tokenizer.decode turns the valid ids into text.
        response_ids = data_item.batch["responses"]
        response_length = response_ids.shape[-1]
        if "response_mask" in data_item.batch.keys():
            valid_response_length = data_item.batch["response_mask"].sum()
        else:
            valid_response_length = data_item.batch["attention_mask"][-response_length:].sum()
        valid_response_ids = response_ids[: int(valid_response_length.item())]
        return await self.loop.run_in_executor(
            None,
            lambda: self.tokenizer.decode(valid_response_ids, skip_special_tokens=True),
        )
