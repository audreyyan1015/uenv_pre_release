# 轨迹采集指南

轨迹采集只有一种方式：**UEnv Server 的集中轨迹存储**。任何任务类型——`run-task`（问答、数学、代码、自定义环境）、`run-swe`（代码修复）、强化学习训练 rollout——在 episode 到达终态（包括失败与超时）时，UEnv Worker 都会自动把完整轨迹封存（seal）为 TrajectoryBundle 并上传到 UEnv Server；上传失败进入本地 spool 重试，不阻断 reward。

结果 JSONL 里内联的 `steps` 和 SWE 实例目录的 `trajectory_ref.json` 只是结果附带的指针与摘要，便于快速核对，**不是**独立的采集方式；轨迹的权威来源始终是集中存储。

## 默认行为：零配置

单机安装时集中存储自动闭环，无需任何手工配置：

- `install.sh` 自动生成轨迹 token 写入 `/etc/uenv/secrets/swe.env`（已存在则复用，不覆盖），并配置 Worker 上传端点；
- Server 单元自动加载同一 token 并启用轨迹存储（`:8077`）；
- 自定义地址时用 `install.sh --trajectory-endpoint` / `--trajectory-server`，或 `uenv evaluate prepare-swe --trajectory-endpoint`。

多机部署需要一次性的 token 分发，见[多机部署](#多机部署)。

## 查询轨迹

统一使用 `uenv trajectory` 子命令，不需要手工 curl、jq 或管理 token 文件。

```bash
uenv trajectory health                          # Server 健康与数据目录
uenv trajectory list --run-id <RUN_ID>          # 按 run 列出轨迹（还有 --batch-id/--instance-id/--worker-id/--episode-id）
uenv trajectory get <TRAJECTORY_ID>             # 摘要 + 每步概览
uenv trajectory get <TRAJECTORY_ID> -o t.json   # 完整 bundle 落盘（0600）
uenv trajectory verify <TRAJECTORY_ID>          # 完整性校验：step 索引连续、reward 为数值
uenv trajectory export --run-id <RUN_ID> -o out/  # 批量导出一个 run 的全部轨迹
uenv trajectory status                          # Server 健康 + 本机 spool 的 pending/failed 概况
```

endpoint 与 token 按以下顺序解析：命令行 `--url/--token` > 环境变量 `UENV_TRAJECTORY_URL`/`UENV_TRAJECTORY_TOKEN` > `~/.config/uenv/trajectory.toml` > 本机 `/etc/uenv/` 配置 > 缺省 `http://127.0.0.1:8077`。在部署机上直接使用即零配置；从其他主机查询时先执行一次：

```bash
uenv trajectory login --url http://<SERVER>:8077   # token 从 stdin 读取，不进入 shell history
```

凭据保存为 `~/.config/uenv/trajectory.toml`（0600）。

## 结果中的轨迹指针

结果用于关联，不用于采集：

- `run-task` 结果 JSONL 每行含 `trajectory_id`（并内联 `steps` 供快速查看）；拿完整 bundle 用 `uenv trajectory get`。
- `run-swe` 每个 completed 实例的 `artifact_dir` 下有 `trajectory_ref.json`，含 `trajectory_id`、`run_id`、`upload_status` 等；`upload_status=pending` 只表示集中上传尚未确认，不影响本地 reward。

核对一次运行的完整性，用 CLI 而不是手写 jq：

```bash
uenv trajectory list --run-id <RUN_ID> --json | jq -r '.[].trajectory_id' | \
  while read -r id; do uenv trajectory verify "$id"; done
```

每个 step 的 `step_index` 从 0 开始连续编号，含 `observation`、`action`、`reward`、终止标记、`info` 与耗时。若 episode 状态为 `failed` 或 `timeout`，轨迹同样已封存，先查 `error_code`、`error_message` 与 `terminate_reason`，再对照 reward。

## 关联 ID

一次运行中的 ID 关系：

```text
run_id -> batch_id/correlation_id -> episode_id -> trajectory_id
```

| ID | 用途 |
|---|---|
| `run_id` | 一次评测或训练作业；非 SWE 任务按 `run_id` → `training_run_id` → `batch_id` → `run-{episode_id}` 的优先级确定 |
| `batch_id` / `correlation_id` | 接入批次和框架侧关联键 |
| `episode_id` | UEnv Server 中的一次环境执行 |
| `trajectory_id` | 一份完整轨迹的全局唯一 ID，格式 `trj-{worker_id}-{unix_ms}-{seq5}` |

不要用文件名、数组下标或完成顺序代替这些 ID。SWE 本地产物目录名中的外层 `RUN_ID` 也不一定等于 bundle 内的 `run_id`，集中查询以 `trajectory_ref.json` 为准。同步失败或 tokio 超时的 episode 没有结果上报，但轨迹已封存，可按 `episode_id`/`run_id` 在集中存储中找到。

## 上传状态与本地目录

UEnv Worker 的默认产物目录（根目录由 `UENV_TRAJECTORY_ARTIFACT_DIR` 指定，未设置时回退 `UENV_SWE_ARTIFACT_DIR`，缺省 `/var/lib/uenv/worker/swe-artifacts`）：

```text
/var/lib/uenv/worker/swe-artifacts/
  bodies/<trajectory_id>.json
  index/by-id/<trajectory_id>.json
  spool/pending/<trajectory_id>.json
  spool/failed/<trajectory_id>.json
```

| 状态 | 含义 | 处理 |
|---|---|---|
| `pending` | 已封存，等待上传或重试 | 检查 8077/TCP、token 与 Server 健康状态（`uenv trajectory health`） |
| `acked` | Server 已持久化正文与索引 | 可用 `uenv trajectory list/get` 查询 |
| `failed` | 达到重试上限 | 保留正文，修复原因后按运维流程重新入队 |

日常检查用 `uenv trajectory status`，不必直接翻目录。上传成功后 Worker 可能删除本地正文；长期取证依赖 Server 存储及其备份。

## 多机部署

Worker 与 Server 不同机时，需要一次性的运维配置（单机安装已自动完成，无需重复）：

| 主机 | 非密钥配置 | 共享 token |
|---|---|---|
| UEnv Server | `/etc/uenv/server.env` | `/etc/uenv/secrets/swe.env` |
| UEnv Worker | `/etc/uenv/swe.env` | `/etc/uenv/secrets/swe.env` |

1. 在受信管理主机生成一次 token（`python3 -c 'import secrets; print(secrets.token_urlsafe(32))'`），通过受保护的运维通道分发到 Server 与所有 Worker，追加到各机 `/etc/uenv/secrets/swe.env` 的 `UENV_TRAJECTORY_TOKEN=`（不得覆盖该文件已有的 Gateway key）。
2. Server 侧 `/etc/uenv/server.env` 设置 `UENV_TRAJECTORY_ENABLED=1`、`UENV_TRAJECTORY_HTTP_LISTEN=<受控内网地址>:8077`、`UENV_TRAJECTORY_DATA_DIR=/var/lib/uenv/server/trajectory`、`UENV_TRAJECTORY_RETENTION_DAYS=30`。不要直接绑定公网地址；必须跨不可信网络时使用 TLS 反向代理并限制来源。`RETENTION_DAYS=0` 表示不自动删除，生产环境应根据容量、数据许可和合规要求设置期限。
3. Worker 侧 `/etc/uenv/swe.env` 设置 `UENV_TRAJECTORY_ENDPOINT=http://<SERVER>:8077`（新装主机直接用 `install.sh --trajectory-endpoint` 生成）。
4. 重启 `uenv-adapter-core.service` 与 `uenv-worker.service`，用 `uenv trajectory health` 确认。

Worker 到 Server 的 `8077/TCP` 只在受控内网放行。轮换 token 时协调重启 Server 与所有 Worker，避免新旧 token 混用；查询端用 `uenv trajectory login` 更新即可。

## 导出与脱敏

```bash
uenv trajectory export --run-id <RUN_ID> -o export/
```

导出至少保留 `trajectory_id`、`run_id`、`instance_id`、环境版本、模型版本、reward 与 `steps`。向外发送前检查 prompt、命令输出、patch、环境变量和日志是否包含私有源码或 token，并确认数据集许可、retention 与删除要求同时覆盖正文、索引和备份。

不要直接把整个 Worker artifact 目录作为公开训练数据发布。

## 完成标准与排障

一次轨迹采集完成应满足：

1. 每个 episode（含失败与超时）都有 `trajectory_id`，且 `uenv trajectory list --run-id` 能列出。
2. `uenv trajectory verify` 通过：step 索引从 0 连续，reward 为数值。
3. SWE bundle 的 run、instance、worker 和 artifact 可关联。

| 现象 | 原因与处理 |
|---|---|
| `list` 查不到某条轨迹 | 上传仍在重试或已失败：`uenv trajectory status` 查 spool；核对 run_id/episode_id |
| 本地有正文、集中查询为空 | 网络/token 错误或达到重试上限；查 spool 与 Worker journal |
| 401/403 | 查询端、Server 与 Worker 的 token 不一致；查询端用 `uenv trajectory login` 修正 |
| 同 ID 上传冲突（409） | 生产者复用了 ID 且正文不同；停止重试并排查 ID 生成 |
| reward 与 bundle 不一致 | 用 episode/trajectory ID 核对是否串批，并检查封存时间 |
| 配置改了但服务未生效 | 变量要写入 systemd 读取的 `/etc/uenv/*.env` 并重启服务，交互 shell 里的 `export` 不影响服务 |

字段、目录、HTTP 校验和兼容规则以仓库的 `Docs/trajectory/frozen-spec-v2.3.md` 为准（v2.3 起集中存储覆盖全部任务类型，并新增通用 step 类型）。软件工程实例的产物布局见[代码修复评测](./06-evaluation-swe-verified.md)。
