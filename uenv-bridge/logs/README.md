# UEnv Bridge Logs

This directory stores selected smoke-test logs that are useful for cross-team debugging and review. Routine runtime artifacts, datasets, checkpoints, and Hydra output should stay under `tmp/` and should not be committed.

Current included run:

- `verl_grpo_1step_agent_loop/layer4_distributed_20260609_154224.log`: real VeRL Layer 4 distributed pre-rollout run. It reached `UEnvAgentLoop` and received a failed episode result from the server-side adapter core: `dispatch failed: transport error`.
- `layer4_distributed/layer4_distributed_20260609_154224/mock-model.log`: local mock OpenAI-compatible model endpoint log for the same run. It only shows startup, which indicates the request did not reach worker model inference in that run.
