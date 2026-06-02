# uenv-server — UEnv 全栈调度服务

UEnv Server 是 UEnv **全栈方案** 的控制平面：接收训练框架（或 Mock 客户端）提交的 Episode，维护 Worker 注册表与调度决策，**主动**调用 Worker `DispatchEpisode`。

> Layer 2 Worker Pool 权威文档：[Docs/worker-pool-layer-design.md](../Docs/worker-pool-layer-design.md)  
> 协议规范：[PROTOCOL.md](../PROTOCOL.md)

## 克隆本项目

`uenv-server` 位于 UEnv **monorepo** 根目录下，需克隆整个仓库（而非单独拉取本 crate）。

| 配置项 | 值 |
|--------|-----|
| 远程地址（HTTP） | `http://8.130.179.41:3000/pku-team/uenv.git` |
| 默认分支 | `main` |
| 开发分支（示例） | `lishuoqi` |

### 推荐方式：浅克隆

仓库体积较大（约 3500+ 文件），完整拉取在慢网下可能需 **10 分钟以上**。实测稳定方案是使用 **HTTP + 浅克隆**（`--depth 1`），只拉取目标分支最新提交：

**macOS / Linux**

```bash
# 目标目录为空，或先 clone 到临时目录再 rsync/cp 到工作目录
export GIT_TERMINAL_PROMPT=0

git -c http.lowSpeedLimit=1000 -c http.lowSpeedTime=60 clone \
  --progress --depth 1 --single-branch --branch main \
  "http://8.130.179.41:3000/pku-team/uenv.git" \
  /path/to/UEnv
```

**Windows（PowerShell）**

```powershell
$env:GIT_TERMINAL_PROMPT = '0'

git -c http.lowSpeedLimit=1000 -c http.lowSpeedTime=60 clone `
  --progress --depth 1 --single-branch --branch main `
  "http://8.130.179.41:3000/pku-team/uenv.git" `
  "D:\code\UEnv_clone_test"
# 若目标目录已有残留 .git 或本地文件，可将克隆结果（含 .git）复制到 D:\code\UEnv 后删除临时目录
```

说明：

- **`GIT_TERMINAL_PROMPT=0`**：避免在无交互终端中卡住等待用户名/密码输入。
- **`http.lowSpeedLimit` / `http.lowSpeedTime`**：慢网（约 300 KiB/s）时降低因低速被误判超时中断的概率。
- **`--progress`**：便于观察 `Updating files` 进度；克隆过程中请勿过早终止进程（退出码非 0 多为手动中断，而非地址错误）。

### 切换到其他分支

浅克隆且 `--single-branch` 时，本地默认只跟踪 `main`。若要切到 `lishuoqi` 等分支：

```bash
cd /path/to/UEnv

# 放宽 fetch 规则（仅需执行一次）
git config remote.origin.fetch "+refs/heads/*:refs/remotes/origin/*"
git fetch origin
git checkout -b lishuoqi --track origin/lishuoqi
```

### 认证与日常 Git

- **克隆（read）**：当前远端对匿名读取可用，一般**无需**浏览器登录即可完成 `clone` / `fetch`。
- **推送（push）**：需在 [Gitea 登录页](http://8.130.179.41:3000/user/login) 使用账号（如 `lsq3497`）配置 HTTP 凭据；本机未安装 Git Credential Manager 时，可在 Gitea 生成 Access Token 后配合 `git credential` 或 URL 内嵌 token 使用。
- **远程协议**：推荐 **HTTP**（`http://8.130.179.41:3000/pku-team/uenv.git`），与 SSH（`git@8.130.179.41:pku-team/uenv.git`）二选一即可，勿混用。

克隆完成后在仓库根目录验证：

```bash
git remote -v          # origin → http://8.130.179.41:3000/pku-team/uenv.git
git branch --show-current
git status
```

## 架构

```
Mock 客户端 / uenv-bridge  --[UEnvService]-------->  uenv-server
Worker                     --[ControlPlaneService]->  uenv-server
uenv-server                --[WorkerGrpcService]-->  Worker
运维工具                    --[AdminService]-------->  uenv-server
```

## gRPC Service（统一 proto）

| Service | Proto | 说明 |
|---------|-------|------|
| `UEnvService` | `proto/uenv/v1/server.proto` | `SubmitEpisode` 等 |
| `ControlPlaneService` | `proto/uenv/v1/scheduler.proto` | Worker 注册、心跳、`ReportResult` |
| `AdminService` | `proto/uenv/v1/server.proto` | 运维查询 |

Server 作为 **客户端** 调用 Worker 的 `uenv.worker.v1.WorkerGrpcService`（见 `uenv-worker/proto/worker_service.proto`）。

## 构建与运行

```bash
cargo build -p uenv-server
./target/debug/uenv-server -b 0.0.0.0:50051
```

Proto 在 `build.rs` 中从 `proto/` 编译，无需单独 `make proto-server`（但 Worker 等 crate 仍需 `make proto`）。

## Worker 接入

Worker 启动后连接同一端口的 `ControlPlaneService`：

| 字段 | 说明 |
|------|------|
| `worker_id` | 唯一标识 |
| `endpoint` | Worker gRPC 地址（Server 回连用） |
| `supported_env_types` | 如 `["gsm8k"]` |
| `max_concurrent` | 最大并发 |

`SubmitEpisode` 流程：调度 Worker → 填充 `dispatch_lease_id` → `DispatchEpisode` → 等待 `ReportResult` → 返回客户端。

## 实机联调

见 [Docs/discussions/a100-server-worker-e2e/README.md](../Docs/discussions/a100-server-worker-e2e/README.md)。
