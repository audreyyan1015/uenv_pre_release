import sys
sys.path.insert(0, "/home/uenv/uenv-server/stress_test")
import stress_test_real as m

m.N_WORKERS            = 64
m.WORKER_CAPACITY      = 8
m.BATCH_SIZE           = 128
m.N_CONCURRENT_BATCHES = 64
m.TEST_DURATION        = 1800
m.LLM_LATENCY_MEAN_MS  = 30_000
m.LLM_LATENCY_STD_MS   = 10_000
m.LLM_LATENCY_MIN_MS   =  5_000
m.LLM_LATENCY_MAX_MS   = 120_000
m.LLM_CORRECT_RATE     = 0.70

import os
os.environ["UENV_WORKER_EPISODE_TIMEOUT_SECS"] = "200"

import asyncio
asyncio.run(m.main())
