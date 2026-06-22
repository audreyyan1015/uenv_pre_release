# 260622\-worker\-hub\-联调报告

# SWE\-bench 环境镜像 → Worker 拉起 → 运行评测 联调报告

> **文档版本**：v1.6  
> **创建时间**：2026\-06\-22  
> **v1.6 变更**：补齐 **M0–M4 代码缺口**并联调：① **M4 `ImageCacheFactory`**（provision 前 inspect→pull→可选 warm tag，离线命中即跳过）；② **M0-1/M2-4** SWE provision 按 `CommandPolicy` 注入 run flags（cap_drop/network/可选 seccomp）；③ **M2-2** native `DispatchEpisode` 与 L4 Gateway **收敛到同一 `SweInstancePool`**（`run_episode` acquire→gold→submit→release）；④ **M2-1/M2-5** 镜像预热 + `uenv_swe_instance_pool_size` 指标；⑤ **M5-5** 网关服务端 `X-API-Key` 鉴权 + **M5-1** 网关 Rust 集成测试；⑥ **M1-1/M6-1** uenv-hub `GET /api/v1/swe/{variant}/instances` 只读 catalog 端点 + seed + e2e；⑦ **M1-3** evaluate 前可选 install 步骤。新增证据 11、§9 缺口补齐矩阵；worker 库单测 **81 passed**、hub e2e `swe_instance_catalog` 通过。  
> **v1.1 变更**：接通 **M5 外部运行时网关**（L4 `runtime_gateway` HTTP）、`SweSession` 会话原语 + `SweInstancePool` L2 池、`Grader` trait + `SwebenchGrader`、`BenchmarkVariant` 枚举。  
> **v1.0**：核心链路"实例镜像 → Worker 容器 → 运行评测"端到端跑通（CLI `swe-run` + gRPC `DispatchEpisode`），Verified 8/10 gold `reward=1.0` + 负向对照。  
> 日期：2026\-06\-22 ｜ 机器：A100 7143（`219.147.100.43`，uenv\-worker）  
> 依据：`260618-swe-bench-env-hub-worker-plan.md`、`README.md`（四端联调拓扑）

---

## 0\. 结论（TL;DR）

**核心目标达成**：在 Worker 层从 SWE\-bench 实例镜像拉起容器、运行真实 SWE\-bench 实例、并产出正确 reward，已端到端跑通。

- 实测 **8/10** 个 Verified 实例在 gold patch 下 `reward = 1.0`（resolved）。

- 负向对照（不打 gold patch）下 `FAIL_TO_PASS` 用例正确失败、`reward = 0`，说明评分有判别力、非"恒为 1"。

- 全程在 7143 **离线**完成（无外网），依赖本机已缓存的 500 个实例镜像 \+ `SWE-bench_Verified` 数据集。

**已接通 gRPC 全自动派发**：`run_instance` 已接入 Worker 的 gRPC `DispatchEpisode`。外部派发方（Server / grpcurl / 本仓 `swe-dispatch` 客户端）只需发 `EpisodeRequest{env_type="swe", payload={instance_id, use_gold_patch, command_mode, benchmark_variant}}`，Worker 即从实例镜像拉起容器、跑评测、经 `StreamReport` 回传 `reward`——已端到端实测 `reward=1.0`（见 §3 证据 5/6）。

**已接通 M5 外部运行时网关**：新增 L4 `runtime_gateway`（HTTP `/runtime/v1/sessions` create/exec/read/write/submit/delete），把外部 Agent（OpenHands Remote Runtime 形态）的调用翻译成 L2 `SweInstancePool` 的 acquire/exec/submit/release；与 native `DispatchEpisode` **共享**同一 L2 池/L1 Backend（`SweSession` 复用，不分叉两套 grader）。已端到端实测 `reward=1.0`（见 §3 证据 7）。

**本轮新增（M5 OpenHands 客户端 + `plugins/swe` + M6 Pro）已全部实现并联调**：

- **M5 OpenHands `UEnvRuntime` 客户端**（`integrations/openhands/`）：依赖无关的网关 HTTP 客户端 + duck-typed `UEnvRuntime` 适配器（把 OpenHands `CmdRunAction/FileReadAction/FileWriteAction` 转发到本网关）。远端实测 gold→`reward=1.0`(3/3)、no-gold→`reward=0.0`(2/3)（见 §3 证据 8）。
- **M5 `plugins/swe/`**（OpenEnv 风格 environment + evaluator + command_policy + HTTP server）：`reset/step/evaluate/close` 经网关复用同一 L2/L1/grader。远端实测 gold→`reward=1.0`(3/3)（见 §3 证据 9）。
- **M6 `SwebenchProGrader`**：多 runner 日志解析（pytest / `go test` / TAP），`grader_for("swebench_pro")` 已分流；Pro 实例经 `to_instance_spec()` 自动选用。单测覆盖。
- **M6 Pro catalog / 命名空间校验 / seed**：Hub pull 端点按变体分桶（`/api/v1/swe/{verified,pro}/instances`）、`config/swe-pro-default-config.json` seed、启动镜像命名空间校验（Pro 禁占 `sweb.eval.*`）、`swe.variants` 配置。远端实测 Pro 目录加载 + 变体路由（见 §3 证据 10；离线无 Pro registry 镜像，故止于 image-pull 边界）。

> **依赖方向（已确认）**：OpenHands 客户端是**独立重写，零依赖 OpenHands 仓库**（不 `import openhands`）。这是对 plan §5.3.3「实现/子类化 OpenHands 经典 `Runtime` + 用 `benchmarks/swe_bench` 驱动」的**有意偏差**。原因：本仓 clone（`openhands-ai`）是**新版 `app_server`/`agent_server`/SDK 架构**，已无经典 `openhands.runtime.base.Runtime`、无 `openhands.events.observation`、无 `evaluation/benchmarks/swe_bench`（新版改用 runtime-api 协议 `/start`、`/sessions`、`/list`、`/pause`…）。故 `UEnvRuntime` 用 **duck-typing**（读 `.command/.path/.content`，返回 OpenHands 同名字段的 dict）对接 UEnv 网关契约，不绑定任何 OpenHands 版本。若后续要落 plan 字面的"真依赖"：pin 一个含经典 `Runtime` + `benchmarks/swe_bench` 的 OpenHands release，加一层子类 shim 委托到 `UEnvGatewayClient` 即可（网关契约不变）。接 LLM 的完整 agent-loop 需 OpenHands + 模型在线，超出 7143 离线范围。

**达成情况**：

- "**实例镜像 → Worker 容器 → 运行评测**"核心链路：已跑通（CLI `swe-run` 与 gRPC `DispatchEpisode` 两条入口）。

- "**Server→Worker 全自动派发**"：Worker 侧已落地（gRPC 路由 `env_type=swe`）；演示用本仓 `swe-dispatch` 客户端充当派发方，与真实 Server 走同一 `DispatchEpisode` 接口。

- "**uenv\-hub 下发实例目录**"：代码已具备（启动时 `GET {hub}/api/v1/swe/instances`，失败回退本地 `swe_instances.json`，与 env manifest 降级策略一致）；当前 uenv\-hub 尚未提供该端点，故实测走本地目录回退。把目录源切到 Hub 仅为一处配置/端点开关（见 §6）。

- 这里的"镜像"是 SWE\-bench 官方评测镜像（`swebench/sweb.eval.*`），并非由 uenv\-hub 存储分发——uenv\-hub 只存元数据索引，不存镜像本体。

---

## 1\. 架构与数据流

```Plaintext
flowchart LR
    A["swe_instances.json<br/>(SWE-bench_Verified 真值:<br/>gold patch / test_patch /<br/>FAIL_TO_PASS / PASS_TO_PASS)"] --> B
    subgraph W["A100 7143 Worker (离线)"]
      B["uenv-worker swe-run<br/>(Rust harness)"] --> C["docker run -d<br/>实例镜像 → 容器"]
      C --> D["git reset --hard base_commit<br/>(净化沙箱, 保留已编译扩展)"]
      D --> E["应用 test_patch (+ gold patch)"]
      E --> F["conda testbed 环境<br/>bash -lc: python -m pytest -rA -v"]
      F --> G["解析 pytest → 评分<br/>reward + EpisodeArtifact"]
    end
    H["docker 本地镜像库<br/>500× swebench/sweb.eval.*"] -.提供镜像.-> C
```

单 episode 的执行步骤（与 plan §1\.4 / §1\.5 / §4\.3 一致）：

```Plaintext
sequenceDiagram
    participant CLI as swe-run
    participant DK as docker
    participant CT as 实例容器(/testbed)
    CLI->>DK: run -d <实例镜像> sleep infinity
    DK-->>CLI: container_id (从镜像拉起实例)
    CLI->>CT: git reset --hard <base_commit> && git clean -fd
    CLI->>CT: 应用 test_patch
    CLI->>CT: 应用 gold patch (use_gold_patch)
    CLI->>CT: bash -lc "activate testbed; pytest -rA -v <F2P> <P2P>"
    CT-->>CLI: pytest 输出
    CLI->>CLI: 解析 → resolved? → reward 1.0/0.0
    CLI->>DK: rm -f <container> (清理沙箱)
```

---

## 2\. 联调结果

|实例|仓库|gold reward|说明|
|---|---|---|---|
|scikit\-learn\_\_scikit\-learn\-14141|scikit\-learn|**1\.0**|✅|
|scikit\-learn\_\_scikit\-learn\-14053|scikit\-learn|**1\.0**|✅|
|sphinx\-doc\_\_sphinx\-8595|sphinx|**1\.0**|✅|
|pytest\-dev\_\_pytest\-5809|pytest|**1\.0**|✅|
|psf\_\_requests\-1142|requests|**1\.0**|✅|
|pydata\_\_xarray\-3677|xarray|**1\.0**|✅|
|astropy\_\_astropy\-7166|astropy|**1\.0**|✅|
|astropy\_\_astropy\-7671|astropy|**1\.0**|✅|
|sympy\_\_sympy\-20916|sympy|0|自定义测试 runner（见 §4\.1）|
|pylint\-dev\_\_pylint\-4661|pylint|0|离线无法装新依赖（见 §4\.2）|

**通过率 8/10**，覆盖 7 个不同上游仓库。负向对照见 §3。

---

## 3\. 实测证据

### 证据 1 — Worker 存活 \+ 业务/探活端口监听

```Plaintext
$ curl http://219.147.100.43:28777/health
ok
$ ss -tlnp | grep -E '28888|28777'
LISTEN 0.0.0.0:28777  users:(("uenv-worker",pid=3925679))   # health/metrics
LISTEN 0.0.0.0:28888  users:(("uenv-worker",pid=3925679))   # gRPC 业务
```

### 证据 2 — 本机已缓存 500 个 SWE\-bench 实例镜像（离线可用）

```Plaintext
$ docker images | grep -c sweb.eval
500
```

### 证据 3 — 从实例镜像拉起容器 \+ gold 评测 reward=1\.0

```Plaintext
==== SWE-bench episode result ====
instance_id : scikit-learn__scikit-learn-14141
use_gold    : true
resolved    : true
reward      : 1
duration_ms : 3513
tests:
  [PASS] sklearn/utils/tests/test_show_versions.py::test_get_deps_info
  [PASS] sklearn/utils/tests/test_show_versions.py::test_get_sys_info
  [PASS] sklearn/utils/tests/test_show_versions.py::test_show_versions_with_blas

--------- 由实例镜像拉起的 worker 容器 ---------
NAMES                                               IMAGE                                                                   STATUS
uenv-swe-scikit-learn--scikit-learn-14141-3240605   swebench/sweb.eval.x86_64.scikit-learn_1776_scikit-learn-14141:latest   Up 4 seconds
```

### 证据 4 — 负向对照：不打 gold patch，FAIL\_TO\_PASS 正确失败、reward=0

```Plaintext
==== SWE-bench episode result ====
instance_id : scikit-learn__scikit-learn-14141
use_gold    : false
resolved    : false
reward      : 0
tests:
  [FAIL] sklearn/utils/tests/test_show_versions.py::test_get_deps_info   <- 修复前失败
  [PASS] sklearn/utils/tests/test_show_versions.py::test_get_sys_info
  [PASS] sklearn/utils/tests/test_show_versions.py::test_show_versions_with_blas
```

### 证据 5 — gRPC `DispatchEpisode` 全自动派发：gold → reward=1\.0

> Worker 以离线降级模式 `serve`（`config/uenv-worker.swe-local.yaml`，gRPC `:38888`），
用本仓 `swe-dispatch` 客户端发真实 `DispatchEpisode(env_type=swe)`：

```Plaintext
$ uenv-worker swe-dispatch --endpoint 127.0.0.1:38888 \
      --instance scikit-learn__scikit-learn-14141 --gold true
dispatching env_type=swe instance=scikit-learn__scikit-learn-14141 gold=true -> http://127.0.0.1:38888
  [stream] phase=episode_complete step=1/1 reward=1
           info.instance_id = scikit-learn__scikit-learn-14141
           info.resolved = true
           info.tests_passed = 3
           info.tests_total = 3
           info.use_gold_patch = true
==== DispatchEpisode 完成：reward = 1 ====
```

### 证据 6 — gRPC 负向对照：不打 gold → reward=0

```Plaintext
$ uenv-worker swe-dispatch --endpoint 127.0.0.1:38888 \
      --instance scikit-learn__scikit-learn-14141 --gold false
  [stream] phase=episode_complete step=1/1 reward=0
           info.resolved = false
           info.tests_passed = 2
           info.tests_total = 3
==== DispatchEpisode 完成：reward = 0 ====
```

> 派发时序（与真实 Server 同接口）：

```Plaintext
sequenceDiagram
    participant SV as 派发方(Server/grpcurl/swe-dispatch)
    participant WK as Worker gRPC :38888
    participant EX as EpisodeExecutor
    participant HN as SWE harness + docker
    SV->>WK: DispatchEpisode(EpisodeRequest{env_type=swe,<br/>payload={instance_id,use_gold_patch}})
    WK->>EX: 路由 env_type==swe
    EX->>HN: run_instance(实例镜像→容器→评测)
    HN-->>EX: EpisodeOutcome{reward, resolved, tests}
    EX-->>WK: StreamReport{reward, info}
    WK-->>SV: stream(StreamReport) → reward
```

### 证据 7 — M5 外部运行时网关（HTTP）：create→write+apply→submit→reward=1.0

> 外部 Agent（OpenHands Remote Runtime 的最小形态）经 Worker L4 HTTP 网关 `:48999`
> 完成 session 全生命周期（`scripts/swe_gateway_demo.py` 仅用标准库 urllib）：

```Plaintext
$ python3 scripts/swe_gateway_demo.py --endpoint 127.0.0.1:48999 \
      --instance scikit-learn__scikit-learn-14141
[1] POST /sessions  instance=scikit-learn__scikit-learn-14141 mode=FullShell
    session_id=sess-scikit-learn--scikit-learn-14141-1
    observation.issue_text[:160]='Add joblib in show_versions ...'
[2] POST /write (gold patch -> /tmp/gold.patch) + POST /exec (git apply)
    git apply exit_code=0
[3] POST /sessions/<id>/submit  (apply test_patch + run tests + grade)
    resolved=True reward=1.0 tests=3/3
      [PASS] sklearn/utils/tests/test_show_versions.py::test_get_deps_info
      [PASS] sklearn/utils/tests/test_show_versions.py::test_get_sys_info
      [PASS] sklearn/utils/tests/test_show_versions.py::test_show_versions_with_blas
[4] DELETE /sessions/<id>
==== Gateway episode 完成：reward = 1.0 ====
```

负向对照（`--no-gold`）：`resolved=False reward=0.0 tests=2/3`（FAIL_TO_PASS 正确失败）。
native gRPC 回归（harness 重构为 `SweSession` 后）：`swe-dispatch --gold true` 仍 `reward=1`。

> Worker 分层（plan §5.2）：L4 Gateway 与 native 路径共享 L2 池/L1 Backend：

```Plaintext
flowchart TB
    OH["外部 Agent / OpenHands<br/>(swe_gateway_demo.py)"] -- HTTP --> GW
    UE["native swe-dispatch"] -- gRPC DispatchEpisode --> EX
    subgraph WK["uenv-worker"]
      GW["L4 Runtime Gateway :48999<br/>/runtime/v1/sessions ..."]
      EX["L3 EpisodeExecutor (env_type=swe)"]
      GW --> PL["L2 SweInstancePool (session=lease 1 SweSession)"]
      EX --> PL
      PL --> SS["SweSession: provision/exec/write/apply/evaluate"]
      SS --> GR["grader (swebench / M6 swebench_pro)"]
      SS --> BK["L1 docker/podman 实例镜像容器"]
    end
```

### 证据 8 — M5 OpenHands `UEnvRuntime` 客户端：gold→reward=1.0 / no-gold→0

> `integrations/openhands/run_swebench.py` 用 `UEnvRuntime` 把 OpenHands 形态的
> `FileWriteAction` / `CmdRunAction` 经适配器转发到网关 `:48999`：

```Plaintext
$ python3 integrations/openhands/run_swebench.py --gateway 127.0.0.1:48999 \
      --instance scikit-learn__scikit-learn-14141
[connect] session=sess-scikit-learn--scikit-learn-14141-1 variant=verified
[prompt ] issue_text[:160]='Add joblib in show_versions ...'
[write  ] {'observation': 'write', 'path': '/tmp/agent.patch', 'ok': True}
[run    ] git apply exit_code=0
[submit ] resolved=True reward=1.0 tests=3/3
          [PASS] sklearn/utils/tests/test_show_versions.py::test_get_deps_info
          [PASS] sklearn/utils/tests/test_show_versions.py::test_get_sys_info
          [PASS] sklearn/utils/tests/test_show_versions.py::test_show_versions_with_blas
==== UEnvRuntime episode done: reward = 1.0 ====

# 负向对照（--no-gold）
[submit ] resolved=False reward=0.0 tests=2/3   # FAIL_TO_PASS 正确失败
```

### 证据 9 — M5 `plugins/swe`（OpenEnv 风格）：reset→apply_patch→evaluate→reward=1.0

> `plugins/swe/run_demo.py` 经 `SweEnvironment`（reset/step/evaluate）驱动网关：

```Plaintext
$ python3 plugins/swe/run_demo.py --gateway 127.0.0.1:48999 \
      --instance scikit-learn__scikit-learn-14141
[reset   ] session=sess-scikit-learn--scikit-learn-14141-2 variant=verified
[issue   ] 'Add joblib in show_versions ...'
[apply   ] exit_code=0
[evaluate] resolved=True reward=1.0 tests=3/3
==== plugins/swe episode done: reward = 1.0 ====
```

### 证据 10 — M6 Pro：catalog 加载 + 命名空间校验 + 变体路由（离线止于 image-pull）

> 以合并目录（verified+pro，11 实例）+ `UENV_SWE_VARIANTS=verified,pro` 启动：

```Plaintext
# 启动日志：Pro 目录合并加载、命名空间校验通过（无 violation 告警）
INFO uenv_worker::runtime: count=11 path=...swe_instances_with_pro.json msg="swe_catalog_loaded_local"

# verified 在合并目录上仍 resolved（merge 未破坏既有路径）
$ python3 plugins/swe/run_demo.py ... --instance scikit-learn__scikit-learn-14141
==== plugins/swe episode done: reward = 1.0 ====

# Pro 实例经 benchmark_variant=pro 创建 —— 已正确路由（非 404 not-in-catalog），
# 仅止于离线无 Pro registry 镜像：
$ curl -XPOST .../runtime/v1/sessions -d '{"instance_id":"swe-pro__example-go-1","benchmark_variant":"pro"}'
{"error":"docker run failed for registry.example.com/swe-pro/example-org_example-go:...:
  Unable to find image ... locally / failed to resolve reference ..."}
```

> 说明：Pro grader 选择（`swebench_pro`）、变体解析、catalog 分桶、命名空间校验均由**单测**与**上述 live 路由**证实；真正跑通 Pro 容器需 Pro registry 镜像（plan M6 验收第 2/3 项），离线不可达，属环境约束而非实现缺失。

---

### 证据 11 — v1.6 M0–M4 缺口补齐 live 验证（7143，同一进程）

> 单进程 Worker（gRPC `:38888` + Gateway `:48999` + metrics `:38777`），`UENV_RUNTIME_GATEWAY_API_KEY=swe-secret`、`UENV_SWE_PREWARM=astropy__astropy-7166` 启动。

**① 启动日志 — M4 镜像预热 + M0-1/M2-4 策略化 run flags + M5-5 鉴权**

```Plaintext
INFO uenv_worker::swe::instance_pool: instance_id=astropy__astropy-7166 image_state=Present msg="swe_prewarm_image_ready"
INFO uenv_worker::runtime: prewarm_ok=1 prewarm_fail=0 msg="swe_prewarm_completed"
INFO uenv_worker::runtime_gateway: gateway_addr=0.0.0.0:48999 catalog=10 auth=x-api-key msg="runtime_gateway_start"
INFO uenv_worker::swe::session: episode_id=sess-astropy--astropy-7166-1 instance_id=astropy__astropy-7166 \
     image=swebench/sweb.eval.x86_64.astropy_1776_astropy-7166:latest image_state=Present \
     seccomp=default network=bridge issue_chars=162 msg="swe_session_provisioned"
```

- `image_state=Present`：ImageCacheFactory `image inspect` 命中本地镜像 → 跳过 pull（离线零 egress，M4-1/M4-2）。
- `seccomp=default network=bridge`：FullShell 策略化 run flags 生效（M0-1/M2-4；seccomp 目录未配置故保留运行时默认，避免破坏 SWE 宽 syscall 依赖）。
- `auth=x-api-key`：网关服务端鉴权启用（M5-5）。

**② M2-2 native 与 Gateway 共享同一 `SweInstancePool`**

```Plaintext
# native gRPC DispatchEpisode → 池 session 序号 -1
$ uenv-worker --config ... swe-dispatch --instance astropy__astropy-7166 --gold true --endpoint 127.0.0.1:38888
  [stream] phase=episode_complete reward=1  info.tests_passed=7 info.tests_total=7
==== DispatchEpisode 完成：reward = 1 ====        # 日志 container=sess-astropy--astropy-7166-1

# L4 Gateway gold 流 → 同一池 session 序号 -2（证明同源单池）
$ python3 scripts/swe_gateway_demo.py --endpoint 127.0.0.1:48999 --instance astropy__astropy-7166 --api-key swe-secret
    session_id=sess-astropy--astropy-7166-2
    resolved=True reward=1.0 tests=7/7
==== Gateway episode 完成：reward = 1.0 ====
```

> 两条入口（native gRPC、L4 Gateway）在**同一运行进程**内产出连续递增的池 session 序号（`-1`、`-2`），证实 M2-2：native `DispatchEpisode` 已从一次性 `harness::run_instance` 收敛到与 Gateway 共享的 `SweInstancePool.run_episode`（acquire→gold→submit→release）。

**③ M5-5 鉴权 401 对照 + 负向对照**

```Plaintext
$ curl -X POST :48999/runtime/v1/sessions -d '{"instance_id":"astropy__astropy-7166"}'           → HTTP 401
$ curl -X POST :48999/runtime/v1/sessions -H 'X-API-Key: nope' -d '{...}'                          → HTTP 401
$ swe_gateway_demo.py ... --api-key swe-secret --no-gold   → resolved=False reward=0.0 tests=6/7   # 评分有判别力
```

**④ M2-5 池指标**

```Plaintext
$ curl :38777/metrics | grep swe_instance_pool
uenv_swe_instance_pool_size 0        # 会话释放后归零（旧 v1.5 二进制无此行，证实运行的是 v1.6 新二进制）
```

**⑤ 回归确认**：native CLI `swe-run astropy__astropy-7166` gold→reward=1.0（新 provision 路径未破坏既有链路）；worker 库单测 81 passed；`plugins/swe`+`integrations/openhands` 离线 9 passed / 1 skipped。

---

## 4\. 两个未通过实例的根因（均非框架 bug，属 SWE\-bench 长尾）

### 4\.1 sympy\_\_sympy\-20916 — 自定义测试 runner

- SWE\-bench 中 sympy 实例的 `FAIL_TO_PASS` / `PASS_TO_PASS` 记录的是 **sympy 自带测试框架的"裸函数名"**（如 `test_super_sub`、`test_requires_partial`），**不是** pytest 的 `文件路径::用例` node id。

- sympy 用自己的 `bin/test` / `sympy.test()` 来发现并运行用例；直接 `python -m pytest test_super_sub` 无法 collect 到这些用例 → 判定失败。

- **本质**：需要按"仓库 \+ 版本"提供专属测试命令（官方 `swebench` 包的 `MAP_REPO_VERSION_TO_SPECS` 即做此事）。属测试命令适配，不是评分逻辑错误。

### 4\.2 pylint\-dev\_\_pylint\-4661 — 离线环境装不上"新引入的依赖"

- 该 issue 的修复**本身就是引入新依赖 ****`appdirs`**：gold patch 往 `pylint/config/__init__.py` 加 `import appdirs`，并往 `setup.cfg` 的 `install_requires` 加 `appdirs>=1.4.0`。

- 实例镜像里的 conda 环境是在**修复前的 base\_commit** 构建的，**不含 appdirs**。官方 harness 在打完 patch 后会执行安装步骤把新依赖装上；但 7143 **无外网、PyPI 不可达**，装不了 `appdirs` → 测试 collect 阶段 `ModuleNotFoundError: No module named 'appdirs'` → 判定失败。

- **本质**：离线环境 \+ 该实例新增外部依赖的组合限制，不是框架问题。绝大多数实例不新增依赖，故离线即可通过（已验证 8 个）。

> 小结：两者都属"需要官方 harness 的 per\-instance 安装/测试规格"的长尾情形。引入 `swebench` 的 `MAP_REPO_VERSION_TO_SPECS` 覆盖即可解决 4\.1，并为 4\.2 提供安装步骤（仍需可达的依赖源/本地 wheel 缓存）。

---

## 5\. 复现方式

```Bash
# 本地仓库根（已配置 SSH：私钥经 UENV_SSH_KEY，不入库）
cd uenv

# 一次性：同步源码到 Worker 隔离目录并离线编译
./scripts/connect-remote.sh sync
./scripts/connect-remote.sh build

# 跑单个实例（默认 docker，默认应用 gold patch）
./scripts/connect-remote.sh swe-run scikit-learn__scikit-learn-14141
# 负向对照
./scripts/connect-remote.sh swe-run scikit-learn__scikit-learn-14141 --no-gold
```

数据来源：`scripts/export_swe_instances.py` 在本地（有 pyarrow）从 HF parquet 导出 `fixtures/swe/swe_instances.json`（Worker 离线读取，无需 `datasets` 库）。

---

## 6\. 局限与下一步

|项|现状|下一步|
|---|---|---|
|执行入口|✅ CLI `swe-run` \+ gRPC `DispatchEpisode`（payload `{instance_id, use_gold_patch}`）两条入口均跑通|由真实 Server 调度器派发（接口已对齐，无需改 Worker）|
|实例元数据来源|代码支持 Hub 下发（`GET /api/v1/swe/instances`）\+ 本地回退；当前走本地 `swe_instances.json`|uenv\-hub 实现该端点后即为 Hub 下发（plan §1\.2 / §6）|
|容器运行时|docker（500 镜像在 docker 库；podman 库为 0）|运行时已可配 `--runtime`；podman flag 映射逻辑已在 `backend::podman`|
|测试命令|通用 `python -m pytest -rA -v <ids>`|引入 `MAP_REPO_VERSION_TO_SPECS` 覆盖 sympy / django 等专属 runner|
|离线新依赖实例|无法在线安装（如 pylint\-4661）|准备本地 wheel 缓存或可达 PyPI 镜像|
|模型路径|当前用 gold patch 验证链路|接 AgentLoop/LLM 生成 patch，跑真实 RL/评测|

---

## 7\. 本次交付物清单

|文件|说明|
|---|---|
|`uenv/uenv-worker/src/swe/dataset.rs`|数据集行 → InstanceSpec/TaskSpec；`instance_id → 镜像名` 映射（`__`→`_1776_`）|
|`uenv/uenv-worker/src/swe/harness.rs`|执行链：provision/reset/apply patch/跑测试/解析评分；含纯函数单测|
|`uenv/uenv-worker/src/swe/resettable.rs`|新增 `reset_script_keep_built`（保留已编译扩展）|
|`uenv/uenv-worker/src/episode/executor.rs`|**新增**：`env_type=swe` 路由 → `execute_swe_episode`，封装 `EpisodeResult`/`StreamReport`|
|`uenv/uenv-worker/src/runtime.rs`|**新增**：SWE 目录加载（Hub 下发 \+ 本地回退）、`UENV_WORKER_ALLOW_DEGRADED_START` 离线降级启动|
|`uenv/uenv-worker/src/hub/mod.rs`|**新增**：`pull_swe_catalog`（Hub 下发实例目录）|
|`uenv/uenv-worker/src/cli`、`main.rs`|`swe-run` \+ **新增 ****`swe-dispatch`**（gRPC 客户端，演示派发）|
|`uenv/config/uenv-worker.swe-local.yaml`|**新增**：离线/本地 gRPC 联调配置（`:38888`）|
|`uenv/scripts/connect-remote.sh`|自动连接 \+ sync/build/swe\-run/health|
|`uenv/scripts/gen-worker-proto.sh`|Worker proto 离线生成|
|`uenv/scripts/export_swe_instances.py`|parquet → JSON 导出|
|`uenv/fixtures/swe/swe_instances.json`|10 个 Verified 实例真值|
|`uenv/uenv-worker/src/runtime_gateway/`|**本轮新增**：L4 外部运行时网关（HTTP session API）|
|`uenv/uenv-worker/src/swe/session.rs`|**本轮新增**：`SweSession` 会话原语（provision/exec/write/read/apply/evaluate）|
|`uenv/uenv-worker/src/swe/instance_pool.rs`|**本轮新增**：`SweInstancePool` L2 会话池|
|`uenv/uenv-worker/src/swe/grader.rs`|`Grader` trait + `SwebenchGrader` + **本轮新增 `SwebenchProGrader`**（多 runner：pytest/go test/TAP）；`grader_for` 已分流|
|`uenv/uenv-worker/src/swe/variant.rs`|**本轮新增**：`BenchmarkVariant`（verified/lite/pro）|
|`uenv/scripts/swe_gateway_demo.py`|网关端到端演示（标准库最小客户端）|
||`uenv/uenv-worker/src/swe/dataset.rs`|**本轮更新**：`benchmark_variant`/`image_cache_key`/`test_cmd` 字段；变体→grader；`image_namespace_violations()`；`merge_from`|
||`uenv/uenv-worker/src/hub/mod.rs`|**本轮更新**：`pull_swe_catalog` 按变体分桶 `/api/v1/swe/{verified,pro}/instances`|
||`uenv/uenv-worker/src/runtime.rs`、`config/mod.rs`|**本轮更新**：`swe.variants` + `UENV_SWE_VARIANTS`；多变体合并加载 + 启动镜像命名空间校验|
||`uenv/integrations/openhands/`|**本轮新增**：`UEnvGatewayClient`/`UEnvSession`、`UEnvRuntime` 适配器、`run_swebench.py`、离线单测、README|
||`uenv/plugins/swe/`|**本轮新增**：`SweEnvironment`（OpenEnv reset/step/evaluate）、`command_policy.py`、`evaluator/`、`server/app.py`、`run_demo.py`、单测、README|
||`uenv/config/swe-pro-default-config.json`|**本轮新增**：M6 Pro Hub `default_config` seed|
||`uenv/fixtures/swe/swe_pro_instances.json`|**本轮新增**：Pro 变体本地 fixture|

**测试**（v1.6 已更新，见 §9）：worker 库单测 **81 passed**（+5：Pro grader / dataset 变体 / 命名空间校验）；集成测试 `swe_mvp_closure` 4 passed；Python 离线单测 `plugins/swe` 6 passed、`integrations/openhands` 4 passed（1 skipped）；远端 **gRPC `DispatchEpisode` E2E**：gold→reward=1\.0、no\-gold→reward=0；**M5 网关 E2E**：gold→reward=1\.0、no\-gold→reward=0；**OpenHands `UEnvRuntime` E2E**：gold→1\.0/no\-gold→0；**`plugins/swe` E2E**：gold→1\.0；**M6 Pro**：目录加载+变体路由（离线止于 image\-pull）；CLI E2E 8/10 reward=1\.0。
（`m4/m5/m6` 用例依赖 math 插件 UDS，本机未起插件而失败，属既有环境问题，与 SWE 改动无关。）

### gRPC 全自动派发复现

```Bash
# Worker：离线降级启动（无需 Server），gRPC :38888
ssh <worker> 'cd /root/UEnv-swe && UENV_WORKER_ALLOW_DEGRADED_START=1 \
  ./target/release/uenv-worker --config config/uenv-worker.swe-local.yaml serve &'
# 派发方：本仓 gRPC 客户端（与真实 Server 同接口）
./target/release/uenv-worker swe-dispatch --endpoint 127.0.0.1:38888 \
  --instance scikit-learn__scikit-learn-14141 --gold true
```

### M5 网关复现

```Bash
# Worker：离线降级 + 启用网关（28999 被本机 vLLM 占用，演示用 48999）
ssh <worker> 'cd /root/UEnv-swe && UENV_WORKER_ALLOW_DEGRADED_START=1 \
  UENV_RUNTIME_GATEWAY_LISTEN=0.0.0.0:48999 \
  ./target/release/uenv-worker --config config/uenv-worker.swe-local.yaml serve &'
# 外部 Agent（OpenHands 形态最小客户端）经 HTTP 网关跑通 session 全生命周期
python3 scripts/swe_gateway_demo.py --endpoint 127.0.0.1:48999 \
  --instance scikit-learn__scikit-learn-14141            # gold → reward=1.0
python3 scripts/swe_gateway_demo.py --endpoint 127.0.0.1:48999 \
  --instance scikit-learn__scikit-learn-14141 --no-gold  # 负向 → reward=0
```

### M5 OpenHands `UEnvRuntime` / `plugins/swe` 复现

```Bash
# OpenHands UEnvRuntime（适配器→网关）
python3 integrations/openhands/run_swebench.py --gateway 127.0.0.1:48999 \
  --instance scikit-learn__scikit-learn-14141              # gold → 1.0
python3 integrations/openhands/run_swebench.py --gateway 127.0.0.1:48999 \
  --instance scikit-learn__scikit-learn-14141 --no-gold    # 负向 → 0
# plugins/swe（OpenEnv environment→网关）
python3 plugins/swe/run_demo.py --gateway 127.0.0.1:48999 \
  --instance scikit-learn__scikit-learn-14141              # gold → 1.0
# 离线单测（无需 Worker）
python3 plugins/swe/tests/test_environment.py
python3 integrations/openhands/tests/test_client_smoke.py
```

### M6 Pro 复现（catalog 加载 / 变体路由；跑通容器需 Pro 镜像）

```Bash
# 启用 Pro 变体加载（合并 verified + pro fixture）
UENV_SWE_VARIANTS=verified,pro \
UENV_SWE_INSTANCES=fixtures/swe/swe_pro_instances.json \
UENV_RUNTIME_GATEWAY_LISTEN=0.0.0.0:48999 UENV_WORKER_ALLOW_DEGRADED_START=1 \
  ./target/release/uenv-worker --config config/uenv-worker.swe-local.yaml serve &
# Pro grader / 变体 / 命名空间 单测
cargo test -p uenv-worker --lib swe
```

---

## 8\. M5 / M6 实现状态矩阵（如实标注）

M5/M6 全部落地：L4 网关、OpenHands `UEnvRuntime` 客户端、`plugins/swe`、M6 `SwebenchProGrader` 与 Pro catalog/校验/seed 均已实现；除"跑通真实 Pro 容器"需 Pro registry 镜像（离线不可达）外，其余均已联调验证。

|plan 项|级别|状态|说明|
|---|---|---|---|
|`env_type=swe` DispatchEpisode 路由|M1/M2|✅ 已实现并联调|gold→1.0 / no-gold→0|
|`SweSession` 会话原语 + harness 复用|M2/M5|✅ 已实现，单测+E2E|provision/exec/write/read/apply/evaluate|
|`SweInstancePool`（L2 池）|M2/M5|✅ 已实现|session 生命周期 + 容量；按需 provision（未预热）|
|`Grader` trait + `SwebenchGrader`|M5|✅ 已实现|`grade()` 抽出，evaluate 复用|
|`BenchmarkVariant`(verified/lite/pro)|M6|✅ 枚举+解析已实现|payload `benchmark_variant` 已解析|
|**L4 External Runtime Gateway（HTTP）**|M5|✅ **已实现并联调**|create/exec/read/write/submit/delete；网关 gold→1.0|
|`command_mode` / `benchmark_variant` payload 对齐|M1/M6|✅ 已实现|FullShell 默认；variant 入 info/log|
|**OpenHands `UEnvRuntime` 客户端**（`integrations/openhands/`）|M5|✅ **已实现并联调**|依赖无关 HTTP 客户端 + duck-typed `UEnvRuntime`（转发 `Cmd/FileRead/FileWrite` action）；gold→1.0/no-gold→0（证据 8）；完整 agent-loop 需 OpenHands+LLM 在线|
|**`plugins/swe/`**（environment.py + evaluator）|M5|✅ **已实现并联调**|OpenEnv `SweEnvironment` + evaluator + command_policy + HTTP server；经网关共用 L2/L1/grader；gold→1.0（证据 9）|
|**M6 `SwebenchProGrader`**|M6|✅ **已实现**|多 runner 解析（pytest/go test/TAP）；`grader_for("swebench_pro")` 分流；Pro 实例经 `to_instance_spec()` 自动选用；单测覆盖|
|**M6 Pro catalog / 命名空间校验 / seed**|M6|✅ **已实现**（跑通 Pro 容器待镜像）|`pull_swe_catalog` 变体分桶 `/api/v1/swe/{verified,pro}/instances`、`swe-pro-default-config.json` seed、`swe.variants` 配置、启动镜像命名空间校验；目录加载+变体路由已 live（证据 10）；真实 Pro 容器需 registry 镜像（离线不可达）|

> 结论：**M5/M6 全量实现并联调**（OpenHands 客户端 / `plugins/swe` / Pro grader / Pro catalog 均 ✅）。唯一未做到的是"跑通真实 SWE-bench Pro 容器"——受限于 7143 离线无 Pro registry 镜像（plan M6 验收第 2/3 项），属环境约束而非实现缺失；其余路径（Pro 目录加载、变体路由、命名空间校验、grader 选择）均已验证。

---

## 9\. v1.6 M0–M4 缺口补齐矩阵（依据 `260622-swe-implementation-gap-analysis.md`）

> 本轮按缺口分析 §5 优先级补齐 **P0/P1 中可离线实现且可验证**的代码缺口；M3（快照 / 内置 AgentLoop）与"真实 Pro/在线 LLM"因 plan 标注 M3+ 或离线环境约束，明确列为遗留。

|缺口 ID|内容|v1.6 状态|证据 / 落点|
|---|---|---|---|
|M4-1/M4-2/M4-3|`ImageCacheFactory`：inspect→pull→可选 warm tag|✅ 已实现并联调|`swe/image_cache.rs` + provision 接入；证据 11① `image_state=Present`；单测 3|
|M0-1 / M2-4|provision 按 `CommandPolicy` 注入 cap_drop/network/可选 seccomp|✅ 已实现并联调|`session.rs::build_swe_run_args`；证据 11① `seccomp=default network=bridge`；单测 3|
|M2-2|native `DispatchEpisode` 与 Gateway 收敛到同一 `SweInstancePool`|✅ 已实现并联调|`instance_pool.rs::run_episode` + `executor.with_swe_pool`；证据 11② 同进程池序号 -1/-2|
|M2-1 / M4-4|池启动镜像预热|✅ 已实现并联调|`prewarm_images` + `swe.prewarm`/`UENV_SWE_PREWARM`；证据 11① `prewarm_ok=1`|
|M2-5|SWE 池 metrics|✅ 已实现并联调|`uenv_swe_instance_pool_size`；证据 11④|
|M5-5|网关服务端 `X-API-Key` 鉴权|✅ 已实现并联调|`runtime_gateway` 中间件；证据 11③ 401×2 / 正确 key 放行|
|M5-1|网关 Rust 集成测试（路由/鉴权/404，无 docker）|✅ 已实现|`runtime_gateway::tests`（health/404/api_key 三例）|
|M1-1 / M6-1|uenv-hub `GET /api/v1/swe/{variant}/instances` 只读 catalog 端点|✅ 已实现|`uenv-hub-server/src/routes.rs` + `config/swe/{verified,pro}.json` seed + hub e2e `swe_instance_catalog`|
|M1-3|evaluate 前可选 install 步骤|✅ 已实现|`session.rs::install_command`（实例 `install_cmd` / `UENV_SWE_INSTALL_CMD`）|
|M1-2 / M1-4|`MAP_REPO_VERSION_TO_SPECS` / 非 pytest 专属 runner|◻️ 部分（实例 `test_cmd` 覆盖；未内置官方 specs 表）|长尾 sympy/django 等仍走通用 pytest（见 §4）|
|M3-1~M3-4|快照 / `SnapshotResettableInstance` / 内置多步 AgentLoop|❌ 遗留（plan 标注 M3+）|`PodmanBackend::snapshot` 仍返回 not implemented|
|M6-4~M6-6|真实 Pro 镜像 pull / 官方 `swe_bench_pro_eval` / Pro live|❌ 遗留（离线无 Pro registry 镜像）|代码路径就绪，止于 image-pull 边界（证据 10）|
|M5-4|OpenHands LLM 全闭环|❌ 遗留（需 OpenHands + 在线模型）|网关契约已就绪，agent-loop 超出 7143 离线范围|

**v1.6 测试汇总**：worker 库单测 **81 passed**（v1.5 起 +9：image_cache 3 / run-flags 3 / gateway 3）；`uenv-hub-server` e2e `swe_instance_catalog_served_by_variant` 通过；`plugins/swe` + `integrations/openhands` 离线 **9 passed / 1 skipped**；远端 7143 live：native(池)/Gateway gold→`reward=1.0`、鉴权 401、负向 `reward=0`（证据 11）。
（既有 `m4_plugin_host_process` 两例因依赖 math 插件 UDS、本机未起插件而失败，属既有环境问题，与本轮 SWE 改动无关。）

### v1.6 新增/变更交付物

|文件|说明|
|---|---|
|`uenv-worker/src/swe/image_cache.rs`|**新增** `ImageCacheFactory`（inspect/pull/warm tag）+ 纯函数单测|
|`uenv-worker/src/swe/session.rs`|**更新** provision 走 `build_swe_run_args`（策略化 flags）+ 镜像缓存 ensure；evaluate 前可选 install|
|`uenv-worker/src/swe/instance_pool.rs`|**更新** `run_episode`/`apply_patch`/`prewarm_images` + metrics 接线|
|`uenv-worker/src/swe/command_policy.rs`|**更新** `seccomp_profile_dir` + `resolve_seccomp_file`|
|`uenv-worker/src/swe/dataset.rs`|**更新** `install_cmd` 字段|
|`uenv-worker/src/episode/executor.rs`|**更新** `with_swe_pool` + native 经共享池 `run_episode`|
|`uenv-worker/src/runtime_gateway/mod.rs`|**更新** `X-API-Key` 中间件 + Rust 集成测试|
|`uenv-worker/src/runtime.rs`、`config/mod.rs`、`main.rs`|**更新** 共享池构建、预热、`gateway.api_key`/`swe.prewarm` 配置 + env override|
|`uenv-worker/src/metrics.rs`|**更新** `uenv_swe_instance_pool_size`|
|`uenv-hub/uenv-hub-server/src/routes.rs`|**新增** `GET /api/v1/swe/:variant/instances` 只读 catalog 端点|
|`uenv-hub/.../tests/e2e.rs`|**新增** `swe_instance_catalog_served_by_variant` e2e|
|`config/swe/{verified,pro}.json`|**新增** Hub catalog seed|
|`scripts/swe_gateway_demo.py`|**更新** `--api-key` 支持（网关鉴权联调）|

