# 获取轨迹

UEnv 提供轨迹采集功能。使用标准单机安装时，UEnv 会自动记录 Episode 中的每一步，并在 Episode 结束时将完整轨迹保存到 UEnv Server，无需额外配置。

## 查询和下载

`run-task` 的结果 JSONL 中包含 `trajectory_id`；`run-swe` 可以从实例产物中的 `trajectory_ref.json` 读取该 ID。获得 `trajectory_id` 后，直接将这条轨迹下载为 JSON 文件：

```bash
uenv trajectory get <TRAJECTORY_ID> -o trajectory.json
```

如果只知道一次运行的 `run_id`，先列出这次运行的轨迹，再选择所需的 `trajectory_id`：

```bash
uenv trajectory list --run-id <RUN_ID>
```

也可以按 `run_id` 批量导出：

```bash
uenv trajectory export --run-id <RUN_ID> -o trajectories/
```

`export` 会在目标目录中为每个 Episode 写入一个 `<trajectory_id>.json` 文件。若命令运行在 UEnv Server 之外的主机上，先执行 `uenv trajectory login --url http://<UENV_SERVER>:8077`，并按提示输入访问 token。

## 轨迹格式

`get -o` 与 `export` 写出的文件都是 UTF-8 编码的 `TrajectoryBundle` JSON。每个文件保存一个 Episode，基本结构如下：

```json
{
  "trajectory_id": "trj-worker-1-1783244550494-00001",
  "run_id": "run-20260820-001",
  "episode_id": "episode-1",
  "session_id": "session-1",
  "instance_id": "sample-1",
  "benchmark_variant": "qa",
  "worker_id": "worker-1",
  "gateway_base_url": "",
  "steps": [
    {
      "step_index": 1,
      "action": {"kind": "generic", "action": "#### 6"},
      "observation": {
        "raw": "The answer is 6.",
        "reward": 1.0,
        "terminated": true,
        "truncated": false
      },
      "timestamp_ms": 1783244550000,
      "duration_ms": 120
    }
  ],
  "artifact": {
    "episode_id": "episode-1",
    "instance_id": "sample-1",
    "stdout_log": [],
    "stderr_log": [],
    "reward": 1.0
  },
  "reward": 1.0,
  "resolved": true,
  "sealed_at_ms": 1783244550494
}
```

`trajectory_id` 用于查询单条轨迹，`run_id` 用于关联同一次运行；`steps` 按执行顺序保存每一步的 action、observation 和耗时；顶层 `reward` 与 `resolved` 表示整个 Episode 的结果，`artifact` 保存环境产生的补充内容。

不同环境会在 `action`、`observation` 和 `artifact` 中写入不同字段。例如，代码修复任务可以包含命令、文件修改、patch 和测试结果；模型能够提供 token 信息时，单步记录还可能包含可选的 `rollout_trace`。完整示例和字段定义分别位于源码中的 `Docs/trajectory/trajectory-bundle.example.json` 与 `Docs/trajectory/frozen-spec-v2.3.md`。
