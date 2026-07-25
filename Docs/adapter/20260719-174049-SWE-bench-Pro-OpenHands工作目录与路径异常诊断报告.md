# SWE-bench-Pro OpenHands 工作目录与路径异常诊断报告

> 诊断时间：2026-07-19  
> 诊断范围：Adapter 请求、Server/Worker 链路、SWE-bench-Pro catalog、OpenHands AgentJob、运行轨迹与最终评测结果

## 1. 结论与建议

### 1.1 结论

本次路径异常的**主要原因是模型在正确仓库中探索或选取了错误文件，而不是 Worker 将错误项目映射到了 OpenHands**。

实机证据表明：

1. Worker 根据 `instance_id` 启动了对应的 SWE-bench-Pro 镜像，qutebrowser 等代表实例的镜像、仓库和 instance 映射正确。
2. SWE-bench-Pro Pro 镜像中的真实仓库根目录为 `/app`。
3. OpenHands official driver 实际使用的 `working_dir` 是 `/app`，instruction 也明确要求只修改 `/app`。
4. 多个失败样例的最终 `git_diff` 为空，或模型在 `/app` 下修改了与目标问题无关的文件。例如 flipt 样例修改了 `/app/utils.py`，而 gold patch 位于 `internal/server/...`。
5. qutebrowser 代表样例在正确工作目录下反复搜索无关的 Agent 本地目录 `/opt/openhands/benchmarks`，最终触发 stuck detector，未形成源码 patch，测试保持在 52/56。
6. 同一 qutebrowser 镜像应用 gold patch 时历史验收可达到 56/56，说明镜像、Gateway、patch 应用和 grader 链路本身可用。

同时发现一项需要修正的配置问题：

- Adapter 评测脚本默认发送 `workspace_dir=/workspace`，但 SWE-bench-Pro Pro 的真实仓库目录是 `/app`。
- 当前 `run_swebenchpro_official.py` 会根据 `benchmark_variant=pro` 将实际工作目录覆盖为 `/app`，所以该不一致**不是本轮失败的直接主因**；但它会污染 AgentJob 元数据，并可能影响其他直接采信 `workspace_dir` 的 driver。

### 1.2 建议

按以下优先级处理：

1. **P0：统一 Pro workspace 配置**
   - 将 `evaluate_swebenchpro_uenv.py` 的 `--workspace-dir` 默认值由 `/workspace` 改为 `/app`。
   - Server、AgentJob、driver 和 prompt 均使用同一个 workspace 值，避免元数据与实际执行目录不一致。

2. **P0：增加启动前 workspace 自检**
   - Agent 开始运行前执行并记录：
     - `pwd`
     - `git rev-parse --show-toplevel`
     - `git rev-parse HEAD`
     - `git status --short`
   - 校验仓库根目录为 `/app`，HEAD 与 catalog 中的 `base_commit` 一致；不一致时应直接返回基础设施错误，不进入模型推理。

3. **P0：保留完整轨迹和最终 patch**
   - Adapter 当前结果中的 `trajectory_id` 为空，但 208.77 运行目录实际保存了轨迹引用。
   - 应将 `trajectory_id`、最终 `git_diff`、修改文件列表和测试摘要回填到 EpisodeResult，便于区分“无修改”“改错文件”和“正确修改但测试失败”。

4. **P1：改进 OpenHands instruction**
   - 不要对所有语言固定使用 `find ... -name '*.py'`。
   - 根据 `repo_language` 选择 `.py`、`.go`、`.ts/.tsx/.js` 等文件模式。
   - 强制模型先查看仓库根目录、目标符号定义和现有调用点，再创建新文件。
   - 明确禁止搜索或修改 `/opt/openhands/benchmarks` 等 Agent 主机本地目录。

5. **P1：增加路径合理性检查**
   - 评测前记录 `git diff --name-only`。
   - 如果 diff 为空、只改测试文件，或新增的顶层目录在仓库基线中不存在，则标记明确诊断原因。
   - 该检查只用于诊断和告警，不应以是否命中 gold 路径作为判分条件。

6. **P2：继续运行当前全量评测**
   - 当前证据不足以支持中断全量任务。
   - 后续统计应将失败划分为：无 patch、路径/子系统选择错误、patch 存在但测试失败、基础设施错误。

## 2. 问题背景

Adapter 侧在 SWE-bench-Pro 全量评测中观察到多条 `resolved=false`，并发现已有直接生成 patch 的文件路径与数据集 gold patch 路径差异较大。需要判断：

1. Worker 是否挂载或启动了错误仓库。
2. catalog 是否将 instance 映射到了错误镜像或 base commit。
3. `/workspace` 与 `/app` 是否发生路径映射错误。
4. OpenHands 是否在正确仓库中运行，但模型选择了错误源码路径。

本次重点核验了以下代表 request：

- qutebrowser：`swebenchpro-instance_qutebrowser__qutebrowser-f91ace96223cac8161c16dd061907e138fe85111-v059c6fdc75567943479b23ebca7c07b5e9a7f34c-7d16ba1b`
- navidrome：`swebenchpro-instance_navidrome__navidrome-7073d18b54da7e53274d11c9e2baef1242e8769e-37992a82`
- vuls：`swebenchpro-instance_future-architect__vuls-407407d306e9431d6aa0ab566baa6e44e5ba2904-9ab61f53`
- protonmail/webclients：`swebenchpro-instance_protonmail__webclients-2c3559cad02d1090985dba7e8eb5a129144d9811-f165a6c9`
- flipt：`swebenchpro-instance_flipt-io__flipt-c12967bc73fdf02054cf3ef8498c05e25f0a18c0-2de64eb7`

## 3. 诊断过程与证据

### 3.1 Adapter 请求中的 workspace 配置不正确

`uenv-bridge/scripts/benchmark/evaluate_swebenchpro_uenv.py` 当前默认配置为：

```text
--workspace-dir /workspace
```

本轮已提交请求中检查到的 69 条记录均携带：

```json
{
  "benchmark_variant": "pro",
  "workspace_dir": "/workspace",
  "driver_entrypoint": "run_swebenchpro_official.py"
}
```

该值随后进入 Server 创建的 AgentJob。208.77 上保存的 `agent_job.json` 同样显示：

```json
{
  "benchmark_variant": "pro",
  "workspace_dir": "/workspace"
}
```

但 SWE-bench-Pro Pro 镜像的约定仓库根目录是 `/app`，因此 Adapter 默认值需要修正。

### 3.2 official driver 实际覆盖为 `/app`

`integrations/openhands/run_swebenchpro_official.py` 根据 benchmark variant 选择目录：

```python
def _pro_workspace_dir(variant: str) -> str:
    return "/app" if variant.lower() == "pro" else "/testbed"
```

后续创建 `UEnvWorkspace` 和 instruction 时均使用该函数返回的 `/app`，而不是 AgentJob 中的 `/workspace`。

208.77 实机日志进一步确认：

```text
Working directory: /app
```

生成的 instruction 也明确写入：

```text
The git repository is already checked out at `/app`.
Start by running `ls -la /app` ...
All edits must be under `/app`.
```

因此，本轮模型实际工具调用面向 `/app`。`/workspace` 配置不一致是真实缺陷，但没有把本轮 official driver 的工具调用导向错误目录。

### 3.3 Worker instance 与镜像映射正确

7143 Worker 日志显示，qutebrowser 代表实例启动的镜像为：

```text
jefzda/sweap-images:qutebrowser.qutebrowser-qutebrowser__qutebrowser-f91ace96223cac8161c16dd061907e138fe85111-v059c6fdc75567943479b23ebca7c07b5e9a7f
```

该镜像 tag 与请求、数据集和 Worker EnvPackage `swe-bench-pro@0.3.4` 的 `images.manifest.json` 一致。

Worker provision 日志中的 `instance_id` 也与请求完全一致。未发现将 qutebrowser request 路由到其他 repo 镜像的证据。

### 3.4 qutebrowser 实际失败行为

qutebrowser 代表 request 的运行结果为：

```text
resolved=false
reward=0.0
tests=52/56
git_diff=""
```

OpenHands 的工作目录显示为 `/app`，但模型后续反复执行：

```text
find /opt/openhands/benchmarks ... | grep ...
```

该路径属于 208.77 上的 OpenHands/SDK 安装区域，并非 Worker 容器内的目标仓库。模型重复相同搜索后触发：

```text
Action, Observation loop detected
Stuck pattern detected
```

最终没有修改目标仓库，因此不是“patch 被收集到错误根目录”，而是“模型未形成仓库 patch”。

历史验收中，同一 qutebrowser 环境应用 gold patch 可达到 56/56；LLM 运行则保持 52/56。该对照进一步排除了 grader 或镜像本身损坏。

### 3.5 flipt 实际修改了错误源码位置

flipt 代表 request 的 OpenHands 日志显示模型编辑了：

```text
/app/utils.py
```

模型声称新增了 `map_error_to_status_code`，但该任务对应的 gold patch 路径为：

```text
internal/cmd/grpc.go
internal/server/auth/middleware.go
internal/server/middleware/grpc/middleware.go
```

这说明模型确实在正确 repo 根目录 `/app` 下操作，但没有理解项目已有的错误映射和中间件结构，转而创建了通用顶层工具文件。

### 3.6 直接生成 patch 与 gold patch 对照

对已有 `patches.json` 与 SWE-bench-Pro 数据集 gold patch 做路径提取，代表样例均无路径重合：

- qutebrowser
  - gold：`qutebrowser/browser/qtnetworkdownloads.py`、`qutebrowser/utils/log.py`、`qutebrowser/utils/qtlog.py`
  - model：`tests/unit/utils/test_qtlog.py`

- navidrome
  - gold：`core/agents/...`
  - model：`internal/domain/lastfm/lastfm.go`

- vuls
  - gold：`contrib/trivy/pkg/converter.go`
  - model：`report/scan.go`

- protonmail/webclients
  - gold：`packages/components/...`
  - model：`src/payment/...`

- flipt
  - gold：`internal/server/...`
  - model：`rpc/flipt/auth/middleware.go`

这些路径差异不能单独证明映射异常。结合实机 workspace、镜像和轨迹后，更符合模型没有充分探索实际仓库结构、按通用结构或先验记忆生成路径的特征。

### 3.7 结果可观测性存在缺口

Adapter 结果文件中代表样例的：

```text
trajectory_id=""
```

但 208.77 的运行目录中实际存在：

- `trajectory_ref.json`
- `trajectory_bundle.json`
- `conversation_events.json`
- `runner_stdout.log`
- `runner_stderr.log`
- `submit_result.json`

这导致仅查看 Adapter 结果时无法知道模型是没有修改、修改了错误文件，还是测试失败。建议将关键轨迹引用和 patch 摘要回填到 EpisodeResult。

## 4. 根因判定

### 4.1 已排除或基本排除

1. **错误 repo/镜像映射**：代表实例的 instance、镜像 tag 和 catalog 对应关系正确。
2. **OpenHands 实际在 `/workspace` 空目录执行**：official driver 和工具日志均显示实际 working directory 为 `/app`。
3. **grader 全局异常**：gold patch 历史验收可以通过全部测试。
4. **patch 收集到了 repo 外部文件**：本轮代表失败样例的 artifact `git_diff` 多为空；flipt 的错误编辑发生在 `/app` 内。

### 4.2 确认存在

1. 模型未正确定位目标源码文件或子系统。
2. 模型在部分任务中按通用项目结构创建文件。
3. 模型可能搜索 Agent 本地安装目录，随后陷入重复动作。
4. Adapter 发送的 workspace 元数据与 Pro 实际目录不一致。
5. instruction 固定使用 Python 文件搜索方式，不适合 Go、JS 和 TS 仓库。
6. Adapter 结果缺少 trajectory 和最终 patch 信息，增加了问题定位成本。

## 5. 建议的修复验收

完成改动后，建议至少执行以下验证：

1. 提交一个 qutebrowser Pro 请求，确认 Adapter request、AgentJob 和 OpenHands working directory 三处均为 `/app`。
2. 在 Agent 启动日志中确认：

```text
repo_root=/app
head=<expected base_commit>
```

3. 对 Python、Go、TypeScript 各选择一个样例，确认 instruction 使用正确文件扩展名。
4. 确认 EpisodeResult 能返回非空 `trajectory_id`，并可关联到 208.77/Server 保存的轨迹。
5. 人工制造一次“无 patch”和一次“修改不存在的顶层路径”，确认结果中可以清楚区分诊断原因。
6. 重新运行 qutebrowser 代表样例；即使仍未 resolved，也应能看到模型完整工具轨迹和 `git diff --name-only`，证明可观测性闭环。

## 6. 最终判断

当前 SWE-bench-Pro 链路、Worker 镜像映射和 `/app` 工作区总体可用。此次观察到的“生成文件目录与项目实际目录差别很大”主要属于模型探索与路径选择质量问题。

Adapter 的 `/workspace` 默认值仍应尽快改为 `/app`，以消除协议层不一致；同时应增强启动自检、语言感知 prompt 和轨迹回填。完成这些改进后，可以更可靠地区分模型能力失败与基础设施失败。
