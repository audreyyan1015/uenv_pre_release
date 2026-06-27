# stress_test_async.py
# 异步训练推理解耦框架的压测：模拟多个 rollout worker 独立并发提交 batch
#
# 真实场景（千卡异步）：
#   8 个 rollout worker group，每组 128 GPU（TP=8, 16 节点）
#   每个 worker 独立循环：submit batch → 等结果 → 立刻 submit 下一个
#   adapter 端：同时 8 个 ExecuteBatch in-flight
#   每 batch 64 episodes → 总并发 8×64=512 = 64 workers × 8 cap
#
# 关键特征（vs 同步框架）：
#   - batch 完成时间参差不齐（不同 worker 速度不同）
#   - adapter 始终有 8 个 batch 在飞，permit 持续被占用
#   - 每个 rollout worker 看到的 step_time = 自己 batch 的完成时间
#     不受其他 worker 拖累（异步解耦的核心优势）

import sys
sys.path.insert(0, "/home/uenv/uenv-server/stress_test")
import stress_test_real as m

# ── 规模：千卡异步，8 rollout worker group ───────────────────
m.N_WORKERS            = 64     # 64 个推理 worker（8 GPU 每个）
m.WORKER_CAPACITY      = 8      # 64 x 8 = 512 并发 slot
m.N_CONCURRENT_BATCHES = 8      # 8 个 rollout worker group 同时 in-flight
m.BATCH_SIZE           = 64     # 每 worker 每次 64 episodes，8x64=512 填满 slot
m.TEST_DURATION        = 1800   # 30 分钟

# ── LLM 延迟：30s 均值，模拟快速推理场景 ──────────────────────
m.LLM_LATENCY_MEAN_MS  = 30_000
m.LLM_LATENCY_STD_MS   = 10_000
m.LLM_LATENCY_MIN_MS   =  5_000
m.LLM_LATENCY_MAX_MS   = 120_000
m.LLM_CORRECT_RATE     = 0.70

import os
os.environ["UENV_WORKER_EPISODE_TIMEOUT_SECS"] = "200"

import asyncio
asyncio.run(m.main())
