#!/bin/bash
ALOG=/home/uenv/uenv-server/stress_test/logs/adapter_core.log

while true; do
    TS=$(date '+%H:%M:%S')
    WORKERS=$(pgrep -fc uenv-worker 2>/dev/null || echo 0)
    PLUGINS=$(pgrep -fc uenv-math-plugin 2>/dev/null || echo 0)
    RETRIES=$(grep -c episode_attempt_failed_retrying "$ALOG" 2>/dev/null || echo 0)
    LLM_CONN=$(ss -tn 2>/dev/null | grep -c ':18080' || echo 0)
    ALIVE=$(pgrep -fc stress_test_real 2>/dev/null || echo 0)
    echo "[$TS] workers=$WORKERS plugins=$PLUGINS llm_conn=$LLM_CONN retries=$RETRIES alive=$ALIVE"
    sleep 120
done
