# CodeEnv / DSCodeBench Worker 实机联调记录

> 日期：2026-07-09  
> Worker：A100 **7143**（`219.147.100.43:7143`）  
> Server：`8.130.75.157:8088`（uenv-adapter-core）

## 部署内容

- 新建 `plugins/code/`（CodeEnv + dscodebench backend）
- 新增 `uenv-code-plugin` 二进制
- 扩展 `build_reset_config` payload 透传
- Worker 配置 `env.types` 增加 `code`
- 联调脚本：`scripts/e2e-code-env-7143.sh`、`scripts/smoke_code_submit.py`

## 7143 验证结果

| 项 | 结果 |
|----|------|
| `cargo build -p uenv-worker --release` | ✅ |
| `cargo test -p uenv-code-env`（5 tests） | ✅ |
| `m4_code_plugin_reset_step_close` | ✅ |
| `m5_single_round_code_dscodebench_smoke` | ✅ |
| Worker 注册 `loaded_envs=code,math` | ✅ |
| code 预热池 `warmup_hit=true` | ✅ |
| `ExecuteBatch` 全链路（经 Server） | ⚠️ 见下文 |

## Worker 日志摘录（code dispatch）

```
plugin_host_loaded loaded_envs=code,math
dispatch_received episode_id=code-smoke-*
acquire warmup_hit=true instance_id=code-*
```

首次 `ExecuteBatch` 失败原因：Bridge 未透传 `response_text`，且 Worker 进程未加载 `uenv-worker-llm.env`（ModelClient 要求 LLM 或 payload 预填答案）。

**Worker 侧已支持**：`payload.response_text` 短路（与 GSM8K 相同）。  
**已补 Bridge 透传**（`uenv-bridge/core/src/core.rs`，code env 字段 + `response_text`）。

## 7143 持久化环境变量（重启后需保留）

```bash
export UENV_CODE_PLUGIN_BIN=/root/UEnv/target/release/uenv-code-plugin
export UENV_CODE_EVAL_SCRIPT=/root/UEnv/plugins/code/scripts/evaluate_code.py
export UENV_MATH_PLUGIN_BIN=/root/UEnv/target/release/uenv-math-plugin
export UENV_PLUGIN_DIR=/root/UEnv/plugins
```

建议写入 `/root/.uenv-worker.env`（权限 600）。

## 复现命令

```bash
# 本地
UENV_SSH_KEY=secrets/9aa460dab6678381f86a1022b8a54c9f_32e42d1c7902ce68ba6719d551645e02_8.143 \
  bash scripts/connect-remote.sh sync
UENV_SSH_KEY=... bash scripts/connect-remote.sh build

# 7143 插件 + Executor 测试
ssh -i secrets/... -p 7143 root@219.147.100.43 \
  'source ~/.cargo/env; cd /root/UEnv; \
   export UENV_CODE_PLUGIN_BIN=/root/UEnv/target/release/uenv-code-plugin; \
   cargo test -p uenv-worker --test m4_code_plugin_host --test m5_code_episode_executor'
```

## Server 侧待办（全链路 ExecuteBatch）

在 **`8.130.75.157`** 重新编译并重启 `uenv-adapter-core`（含 Bridge code 字段透传）后：

```bash
python3 scripts/smoke_code_submit.py 8.130.75.157:8088
# 期望：status=completed reward=1.0
```

## DSCodeBench 官方数据（后续）

设置环境变量后可用 `test_script_path` 模式：

```bash
export UENV_DSCODEBENCH_ROOT=/path/to/DSCodeBench/benchmark
export UENV_DSCODEBENCH_EVAL_ROOT=/path/to/DSCodeBench  # 可选，官方 evaluate.py
```
