# SWE-smith Checkpoint 评测 LLM 配置说明

## 背景

本次需要使用训练后 checkpoint：

`/data/ronghao/uenv/uenv-bridge/checkpoints/uenv_grpo/verl_swesmith_grpo_train_20260812_184238/global_step_2100`

在 SWE-smith 数据集上通过 UEnv + OpenHands 链路做评测。

## 问题

评测请求中虽然传入了 `model_endpoint`，但当前 OpenHands runner 实际会读取 agent 机器上的 `llm_config_path`。原有配置文件：

`/root/UEnv/config/openhands-llm-qwen3-thinking-max-token-8192.json`

内部的 `base_url` 指向 `http://127.0.0.1:18199/v1`，这是 agent 机器本地地址，无法访问本次在训练机启动的 checkpoint model gateway。

## 本次新增

在 agent 机器 `8.130.208.77` 上新增配置文件：

`/root/UEnv/config/openhands-llm-qwen3-thinking-max-token-8192-uenv-eval-18089.json`

该文件不覆盖原有配置，只把 `base_url` 指向本次评测使用的模型网关：

`http://10.10.20.142:18089/v1`

后续评测脚本通过 `LLM_CONFIG_PATH` 显式传入该路径，使 worker/agent 侧访问训练后 checkpoint 对应的模型服务。
