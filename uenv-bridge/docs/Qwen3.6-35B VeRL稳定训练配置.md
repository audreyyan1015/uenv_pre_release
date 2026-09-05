# Qwen3.6-35B VeRL 稳定训练配置

> 本文记录 `Qwen/Qwen3.6-35B-A3B` 在 UEnv Adapter + VeRL + vLLM 链路中已经跑通的 8GPU smoke 与 10-step 稳定性验证配置。目标是为后续正式后训练提供可复用的稳定起点。

## 1. 当前结论

| 项目 | 结论 |
|---|---|
| 基座模型 | `Qwen/Qwen3.6-35B-A3B` |
| 模型路径 | `/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B` |
| VeRL 镜像 | `localhost/uenv-bridge-verl:qwen35-torch210-vllm019-tf514-kernelfix` |
| GPU 配置 | 8 张 A100，容器内 `CUDA_VISIBLE_DEVICES=0,1,2,3,4,5,6,7` |
| rollout 形态 | 单个 vLLM endpoint，`tensor_parallel_size=8` |
| UEnv 接入方式 | VeRL `AgentLoopWorker` 通过 UEnv AgentLoop batch 提交到 Server Adapter Core |
| gateway | Adapter 侧启动 gateway，Worker 访问 gateway URL，gateway 转发到 VeRL 内部 vLLM endpoint |
| 验证状态 | 3-step smoke、10-step `response_length=64`、`256`、`512`、`1024` 均完成；四组 10-step 中 request/result/gateway 均为 80 条，gateway 状态码均为 200 |
| 当前推荐稳定配置 | `ROLLOUT_ENABLE_SLEEP_MODE=False`、`ROLLOUT_FREE_CACHE_ENGINE=False`、`ROLLOUT_GPU_MEMORY_UTILIZATION=0.20` |

本轮验证已经不再依赖下面两个临时规避开关：

```text
UENV_PATCH_VLLM_DISABLE_DEEPEP
UENV_PATCH_VLLM_DISABLE_FLASH_ATTN
```

当前稳定镜像中已重新构建 `flash-attn` 与 `deep_ep_cpp`，真实 UEnv 训练链路没有再出现 `undefined symbol` 类 ABI 错误。

## 2. 已验证运行命令

下面命令对应已跑通的 8GPU / Qwen3.6-35B / 3-step smoke。10-step 稳定性验证已使用同一套资源参数完成，除 `RUN_ID` 与 `TRAINING_STEPS` 外不需要改动其他配置。

```bash
cd /data/ronghao/uenv/uenv-bridge

RUN_ID=olymmath_grpo_35b_3step_kernelfix_default_nosleep384_20260730_184811 \
MODEL_PATH=/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B \
CONTAINER_MODEL_PATH=/models/modelscope/Qwen/Qwen3___6-35B-A3B \
DATA_DIR=/data/ronghao/uenv/uenv-bridge/temp/training_data/olymmath_easy_smoke \
CONTAINER_DATA_DIR=/data/olymmath_easy_smoke \
UENV_MODEL_GATEWAY_ENABLED=1 \
UENV_MODEL_GATEWAY_PORT=18194 \
UENV_MODEL_GATEWAY_PUBLIC_URL=http://10.10.20.142:18194/v1 \
TRAINING_STEPS=3 \
TRAIN_BATCH_SIZE=4 \
PPO_MINI_BATCH_SIZE=4 \
PPO_MICRO_BATCH_SIZE_PER_GPU=1 \
ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=1 \
REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=1 \
ROLLOUT_N=2 \
ROLLOUT_TP=8 \
MAX_PROMPT_LENGTH=512 \
DATA_MAX_RESPONSE_LENGTH=64 \
TEST_FREQ=-1 \
SAVE_FREQ=-1 \
PODMAN_GPU_ARGS='nvidia.com/gpu=all' \
CUDA_VISIBLE_DEVICES_IN_CONTAINER=0,1,2,3,4,5,6,7 \
NGPUS_PER_NODE=8 \
RAY_NUM_CPUS=32 \
ROLLOUT_GPU_MEMORY_UTILIZATION=0.20 \
ROLLOUT_ENABLE_SLEEP_MODE=False \
ROLLOUT_FREE_CACHE_ENGINE=False \
EXTRA_VERL_ARGS='+actor_rollout_ref.model.override_config.attn_implementation=sdpa actor_rollout_ref.rollout.max_model_len=2048 actor_rollout_ref.rollout.max_num_batched_tokens=384 actor_rollout_ref.actor.fsdp_config.optimizer_offload=True' \
./scripts/train/launchers/common/run_verl_uenv_grpo.sh
```

10-step 稳定性验证使用的差异如下：

```bash
RUN_ID=olymmath_grpo_35b_10step_kernelfix_default_nosleep384_20260730_201527
TRAINING_STEPS=10
```

10-step / response length 256 放大验证使用的差异如下：

```bash
RUN_ID=olymmath_grpo_35b_10step_resp256_kernelfix_default_nosleep384_20260730_203617
TRAINING_STEPS=10
DATA_MAX_RESPONSE_LENGTH=256
```

10-step / response length 512 放大验证使用的差异如下：

```bash
RUN_ID=olymmath_grpo_35b_10step_resp512_kernelfix_default_nosleep384_20260730_210130
TRAINING_STEPS=10
DATA_MAX_RESPONSE_LENGTH=512
```

10-step / response length 1024 放大验证使用的差异如下：

```bash
RUN_ID=olymmath_grpo_35b_10step_resp1024_kernelfix_default_nosleep384_20260730_213316
TRAINING_STEPS=10
DATA_MAX_RESPONSE_LENGTH=1024
```

## 3. 关键参数说明

| 参数 | 当前稳定值 | 作用与说明 |
|---|---:|---|
| `IMAGE` | `localhost/uenv-bridge-verl:qwen35-torch210-vllm019-tf514-kernelfix` | 默认训练镜像已切换到 kernelfix 版本。 |
| `NGPUS_PER_NODE` | `8` | VeRL trainer 侧声明 8 张 GPU。 |
| `PODMAN_GPU_ARGS` | `nvidia.com/gpu=all` | 将全部 GPU 暴露给容器。 |
| `CUDA_VISIBLE_DEVICES_IN_CONTAINER` | `0,1,2,3,4,5,6,7` | 容器内部使用 8 张可见 GPU。 |
| `ROLLOUT_TP` | `8` | vLLM rollout 使用单个 8 卡 tensor-parallel endpoint。 |
| `TRAIN_BATCH_SIZE` | `4` | 当前 smoke 先用小 batch 验证链路稳定性。 |
| `PPO_MINI_BATCH_SIZE` | `4` | 与 train batch 对齐，避免 batch 切分约束报错。 |
| `PPO_MICRO_BATCH_SIZE_PER_GPU` | `1` | 降低 actor update 显存压力。 |
| `ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU` | `1` | 降低 rollout logprob 显存压力。 |
| `REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU` | `1` | 降低 ref logprob 显存压力。 |
| `ROLLOUT_N` | `2` | 每条 prompt 采样 2 条 response，因此每 step 产生 8 个 episode。 |
| `DATA_MAX_RESPONSE_LENGTH` | `64` / `256` / `512` / `1024` | `64` 用作短输出链路 smoke；`256`、`512`、`1024` 已完成 10-step 放大验证。四组验证主要用于链路和显存稳定性确认，正式数学训练仍需单独确定质量配置。 |
| `actor_rollout_ref.rollout.max_model_len` | `2048` | 控制 vLLM 最大上下文长度，先压低以减少显存压力。 |
| `actor_rollout_ref.rollout.max_num_batched_tokens` | `384` | Qwen3.6 MoE 在当前 vLLM 配置下受 Mamba cache align 约束，384 已验证稳定。 |
| `ROLLOUT_GPU_MEMORY_UTILIZATION` | `0.20` | 降低 vLLM KV/cache 预留，给 FSDP actor/ref/update 留显存空间；0.25 已验证更慢，0.30 历史上触发过 wake_up OOM。 |
| `ROLLOUT_ENABLE_SLEEP_MODE` | `False` | sleep mode 0.20/0.25 已完成 3-step 对照，未带来加速，且启动开销更高；当前推荐关闭。 |
| `ROLLOUT_FREE_CACHE_ENGINE` | `False` | 与 sleep mode 一起保持关闭，减少 vLLM cache 生命周期变化。 |
| `actor_rollout_ref.actor.fsdp_config.optimizer_offload` | `True` | 将 optimizer 状态 offload 到 CPU，换取 35B 模型在 8 卡上的训练显存余量。 |
| `actor_rollout_ref.model.override_config.attn_implementation` | `sdpa` | 当前稳定 smoke 使用 HF SDPA 路径，避免训练侧 attention 实现差异引入额外变量。 |

## 4. 验证证据

### 4.1 3-step smoke

| 项目 | 路径或结果 |
|---|---|
| VeRL 主日志 | `/data/ronghao/uenv/uenv-bridge/temp/logs/verl_layer4_agent_loop/olymmath_grpo_35b_3step_kernelfix_default_nosleep384_20260730_184811.log` |
| request 日志 | `/data/ronghao/uenv/uenv-bridge/temp/logs/layer4_distributed/olymmath_grpo_35b_3step_kernelfix_default_nosleep384_20260730_184811/agent-loop-requests.jsonl` |
| result 日志 | `/data/ronghao/uenv/uenv-bridge/temp/logs/layer4_distributed/olymmath_grpo_35b_3step_kernelfix_default_nosleep384_20260730_184811/agent-loop-results.jsonl` |
| gateway 日志 | `/data/ronghao/uenv/uenv-bridge/temp/logs/layer4_distributed/olymmath_grpo_35b_3step_kernelfix_default_nosleep384_20260730_184811/model-gateway.jsonl` |
| 训练进度 | `Training Progress: 100%|3/3` |
| request/result/gateway | `24 / 24 / 24` |
| gateway status | `200: 24` |
| vLLM upstream | `http://10.10.20.142:46123/v1` |

三步耗时如下：

| step | `timing_s/gen` | `timing_s/ref` | `timing_s/update_actor` | `timing_s/update_weights` | `timing_s/step` |
|---:|---:|---:|---:|---:|---:|
| 1 | 25.85 | 37.51 | 28.31 | 9.83 | 104.54 |
| 2 | 18.69 | 2.53 | 14.78 | 17.18 | 54.90 |
| 3 | 13.67 | 2.58 | 14.08 | 17.74 | 50.07 |

第一步耗时明显更高，主要是冷启动和首次 ref/update 路径初始化影响；后两步更接近稳定 step 时间。

### 4.2 10-step 稳定性验证

| 项目 | 路径或结果 |
|---|---|
| VeRL 主日志 | `/data/ronghao/uenv/uenv-bridge/temp/logs/verl_layer4_agent_loop/olymmath_grpo_35b_10step_kernelfix_default_nosleep384_20260730_201527.log` |
| request 日志 | `/data/ronghao/uenv/uenv-bridge/temp/logs/layer4_distributed/olymmath_grpo_35b_10step_kernelfix_default_nosleep384_20260730_201527/agent-loop-requests.jsonl` |
| result 日志 | `/data/ronghao/uenv/uenv-bridge/temp/logs/layer4_distributed/olymmath_grpo_35b_10step_kernelfix_default_nosleep384_20260730_201527/agent-loop-results.jsonl` |
| gateway 日志 | `/data/ronghao/uenv/uenv-bridge/temp/logs/layer4_distributed/olymmath_grpo_35b_10step_kernelfix_default_nosleep384_20260730_201527/model-gateway.jsonl` |
| 训练进度 | `Training Progress: 100%|10/10` |
| 最终验证指标 | `Final validation metrics: None` |
| request/result/gateway | `80 / 80 / 80` |
| gateway status | `200: 80` |
| vLLM upstream | `http://10.10.20.142:42811/v1` |
| 致命错误检查 | 未出现 `Traceback`、`AssertionError`、`CUDA Error`、`out of memory`、`undefined symbol` |

10-step 耗时如下：

| step | `timing_s/gen` | `timing_s/ref` | `timing_s/update_actor` | `timing_s/update_weights` | `timing_s/step` |
|---:|---:|---:|---:|---:|---:|
| 1 | 25.86 | 34.86 | 25.81 | 10.46 | 100.92 |
| 2 | 18.70 | 2.59 | 14.69 | 16.41 | 54.57 |
| 3 | 14.17 | 2.58 | 13.76 | 16.30 | 48.84 |
| 4 | 16.18 | 2.52 | 13.74 | 15.54 | 49.74 |
| 5 | 11.66 | 2.59 | 13.90 | 16.05 | 45.91 |
| 6 | 11.15 | 2.61 | 14.01 | 16.28 | 45.94 |
| 7 | 15.67 | 2.79 | 13.77 | 16.11 | 50.26 |
| 8 | 10.66 | 2.64 | 13.60 | 15.61 | 44.42 |
| 9 | 12.16 | 2.56 | 14.05 | 16.09 | 46.62 |
| 10 | 10.65 | 2.68 | 14.45 | 17.02 | 46.58 |

整体进度显示 `10/10 [08:53<00:00, 53.39s/it]`。去掉第一步冷启动后，step 2-10 的平均 `timing_s/step` 约为 `48.10s`，稳定阶段主要落在 `44s-50s`。

### 4.3 10-step / response length 256 放大验证

| 项目 | 路径或结果 |
|---|---|
| VeRL 主日志 | `/data/ronghao/uenv/uenv-bridge/temp/logs/verl_layer4_agent_loop/olymmath_grpo_35b_10step_resp256_kernelfix_default_nosleep384_20260730_203617.log` |
| request 日志 | `/data/ronghao/uenv/uenv-bridge/temp/logs/layer4_distributed/olymmath_grpo_35b_10step_resp256_kernelfix_default_nosleep384_20260730_203617/agent-loop-requests.jsonl` |
| result 日志 | `/data/ronghao/uenv/uenv-bridge/temp/logs/layer4_distributed/olymmath_grpo_35b_10step_resp256_kernelfix_default_nosleep384_20260730_203617/agent-loop-results.jsonl` |
| gateway 日志 | `/data/ronghao/uenv/uenv-bridge/temp/logs/layer4_distributed/olymmath_grpo_35b_10step_resp256_kernelfix_default_nosleep384_20260730_203617/model-gateway.jsonl` |
| 训练进度 | `Training Progress: 100%|10/10` |
| 最终验证指标 | `Final validation metrics: None` |
| request/result/gateway | `80 / 80 / 80` |
| gateway status | `200: 80` |
| vLLM upstream | `http://10.10.20.142:44273/v1` |
| 致命错误检查 | 未出现 `Traceback`、`AssertionError`、`CUDA Error`、`out of memory`、`undefined symbol` |

10-step / response length 256 耗时如下：

| step | `timing_s/gen` | `timing_s/ref` | `timing_s/update_actor` | `timing_s/update_weights` | `timing_s/step` |
|---:|---:|---:|---:|---:|---:|
| 1 | 54.34 | 35.58 | 25.26 | 10.18 | 128.74 |
| 2 | 38.81 | 2.68 | 16.80 | 16.69 | 77.19 |
| 3 | 42.32 | 2.94 | 14.42 | 15.37 | 77.36 |
| 4 | 39.81 | 2.61 | 14.02 | 16.00 | 74.40 |
| 5 | 42.30 | 2.87 | 14.04 | 15.55 | 77.30 |
| 6 | 39.81 | 2.57 | 14.51 | 16.58 | 75.27 |
| 7 | 40.82 | 2.64 | 13.71 | 15.39 | 74.86 |
| 8 | 41.81 | 2.84 | 14.57 | 15.60 | 76.94 |
| 9 | 40.81 | 2.76 | 13.97 | 16.46 | 75.97 |
| 10 | 40.30 | 2.65 | 14.73 | 15.38 | 75.25 |

整体进度显示 `10/10 [13:33<00:00, 81.33s/it]`。去掉第一步冷启动后，step 2-10 的平均 `timing_s/step` 约为 `76.06s`，平均 `timing_s/gen` 约为 `40.75s`。本轮所有 step 的 `response_length/mean`、`response_length/max`、`response_length/min` 均为 `256`，说明输出仍然触达上限；正式数学训练还需要继续提高 response length。退出清理阶段出现过 `multiprocessing.resource_tracker` warning，但脚本最终返回 `Distributed Layer 4 smoke test completed`，未影响训练完成。

### 4.4 10-step / response length 512 放大验证

| 项目 | 路径或结果 |
|---|---|
| VeRL 主日志 | `/data/ronghao/uenv/uenv-bridge/temp/logs/verl_layer4_agent_loop/olymmath_grpo_35b_10step_resp512_kernelfix_default_nosleep384_20260730_210130.log` |
| request 日志 | `/data/ronghao/uenv/uenv-bridge/temp/logs/layer4_distributed/olymmath_grpo_35b_10step_resp512_kernelfix_default_nosleep384_20260730_210130/agent-loop-requests.jsonl` |
| result 日志 | `/data/ronghao/uenv/uenv-bridge/temp/logs/layer4_distributed/olymmath_grpo_35b_10step_resp512_kernelfix_default_nosleep384_20260730_210130/agent-loop-results.jsonl` |
| gateway 日志 | `/data/ronghao/uenv/uenv-bridge/temp/logs/layer4_distributed/olymmath_grpo_35b_10step_resp512_kernelfix_default_nosleep384_20260730_210130/model-gateway.jsonl` |
| 训练进度 | `Training Progress: 100%|10/10` |
| 最终验证指标 | `Final validation metrics: None` |
| request/result/gateway | `80 / 80 / 80` |
| gateway status | `200: 80` |
| vLLM upstream | `http://10.10.20.142:37931/v1` |
| 致命错误检查 | 未出现 `Traceback`、`AssertionError`、`CUDA Error`、`out of memory`、`undefined symbol` |

10-step / response length 512 耗时如下：

| step | `timing_s/gen` | `timing_s/old_log_prob` | `timing_s/ref` | `timing_s/update_actor` | `timing_s/update_weights` | `timing_s/step` |
|---:|---:|---:|---:|---:|---:|---:|
| 1 | 89.50 | 3.52 | 34.45 | 26.39 | 9.80 | 163.68 |
| 2 | 79.50 | 2.12 | 2.42 | 15.64 | 16.79 | 116.47 |
| 3 | 77.97 | 2.52 | 2.71 | 14.73 | 16.64 | 114.57 |
| 4 | 75.01 | 2.46 | 2.39 | 15.47 | 15.30 | 110.64 |
| 5 | 75.01 | 2.14 | 2.44 | 14.86 | 15.62 | 110.07 |
| 6 | 76.48 | 2.43 | 2.38 | 14.73 | 15.40 | 111.42 |
| 7 | 75.98 | 2.15 | 2.41 | 13.78 | 16.03 | 110.36 |
| 8 | 76.48 | 2.01 | 2.39 | 15.29 | 15.06 | 111.25 |
| 9 | 76.52 | 2.63 | 2.38 | 14.24 | 14.89 | 110.66 |
| 10 | 77.02 | 2.49 | 2.44 | 14.36 | 15.69 | 112.01 |

整体进度显示 `10/10 [19:31<00:00, 117.12s/it]`。去掉第一步冷启动后，step 2-10 的平均 `timing_s/step` 约为 `111.94s`，平均 `timing_s/gen` 约为 `76.66s`，平均 `timing_s/update_actor` 约为 `14.79s`，平均 `timing_s/update_weights` 约为 `15.71s`。本轮所有 step 的 `response_length/mean`、`response_length/max`、`response_length/min` 均为 `512`，`response_length/clip_ratio` 为 `1.0`，说明输出仍然触达上限；正式数学训练仍需要继续提高 response length。退出清理阶段出现过 vLLM `Engine core proc EngineCore died unexpectedly, shutting down client` 日志，但该日志发生在 `Final validation metrics: None` 与脚本完成提示之后，训练主流程已完成，GPU 资源随后释放。

### 4.5 10-step / response length 1024 放大验证

| 项目 | 路径或结果 |
|---|---|
| VeRL 主日志 | `/data/ronghao/uenv/uenv-bridge/temp/logs/verl_layer4_agent_loop/olymmath_grpo_35b_10step_resp1024_kernelfix_default_nosleep384_20260730_213316.log` |
| request 日志 | `/data/ronghao/uenv/uenv-bridge/temp/logs/layer4_distributed/olymmath_grpo_35b_10step_resp1024_kernelfix_default_nosleep384_20260730_213316/agent-loop-requests.jsonl` |
| result 日志 | `/data/ronghao/uenv/uenv-bridge/temp/logs/layer4_distributed/olymmath_grpo_35b_10step_resp1024_kernelfix_default_nosleep384_20260730_213316/agent-loop-results.jsonl` |
| gateway 日志 | `/data/ronghao/uenv/uenv-bridge/temp/logs/layer4_distributed/olymmath_grpo_35b_10step_resp1024_kernelfix_default_nosleep384_20260730_213316/model-gateway.jsonl` |
| 训练进度 | `Training Progress: 100%|10/10` |
| 最终验证指标 | `Final validation metrics: None` |
| request/result/gateway | `80 / 80 / 80` |
| gateway status | `200: 80` |
| vLLM upstream | `http://10.10.20.142:45173/v1` |
| 致命错误检查 | 未出现 `Traceback`、`AssertionError`、`CUDA Error`、`out of memory`、`undefined symbol` |

10-step / response length 1024 耗时如下：

| step | `timing_s/gen` | `timing_s/old_log_prob` | `timing_s/ref` | `timing_s/update_actor` | `timing_s/update_weights` | `timing_s/step` |
|---:|---:|---:|---:|---:|---:|---:|
| 1 | 162.45 | 3.54 | 34.00 | 28.64 | 10.26 | 238.89 |
| 2 | 148.40 | 2.18 | 2.72 | 16.90 | 16.76 | 186.96 |
| 3 | 147.36 | 2.53 | 2.90 | 15.38 | 16.11 | 184.29 |
| 4 | 150.37 | 2.19 | 2.71 | 15.67 | 16.83 | 187.78 |
| 5 | 149.38 | 2.46 | 2.71 | 15.45 | 16.26 | 186.27 |
| 6 | 150.83 | 2.86 | 2.74 | 14.95 | 16.55 | 187.94 |
| 7 | 150.35 | 2.62 | 2.74 | 15.33 | 15.91 | 186.95 |
| 8 | 150.37 | 2.32 | 2.76 | 16.47 | 15.95 | 187.88 |
| 9 | 149.34 | 2.25 | 2.99 | 16.18 | 15.42 | 186.18 |
| 10 | 150.89 | 3.13 | 2.73 | 15.12 | 17.16 | 189.04 |

整体进度显示 `10/10 [32:02<00:00, 192.22s/it]`。去掉第一步冷启动后，step 2-10 的平均 `timing_s/step` 约为 `187.03s`，平均 `timing_s/gen` 约为 `149.70s`，平均 `timing_s/update_actor` 约为 `15.72s`，平均 `timing_s/update_weights` 约为 `16.33s`。本轮所有 step 的 `response_length/mean`、`response_length/max`、`response_length/min` 均为 `1024`，`response_length/clip_ratio` 为 `1.0`，`response/aborted_ratio` 为 `0.0`。这说明链路和显存配置可以支撑 1024 长度的 10-step smoke，但输出仍然触达上限；正式数学训练如果追求完整长思考答案，需要继续评估更长 response length、数据难度和提示格式。退出清理阶段同样出现过 vLLM `Engine core proc EngineCore died unexpectedly, shutting down client` 日志，但该日志发生在 `Final validation metrics: None` 之后，训练主流程已经完成。

### 4.6 sleep mode / GPU memory utilization 对照

为确认 VeRL hybrid engine sleep/free cache 是否能给 Qwen3.6-35B 训练带来收益，在 `DATA_MAX_RESPONSE_LENGTH=1024`、`TRAIN_BATCH_SIZE=4`、`ROLLOUT_N=2`、`ROLLOUT_TP=8`、`max_model_len=2048`、`max_num_batched_tokens=384` 的相同条件下，额外对照了 sleep mode 与 `gpu_memory_utilization`。

| 配置 | 步数 | request/result/gateway | gateway status | 稳态 `gen` | 稳态 `update_actor` | 稳态 `update_weights` | 稳态 `step` | 结论 |
|---|---:|---:|---|---:|---:|---:|---:|---|
| no-sleep，`util=0.20` | 10 | `80 / 80 / 80` | `200: 80` | 149.70s | 15.72s | 16.33s | 187.03s | 当前最佳稳定配置 |
| sleep/free-cache，`util=0.20` | 3 | `24 / 24 / 24` | `200: 24` | 150.55s | 13.15s | 18.73s | 187.54s | 稳态基本持平，但启动更慢 |
| sleep/free-cache，`util=0.25` | 3 | `24 / 24 / 24` | `200: 24` | 156.30s | 12.96s | 18.92s | 193.58s | 更慢，不继续上探 |

对应日志如下：

| 配置 | VeRL 主日志 |
|---|---|
| no-sleep，`util=0.20` | `/data/ronghao/uenv/uenv-bridge/temp/logs/verl_layer4_agent_loop/olymmath_grpo_35b_10step_resp1024_kernelfix_default_nosleep384_20260730_213316.log` |
| sleep/free-cache，`util=0.20` | `/data/ronghao/uenv/uenv-bridge/temp/logs/verl_layer4_agent_loop/olymmath_grpo_35b_3step_resp1024_sleep_util020_20260730_224305.log` |
| sleep/free-cache，`util=0.25` | `/data/ronghao/uenv/uenv-bridge/temp/logs/verl_layer4_agent_loop/olymmath_grpo_35b_3step_resp1024_sleep_util025_20260730_230152.log` |

本轮没有继续运行 `sleep/free-cache + util=0.30`，原因有两点：第一，`util=0.25` 已经比 `util=0.20` 慢约 `6.0s/step`；第二，历史运行中 `sleep/free-cache + util=0.30` 曾在 `update_weights` 的 `wake_up(tags=["weights"])` 阶段触发 vLLM `cumem_allocator` CUDA OOM。当前最佳配置因此保持为 `ROLLOUT_ENABLE_SLEEP_MODE=False`、`ROLLOUT_FREE_CACHE_ENGINE=False`、`ROLLOUT_GPU_MEMORY_UTILIZATION=0.20`。

## 5. 已知不稳定配置

| 配置 | 现象 | 当前处理 |
|---|---|---|
| `ROLLOUT_ENABLE_SLEEP_MODE=True`、`ROLLOUT_FREE_CACHE_ENGINE=True`、`ROLLOUT_GPU_MEMORY_UTILIZATION=0.30` | 在 `update_weights` 的 `wake_up(tags=["weights"])` 阶段触发 vLLM `cumem_allocator` CUDA OOM。 | 当前关闭 sleep/free cache，并将 GPU memory utilization 降为 `0.20`。 |
| `actor_rollout_ref.rollout.max_num_batched_tokens=256` | vLLM 初始化时报 `Mamba cache align mode, block_size (272) must be <= max_num_batched_tokens (256)`。 | 当前使用 `384`。后续优先沿用 `384`，并把 `272` 作为下限约束。 |
| `DATA_MAX_RESPONSE_LENGTH=64`、`256`、`512` 或 `1024` | 四组验证中 response 都会触达对应 token 上限，说明这些长度适合链路和显存验证；正式数学训练的完整长解答需要继续评估更长输出。 | 正式训练前结合数据难度继续评估 response length，并同步重新评估显存、耗时和超时设置。 |
| `DATA_MAX_RESPONSE_LENGTH=8192`、`max_model_len=8704` | 探索性运行在首个 step 生成阶段耗时过长，已手动中断；对应日志为 `/data/ronghao/uenv/uenv-bridge/temp/logs/verl_layer4_agent_loop/olymmath_grpo_35b_3step_resp8192_kernelfix_default_nosleep384_20260730_221750.log`。 | 当前正式训练起点推荐从 `1536`、`2048` 逐级评估；`8192` 作为长思考探索记录保留。 |
| 默认 `ROLLOUT_GPU_MEMORY_UTILIZATION=0.8` | 未在 Qwen3.6-35B 训练链路中验证，预期与 FSDP actor/ref 共卡时显存压力偏大。 | 35B smoke 先固定 `0.20`。 |

## 6. 放大策略

后续建议分阶段放大，每次只扩大 batch、response length、训练步数中的一个维度。

1. 10-step 稳定性验证已经完成：
   - 只改 `TRAINING_STEPS=10`；
   - 保持 `TRAIN_BATCH_SIZE=4`、`ROLLOUT_N=2`、`DATA_MAX_RESPONSE_LENGTH=64`；
   - request/result/gateway 已完全对齐，均为 `80` 条。

2. response length 256 放大验证已经完成：
   - 只改 `DATA_MAX_RESPONSE_LENGTH=256`；
   - 保持 `TRAIN_BATCH_SIZE=4`、`ROLLOUT_N=2`、`TRAINING_STEPS=10`；
   - request/result/gateway 已完全对齐，均为 `80` 条；
   - 去掉第一步冷启动后，step 2-10 平均耗时约 `76.06s`。

3. response length 512 放大验证已经完成：
   - 只改 `DATA_MAX_RESPONSE_LENGTH=512`；
   - 保持 `TRAIN_BATCH_SIZE=4`、`ROLLOUT_N=2`、`TRAINING_STEPS=10`；
   - request/result/gateway 已完全对齐，均为 `80` 条；
   - 去掉第一步冷启动后，step 2-10 平均耗时约 `111.94s`。

4. response length 1024 放大验证已经完成：
   - 只改 `DATA_MAX_RESPONSE_LENGTH=1024`；
   - 保持 `TRAIN_BATCH_SIZE=4`、`ROLLOUT_N=2`、`TRAINING_STEPS=10`；
   - request/result/gateway 已完全对齐，均为 `80` 条；
   - 去掉第一步冷启动后，step 2-10 平均耗时约 `187.03s`；
   - `response_length/clip_ratio` 仍为 `1.0`，说明 1024 仍可能不足以覆盖正式数学长答案。

5. 下一步继续提高输出长度或降低样本难度：
   - 如果继续验证 `DATA_MAX_RESPONSE_LENGTH=1536`，当前 `MAX_PROMPT_LENGTH=512` 与 `actor_rollout_ref.rollout.max_model_len=2048` 仍可以覆盖 `prompt + response <= 2048` 的组合，但余量很小；
   - 如果后续 response length 超过 `1536`，需要同步提高 `actor_rollout_ref.rollout.max_model_len`；
   - 如果显存吃紧，优先保持 `TRAIN_BATCH_SIZE=4`，先稳定 response length 和训练步数。

6. 最后扩大有效 batch：
   - 优先尝试 `TRAIN_BATCH_SIZE=8`、`PPO_MINI_BATCH_SIZE=4` 或 `8`；
   - 保持各 micro batch per GPU 为 `1`；
   - 每次调整后检查 VeRL 对 `force_group_size * micro_batch_size_per_gpu` 的整除约束。

## 7. 正式训练数据与训练策略

本章只讨论正式训练前需要先定下来的宏观因素：训练数据怎么选、训练方式怎么组织、同步还是异步、是否使用 LoRA、是否做多任务混合。具体 batch、micro batch、timeout 等执行细节继续放在前文稳定配置和后续实验记录中维护。

### 7.1 数据选择

当前建议从数学单任务开始，先把 UEnv + VeRL + Qwen3.6-35B 的训练闭环跑稳定，再逐步加入 PubMedQA、SciTab、代码生成和 SWE 类任务。

#### 7.1.1 第一阶段：数学数据

第一阶段正式数学训练采用 **GSM8K train**。当前 smoke 使用的 OlymMATH 样本继续作为链路验证数据，OlymMATH 全量评测集继续作为训练外数学 benchmark，用于观察训练后数学能力是否有提升。

| 项目 | 选择 |
|---|---|
| 正式数学训练数据 | `/data/ronghao/uenv/uenv-bridge/data/gsm8k/train.parquet` |
| 数学验证数据 | `/data/ronghao/uenv/uenv-bridge/data/gsm8k/test.parquet`，或从 train 中再切少量 validation 子集 |
| 外部数学 benchmark | OlymMATH EN/ZH EASY/HARD 保留为训练外回归评测集 |
| 当前 smoke 数据 | `/data/ronghao/uenv/uenv-bridge/temp/training_data/olymmath_easy_smoke`，只用于链路和稳定性验证 |

本地 GSM8K 数据情况如下：

| 文件 | 样本数 | `data_source` | `ability` | reward |
|---|---:|---|---|---|
| `data/gsm8k/train.parquet` | 7473 | `openai/gsm8k` | `math` | `reward_model.style=rule`，`reward_model.ground_truth` 为最终答案 |
| `data/gsm8k/test.parquet` | 1319 | `openai/gsm8k` | `math` | `reward_model.style=rule`，`reward_model.ground_truth` 为最终答案 |

选择 GSM8K 的原因：

1. 与当前 OlymMATH benchmark 分离，避免把评测样本混入训练集导致 benchmark 泄漏。
2. 答案格式稳定，样本中要求最终答案使用 `#### <number>`，便于 Worker 侧规则判分。
3. 样本长度和解题难度适中，比 OlymMATH 更适合作为 Qwen3.6-35B 正式训练前的第一批稳定训练数据。
4. 当前 adapter 的 `env_type` 归一逻辑会把 `gsm8k/math/olymmath` 等数据路由到 `qa` 验证型环境，和 Worker 侧 math 到 qa 的改造方向一致。

因此，OlymMATH 继续作为数学能力的外部回归评测；正式训练从 GSM8K 开始。等 GSM8K 训练链路稳定后，再考虑加入 MATH train、NuminaMath、OpenMathReasoning 等更难或更大规模的数学数据。

数学类任务推荐统一采用 **boxed answer protocol**。训练数据构造 prompt 时，直接写入最终答案格式要求：

```text
Solve the problem step by step. Put your final answer in \boxed{}.
```

对于中文数学题，可以使用：

```text
请逐步解答题目，并将最终答案写在 \boxed{} 中。
```

Worker 侧数学 reward 推荐统一抽取模型输出中的最后一个 `\boxed{...}` 作为最终答案，再与 `reward_model.ground_truth` 做归一化比较。GSM8K 原始答案中的 `#### <answer>` 可以在数据预处理或 Worker 判分阶段归一化为 `<answer>`，再进入同一套 boxed 抽取与答案等价判断逻辑。这样 GSM8K、MATH、OlymMATH、NuminaMath 等数学数据可以共享同一套 prompt 约束和 reward 抽取逻辑。

#### 7.1.2 PubMedQA 与 SciTab 数据

PubMedQA 和 SciTab 属于单轮 QA / 表格验证任务。训练效果更依赖数据质量、数据分布、reward 设计和输出格式。推荐将 benchmark 测试集固定为评测集，并使用与 benchmark 同分布或相邻分布的数据作为训练来源。

PubMedQA 推荐使用官方训练数据，保留官方 500 条 expert-labeled test 作为最终评测。

| 数据 | 用途 | 说明 |
|---|---|---|
| `PQA-A` | 主训练数据 | 人工生成标签规模较大，适合作为 PubMedQA 正式训练主数据。 |
| `PQA-L` 非 test 部分 | 高质量小规模校准 | 本地 `ori_pqal.json` 有 1000 条 expert-labeled 样本，其中 `test_ground_truth.json` 对应的 500 条保留为 test。 |
| `PQA-U` | 后续伪标签或 SFT 数据 | 无标签数据更适合进入伪标签或 SFT 阶段。 |
| 官方 500 条 test | 固定评测 | 作为 PubMedQA benchmark 和参数冻结后的最终评测集。 |

SciTab 推荐使用相邻分布数据训练，并将当前 full benchmark 固定为外部评测集。当前本地 `data/benchmarks/scitab/sci_tab.json` 是 1224 条完整 SciTab 样本，适合作为外部 benchmark 保留。

| 数据 | 用途 | 说明 |
|---|---|---|
| `TabFact` | 主训练数据 | 表格事实验证数据，适合训练“读表 + 判断 claim 是否成立”的基础能力。 |
| `FEVEROUS` | 补充训练数据 | 同时包含文本和表格证据，标签包含 support/refute/NEI，更接近 SciTab 三分类判断。 |
| `SciFact` | 辅助训练数据 | 科学 claim verification 语言分布更接近 SciTab，但证据主要是 abstract，不是表格。 |
| `SciTabQA` / 科学表格 QA 类数据 | 可选辅助 | 用于增强科学表格理解，但任务形式与 SciTab claim verification 不完全一致。 |
| `sci_tab.json` | 固定评测 | 当前 1224 条样本作为 SciTab benchmark。 |

如果后续使用 SciTab 自身样本做训练，推荐按 `paper_id` 或 `table_id` 切分 train/dev/test，保证同一张表的 claim 只出现在一个 split 中，从表格级别控制数据泄漏风险。

#### 7.1.3 代码与 SWE 类数据

代码生成和 SWE 类任务更能体现 UEnv 环境交互价值，因为 reward 不只是文本匹配，而是依赖执行、测试、文件修改和工具轨迹。

| 任务 | 数据选择原则 | 评测隔离 |
|---|---|---|
| 代码生成 | 优先选择有单元测试或输入输出用例的数据，训练时要保留可执行 reward。 | DSCodeBench full test 固定作为 benchmark。 |
| 程序修复 / SWE | 优先使用训练 split 或公开 issue-fix 数据，要求 repo checkout、patch、test 逻辑可复现。 | SWE-bench-Pro test 固定作为 benchmark。 |
| Agentic 工具任务 | 数据需要包含明确目标、工具可执行环境和可回放轨迹。 | benchmark 轨迹和测试用例固定隔离为评测资产。 |

这类任务推荐放在第二阶段之后进入训练。原因是链路变量更多：OpenHands 或工具 worker 的稳定性、repo 映射、测试执行、trajectory 记录、超时和失败恢复都会影响 reward。推荐先用数学 / QA 任务验证训练链路，再把代码和 SWE 纳入后续阶段。

#### 7.1.4 数据格式要求

正式训练数据应保持 VeRL parquet 格式，并保留 UEnv 能够识别的任务信息：

| 字段 | 要求 |
|---|---|
| `data_source` | 建议使用稳定数据集名，例如 `openai/gsm8k`。 |
| `prompt` | Chat messages 结构，至少包含 system 和 user；数学类 prompt 需要显式要求最终答案写入 `\boxed{}`。 |
| `ability` | 现有 GSM8K 为 `math`；adapter 会将 `math/gsm8k/olymmath` 归一到 `env_type=qa`。 |
| `reward_model.style` | 数学首轮使用 `rule`。 |
| `reward_model.ground_truth` | 最终标准答案，例如 `72`。 |
| `extra_info.solution` | 保留原始解析过程，便于排查 reward 和模型输出差异。 |

训练侧可以继续保留 `ability=math` 以兼容 VeRL 数据习惯；发送到 UEnv 时由 adapter 归一为 `env_type=qa`，Worker 侧按 dataset/task 路由到对应判分逻辑。后续如果重新生成数据，也可以显式增加 `extra_info.dataset=gsm8k`、`extra_info.split=train`、`extra_info.index`，便于日志回溯。

### 7.2 训练策略

正式训练的宏观策略主要由五个变量决定：训练方式、同步/异步模式、response length、训练规模和多任务混合方式。当前推荐以稳定链路为中心逐层放大，每轮只调整一个主要变量。

#### 7.2.1 总体路线

推荐采用阶段式混合路线：先建立数学单任务基线，再加入第二任务，并在新任务训练时保留旧任务 replay。这样可以同时观察新任务收益和旧任务回归表现。五类任务的大规模混合适合放在单任务链路、reward、评测回归都稳定之后。

推荐路线如下：

| 阶段 | 目标 | 数据策略 | 通过标准 |
|---|---|---|---|
| A. 数学单任务 smoke | 验证 Qwen3.6-35B 训练链路、显存和长 response 配置 | GSM8K 小子集，100 到 500 条 | request/result/gateway 对齐，无 OOM，无 batch 卡死 |
| B. 数学正式小跑 | 验证 reward 曲线和数学 benchmark 是否有正向变化 | GSM8K train 子集 1000 到 5000 条，固定 OlymMATH 外部评测 | reward 不崩，OlymMATH/GSM8K 回归指标可解释 |
| C. 加入第二任务 | 验证多任务混合与旧任务回放 | 新任务数据 + GSM8K replay + 少量通用 QA | 新任务提升时，数学 benchmark 不明显回退 |
| D. 五任务混合 | 面向最终任务书指标进行联合后训练 | 五类任务按成本和收益配比混合 | 五类 benchmark 均完成回归评测 |
| E. 固化配置 | 形成可复现训练配置 | 固定镜像、数据版本、随机种子、训练参数和评测命令 | 结果可复现，日志证据完整 |

#### 7.2.2 全参训练与 LoRA

训练方式是正式训练前需要明确的第一类宏观选择。

| 训练方式 | 作用 | 优点 | 风险 | 建议 |
|---|---|---|---|---|
| 全参训练 | 直接更新 Qwen3.6-35B-A3B 全部可训练参数 | 能力上限最高，最接近最终后训练目标 | 显存、时间和失败成本高 | 最终正式训练需要走全参，前期以小规模全参验证为主 |
| LoRA / Adapter | 只训练低秩增量参数 | 成本低，适合快速验证数据、reward 和趋势 | 能力上限和全参存在差异 | 适合第一轮数据和 reward 试错，关键结论再用全参复验 |
| 先 LoRA 后全参 | 先低成本验证，再放大全参 | 能降低正式全参试错成本 | 流程更长，需要维护两套配置 | 当前较推荐 |

当前 8GPU Qwen3.6-35B 全参 smoke 已经跑通，因此从工程可行性上可以做全参训练。但从实验效率上，若目标是先判断某个数据集和 reward 是否有效，LoRA 更适合作为前置筛选；若目标是形成最终可交付模型，则需要切回全参训练。

#### 7.2.3 同步、One-step off-policy 与 Fully async

训练并行模式决定 rollout 和 update 是否重叠，也是影响训练效率的核心宏观变量。

| 模式 | 特点 | 优点 | 风险 | 当前建议 |
|---|---|---|---|---|
| 同步模式 | 当前 step rollout 完成后再 update | 逻辑最简单，版本一致性最好，最适合建立基线 | rollout 慢时 trainer 等待明显 | 第一轮正式训练使用 |
| One-step off-policy | 当前 update 与下一步 rollout 部分重叠 | 能降低 trainer idle，改动复杂度低于 fully async | 需要管理 old logprob、policy version 和 staleness | 同步基线稳定后再测 |
| Fully async | rollout 和 training 通过 queue/pool 解耦 | 吞吐潜力最大，适合 rollout 极慢场景 | 复杂度最高，对版本、队列、日志和失败恢复要求高 | 放在同步基线和 one-step off-policy 对照之后 |

当前建议先用同步模式训练 GSM8K 小规模基线。只有当确认 rollout 是主要瓶颈，并且同步模式下 reward 与 benchmark 趋势可信之后，再比较 One-step off-policy 和 Fully async。否则异步带来的收益和质量影响会混在一起，很难判断问题来自数据、reward 还是并行策略。

#### 7.2.4 资源形态与 rollout 吞吐

当前稳定配置是 `ROLLOUT_TP=8`，即一个 8 卡 tensor-parallel vLLM endpoint。该配置区别于 8 个单卡模型副本：它的优点是实现简单，权重同步由 VeRL 内部处理；更高 episode 并发需要后续评估多副本方案。

| 资源形态 | 含义 | 优点 | 风险 |
|---|---|---|---|
| 单个 8 卡 TP endpoint | 一个 vLLM 实例使用 8 卡做 TP | 当前已验证稳定，权重同步简单 | 并发扩展能力有限 |
| 多个相同 policy 副本 | 多个 vLLM endpoint 分担 rollout | rollout 吞吐更高 | 需要确保所有副本同步更新到同一 policy version |
| rollout/update 资源切分 | 部分 GPU 专门 rollout，部分 GPU update | 理论上适合 rollout 瓶颈 | VeRL 当前 UEnv 链路需要额外设计和验证 |

第一轮正式训练建议继续采用当前单个 8 卡 TP endpoint。等同步基线结果可信后，再讨论多副本或 rollout/update 资源切分。

#### 7.2.5 第一轮正式数学训练建议

第一轮正式数学训练建议从 GSM8K 小规模开始：

| 参数 | 建议 |
|---|---|
| 数据 | `data/gsm8k/train.parquet` 的 1000 到 5000 条子集 |
| 验证 | `data/gsm8k/test.parquet` 抽 100 到 500 条做训练内验证；OlymMATH 作为训练外 benchmark |
| response length | 以已验证的 `1024` 为起点；若 `clip_ratio` 仍高，再评估 `1536`、`2048` 或降低样本难度 |
| batch | 先保持 `TRAIN_BATCH_SIZE=4`、`ROLLOUT_N=2`，稳定后再放大 |
| 并行模式 | 第一轮使用同步模式；one-step off-policy 和 fully async 放到后续对照实验 |
| 训练方式 | 可先 LoRA 小跑验证数据和 reward；正式结果需要全参训练复验 |
| 评测 | 每个阶段固定跑 GSM8K test 子集、OlymMATH 全量或固定子集，以及五类 benchmark 的回归入口 |

如果 `DATA_MAX_RESPONSE_LENGTH` 提高后仍有较高 `clip_ratio`，但继续提高会 OOM，则优先考虑降低 `TRAIN_BATCH_SIZE`、降低 `ROLLOUT_N`、继续保持 micro batch 为 `1`、开启更多 offload，或者把训练拆成更短 prompt/response 的数据阶段。

#### 7.2.6 多任务混合建议

加入新任务时，推荐保留旧任务数据作为 replay。推荐采用“新任务数据 + 旧任务 replay + 少量通用 QA/指令数据”的混合方式。

一个可执行的起始配比是：

| 数据类型 | 建议比例 | 作用 |
|---|---:|---|
| 当前新任务 | 60% 到 70% | 让模型主要学习本阶段新增能力 |
| 旧任务 replay | 15% 到 25% | 抑制数学等已有能力遗忘 |
| 通用 QA/指令数据 | 10% 到 15% | 保持基础指令跟随和输出格式稳定 |

每一阶段训练后都需要跑五类 benchmark 回归评测，同时观察当前任务收益和旧任务表现。若新增任务带来旧任务明显下降，优先调整 replay 比例和学习率，再考虑是否拆成独立 LoRA/adapter 或分任务训练。

#### 7.2.7 通用训练命令模板

下面给出两套可直接复用的命令模板，分别对应同步训练和异步训练。两套命令都默认走 UEnv model gateway，worker 侧统一访问 `UENV_MODEL_GATEWAY_PUBLIC_URL`。未列出的参数继续沿用脚本默认值。

`ROLLOUT_CALCULATE_LOG_PROBS` 按并行模式设置：同步训练设为 `False`，由训练侧重算 log prob；异步训练设为 `True`，让 rollout 侧把 token log prob 带回结果队列。

同步训练模板：

```bash
SERVER_ADAPTER_CORE_ENDPOINT=8.130.75.157:8088 \
MODEL_PATH=/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B \
CONTAINER_MODEL_PATH=/models/modelscope/Qwen/Qwen3___6-35B-A3B \
UENV_MODEL_GATEWAY_ENABLED=1 \
UENV_MODEL_GATEWAY_PORT=18088 \
UENV_MODEL_GATEWAY_PUBLIC_URL=http://10.10.20.142:18088/v1 \
UENV_AGENT_LOOP_PARALLEL_MODE=sync \
TRAINING_STEPS=10 \
TRAIN_BATCH_SIZE=4 \
PPO_MINI_BATCH_SIZE=4 \
PPO_MICRO_BATCH_SIZE_PER_GPU=1 \
ROLLOUT_N=4 \
ROLLOUT_TP=4 \
ROLLOUT_CALCULATE_LOG_PROBS=False \
ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=1 \
REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=1 \
DATA_MAX_RESPONSE_LENGTH=1024 \
TEST_FREQ=-1 \
ROLLOUT_GPU_MEMORY_UTILIZATION=0.60 \
ROLLOUT_ENABLE_SLEEP_MODE=True \
ROLLOUT_FREE_CACHE_ENGINE=True \
PODMAN_GPU_ARGS="nvidia.com/gpu=0,1,2,3,4,5,6,7" \
CUDA_VISIBLE_DEVICES_IN_CONTAINER=0,1,2,3,4,5,6,7 \
NGPUS_PER_NODE=8 \
./scripts/train/launchers/common/run_verl_uenv_grpo.sh
```

异步训练模板（以 fully async 为例）：

```bash
SERVER_ADAPTER_CORE_ENDPOINT=8.130.75.157:8088 \
MODEL_PATH=/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B \
CONTAINER_MODEL_PATH=/models/modelscope/Qwen/Qwen3___6-35B-A3B \
UENV_MODEL_GATEWAY_ENABLED=1 \
UENV_MODEL_GATEWAY_PORT=18088 \
UENV_MODEL_GATEWAY_PUBLIC_URL=http://10.10.20.142:18088/v1 \
UENV_AGENT_LOOP_PARALLEL_MODE=fully_async \
TRAINING_STEPS=10 \
TRAIN_BATCH_SIZE=0 \
PPO_MINI_BATCH_SIZE=8 \
PPO_MICRO_BATCH_SIZE_PER_GPU=1 \
ROLLOUT_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=1 \
REF_LOG_PROB_MICRO_BATCH_SIZE_PER_GPU=1 \
DATA_MAX_RESPONSE_LENGTH=1024 \
TEST_FREQ=-1 \
ROLLOUT_N=2 \
ROLLOUT_TP=2 \
ROLLOUT_CALCULATE_LOG_PROBS=True \
ROLLOUT_CORRECTION_BYPASS_MODE=True \
FULLY_ASYNC_REQUIRE_BATCHES=1 \
FULLY_ASYNC_TRIGGER_PARAMETER_SYNC_STEP=1 \
FULLY_ASYNC_STALENESS_THRESHOLD=0.1 \
FULLY_ASYNC_USE_ROLLOUT_LOG_PROBS=True \
TRAINING_GPUS_PER_NODE=6 \
ROLLOUT_GPUS_PER_NODE=2 \
PODMAN_GPU_ARGS="nvidia.com/gpu=0,1,2,3,4,5,6,7" \
CUDA_VISIBLE_DEVICES_IN_CONTAINER=0,1,2,3,4,5,6,7 \
NGPUS_PER_NODE=8 \
AGENT_NUM_WORKERS=1 \
./scripts/experiments/fully_async_policy/run_verl_grpo_fully_async_uenv.sh
```

如果后续需要切换到 one-step off-policy，只需把入口脚本替换为 `./scripts/experiments/onestep_offpolicy/run_verl_grpo_onestep_offpolicy_uenv.sh`，并保持 `UENV_AGENT_LOOP_PARALLEL_MODE=one_step_off_policy` 即可。

### 7.3 Open Questions

下面这些问题还没有在当前实验中定论，需要后续结合训练目标、GPU 预算和阶段性评测结果继续讨论。

| 问题 | 为什么重要 | 倾向性判断 |
|---|---|---|
| 第一轮是否先 LoRA，再全参？ | LoRA 可以快速验证数据和 reward，但最终结果仍要全参复验。 | 若时间紧，先 LoRA；若目标是直接形成正式模型，先小规模全参。 |
| RL 前是否先做 SFT / 指令微调？ | SFT 可以先统一输出格式、答案风格和工具调用习惯，降低 RL 早期 reward 稀疏和格式错误的比例。 | 推荐把 SFT 作为可选前置阶段；当数据中有高质量解答、标准轨迹或格式示范时，先 SFT 再 RL。 |
| 数学 response length 最终设多少？ | 当前 `1024` 仍有 `clip_ratio=1.0`，但更长会显著拖慢 rollout 并增加显存压力。 | 推荐先试 `1536`、`2048`，再根据 clip ratio 和耗时决定是否继续放大。 |
| 第一阶段是否只训 GSM8K？ | 只训 GSM8K 稳定，但覆盖的数学难度有限。 | 先 GSM8K 小跑，稳定后加入 MATH/NuminaMath/OpenMathReasoning 子集。 |
| 多任务是增量训练还是混合训练？ | 纯增量容易遗忘，直接五任务混合又难定位问题。 | 倾向阶段式混合：新任务 + 旧任务 replay + 通用 QA。 |
| 什么时候引入 one-step off-policy / fully async？ | 异步可以提升吞吐，但会引入版本和 old logprob 管理复杂度。 | 同步基线稳定且 rollout 确认为瓶颈后再引入。 |
| 是否需要多个 rollout 模型副本？ | 多副本可能提升吞吐，但要求多个副本权重同步一致。 | 当前阶段先评估单 endpoint 同步基线，稳定后再评估多副本。 |
| QA 类任务是否值得优先用 RL？ | PubMedQA/SciTab 更依赖数据和判分，UEnv 环境交互价值不如 SWE/代码明显。 | QA 可用于验证泛化和多任务，但复杂环境价值主要看代码/SWE。 |
| 五类 benchmark 回归频率如何设置？ | 回归频率需要在评测成本和遗忘发现能力之间平衡。 | 小跑阶段固定子集回归，阶段结束跑全量。 |
| SWE 类任务何时进入训练？ | SWE reward 成本高，且 worker/openhands/repo 映射会影响结果。 | 推荐等数学/QA 训练闭环稳定后再接入。 |

## 8. 注意事项

- 当前 `ROLLOUT_TP=8` 表示一个 8 卡 TP vLLM endpoint，不是 8 个单卡 rollout replica。Worker 看到的是 gateway URL，实际由 gateway 转发到 VeRL 内部 vLLM endpoint。
- 当前验证使用短 response 和小 batch，主要用于说明训练链路稳定；最终任务训练参数需要结合正式训练数据继续确定。
- `flash-attn` 与 `DeepEP` 的 ABI 问题已经在镜像层修复；但本配置没有强制验证 DeepEP all-to-all kernel 的最优路径，日志中仍可能出现 vLLM MoE 默认配置性能提示。
- 后续正式训练如果开启长思考或长答案，需要重新评估 gateway/Server/Worker 的超时和单条 episode 执行时间。
