# 轨迹采集指南

轨迹在哪里，取决于你跑了什么任务：

| 你跑了什么 | 轨迹在哪里 | 需要做什么 |
|---|---|---|
| `run-task`（问答、代码、自定义环境） | 结果 JSONL 里内联 `steps` | 直接用 `jq` 读，见[读取结果中的轨迹](#读取结果中的轨迹) |
| `run-swe`（代码修复） | 每个实例目录的 `trajectory_ref.json` | 同上 |
| 任何任务，且需要跨主机查询、长期保留或集中审计 | UEnv Server 的集中轨迹存储 | 额外配置，见[按需启用集中存储](#按需启用集中存储) |

默认不需要集中存储；它是一项可选的运维能力，不是使用 UEnv 的前置。

## 读取结果中的轨迹

### run-task 结果

结果文件的每一行已经内联 `steps`，无需任何额外配置：

```bash
export RESULT_JSONL="$PWD/results/evaluation-YYYYMMDD-HHMMSS/results.jsonl"

test -r "$RESULT_JSONL"
jq -c '{case_id,status,reward,trajectory_id,steps,error_code,error_message}' "$RESULT_JSONL"
```

每个 step 包含 `step_index`（从 1 开始编号）、`observation`、`action`、`reward`、终止标记、`info` 和耗时。若 `status` 为 `failed` 或 `timeout`，优先检查 `error_code`、`error_message` 与 `terminate_reason`，再对照 reward。

核对轨迹完整性（step 索引连续、reward 为数值）：

```bash
jq -e -s '
  all(.[];
    .status == "completed" and
    (.steps | type) == "array" and
    ([.steps[].step_index] == [range(1; (.steps | length) + 1)]) and
    ((.reward | type) == "number")
  )
' "$RESULT_JSONL" >/dev/null && echo 'inline trajectories valid'
```

### run-swe 结果

SWE 轨迹的稳定入口是每个实例的 `artifact_dir` 下的 `trajectory_ref.json`：

```bash
export SWE_RESULT_JSONL="$PWD/results/swe-evaluation-YYYYMMDD-HHMMSS/results.jsonl"

test -r "$SWE_RESULT_JSONL"
while IFS= read -r artifact_dir; do
  sudo jq -c '{run_id,instance_id,trajectory_id,upload_status}' \
    "$artifact_dir/trajectory_ref.json"
done < <(jq -r 'select(.status == "completed") | .artifact_dir' "$SWE_RESULT_JSONL")
```

结果行是否额外包含顶层 `trajectory_id` 取决于发行版，自动化脚本应使用实例目录中的稳定文件。completed 实例应有非空 `trajectory_id`，且实例目录中能找到 submit、patch、测试或相应日志；`upload_status=pending` 只表示集中上传尚未确认，不影响本地 reward。

## 关联 ID

一次运行中的 ID 关系：

```text
run_id -> batch_id/correlation_id -> episode_id -> trajectory_id
```

| ID | 用途 |
|---|---|
| `run_id` | 一次评测或训练作业 |
| `batch_id` / `correlation_id` | 接入批次和框架侧关联键 |
| `episode_id` | UEnv Server 中的一次环境执行 |
| `trajectory_id` | 一份完整轨迹的全局唯一 ID |

不要用文件名、数组下标或完成顺序代替这些 ID。SWE 本地产物目录名中的外层 `RUN_ID` 也不一定等于 bundle 内的 `run_id`，集中查询以 `trajectory_ref.json` 为准。

## 按需启用集中存储

集中存储当前主要用于 SWE 的完整 `TrajectoryBundle`，分三步：生成并分发 token、配置 UEnv Server、配置 UEnv Worker。配置写入 systemd 读取的文件，不在交互 shell 中 `export`：

| 主机 | 非密钥配置 | 共享 token |
|---|---|---|
| UEnv Server | `/etc/uenv/server.env` | `/etc/uenv/secrets/swe.env` |
| UEnv Worker | `/etc/uenv/swe.env` | `/etc/uenv/secrets/swe.env` |

### 第一步：生成并分发 token

在受信管理主机生成一次，不把值写进文档、仓库或命令参数：

```bash
umask 077
python3 -c 'import secrets; print(secrets.token_urlsafe(32))' > "$PWD/uenv-trajectory.token"
chmod 600 "$PWD/uenv-trajectory.token"
```

通过受保护的运维通道把同一个值分发到 UEnv Server 和所有相关 UEnv Worker。每台主机把 token 追加到 `/etc/uenv/secrets/swe.env`（文件可能已存有 Gateway key，不得覆盖原内容）：

```bash
sudo test -e /etc/uenv/secrets/swe.env || \
  sudo install -o root -g uenv -m 0640 /dev/null /etc/uenv/secrets/swe.env
sudoedit /etc/uenv/secrets/swe.env   # 新增或更新一行 UENV_TRAJECTORY_TOKEN=<同一个值>
sudo chown root:uenv /etc/uenv/secrets/swe.env
sudo chmod 0640 /etc/uenv/secrets/swe.env
```

需要执行集中查询的受信用户把同一个 token 保存到自己的只读配置：

```bash
install -d -m 0700 "$HOME/.config/uenv"
install -m 0600 "$PWD/uenv-trajectory.token" "$HOME/.config/uenv/trajectory.token"
```

轮换 token 时协调重启 UEnv Server 与所有 UEnv Worker，避免新旧 token 混用。

### 第二步：配置 UEnv Server

备份后编辑 `/etc/uenv/server.env`：

```bash
sudo cp -a /etc/uenv/server.env "/etc/uenv/server.env.backup.$(date +%Y%m%d-%H%M%S)"
sudoedit /etc/uenv/server.env
```

写入或更新（把 `10.0.0.10` 换成 UEnv Server 的受控内网接口）：

```ini
UENV_TRAJECTORY_ENABLED=1
UENV_TRAJECTORY_HTTP_LISTEN=10.0.0.10:8077
UENV_TRAJECTORY_DATA_DIR=/var/lib/uenv/server/trajectory
UENV_TRAJECTORY_RETENTION_DAYS=30
```

不要直接绑定公网地址；必须跨不可信网络时，在前面使用 TLS 反向代理并限制来源。`RETENTION_DAYS=0` 表示不自动删除，生产环境应根据容量、数据许可和合规要求设置期限。

校验并重启：

```bash
sudo -u uenv env UENV_CONFIG_PATH=/etc/uenv/server.yaml \
  /opt/uenv/current/bin/uenv-adapter-core --validate-config
sudo systemctl restart uenv-adapter-core.service
sudo systemctl is-active uenv-adapter-core.service
curl -fsS 'http://10.0.0.10:8077/control/v1/trajectories/health' | jq .
```

健康响应应包含 `"db":"ok"` 和实际数据目录。

### 第三步：配置 UEnv Worker 上传

备份后编辑 `/etc/uenv/swe.env`：

```bash
sudo cp -a /etc/uenv/swe.env "/etc/uenv/swe.env.backup.$(date +%Y%m%d-%H%M%S)"
sudoedit /etc/uenv/swe.env
```

写入或更新实际 UEnv Server 地址和本地目录：

```ini
UENV_TRAJECTORY_ENDPOINT=http://10.0.0.10:8077
UENV_SWE_ARTIFACT_DIR=/var/lib/uenv/worker/swe-artifacts
```

重启并确认 Worker 恢复 `ready`：

```bash
sudo systemctl restart uenv-worker.service
sudo systemctl is-active uenv-worker.service
uenv status
```

UEnv Worker 到 UEnv Server 的 `8077/TCP` 只在受控内网放行。新部署优先用 `uenv evaluate prepare-swe --trajectory-endpoint` 或安装参数生成同一配置；手工编辑适用于已安装主机的明确变更。

## 查询集中轨迹

SWE submit 后，UEnv Worker 先在本地封存完整 bundle（正文与索引），`TrajectoryRef` 立即随结果返回，后台 uploader 再上传到 UEnv Server；上传失败进入 spool 重试，不阻断 reward。

在受信主机设置实际 URL、run ID 和 token（从 `0600` 文件读取，避免 token 进入命令历史）：

```bash
export UENV_TRAJECTORY_URL='http://10.0.0.10:8077'
export ACTUAL_RUN_ID='run-id-from-trajectory-ref'
read -r UENV_TRAJECTORY_TOKEN < "$HOME/.config/uenv/trajectory.token"
export UENV_TRAJECTORY_TOKEN
```

按 `run_id` 列出索引：

```bash
curl --fail --silent --show-error \
  -H "X-Trajectory-Token: $UENV_TRAJECTORY_TOKEN" \
  "$UENV_TRAJECTORY_URL/control/v1/trajectories?run_id=$ACTUAL_RUN_ID" \
  | jq .
```

选择实际 ID 取得正文：

```bash
export TRAJECTORY_ID='trajectory-id-from-list'

curl --fail --silent --show-error \
  -H "X-Trajectory-Token: $UENV_TRAJECTORY_TOKEN" \
  "$UENV_TRAJECTORY_URL/control/v1/trajectories/$TRAJECTORY_ID" \
  > "$PWD/$TRAJECTORY_ID.json"

jq '{trajectory_id,run_id,instance_id,worker_id,step_count:(.steps|length),reward}' \
  "$PWD/$TRAJECTORY_ID.json"
```

集中接口只列出正文存在且上传已确认的轨迹。查不到时依次核对 `run_id`、`trajectory_id`、UEnv Worker 本地 index、`spool/pending` 和 `spool/failed`。

## 上传状态与本地目录

UEnv Worker 的默认产物目录：

```text
/var/lib/uenv/worker/swe-artifacts/
  bodies/<trajectory_id>.json
  index/by-id/<trajectory_id>.json
  spool/pending/<trajectory_id>.json
  spool/failed/<trajectory_id>.json
```

| 状态 | 含义 | 处理 |
|---|---|---|
| `pending` | 已封存，等待上传或重试 | 检查 8077/TCP、token 与 UEnv Server 健康状态 |
| `acked` | UEnv Server 已持久化正文与索引 | 可用集中 LIST/GET 查询 |
| `failed` | 达到重试上限 | 保留正文，修复原因后按运维流程重新入队 |

上传成功后 UEnv Worker 可能删除本地正文；长期取证依赖 UEnv Server 存储及其备份。

## 导出与脱敏

导出至少保留 `trajectory_id`、`run_id`、`instance_id`、环境版本、模型版本、reward 与 `steps`。向外发送前检查 prompt、命令输出、patch、环境变量和日志是否包含私有源码或 token，并确认数据集许可、retention 与删除要求同时覆盖正文、索引和备份。

不要直接把整个 UEnv Worker artifact 目录作为公开训练数据发布。

## 完成标准与排障

一次轨迹采集完成应满足：

1. 结果含可关联的内联 steps 或 `trajectory_id`。
2. step 索引连续，最终 reward 与结果一致。
3. SWE bundle 的 run、instance、worker 和 artifact 可关联。
4. 启用集中上传时，LIST 能找到索引，GET 能读取正文。

| 现象 | 原因与处理 |
|---|---|
| 当前 shell 有变量但服务未启用 | 变量没有写入 systemd EnvironmentFile；检查 `/etc/uenv/*.env` 并重启 |
| 本地有正文、集中查询为空 | 上传未启用、网络/token 错误或仍在重试；查 spool 和 UEnv Worker journal |
| HTTP 401/403 | 查询端、UEnv Server 与 UEnv Worker token 不一致 |
| 同 ID 上传冲突 | 生产者复用了 ID 且正文不同；停止重试并排查 ID 生成 |
| reward 与 bundle 不一致 | 用 episode/trajectory ID 核对是否串批，并检查封存时间 |

字段、目录、HTTP 校验和兼容规则以仓库的 `Docs/trajectory/frozen-spec-v2.2.md` 为准。软件工程实例的产物布局见[代码修复评测](./06-evaluation-swe-verified.md)。
