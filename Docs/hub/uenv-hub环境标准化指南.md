# UEnvHub 标准化环境指南（EnvPackage 环境组合包）

> **版本**：v1.0 ｜ **日期**：2026-06-30
> **状态**：已实现并本地端到端联调通过
> **实现依据**：`Docs/260629-hub-env-package-design.md`（方案冻结 v1.1）
> **代码**：`uenv-hub/*`（types/core/server/client）、`uenv-worker/src/swe/{env_package,image_cache}.rs`、`uenv-worker/src/runtime.rs`
> **参考**：[HuggingFace OpenEnv](https://github.com/huggingface/OpenEnv)（Environment 契约：typed Action/Observation/State）

本指南定义 UEnvHub 把环境**封装为可版本化、内容寻址的组合包（EnvPackage）并分发**的标准方法：Hub 一次性提供「目录 + 镜像索引 + 评测规格 + Worker 配置 overlay + Agent 桥接引用 + 平台约束」的完整环境，Worker/Agent 节点经 `uenv env sync` 预制到本地，**运行时不再从第三方重新拉取**。

---

## 1. 为什么需要 EnvPackage

此前 Hub 只是「元数据 + SWE catalog JSON」索引站：Worker 启动时按变体 `GET /api/v1/swe/{variant}/instances` 拉一份实例目录，**镜像仍由 Worker 自行从第三方 registry 拉取**，grader/variant/command_mode/池/轨迹等配置散落各机 yaml/env，无法作为「同一个可版本化、可回滚的环境整体」发布。

EnvPackage 把这些收敛成一个 `package_id@version` 发布单元，带来：

- **可复现**：每个制品 `sha256`，整包 `bundle_digest`；同一版本任何节点拉到的字节一致。
- **离线可控**：镜像由 digest 索引，`image_pull_policy=local_only` 时 Worker 只用本地镜像、miss 即失败、零公网 egress。
- **可回滚**：catalog、overlay、grader 配置、Agent 桥接版本随同一包发布/回滚。

### 1.1 科学严谨的封装方法（设计原则）

| 原则 | 落地 |
|------|------|
| **内容寻址** | 每制品 `sha256:<hex>`，读取/同步双向校验；`.synced` 写 `bundle_digest`（对排序后的 `name=digest` 取 sha256） |
| **不可变版本** | `package_id@version` 唯一；重复发布同版本被拒（`409`） |
| **组合非单体** | 一个包引用多类制品（catalog / images.manifest / eval_spec / overlay），各自独立 digest；**不**打成一个大 OCI 镜像 |
| **三层分离** | 平台代码（Gateway/池/grader/轨迹）随 Worker 发版，不进包；包只带数据与配置；`gateway_url`/`session_id` 等调度态不入 Hub |
| **镜像字节不进 Hub** | Hub 存 manifest + 小制品（含 `images.manifest.json` 的 digest 索引）；镜像本体走内网 registry/tarball，由 digest 引用（设计 §2.1 / §12，对象存储为 P2） |
| **OpenEnv 对齐** | 复用 Hub 既有 `InterfaceSchema`（action/observation/state）作为环境契约；EnvPackage 是其上的「可复现分发层」 |

### 1.2 三层模型

```
A. 平台（Platform Release）   uenv-worker/uenv-server 二进制；全 env_type 复用；随 Git tag
            ▲ 读取 EnvPackage 配置与制品
B. 环境组合包（Hub EnvPackage） 本指南；版本化；节点 `uenv env sync` 预制
            ▲ 任务调度时注入
C. 运行时调度态（Server 控制面） 每 Episode 不同：gateway_url、session_id、run_id、租约
```

---

## 2. EnvPackage Manifest 结构

`GET /api/v1/packages/{package_id}/versions/{version}` 返回的权威 manifest（`EnvPackageManifest`，`uenv-hub-types`）：

```jsonc
{
  "package_id": "swe-bench-verified",
  "version": "1.0.0",
  "published_at": 1782787910,
  "publisher": "org-uenv-swe",
  "changelog": "…",
  "platform": {                       // A 层约束
    "uenv_worker_min": "0.1.0",
    "uenv_server_min": null,
    "features": ["runtime_gateway", "swe_instance_pool"]
  },
  "artifacts": [                      // B 层制品（内容寻址）
    {
      "name": "catalog.json",
      "kind": "catalog",              // catalog|images|eval_spec|overlay|agent_bridge|other
      "url": "/api/v1/packages/swe-bench-verified/versions/1.0.0/artifacts/catalog.json",
      "digest": "sha256:7e51ed12…",
      "size_bytes": 35913,
      "sync_mode": "inline",          // inline(Hub 下发) | registry | tarball | rsync
      "media_type": "application/json",
      "target_rel_path": "catalog.json"
    }
    // images.manifest.json / eval_spec.json / worker.overlay.yaml …
  ],
  "worker_overlay": {                 // 合并进 Worker 配置（开放 schema）
    "swe": { "benchmark_variant": "verified", "command_mode": "FullShell",
             "grader": "swebench", "image_pull_policy": "local_only" },
    "runtime_gateway": { "enabled": true },
    "trajectory": { "enabled": true, "artifact_dir": "/var/lib/uenv/trajectories" }
  },
  "agent_defaults": {                 // Agent Job 模板默认（开放 schema）
    "driver_entrypoint": "run_swebench.py", "workspace_dir": "/app",
    "tools": ["terminal", "file_editor"], "max_iterations_default": 30
  },
  "contracts": {                      // 接口契约版本（非运行时 URL）
    "runtime_gateway_api": "runtime/v1",
    "trajectory_bundle_schema": "v2.2",
    "tool_bridge_schema": "openhands-uenv-v1"
  }
}
```

### 2.1 标准制品（kind）

| kind | 文件名（target_rel_path） | 内容 |
|------|---------------------------|------|
| `catalog` | `catalog.json` | 实例目录，`{ instance_id: { … } }`，与 Worker `InstanceStore::from_json` 同构 |
| `images` | `images.manifest.json` | `{schema, variant, pull_policy, images:[{instance_id,image,digest}]}` —— 镜像 digest 索引；`digest` 为空表示未 pin |
| `eval_spec` | `eval_spec.json` | `{grader, log_parser, variant}` |
| `overlay` | `worker.overlay.yaml` | `worker_overlay` 的副本，供运维合并进 Worker yaml；内容为 JSON（JSON 是合法 YAML） |

> **说明（与设计文档的工程取舍）**：机器消费的制品统一用 **JSON**（`eval_spec.json` 而非 `.yaml`），避免在 core/worker 引入 YAML 解析；`worker.overlay.yaml` 文件内是 JSON 文本（合法 YAML），既能被 YAML 工具读取，Worker 也直接从 `manifest.json` 的 `worker_overlay` 字段消费，无需解析 YAML。

---

## 3. HTTP API

| 方法 | 路径 | 角色 | 说明 |
|------|------|------|------|
| `GET` | `/api/v1/packages` | reader | 分页列出包（`?page=&per_page=`） |
| `POST` | `/api/v1/packages/{package_id}/versions` | publisher | 发布一个版本（inline 制品，见 §4） |
| `GET` | `/api/v1/packages/{package_id}/versions/{version}` | reader | 完整 manifest；`version=latest` 解析到最新非 yank 版本 |
| `GET` | `/api/v1/packages/{package_id}/versions/{version}/sync-plan` | reader | 确定性同步计划（文件列表 + digest + `bundle_digest`） |
| `GET` | `/api/v1/packages/{package_id}/versions/{version}/artifacts/{name}` | reader | 制品字节流；`ETag` = digest，读时按 digest 复校 |

错误用统一 `ErrorResponse` 信封（见 `uenv-hub/docs/errors.md`）。鉴权同其它端点（Bearer Token；`require_token=false` 时全部视为 admin）。

---

## 4. 发布（Publish）

`PublishPackageRequest`：每个 inline 制品携带 `content`（UTF-8 文本）或 `content_b64`（二进制）；服务端落盘到内容寻址存储、计算 `sha256`、组装 manifest。示例：

```bash
curl -X POST http://<hub>/api/v1/packages/swe-bench-verified/versions \
  -H 'Authorization: Bearer <publisher-token>' -H 'Content-Type: application/json' \
  -d '{
    "version": "1.0.0",
    "publisher": "org-uenv-swe",
    "platform": {"uenv_worker_min": "0.1.0", "features": ["runtime_gateway","swe_instance_pool"]},
    "worker_overlay": {"swe": {"benchmark_variant": "verified", "grader": "swebench", "image_pull_policy": "local_only"}},
    "contracts": {"runtime_gateway_api": "runtime/v1"},
    "artifacts": [
      {"name":"catalog.json","kind":"catalog","media_type":"application/json","content":"{…}"},
      {"name":"images.manifest.json","kind":"images","media_type":"application/json","content":"{…}"}
    ]
  }'
```

落盘位置由 Hub 配置 `packages.artifact_dir` 决定：`<artifact_dir>/<package_id>/<version>/<name>`。

### 4.1 种子包（开箱即用）

Hub 启动时（`packages.seed_examples=true`，默认开）从 `packages.catalog_seed_dir`（默认 `config/swe`）幂等种子两个包：

- **`swe-bench-verified@1.0.0`**：来自 `config/swe/verified.json`（10 个真实 Verified 实例）；`images.manifest.json` 自动按 Worker 命名规则推导镜像（`swebench/sweb.eval.x86_64.<id 的 __→_1776_>:latest`）。
- **`swe-bench-pro@0.1.0`**：来自 `config/swe/pro.json`。**诚实标注**：当前 Pro catalog 只含**占位样例** `swe-pro__example-go-1`（base_commit 全 0、镜像指 `registry.example.com`），用于验证封装/路由/grader 选择；**真实 Pro 容器评测需内网 Pro registry 镜像**（离线不可达）。

---

## 5. 同步（`uenv env sync`）

```bash
uenv --endpoint http://<hub> env sync swe-bench-verified \
  --version latest --target-dir /var/lib/uenv --worker-version 0.2.0
# --dry-run 仅打印 sync-plan；--worker-version 校验 platform.uenv_worker_min
```

行为：拉 manifest → 校验平台 → 逐 inline 制品下载并**校验 sha256** → 写入 `<target>/envs/<pkg>/<ver>/<target_rel_path>` → 额外写 `manifest.json` 与 `.synced`。本地布局：

```
/var/lib/uenv/envs/swe-bench-verified/1.0.0/
├── catalog.json
├── images.manifest.json
├── eval_spec.json
├── worker.overlay.yaml
├── manifest.json          # 完整 manifest（Worker 读 worker_overlay 由此）
└── .synced                # {package_id, version, bundle_digest, synced_at}
```

镜像字节不由 `sync` 下载（`sync_mode != inline` 的制品打印指引跳过）；镜像经内网 registry 批量 `pull`/`docker load`，按 `images.manifest.json` 的 digest 核对。

---

## 6. Worker 消费

Worker 启动（`serve`）时，若指定了已同步的包目录，**catalog 最高优先级从该目录加载**，不再走 Hub/本地回退：

```bash
export UENV_SWE_ENV_PACKAGE=/var/lib/uenv/envs/swe-bench-verified/1.0.0
export UENV_SWE_IMAGE_PULL_POLICY=local_only      # 强制只用本地镜像
uenv-worker --config config/uenv-worker.swe-local.yaml serve
# 期望日志：msg="swe_catalog_loaded_from_env_package" count=10 images=10 image_pull_policy=Some(LocalOnly)
```

| 配置 | 含义 |
|------|------|
| `swe.env_package_dir` / `UENV_SWE_ENV_PACKAGE` | 已同步包目录；设置后从 `<dir>/catalog.json` 加载，读 `<dir>/manifest.json` 的 `worker_overlay`（variant/pull policy），读 `<dir>/images.manifest.json`（digest 索引） |
| `UENV_SWE_IMAGE_PULL_POLICY` | `local_only`/`mirror`/`allow_public`；`local_only` 时 `ImageCacheFactory` 不 pull、miss 清晰报错；并提供 `verify_local_digest`（`image inspect --format '{{index .RepoDigests 0}}'`）按 `images.manifest.json` 校验 |
| `UENV_SWE_IMAGE_PULL`（旧） | 布尔，兼容保留（`0` ≈ local_only） |

> Worker **不会**在运行时改写进程环境变量（edition 2024 下 `set_var` 不安全）。`worker_overlay` 声明的 `image_pull_policy=local_only` 仅作运维提示；要真正强制，请显式 `export UENV_SWE_IMAGE_PULL_POLICY=local_only`（`uenv env sync` 输出已写入 `worker.overlay.yaml` 供合并）。

---

## 7. Hub 配置（`PackagesConfig`）

figment：默认 < TOML < 环境变量（`UENV_HUB_` 前缀，`__` 分隔）。

| 字段 | 默认 | 环境变量 | 说明 |
|------|------|----------|------|
| `packages.artifact_dir` | `data/artifacts` | `UENV_HUB_PACKAGES__ARTIFACT_DIR` | 制品内容寻址存储根 |
| `packages.catalog_seed_dir` | `config/swe` | `UENV_HUB_PACKAGES__CATALOG_SEED_DIR` | 种子读取 `<variant>.json` 的目录 |
| `packages.seed_examples` | `true` | `UENV_HUB_PACKAGES__SEED_EXAMPLES` | 启动是否种子示例包（幂等、容错） |

---

## 8. 远程四端联调 Runbook（对照 `Docs/README.md`）

> 机器：Hub `8.130.95.176:8088`、Worker 7143（`219.147.100.43`）、Server/Adapter `8.130.75.157:8088`、VeRL 7142。

**① Hub（8.130.95.176）发布/确认包**
```bash
# Hub 进程已带 packages.artifact_dir、catalog_seed_dir=config/swe、seed_examples=true
curl -s -H "Authorization: Bearer $UENV_HUB_TOKEN" \
  http://8.130.95.176:8088/api/v1/packages | python3 -m json.tool
curl -s -H "Authorization: Bearer $UENV_HUB_TOKEN" \
  http://8.130.95.176:8088/api/v1/packages/swe-bench-verified/versions/latest/sync-plan
```

**② Worker（7143）同步并启动**
```bash
export UENV_HUB_TOKEN=...   # 见 README §1.5（勿提交仓库）
uenv --endpoint http://8.130.95.176:8088 env sync swe-bench-verified \
  --target-dir /var/lib/uenv --worker-version <worker 版本>
export UENV_SWE_ENV_PACKAGE=/var/lib/uenv/envs/swe-bench-verified/1.0.0
export UENV_SWE_IMAGE_PULL_POLICY=local_only
UENV_WORKER_ALLOW_DEGRADED_START=1 \
  ./target/release/uenv-worker --config config/uenv-worker.swe-local.yaml serve &
# 日志期望：swe_catalog_loaded_from_env_package count=N images=N
```

**③ 跑评测（7143 本地已缓存 500 个 Verified 镜像）**
```bash
uenv-worker swe-dispatch --endpoint 127.0.0.1:38888 \
  --instance scikit-learn__scikit-learn-14141 --gold true   # 期望 reward=1.0
```
真实 reward=1.0 依赖 7143 本机镜像与既有 SWE 链路；EnvPackage 的职责是把 catalog/overlay/镜像索引**完整下发**，使 Worker 无需再从第三方拉取目录或镜像。

> 我无法 SSH 登录你们的机器，上述远程步骤需你方执行；本地端到端（publish→sync→worker 加载，§9）已验证通过。

---

## 9. 本地端到端验证（已通过）

```
hub-server（temp DB + artifact_dir + catalog_seed_dir=config/swe, seed_examples=true）
  → GET /packages 返回 swe-bench-verified / swe-bench-pro
  → uenv env sync swe-bench-verified --target-dir /tmp/uenv-e2e/sync
      catalog.json 独立 shasum == manifest digest（sha256:7e51ed12…）✓
      images.manifest.json: 10 images, pull_policy=local_only ✓
      .synced.bundle_digest == sync-plan.bundle_digest ✓
  → uenv-worker serve（UENV_SWE_ENV_PACKAGE=…）
      日志 swe_catalog_loaded_from_env_package count=10 images=10 image_pull_policy=Some(LocalOnly) ✓
```
自动化测试：`uenv-hub-core` 9 + `uenv-hub-server` e2e 5（含 `env_package_publish_manifest_artifact_and_sync_plan`）+ `uenv-worker` lib 93（含 `image_cache`/`env_package` 新增 3）全绿。

---

## 10. 范围与遗留（诚实登记）

| 项 | 状态 |
|----|------|
| Hub manifest + 内容寻址制品 + publish/get/sync-plan/artifact API | ✅ 已实现并联调 |
| `uenv env sync` + Worker 消费（catalog + overlay + local_only digest 校验） | ✅ 已实现并联调 |
| 镜像字节对象存储 / registry 推送 | ❌ 遗留（设计 P2；当前 digest 索引 + 内网 registry/tarball 带外） |
| `uenv agent-bridge sync` 独立 Agent 桥接包发布 | ❌ 遗留（设计 P1；当前 manifest 仅 `agent_defaults` 引用） |
| Server `AgentJob.gateway_url` 字段冻结（C 层调度下发） | ❌ 遗留（设计 P1） |
| 真实 SWE-bench Pro 容器评测 | ❌ 遗留（占位样例 + 离线无 Pro registry 镜像） |

---

## 11. 关键代码位置

| 关注点 | 位置 |
|--------|------|
| DTO（manifest/请求/sync-plan） | `uenv-hub/uenv-hub-types/src/lib.rs`（EnvPackage 段） |
| 制品落盘 + digest + 装配 | `uenv-hub/uenv-hub-core/src/package.rs` |
| DB schema | `uenv-hub/migrations/0002_env_packages.sql` |
| 仓储（publish/get/list/artifact_meta） | `uenv-hub/uenv-hub-core/src/repository.rs` |
| 种子（swe-bench-verified/pro） | `uenv-hub/uenv-hub-core/src/seed.rs::seed_packages` |
| 服务编排 + 路由 + 配置 | `uenv-hub/uenv-hub-server/src/{service,routes,config}.rs` |
| 客户端 SDK + `uenv env sync` | `uenv-hub/uenv-hub-client/src/{client.rs,bin/uenv.rs}` |
| Worker 消费 | `uenv-worker/src/swe/env_package.rs`、`runtime.rs::load_swe_catalog`、`swe/image_cache.rs::ImagePullPolicy` |
