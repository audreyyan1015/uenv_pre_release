# SWE-smith 示例：任务环境、EnvPackage 与 Agent Package 字段与制品形态

> **日期**：2026-08-25  
> **用途**：用 `swe-bench-smith` 完整链路，说明 Hub 上「任务环境 / EnvPackage / Agent Package」各自包含哪些字段、落盘形态是什么（文件夹 / 声明式文本 / Python 源码 / 二进制镜像 tar），并对照通用定义规范。  
> **实机核对**：Hub `8.130.95.176:8088`（`secrets/README.md` §1.1 / §2.5 / §3.5），Token 读自 `data/.admin_token`；本文字段与尺寸以 **2026-08-25** 实机 `GET` 为准。  
> **关联**：
> - [Hub 控制台信息架构与 SWE-smith 完整包入库](../../hub/260806-Hub控制台信息架构与SWE-smith完整包入库.md)
> - [UEnvHub 标准化环境指南](../../hub/uenv-hub环境标准化指南.md)
> - [概念对照表](../概念对照表-代码协议字段与对外展示名称.md)

---

## 0. 一句话结论

| 概念 | Hub 上「是什么」 | 主要形态 | swe-smith 实例 |
|---|---|---|---|
| **任务环境（Task Environment）** | 能力契约注册表条目（`env_type@version`），描述 Action/Observation/State 与 `config_schema` | **SQLite 元数据 + JSON API**；**不是**一个可 sync 的制品文件夹 | `swe@0.1.0` |
| **EnvPackage（基准数据集包）** | 可版本化分发单元：题目目录 + 评测规格 +（可选）镜像 tar | **版本目录**，内含 JSON/YAML 文本 + **GB 级 `image_tar` 二进制** | `swe-bench-smith@0.2.0` |
| **Agent Package（Agent 脚手架）** | 声明了 `agent_kind` 的 EnvPackage | **版本目录**，内含 **Python 源码** + `MANIFEST.json` / `PIN.md` | `uenv-agent-openhands@1.0.1` |
| **Episode Stack（可运行配方）** | 钉版本引用三者（自身**不含字节**） | **JSON 配方**，存在 Stack 注册表 | `swe-bench-smith-openhands@1.1.0` |

绑定读法（与对外示意图一致）：

```text
任务环境 swe@0.1.0
  + dataset = swe-bench-smith
  + EnvPackage swe-bench-smith@0.2.0
  + Agent Package uenv-agent-openhands@1.0.1
  → Episode Stack swe-bench-smith-openhands@1.1.0
```

---

## 1. 通用定义规范

### 1.1 三层分离（不要混）

```text
A. 平台（Platform）
   uenv-worker / uenv-server 二进制与代码；全 env_type 复用；随 Git 发版。
   任务环境的「交互实现」在这一层（Worker 运行时 + Gateway），不进 EnvPackage。

B. Hub 分发物（EnvPackage / Agent Package）
   版本化、内容寻址；节点 `uenv env sync` / `uenv agent-bridge sync` 预制。
   只带数据、配置、脚手架源码、镜像 tar —— 不带 gateway_url / session_id。

C. 运行时调度态（Server）
   每 Episode 不同：gateway_url、session_id、run_id、lease —— 不进 Hub 包。
```

### 1.2 对象角色（是什么 / 不是什么）

| 名称 | 是什么 | 不是什么 |
|---|---|---|
| **任务环境** | 能力抽象：`reset`/`step`/`state` 语义、奖励怎么算、`config_schema.dataset` 枚举 | 不是题目集；不是镜像包；不是 Agent |
| **EnvPackage（benchmark kind）** | 某契约下的题目 + 评测规格 +（可选）镜像字节 | 不是新的 `env_type`；`verified`/`pro`/`smith` 都是 **swe 的变体** |
| **Agent Package（agent_scaffold kind）** | 声明 `agent_defaults.agent_kind` 的包：接收观测、生成动作的脚手架 | 不是 Bridge 进程本身；`GET /agent-bridges` 只是脚手架的投影视图 |
| **Episode Stack** | 可运行配方：钉住契约 × 数据集 × 脚手架 × Gateway 要求 | 自身不含 catalog / tar 字节 |

### 1.3 标识与路由字段（协议名保持不变）

| 字段 | 含义 | 展示建议 |
|---|---|---|
| `env_type` / `version` | 任务环境标识，如 `swe@0.1.0` | 「环境契约」 |
| `dataset` / `benchmark_variant` | 契约内路由键；Stack 常用完整名 `swe-bench-smith`，overlay 常用短键 `smith` | 「数据集 / 变体」 |
| `package_id` / `version` | EnvPackage / Agent Package 标识 | 「环境包 / Agent 脚手架」 |
| `agent_kind` | 脚手架族，如 `openhands` | 「Agent 类型」 |
| `bundle_digest` | 制品集合内容寻址摘要 | 「包摘要」 |
| `stack_id` / `stack_digest` | 可运行组合及其钉版摘要 | 「Episode Stack」 |

> **展示名可变，协议字段不要为了文案擅自改名**（见概念对照表）。

### 1.4 制品形态通则

| kind | 典型文件名 | 形态 | 说明 |
|---|---|---|---|
| `catalog` | `catalog.json` | **声明式 JSON 文本**（可很大） | 实例目录；机器消费 |
| `images` | `images.manifest.json` | **声明式 JSON** | 镜像索引（digest / tar 相对路径）；**不是**镜像本体 |
| `image_tar` | `images/<name>.tar` | **二进制**（`docker save` 归档） | Hub 托管；Worker `docker load` |
| `eval_spec` | `eval_spec.json` | **声明式 JSON** | grader / workspace / 判分规则摘要 |
| `overlay` | `worker.overlay.yaml` | **文本**（内容常为合法 JSON） | Worker 配置覆盖；运行时主要读 manifest 内 `worker_overlay` |
| `other`（脚手架） | `*.py` / `MANIFEST.json` / `PIN.md` | **Python 源码** + 声明式元数据 | Agent Package 主体 |
| （任务环境） | — | **无独立制品目录** | 契约以 Hub env registry 的 JSON Schema 字段存在 |

工程取舍：机器消费制品优先 **JSON**（避免 Worker/Hub core 再引 YAML 解析）；`worker.overlay.yaml` 文件名带 yaml，内容可为 JSON。

### 1.5 与示意图的对齐说明

对外示意图里任务环境常写「实现：`src/env.py` / `models.py`」。在 **UEnv Hub 现状**下：

- 契约侧（Action/Observation/State、`config_schema`）登记在 Hub **`/api/v1/envs/{env_type}`**；
- 交互执行侧在 **Worker 平台代码**（Rust SWE runtime / Gateway）+ **实例容器镜像**；
- **不会**把一整份 OpenEnv 风格的 `env.py` 作为 Task Environment 的 Hub 制品下发。

EnvPackage / Agent Package 才是「文件夹 + 文件 / 二进制」意义上的分发单元。

---

## 2. swe-smith 绑定全景（实机）

| 角色 | 标识 | Hub API |
|---|---|---|
| 任务环境 | `swe@0.1.0` | `GET /api/v1/envs/swe/versions/0.1.0` |
| EnvPackage | `swe-bench-smith@0.2.0`（`kind=benchmark`，`env_type=swe`，`dataset=smith`，`instance_count=8226`） | `GET /api/v1/packages/swe-bench-smith/versions/0.2.0` |
| Agent Package | `uenv-agent-openhands@1.0.1`（`kind=agent_scaffold`，`agent_kind=openhands`） | `GET /api/v1/packages/uenv-agent-openhands/versions/1.0.1` |
| Episode Stack | `swe-bench-smith-openhands@1.1.0` | `GET /api/v1/episode-stacks/swe-bench-smith-openhands/versions/1.1.0` |

Stack 钉住字段（实机摘录）：

```json
{
  "stack_id": "swe-bench-smith-openhands",
  "version": "1.1.0",
  "execution_mode": "agent",
  "task_env": {
    "env_type": "swe",
    "version": "latest",
    "dataset": "swe-bench-smith"
  },
  "agent_scaffold": {
    "package_id": "uenv-agent-openhands",
    "version": "latest",
    "agent_kind": "openhands",
    "consumer": "openhands-agent"
  },
  "runtime_gateway": {
    "required": true,
    "api": "runtime/v1",
    "api_key_required": true
  },
  "env_packages": ["swe-bench-smith@0.2.0"],
  "required_worker_features": [
    "runtime_gateway",
    "swe_instance_pool",
    "trajectory_v2_2"
  ]
}
```

落盘根（Hub 主机）：

```text
/root/uenv/uenv-hub/data/artifacts/swe-bench-smith/0.2.0/     ≈ 15 GiB
/root/uenv/uenv-hub/data/artifacts/uenv-agent-openhands/1.0.1/  ≈ 100 KiB
```

任务环境 **没有**对应的 `artifacts/swe/` 目录。

---

## 3. 任务环境 `swe@0.1.0`：字段清单

### 3.1 形态

- **存储**：Hub SQLite（env 行 + version/manifest JSON），经 REST 暴露。
- **不是**：文件夹、OCI 镜像、Python 包目录。
- **实现落点**：Worker 容器后端 + Runtime Gateway；镜像字节来自 EnvPackage 的 `image_tar`。

### 3.2 注册表级字段

| 字段 | 实机值 / 说明 |
|---|---|
| `env_type` | `swe` |
| `version` | `0.1.0` |
| `description`（env 行） | SweEnv — 仓库级缺陷修复（Verified / Pro / Smith，容器内 FullShell） |
| `lifecycle` | Canonical（正式契约） |
| `compat_aliases` | 含 `swebench` |
| `entrypoint` | `null`（无进程入口；回合在实例镜像内，经 Gateway） |
| `supported_backends` | `["container"]` |
| `min_uenv_version` | `0.1.0` |
| `base_image` / `image` | 无（镜像按实例/仓库来自 EnvPackage） |

### 3.3 `config_schema`（路由与运行参数）

| 属性 | 类型 | 说明 |
|---|---|---|
| `dataset` | `string`，**required**；enum：`swe-bench-verified` / `swe-bench-pro` / `swe-bench-smith` | 与 EnvPackage id / Worker `benchmark_variant` 对齐的路由键 |
| `instance_id` | `string` | 如 `oauthlib__oauthlib.1fd52536.combine_file__09vlzwgc` |
| `command_mode` | `FullShell` \| `Restricted` | 默认 `FullShell` |
| `max_iterations` | `integer` ≥ 1 | Agent 多轮上限相关 |

`default_config`：`{"dataset":"swe-bench-verified","command_mode":"FullShell"}`。

`resources`：`cpu=4`，`memory_mb=8192`，`gpu=0`，`disk_mb=20480`。

### 3.4 `interface`（Action / Observation / State）

均为 **JSON Schema**（声明式文本，嵌在 manifest 里），标题：`SweAction` / `SweObservation` / `SweState`。

| 契约 | 要点字段 |
|---|---|
| **Action** | `type` ∈ `exec` / `write_file` / `read_file` / `apply_patch` / `submit`；另有 `command` / `path` / `content` / `patch` |
| **Observation** | `issue_text`、`stdout`、`stderr`、`exit_code`、`read_content`、`write_ok`、`truncated` |
| **State** | **required**：`instance_id`、`benchmark_variant`（enum 含三个 swe-bench-*）；另有 `base_commit`、`resolved`、`step_count` |

这就是示意图里「接口：动作 / 观测 / 状态」在 Hub 上的真实载体——**Schema，不是 `.py` 文件**。

---

## 4. EnvPackage `swe-bench-smith@0.2.0`：字段与制品

### 4.1 Manifest 顶层字段

| 字段 | 实机值 |
|---|---|
| `package_id` / `version` | `swe-bench-smith` / `0.2.0` |
| `publisher` | `org-uenv-swe` |
| `kind`（派生） | `benchmark` |
| `env_type` / `dataset`（派生） | `swe` / `smith` |
| `instance_count` | `8226`（10 仓完备；有效题面约 7668） |
| `platform.uenv_worker_min` | `0.1.0` |
| `platform.features` | `runtime_gateway`、`swe_instance_pool`、`trajectory_v2_2`、`hub_hosted_image_tar` |
| `platform.consumers` | `["worker"]` |
| `contracts` | `runtime_gateway_api=runtime/v1`，`trajectory_bundle_schema=v2.2`，`tool_bridge_schema=openhands-uenv-v1` |

说明：该版本 publish 时 manifest 内 `interface` 可为空对象；**契约权威在任务环境 `swe@0.1.0`**（及早期 smoke 种子包）。消费方应以 env registry `/interface` 为准做 RL 绑定。

### 4.2 `worker_overlay`（合并进 Worker 配置）

```json
{
  "swe": {
    "benchmark_variant": "smith",
    "command_mode": "FullShell",
    "grader": "swesmith",
    "image_pull_policy": "local_only",
    "load_images_from_package": true,
    "workspace_dir": "/testbed",
    "instance_count": 8226,
    "repo_count": 10,
    "image_count": 10,
    "package_scope": "repo-complete"
  },
  "runtime_gateway": { "enabled": true },
  "trajectory": {
    "enabled": true,
    "artifact_dir": "/var/lib/uenv/trajectories"
  }
}
```

要点：`image_pull_policy=local_only` + `load_images_from_package=true` → Worker **只**用本包 tar，`docker load`，零第三方 pull。

### 4.3 `agent_defaults`（给 AgentJob 的默认提示）

| 字段 | 值 | 说明 |
|---|---|---|
| `agent_bridge_id` | `uenv-agent-openhands` | 指向脚手架包 |
| `agent_bridge_version` | `1.0.0` | 历史钉版字符串；Hub latest 脚手架为 `1.0.1`（制品 digest 与 1.0.0 一致） |
| `driver_entrypoint` | `run_swesmith_official.py` | Smith 专用驱动文件名 |
| `workspace_dir` | `/testbed` | 与 Verified 的 `/app` 不同 |
| `tools` | `terminal`、`file_editor` | |
| `max_iterations_default` | `30` | |

> **注意**：Hub 已发布的 `uenv-agent-openhands@1.0.1` 制品列表里目前是 `run_swebench*.py` / `run_pro_agent.py`，**尚未把** `run_swesmith_official.py` 打进该包；仓库与 Hub 主机检出中存在 `integrations/openhands/run_swesmith_official.py`。联调时驱动可能来自 Agent 机本地 tree，而不只是 `agent-bridge sync` 目录——属已知缝隙，阅读 `agent_defaults` 时不要默认「包内一定有该文件」。

### 4.4 制品清单（14 项）

Hub 目录：`/root/uenv/uenv-hub/data/artifacts/swe-bench-smith/0.2.0/`

| 文件 | kind | 形态 | 约大小 | 内容 |
|---|---|---|---:|---|
| `catalog.json` | `catalog` | **JSON 文本**（巨型） | **569 MiB** | `{ instance_id: {…} }` 共 8226 条 |
| `images.manifest.json` | `images` | **JSON 文本** | 3.9 KiB | 10 仓镜像索引 + tar 名 / sha256 |
| `eval_spec.json` | `eval_spec` | **JSON 文本** | 231 B | grader / workspace / 判分摘要 |
| `worker.overlay.yaml` | `overlay` | **文本**（JSON 内容） | 498 B | 与 manifest `worker_overlay` 同构副本 |
| `swesmith.x86_64.*.tar` ×10 | `image_tar` | **二进制 tar** | **各约 1.2–1.9 GiB**，合计 ≈13.8 GiB | `docker save` 产物 |

#### `eval_spec.json`（全文级字段）

```json
{
  "grader": "swesmith",
  "workspace_dir": "/testbed",
  "log_parser": "pytest",
  "variant": "smith",
  "install_cmd": "pip install -e . -q",
  "scoring": "FAIL_TO_PASS 全部转通过且 PASS_TO_PASS 不回归，记 resolved"
}
```

#### `images.manifest.json`（条目结构）

每仓一项，例如：

| 字段 | 示例 |
|---|---|
| `repo` | `swesmith/oauthlib__oauthlib.1fd52536` |
| `image` | `jyangballin/swesmith.x86_64.oauthlib_1776_oauthlib.1fd52536:latest` |
| `tar_name` | `swesmith.x86_64.oauthlib_1776_oauthlib.1fd52536.tar` |
| `tar_sha256` | `sha256:cfcc89da…` |
| `tar_size_bytes` | `1336050176` |
| `instances` | `940` |

顶层另有：`variant=smith`，`hosted_by_hub=true`，`source=https://huggingface.co/datasets/SWE-bench/SWE-smith`。

#### `catalog.json` 单实例字段（实机抽样）

| 字段 | 含义 / 形态 |
|---|---|
| `instance_id` | 主键 |
| `repo` | 源仓，如 `swesmith/oauthlib__oauthlib.1fd52536` |
| `version` / `benchmark_variant` | `smith` |
| `problem_statement` | 题面文本 |
| `patch` | **造 bug 的 unified diff**（Worker provision 正向应用；gold 反向修复） |
| `test_patch` | 可为空 |
| `FAIL_TO_PASS` / `PASS_TO_PASS` | 测试名列表（JSON 数组） |
| `image_cache_key` | 对应 Docker 镜像名 |
| `install_cmd` / `test_cmd` | 安装与测试命令 |
| `base_commit` / `environment_setup_commit` | 可为空白字符串（Smith 镜像已 bake 环境） |

### 4.5 Worker 同步后的本地布局（规范）

```text
/var/lib/uenv/envs/swe-bench-smith/0.2.0/
├── catalog.json
├── images.manifest.json
├── eval_spec.json
├── worker.overlay.yaml
├── images/*.tar          # sync 时按 target_rel_path 落地
├── manifest.json         # 完整 EnvPackageManifest
└── .synced               # {package_id, version, bundle_digest, synced_at}
```

---

## 5. Agent Package `uenv-agent-openhands@1.0.1`：字段与制品

### 5.1 形态

- **是**：一个 EnvPackage，因 `agent_defaults.agent_kind` 非空被分类为 `agent_scaffold`。
- **目录**：`/root/uenv/uenv-hub/data/artifacts/uenv-agent-openhands/1.0.1/`（约 100 KiB 级文本）。
- **内容主体**：**Python 程序源码**（`text/plain` 入库）+ JSON/Markdown 元数据；**无**镜像 tar。

### 5.2 Manifest 关键字段

| 字段 | 实机值 |
|---|---|
| `package_id` / `version` | `uenv-agent-openhands` / `1.0.1` |
| `publisher` | `org-uenv-agent` |
| `platform.features` | `["runtime_gateway"]` |
| `platform.consumers` | `["openhands-agent"]`（Agent 机 sync，不是 Worker） |
| `agent_defaults.agent_kind` | `openhands` |
| `agent_defaults.driver_entrypoint` | `run_swebenchpro_official.py`（脚手架包默认；Smith 场景由 EnvPackage overlay 覆盖为 `run_swesmith_official.py`） |
| `agent_defaults.workspace_dir` | `/app`（脚手架默认；Smith 用 `/testbed`） |
| `agent_defaults.required_env_types` | `["swe"]` |
| `agent_defaults.tools` | `terminal`、`file_editor` |
| `contracts` | `runtime_gateway_api=runtime/v1`，`tool_bridge_schema=openhands-uenv-v1` |
| `bundle_digest`（agent-bridges 投影） | `sha256:7a0af55952705af445f9bdf32f0930563e7be37aed583fb598c34312cd31ab74` |

`1.0.1` 相对 `1.0.0`：补登记 `agent_kind` / `consumers` 等目录字段；**制品字节与 1.0.0 相同**，故已 sync 1.0.0 的 Agent 上报 digest 仍可对齐。

### 5.3 制品清单（10 项）

| Hub 制品名 | sync 相对路径 | 形态 | 作用 |
|---|---|---|---|
| `MANIFEST.json` | `MANIFEST.json` | JSON | 包元数据、`openhands_sdk_pin`、drivers 列表 |
| `PIN.md` | `PIN.md` | Markdown | SDK / 依赖钉版说明 |
| `uenv_runtime-client.py` | `uenv_runtime/client.py` | **Python** | Gateway / Server 客户端 |
| `uenv_runtime-workspace.py` | `uenv_runtime/workspace.py` | **Python** | 工作区绑定 |
| `uenv_runtime-gateway_tools.py` | `uenv_runtime/gateway_tools.py` | **Python** | 终端/文件工具桥 |
| `uenv_runtime-runtime.py` | `uenv_runtime/runtime.py` | **Python** | runtime 封装 |
| `uenv_runtime-agent_job.py` | `uenv_runtime/agent_job.py` | **Python** | AgentJob poll/complete |
| `drivers-run_swebenchpro_official.py` | `drivers/run_swebenchpro_official.py` | **Python** | Pro 官方驱动 |
| `drivers-run_swebench.py` | `drivers/run_swebench.py` | **Python** | Verified 类驱动 |
| `drivers-run_pro_agent.py` | `drivers/run_pro_agent.py` | **Python** | Pro agent 驱动 |

`MANIFEST.json` 要点：

```json
{
  "package_id": "uenv-agent-openhands",
  "version": "1.0.0",
  "openhands_sdk_pin": "1.27.0",
  "drivers": [
    "run_swebenchpro_official.py",
    "run_swebench.py",
    "run_pro_agent.py"
  ],
  "uenv_runtime_modules": [
    "client.py", "workspace.py", "gateway_tools.py", "runtime.py", "agent_job.py"
  ]
}
```

预制目标目录约定：`/opt/uenv/agent-bridges/uenv-agent-openhands/<version>/`（`uenv agent-bridge sync`）。

### 5.4 Agent Bridge 投影（不是第四种包）

`GET /api/v1/agent-bridges` 对声明了 `agent_kind` 的包做投影，字段与 `RegisterAgent.synced_agent_bridges` 对齐：

- `package_id` / `version` / `bundle_digest`
- `agent_kind` / `required_env_types` / `required_worker_features`

swe-smith 链路对应：`openhands` + `required_env_types=["swe"]`。

---

## 6. 三者如何拼成一次可运行 Episode

```text
1. Stack 解析
   task_env.dataset=swe-bench-smith
     → 必须落在 swe.config_schema.dataset 枚举内
   env_packages=["swe-bench-smith@0.2.0"]
     → Worker sync catalog + eval_spec + image_tar
   agent_scaffold=uenv-agent-openhands@latest
     → Agent 机 sync Python 脚手架；RegisterAgent 上报 bundle_digest

2. Server 调度（C 层）
   选 Worker（含 runtime_gateway / swe_instance_pool）
   DispatchEpisode(env_type=swe, …)
   创建 AgentJob：注入 gateway_url、session_id、driver 提示、workspace=/testbed

3. Worker
   从 EnvPackage 选 instance → docker load 对应仓 tar → provision
   Smith：正向 apply 造 bug patch；评测 grader=swesmith

4. Agent
   经 Gateway 收观测、发动作（exec/write/read/apply_patch/submit）
   驱动脚本以 EnvPackage agent_defaults 为准（smith → run_swesmith_official.py）
```

职责边界：

| 谁 | 提供什么 |
|---|---|
| 任务环境 | 「怎么交互、怎么路由 dataset」的契约 |
| EnvPackage | 「考哪些题、用哪张镜像、怎么判分」的数据与二进制 |
| Agent Package | 「谁来答题」的 Python 脚手架与工具桥 |
| Stack | 把上面三者钉成可解析、可复现的配方 |
| Server | 运行时 URL / lease（不进包） |

---

## 7. 实机核对命令（Hub）

```bash
# SSH：secrets/README.md → root@8.130.95.176，密码见文档（勿提交 Token）
TOKEN=$(cat /root/uenv/uenv-hub/data/.admin_token)
H=http://127.0.0.1:8088

curl -s -H "Authorization: Bearer $TOKEN" "$H/api/v1/envs/swe/versions/0.1.0" | jq .
curl -s -H "Authorization: Bearer $TOKEN" \
  "$H/api/v1/packages/swe-bench-smith/versions/0.2.0" \
  | jq '{package_id,version,platform,worker_overlay,agent_defaults,artifacts:[.artifacts[]|{name,kind,size_bytes,target_rel_path}]}'
curl -s -H "Authorization: Bearer $TOKEN" \
  "$H/api/v1/packages/uenv-agent-openhands/versions/1.0.1" \
  | jq '{package_id,version,agent_defaults,artifacts:[.artifacts[]|{name,target_rel_path,size_bytes}]}'
curl -s -H "Authorization: Bearer $TOKEN" \
  "$H/api/v1/episode-stacks/swe-bench-smith-openhands/versions/1.1.0" | jq .

ls -lah /root/uenv/uenv-hub/data/artifacts/swe-bench-smith/0.2.0/ | head
du -sh /root/uenv/uenv-hub/data/artifacts/swe-bench-smith/0.2.0
```

---

## 8. 速查：形态判断口诀

```text
任务环境 swe
  = Hub 注册表里的契约 JSON（Schema + config_schema）
  ≠ 文件夹；≠ 镜像；≠ catalog

EnvPackage swe-bench-smith
  = 版本目录
  = 大 JSON（题目）+ 小 JSON（评测/索引/overlay）+ 大 tar（镜像二进制）

Agent Package uenv-agent-openhands
  = 版本目录
  = 小 JSON/Markdown + 一堆 .py 源码
  ≠ 容器镜像

Episode Stack swe-bench-smith-openhands
  = 只引用、不装字节的配方
```

---

## 9. 文档元数据

| 项 | 值 |
|---|---|
| Hub | `8.130.95.176:8088` |
| 核对日 | 2026-08-25 |
| EnvPackage | `swe-bench-smith@0.2.0`（14 artifacts，≈15 GiB） |
| Agent Package | `uenv-agent-openhands@1.0.1`（10 artifacts，≈100 KiB） |
| Task Environment | `swe@0.1.0` |
| Stack | `swe-bench-smith-openhands@1.1.0` |
| 通用规范来源 | Hub IA 文档、`uenv-hub-types` EnvPackage / PackageKind、环境标准化指南、seed `swe_manifest` / `seed_agent_bridge_openhands` |
