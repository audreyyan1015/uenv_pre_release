# stress_test_train.py
# 训练模式压测：模拟真实 RL step 的调用模式
#
# 关键参数：
#   N_CONCURRENT_BATCHES = 1   训练是同步的，等全部结果才发下一个 batch
#   BATCH_SIZE = 1024          千卡 RL 训练的典型 batch size
#   64 workers x 8 cap = 512 slots，1024 episodes = 2 轮满载
#
# 关键指标：
#   step_time = 一个 ExecuteBatch(1024) 的完成时间
#   tail latency = 最慢 episode 决定 step 时长（木桶效应）
#   512 slots 时 step_time ≈ ceil(1024/512) x mean_latency = 2 x 30s ≈ 60s/step

import sys
sys.path.insert(0, "/home/uenv/uenv-server/stress_test")
import stress_test_real as m

m.N_WORKERS            = 64
m.WORKER_CAPACITY      = 8       # 64 x 8 = 512 并发 slot
m.BATCH_SIZE           = 1024    # 一个训练 step 的 episode 数
m.N_CONCURRENT_BATCHES = 1       # 同步：等上一个 batch 全部完成再发下一个
m.TEST_DURATION        = 1800    # 30 分钟，约 30 个 step
m.LLM_LATENCY_MEAN_MS  = 30_000
m.LLM_LATENCY_STD_MS   = 10_000
m.LLM_LATENCY_MIN_MS   =  5_000
m.LLM_LATENCY_MAX_MS   = 120_000
m.LLM_CORRECT_RATE     = 0.70

import os
os.environ["UENV_WORKER_EPISODE_TIMEOUT_SECS"] = "200"

import asyncio
asyncio.run(m.main())
