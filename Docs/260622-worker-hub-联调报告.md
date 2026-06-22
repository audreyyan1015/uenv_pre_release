# 260622\-worker\-hub\-联调报告

# SWE\-bench 环境镜像 → Worker 拉起 → 运行评测 联调报告

> 日期：2026\-06\-22 ｜ 机器：A100 7143（`219.147.100.43`，uenv\-worker）
依据：`260618-swe-bench-env-hub-worker-plan.md`、`README.md`（四端联调拓扑）

---

## 0\. 结论（TL;DR）

**核心目标达成**：在 Worker 层从 SWE\-bench 实例镜像拉起容器、运行真实 SWE\-bench 实例、并产出正确 reward，已端到端跑通。

- 实测 **8/10** 个 Verified 实例在 gold patch 下 `reward = 1.0`（resolved）。

- 负向对照（不打 gold patch）下 `FAIL_TO_PASS` 用例正确失败、`reward = 0`，说明评分有判别力、非"恒为 1"。

- 全程在 7143 **离线**完成（无外网），依赖本机已缓存的 500 个实例镜像 \+ `SWE-bench_Verified` 数据集。

**已接通 gRPC 全自动派发**（本次新增）：`run_instance` 已接入 Worker 的 gRPC `DispatchEpisode`。外部派发方（Server / grpcurl / 本仓 `swe-dispatch` 客户端）只需发 `EpisodeRequest{env_type="swe", payload={instance_id, use_gold_patch}}`，Worker 即从实例镜像拉起容器、跑评测、经 `StreamReport` 回传 `reward`——已端到端实测 `reward=1.0`（见 §3 证据 5/6）。

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

**测试**：worker 库单测 61 passed；集成测试 `swe_mvp_closure` 4 passed；远端 **gRPC ****`DispatchEpisode`**** E2E**：gold→reward=1\.0、no\-gold→reward=0；CLI E2E 8/10 reward=1\.0。
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

