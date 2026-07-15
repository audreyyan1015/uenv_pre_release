# UEnv-Hub 服务接口对接文档（联调版）

> 面向：L2 Worker、CLI/运维、以及任何需要读取/发布环境制品的层。
> 角色：UEnv-Hub = **内网环境预缓存与制品分发中心**（类比 Docker Hub / HF Hub，但托管 **manifest + 制品字节**）。
> **不参与 Episode 运行时调度**；在 **部署/扩缩容期** 向 Worker 提供 manifest、镜像 tar、benchmark 包、catalog 等，使 Worker **无需访问公网** 即可完成环境预制。

> **EnvPackage 与零 egress 能力**（镜像 tar 入库、`uenv env sync --docker-load`、SWE catalog 等）见 **[uenv-hub环境标准化指南.md](./uenv-hub环境标准化指南.md)**。本文侧重 REST API 与经典 env registry；**内网生产不应仅使用 manifest 同步而忽略制品字节**。

---

## 1. 服务接入信息

| 项 | 值 |
|----|-----|
| 协议 | HTTP REST |
| 监听地址 | `0.0.0.0:8088`（绑定全网卡，外部可达） |
| **Base URL（其他服务器对接，公网 EIP）** | **`http://8.130.95.176:8088`** |
| Base URL（同 VPC 内网，可选） | `http://192.168.0.133:8088` |
| Content-Type | `application/json` |
| 鉴权 | **已开启**（`auth.require_token=true`），单一共享 Bearer Token |
| 数据库 | SQLite（WAL）`/root/uenv/uenv-hub/data/hub.db` |

> **端口说明**：四机联调文档中 Hub 默认写的是 `8080`，但本机仅开放
> `8000 / 8077 / 8088 / 8099`，因此 Hub 实际使用 **8088**。其他层对接时请把
> 文档里的 `:8080` 替换为 `:8088`，例如 `UENV_HUB_ENDPOINT=http://8.130.95.176:8088`。

## 2. 鉴权（单一共享 Token，最简方案）

服务已公网暴露，为阻断匿名越权，启用了 Token 鉴权（`require_token=true`）。
为尽量简化，采用**单一共享 Bearer Token**：所有层（Worker / Server / CLI / 运维）
都使用同一个 token，无需按角色分发多 token。

- **公开端点不需要 token**：`GET /healthz`、`GET /metrics`、`GET /version`
  （探活 / 监控 / 版本，照常可用）。
- **`/api/v1/**` 全部需要 token**（读和写）。无 token 或 token 无效返回 `401 UNAUTHORIZED`。
- 两种携带方式任选其一：

```
Authorization: Bearer uenvh_xxxxxxxx
X-Api-Token: uenvh_xxxxxxxx
```

**Token 获取**：保存在 Hub 主机 `/root/uenv/uenv-hub/data/.admin_token`（权限 600）。
运维向各层下发同一字符串即可。该 token 为 admin 角色，覆盖读 / 发布 / 运维全部操作，
因此单一 token 即可满足所有层。

> 安全说明：token 不写入仓库内的 `config/hub.prod.toml`，而是通过环境变量
> `UENV_HUB_AUTH__BOOTSTRAP_ADMIN_TOKEN` 在首次启动注入（见 `scripts/start-hub.sh`）；
> 首次创建后已持久化进 SQLite，后续重启即使不带该变量也仍生效。
> 如需轮换：删除该 token（`DELETE /api/v1/admin/tokens/{id}`）或重置 DB 后用新 token 重新 bootstrap。

各层环境变量示例：

```bash
export UENV_HUB_ENDPOINT=http://8.130.95.176:8088
export UENV_HUB_TOKEN=uenvh_xxxxxxxx        # 与 data/.admin_token 内容一致
```

---

## 3. 路由全集

| # | 方法 | 路径 | 角色 | 说明 |
|---|------|------|------|------|
| 1 | GET | `/healthz` | public | 探活（含 DB 状态） |
| 2 | GET | `/metrics` | public | Prometheus 文本 |
| 3 | GET | `/version` | public | 版本信息 |
| 4 | GET | `/api/v1/envs` | reader | 环境列表（分页/过滤） |
| 5 | GET | `/api/v1/envs?since={unix秒}` | reader | 增量同步（Server 用） |
| 6 | GET | `/api/v1/envs/{env_type}` | reader | 环境详情 |
| 7 | GET | `/api/v1/envs/{env_type}/versions` | reader | 版本列表 |
| 8 | GET | `/api/v1/envs/{env_type}/versions/{version}` | reader | 指定版本 manifest |
| 9 | GET | `/api/v1/envs/{env_type}/versions/latest` | reader | **Worker 主路径** |
| 10 | GET | `/api/v1/envs/{env_type}/resolve?constraint=` | reader | 语义化版本解析（如 `^1.0`） |
| 11 | GET | `/api/v1/envs/{env_type}/versions/{version}/interface` | reader | interface JSON Schema |
| 12 | GET | `/api/v1/envs/{env_type}/versions/{version}/examples` | reader | 示例 |
| 13 | GET | `/api/v1/search?q=` | reader | 搜索 |
| 14 | POST | `/api/v1/envs` | publisher | 创建环境 |
| 15 | POST | `/api/v1/envs/{env_type}/versions` | publisher | 发布版本 |
| 16 | PATCH | `/api/v1/envs/{env_type}` | publisher | 更新元数据 |
| 17 | POST | `/api/v1/envs/{env_type}/versions/{version}/yank` | publisher | 下架版本 |
| 18 | DELETE | `/api/v1/envs/{env_type}` | admin | 删除环境 |
| 19 | GET | `/api/v1/templates` | reader | 模板列表 |
| 20 | GET | `/api/v1/templates/{name}/archive` | reader | 模板 gzip 包 |
| 21 | POST | `/api/v1/admin/tokens` | admin | 创建 Token |
| 22 | DELETE | `/api/v1/admin/tokens/{id}` | admin | 吊销 Token |
| 23 | GET | `/api/v1/admin/audit-log` | admin | 审计日志 |

常用 Query 参数：

| 端点 | 参数 |
|------|------|
| `GET /api/v1/envs` | `page`, `per_page`, `namespace`, `author`, `tag`, `since` |
| `GET .../resolve` | `constraint`（如 `^1.0`、`1.0.0`） |
| `GET /api/v1/search` | `q`, `tag`, `author`, `namespace`, `page`, `per_page` |

---

## 4. Worker 对接（核心热路径）

Worker 启动 / spawn 插件前，从 Hub 拉取环境 manifest；**失败可降级本地**
`plugins/<env>/manifest.yaml`，不阻塞 Episode 热路径。

Worker 侧环境变量：

```bash
export UENV_HUB_ENDPOINT=http://8.130.95.176:8088   # 公网 EIP，跨服务器可达
export UENV_HUB_ENABLED=true
export UENV_HUB_TOKEN=uenvh_xxxxxxxx                 # 单一共享 token（见 §2）
```

主路径请求（需带 token）：

```bash
curl -H "Authorization: Bearer $UENV_HUB_TOKEN" \
  http://8.130.95.176:8088/api/v1/envs/math/versions/latest
```

返回 `FullManifest`（实测响应）核心字段：

| 字段 | 类型 | Worker 使用 |
|------|------|-------------|
| `env_type` | string | ✅ 调度键 |
| `version` | string | ✅ |
| `entrypoint` | string? | 参考（spawn 优先本地 `./run.sh`） |
| `supported_backends` | string[] | ✅ 如 `["process","podman"]` |
| `image` | ImageSpec? | 制品索引（`url/digest/size_bytes/arch/base_image_ref`） |
| `config_schema` | JSON Schema? | payload 约束 |
| `default_config` | JSON? | 默认配置 |
| `resources` | object | `cpu / memory_mb / gpu / gpu_type / disk_mb` |
| `interface` | object | `action / observation / state`（均 JSON Schema） |
| `examples` | array | 示例 |
| `is_yanked` / `yank_reason` | bool / string? | 下架标记 |
| `published_at` | int64 | Unix 秒 |

> Hub 拉取失败时，Worker 记录 `hub_pull_failed_using_local_manifest` 并使用本地 manifest。

---

## 5. 已注册环境（seed 默认数据）

| env_type | latest_version | tags | 备注 |
|----------|----------------|------|------|
| `math` | `1.0.0` | math, reasoning | 含完整 interface/config schema/image |
| `code` | `1.0.0` | code, execution | 代码执行奖励环境 |
| `agent` | `0.1.0` | agent, multi-turn | 多轮工具调用环境 |

模板（scaffold）：`math` / `code` / `agent` / `echo`。

---

## 6. 响应 DTO 速查

**`GET /healthz`** → `{ "status": "ok", "db": "up" }`

**`GET /version`** → `{ "name": "uenv-hub", "version": "0.1.0", "git_sha": null }`

**`EnvSummary`**（列表项）：`env_type`, `namespace`, `description?`, `author?`,
`latest_version?`, `tags[]`, `created_at`, `updated_at`（Unix 秒）。

**`EnvDetail`**：`EnvSummary` + `homepage?`, `repository?`, `license?`, `latest_manifest?`。

**`FullManifest`**：见 §4 字段表。

**`VersionSummary`**（版本列表项）：`version`, `changelog?`, `is_yanked`, `published_at`。

**`SearchResponse`**：`results[]`(EnvSummary), `total`, `page`, `per_page`。

**发布请求 `PublishVersionRequest`**：`version`, `changelog`, `image`, `base_image`,
`health_check_path`, `entrypoint`, `supported_backends`, `config_schema`,
`default_config`, `resources`, `interface`, `examples`, `dependencies`, `min_uenv_version`。

**发布响应 `PublishVersionResponse`**：`env_type`, `version`, `published_at`, `manifest_url`。

**`CreateEnvRequest`**：`env_type`, `namespace?`, `description?`, `author?`,
`homepage?`, `repository?`, `license?`, `tags[]`。

---

## 7. 错误响应

所有非 2xx 使用统一信封：

```json
{
  "error": { "code": "NOT_FOUND", "message": "...", "details": {} },
  "request_id": "req_abc123"
}
```

| code | HTTP | 含义 |
|------|------|------|
| `UNAUTHORIZED` | 401 | token 缺失/无效 |
| `FORBIDDEN` | 403 | 角色/命名空间不允许 |
| `NOT_FOUND` | 404 | 环境/版本/Token 不存在 |
| `VERSION_ALREADY_EXISTS` | 409 | 版本已发布（不可覆盖） |
| `ENV_ALREADY_EXISTS` | 409 | 环境已存在 |
| `CONFLICT` | 409 | 其他唯一性/状态冲突 |
| `INVALID_MANIFEST` | 422 | manifest 结构非法 |
| `INVALID_VERSION` | 422 | 非合法 semver |
| `INVALID_CONSTRAINT` | 422 | 版本约束无法解析 |
| `SCHEMA_VALIDATION_FAILED` | 422 | config/interface JSON Schema 校验失败 |
| `RATE_LIMITED` | 429 | 超过限流 |
| `INTERNAL_ERROR` | 500 | 内部错误 |

> 关注 `error.code`（稳定机读标识），`message` 可能变化。每个响应带 `x-request-id` 头便于对日志。

---

## 8. curl 示例

```bash
HUB=http://8.130.95.176:8088   # 公网 EIP；同 VPC 内网可用 http://192.168.0.133:8088
TOKEN=uenvh_xxxxxxxx           # 单一共享 token（Hub 主机 data/.admin_token）
AUTH="Authorization: Bearer $TOKEN"

# 探活 / 版本（公开，无需 token）
curl $HUB/healthz
curl $HUB/version

# 列表 / 详情 / 版本（需 token）
curl -H "$AUTH" $HUB/api/v1/envs
curl -H "$AUTH" $HUB/api/v1/envs/math
curl -H "$AUTH" $HUB/api/v1/envs/math/versions

# Worker 主路径：拉取 latest manifest
curl -H "$AUTH" $HUB/api/v1/envs/math/versions/latest

# 仅取 interface schema
curl -H "$AUTH" $HUB/api/v1/envs/math/versions/1.0.0/interface

# 语义化版本解析
curl -H "$AUTH" "$HUB/api/v1/envs/math/resolve?constraint=^1.0"

# 搜索
curl -H "$AUTH" "$HUB/api/v1/search?q=math"

# 增量同步（Server 用，since 为 Unix 秒）
curl -H "$AUTH" "$HUB/api/v1/envs?since=0"

# 发布版本
curl -X POST -H "$AUTH" $HUB/api/v1/envs/math/versions \
  -H 'Content-Type: application/json' \
  -d '{"version":"1.1.0","supported_backends":["process"],"resources":{"cpu":2,"memory_mb":4096,"gpu":0},"interface":{}}'
```

---

## 9. 运维：服务管理

```bash
cd /root/uenv/uenv-hub

# 启动（推荐用启动脚本，自动从 data/.admin_token 注入共享 token）
nohup ./scripts/start-hub.sh > logs/uenv-hub.log 2>&1 &

# 查看日志（首次启动会打印 "bootstrapped admin token from config"）
tail -f logs/uenv-hub.log

# 查看监听 / 进程（注意：用具体 PID，勿用会误杀自身的 pkill 模式）
ss -tlnp | grep 8088
PID=$(ss -tlnp | grep 8088 | grep -oP 'pid=\K[0-9]+')

# 停止
kill "$PID"

# 备份（VACUUM INTO）
bash scripts/backup.sh
```

- 配置文件：`config/hub.prod.toml`（端口 8088、`require_token=true`、SQLite 持久化）。
- 共享 token：`data/.admin_token`（权限 600），由 `scripts/start-hub.sh` 在首次启动注入；
  之后已存入 DB，重启无需该文件也生效。
- 环境变量可覆盖（前缀 `UENV_HUB_`，`__` 嵌套），如临时换端口：

```bash
UENV_HUB_SERVER__PORT=8099 ./scripts/start-hub.sh
```

**Token 轮换**：

```bash
# 1) 用当前 token 创建新 admin token
curl -X POST -H "Authorization: Bearer $(cat data/.admin_token)" \
  -H 'Content-Type: application/json' \
  -d '{"name":"shared","role":"admin","namespaces":["*"]}' \
  http://localhost:8088/api/v1/admin/tokens   # 响应里的 "token" 字段仅返回一次
# 2) 吊销旧 token：DELETE /api/v1/admin/tokens/{id}
```

---

## 10. 与全链路的关系

```
[导入机，一次性]  docker save / benchmark 打包 / wheel 收集
        │
        ▼ publish / publish-image / artifact POST
┌───────────────────┐
│ Hub（内网）        │  manifest + 制品字节（EnvPackage / env registry）
└─────────┬─────────┘
          │ 部署期：uenv env sync / GET artifact / GET versions/latest
          ▼
┌───────────────────┐
│ Worker(L2)        │  本地 plugins/、/var/lib/uenv/envs/…
└─────────┬─────────┘
          │ Episode 热路径（只读本地，零 egress）
          ▼
       Server 调度
```

- Hub **不参与** Episode 运行时调度；**负责部署期的制品分发与预缓存**（含镜像 tar、benchmark 数据包，不仅是 manifest）。
- Worker 启动/spawn 前：优先 **Hub sync 到本地**；开发态可降级到仓库内 `plugins/<env>/manifest.yaml`。
- Server 运行时不依赖 Hub HTTP；Hub 在 Worker **部署与预热** 阶段提供制品。
- EnvPackage（SWE、DSCodeBench 等）完整流程见 **[uenv-hub环境标准化指南.md](./uenv-hub环境标准化指南.md)** §5–§7。
- 详细全链路接口见 `全链路联调-各层接口与参数字段.md` §5。

---

## 11. 命令行工具（CLI）

Hub 提供 `uenv` CLI（`uenv env` / `uenv hub` 子命令）用于查询、发布、同步等操作。
完整安装与操作说明见 **`docs/uenv-cli-guide.md`**。快速上手：

```bash
cd /root/uenv/uenv-hub && ~/.cargo/bin/cargo build --release -p uenv-hub-client
./target/release/uenv hub login --endpoint http://8.130.95.176:8088 \
  --token "$(cat data/.admin_token)"
./target/release/uenv env list
```
