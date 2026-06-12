# UEnv CLI 操作文档（`uenv`）

> 配套服务：UEnv-Hub（L1 注册中心）。Hub 接口对接见
> `docs/uenv-hub-service-integration.md`。
> CLI 由 `uenv-hub-client` crate 构建，二进制名 **`uenv`**，通过 HTTP 调用 Hub。

---

## 1. 安装 / 获取二进制

```bash
cd /root/uenv/uenv-hub
~/.cargo/bin/cargo build --release -p uenv-hub-client
# 产物：target/release/uenv
ls -la target/release/uenv
```

可选：加入 PATH 方便调用：

```bash
export PATH="/root/uenv/uenv-hub/target/release:$PATH"
uenv --help
```

本文后续以 `uenv` 代指该二进制（未加入 PATH 时用 `./target/release/uenv`）。

---

## 2. 命令总览

```
uenv [--endpoint <URL>] <env|hub> <子命令>
```

| 组 | 子命令 | 作用 | 是否需要 token |
|----|--------|------|----------------|
| env | `list` | 列出已注册环境 | 是 |
| env | `info <env>` | 环境详情（JSON） | 是 |
| env | `versions <env>` | 列出版本（标记 yanked） | 是 |
| env | `search [keyword] [--tag] [--author]` | 搜索 | 是 |
| env | `init <name> [--template] [--dir]` | 用模板脚手架新建项目 | 否（仅拉模板可能需） |
| env | `validate [--manifest]` | 本地校验 manifest + schema | 否（纯本地） |
| env | `build [--manifest] [--engine]` | 构建镜像（docker/podman） | 否 |
| env | `push [--manifest] [--engine]` | 构建+推送镜像并发布 manifest | 是 |
| env | `publish [--manifest]` | 仅发布元数据（镜像已在仓库） | 是 |
| env | `yank <env> --version --reason` | 下架版本 | 是 |
| hub | `login --token [--endpoint]` | 保存 token/endpoint 到配置 | — |
| hub | `status` | 显示 endpoint + 连通状态 | 读取需 token |
| hub | `sync [--since] [--dry-run]` | 增量同步元数据 | 是 |
| hub | `config set <key> <value>` | 设置配置（key=endpoint/token） | — |
| hub | `config show` | 打印当前配置 | — |

> 公开端点（healthz/metrics/version）不经 CLI；CLI 的查询/写操作都走
> `/api/v1/**`，当前 Hub 已开启鉴权，故除纯本地命令外都需 token。

---

## 3. 首次配置（连接 Hub + 鉴权）

CLI 配置优先级：**命令行 `--endpoint` > 环境变量 > 配置文件 > 默认值**。
配置文件路径：`~/.config/uenv/hub.toml`。

### 方式 A：持久化登录（推荐）

```bash
uenv hub login \
  --endpoint http://8.130.95.176:8088 \
  --token "$(cat /root/uenv/uenv-hub/data/.admin_token)"
# -> saved credentials for http://8.130.95.176:8088
```

写入 `~/.config/uenv/hub.toml`：

```toml
endpoint = "http://8.130.95.176:8088"
token = "uenvh_xxxxxxxx"
```

> 安全提示：该文件以明文保存 token，建议 `chmod 600 ~/.config/uenv/hub.toml`，
> 勿提交到仓库。

### 方式 B：环境变量（不落盘，适合 CI / 临时）

```bash
export UENV_HUB_ENDPOINT=http://8.130.95.176:8088
export UENV_HUB_TOKEN=uenvh_xxxxxxxx
```

### 方式 C：单次覆盖 endpoint

```bash
uenv --endpoint http://8.130.95.176:8088 env list   # token 仍取自配置/环境变量
```

验证：

```bash
uenv hub status
# endpoint: http://8.130.95.176:8088
# token:    configured
# status:   reachable (3 environments)
```

---

## 4. 查询环境（只读）

```bash
# 列表
uenv env list
#   3 environment(s) (page 1/1):
#     agent   default  latest=0.1.0
#     code    default  latest=1.0.0
#     math    default  latest=1.0.0

# 详情（完整 JSON）
uenv env info math

# 版本列表（下架版本带 (yanked)）
uenv env versions math
#   1.0.0

# 搜索
uenv env search math
uenv env search --tag reasoning
uenv env search --author uenv-team
```

分页：`uenv env list --page 2 --per-page 50`。

---

## 5. 发布流程（开发者）

### 5.1 脚手架新建项目

```bash
uenv env init demoenv --template math      # 模板: echo(默认)/math/code/agent
cd demoenv
ls
# Dockerfile  README.md  examples/  manifest.toml  requirements.txt  src/  tests/
```

`--dir` 可指定输出目录；`init` 会校验模板 gzip 包的 sha256。

### 5.2 编辑并本地校验

编辑 `manifest.toml`（`env_type`、`[version]`、`[image].url`、`[interface]` 等），
然后本地校验（纯本地，不连网，不需 token）：

```bash
uenv env validate
# manifest is valid          # 失败时逐条打印 [error]/[warning] location: message
```

### 5.3 发布

镜像已在仓库时，仅发布元数据：

```bash
uenv env publish               # 默认读 ./manifest.toml
# created environment 'demoenv'        （环境不存在时自动创建）
# published demoenv@0.1.0 -> /api/v1/envs/demoenv/versions/0.1.0
```

需要构建/推送镜像时（要求本机装有 docker 或 podman）：

```bash
uenv env build                 # 仅构建：docker build -t <image.url> .
uenv env push                  # 构建 + 推送 + 发布 manifest
uenv env build --engine podman # 切换引擎
```

> 当前主机未安装 docker/podman，`build`/`push` 会报
> “failed to run 'docker' (is it installed and on PATH?)”。如需用，请先安装引擎；
> 已有镜像时直接用 `publish` 即可，无需引擎。

`examples/*.json` 会在发布时自动作为示例附加到版本。

### 5.4 下架版本

```bash
uenv env yank demoenv --version 0.1.0 --reason "broken build"
# yanked demoenv@0.1.0
```

下架后该版本不再作为 `latest` 返回（仅有 yanked 版本时 `list` 显示 `latest=-`）。

---

## 6. 运维 / 同步

```bash
# 增量同步：拉取 since(Unix 秒) 之后变更的 manifest
uenv hub sync --since 0
#   3 manifest(s) changed since 0 (server_time=...)
#     math@1.0.0
#     code@1.0.0
#     agent@0.1.0
uenv hub sync --since 0 --dry-run     # 只看不写

# 查看 / 修改配置
uenv hub config show
uenv hub config set endpoint http://8.130.95.176:8088
uenv hub config set token uenvh_xxxxxxxx
```

---

## 7. 鉴权与错误

- 除 `init`/`validate`/`config`/`login` 外，命令都需有效 token；无 token 或失效时报：

```
error: API error [Unauthorized] missing or invalid API token (use 'Authorization: Bearer <token>')
```

- CLI 通过 `Authorization: Bearer <token>` 调用 Hub，与服务端单一共享 Token 方案一致。
- 退出码：成功 0，失败 1（错误打印到 stderr，前缀 `error:`）。
- 常见错误码（与 Hub 一致）：`Unauthorized`(401)、`NotFound`(404)、
  `VersionAlreadyExists`(409)、`InvalidManifest`/`SchemaValidationFailed`(422)、
  `RateLimited`(429)。

---

## 8. 实测验证记录（本环境）

| 命令 | 结果 |
|------|------|
| `uenv hub status` | reachable (3 environments) ✅ |
| `uenv hub config show` | endpoint/token/config 路径正确 ✅ |
| `uenv env list` | agent/code/math ✅ |
| `uenv env versions math` | 1.0.0 ✅ |
| `uenv env search math` | 1 result ✅ |
| `uenv env info math` | 完整 JSON ✅ |
| `uenv hub sync --since 0 --dry-run` | 3 manifests ✅ |
| `uenv hub login` | 写入 ~/.config/uenv/hub.toml ✅ |
| `uenv env init demoenv --template math` | 脚手架 9 文件 ✅ |
| `uenv env validate` | manifest is valid ✅ |
| `uenv env publish` | 自动建环境 + 发布 0.1.0 ✅ |
| `uenv env yank` | 下架成功，latest 变 `-` ✅ |
| 无 token 调用（纯净 HOME） | 正确返回 Unauthorized ✅ |
| `env build`/`env push` | 需 docker/podman（本机未装） ⚠️ |

> 审计后已重置 Hub 数据库，测试产生的 `_cli_demo` 环境已清除，恢复纯净 seed
> （`agent`/`code`/`math`）。
