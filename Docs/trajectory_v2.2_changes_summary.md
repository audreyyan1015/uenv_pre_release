# UEnv 代码改动总结：轨迹聚合存储 v2.2

> 对比范围：最新提交 `12faf9fe`（"0626"，作者 uenv-dev，2026-06-26 21:09）vs 上一次提交 `06669d69`（合并 worker-pool 分支）
> 工作目录：服务器 `8.130.86.71:/home/uenv`，分支 `bridge-alignment`，工作区干净（无未提交改动）
> 改动规模：26 个文件，+3211 行 / −37 行

---

## 一句话总结

这次提交实现了 **SWE 轨迹的"统一聚合存储 v2.2"**：worker 把每条 episode 轨迹封存（seal）后，通过 HTTP 上传到 server 端集中存储（SQLite 索引 + JSON 正文），上传走"本地落盘 + 后台异步重试"的可靠队列，**绝不阻塞或影响训练 reward**。

---

## 实现了什么功能

### 1. Worker 端：轨迹上传链路（核心新增）
- 轨迹封存后，先写本地（`bodies/{id}.json`），再往 `spool/pending` 写一个 marker 文件就立即返回——**同步路径不做网络 IO，不影响 reward 计算**。
- 一个后台线程（`trj-uploader`）轮询 `spool/pending`，把轨迹正文 gzip 压缩后 POST 到 server；成功就删 marker（可选连本地正文一起删），失败累计重试，超过 10 次挪到 `spool/failed`。
- 整条链路用 `reqwest::blocking` + `std::thread`，**不依赖 tokio runtime**，native 路径和 gateway 路径都能用。
- 上传地址/token 支持两种配置：YAML（`trajectory_upload.endpoint/token`）和环境变量（`UENV_TRAJECTORY_ENDPOINT/TOKEN`），**环境变量优先级更高**。

### 2. Server 端：轨迹聚合存储服务（核心新增）
- 新增一个独立的 HTTP 服务（默认监听 `:8077`），由 bridge 启动时按需拉起，与 gRPC 服务共用同一个存储。
- 存储双层结构：`trajectory.db`（SQLite WAL 模式，存元数据 + 关键指标 + body 指针）+ `bodies/{id}.json`（轨迹正文）。
- 写入顺序严格保证一致性：**先落 blob（tmp→fsync→rename）再 INSERT 数据库**，杜绝"半截文件"。
- 幂等：同 id + 同 sha256 视为重复（返回 duplicate）；同 id 但内容不同返回 409 冲突。
- 提供 REST 接口：上传/列表/按 id 取正文/HEAD/健康检查/指标/reconcile 对账/按 episode 查结果。
- 内置可观测指标（acked / duplicate / conflict / error 原子计数）与数据保留（retention）能力。

### 3. 控制面打通：episode 结果落库
- worker 上报 episode 结果时，server 在 ack 阶段把摘要（reward、步数、`trajectory_id`、存储 URL）写入 `episode_results` 表，让 native 路径也能把轨迹和 episode 关联起来。

### 4. run_id 贯穿（一次完整作业 ID）
- driver 通过 HTTP 头 `X-UEnv-Run-Id` 注入 run_id，gateway 在 `create_session` 时取出并绑定到 session，最终写进轨迹 bundle，方便按"一次作业"聚合多条轨迹。

---

## 修改了哪些文件

### 协议 / 公共契约
| 文件 | 改动 |
|------|------|
| `proto/uenv/v1/episode.proto` | `EpisodeResult` 新增 `trajectory_id`、`trajectory_storage_url` 两个字段 |
| `uenv-common/src/lib.rs`（新增） | 新建 worker/server 共享的契约 crate 入口 |
| `uenv-common/src/trajectory.rs`（新增，79 行） | 定义共享类型 `TrajectoryRef`、`TrajectoryHeader`、`UploadStatus` |
| `uenv-common/Cargo.toml`（新增） | 新 crate 的依赖声明 |

### Server 端
| 文件 | 改动 |
|------|------|
| `uenv-server/src/trajectory.rs`（新增，953 行） | **聚合存储服务全部实现**：SQLite 建表、上传/查询逻辑、HTTP 路由、对账、保留、指标 |
| `uenv-server/src/control_plane.rs` | ack 时把 episode 结果摘要 upsert 进 `episode_results` 表 |
| `uenv-server/src/state.rs` | `ServerState` 新增 `trajectory_store`（OnceLock 注入） |
| `uenv-server/src/lib.rs` | 注册 `pub mod trajectory` |
| `uenv-server/Cargo.toml` | 新增 rusqlite、sha2 等依赖 |
| `uenv-server/stress_test/trajectory_stress_test.py`（新增，290 行） | 压测脚本：N 并发上传，验证零丢失/幂等/gzip/一致性 |

### Worker 端
| 文件 | 改动 |
|------|------|
| `uenv-worker/src/swe/trajectory_upload.rs`（新增，283 行） | **上传器全部实现**：spool 队列、后台 drainer、gzip、重试 |
| `uenv-worker/src/swe/trajectory.rs` | `TrajectoryBundle` 增加 run_id 等聚合字段和 reward/resolved；`TrajectoryRef` 改为复用 common；seal 时把 reward 写进 body |
| `uenv-worker/src/swe/instance_pool.rs` | 持有 `uploader`，seal 后入队上传；新增 `seal_and_upload`（native 路径）和 `set_session_run_id` |
| `uenv-worker/src/swe/session.rs` | session 增加 run_id 字段与 `set_run_id`；seal 时填充聚合字段 |
| `uenv-worker/src/swe/mod.rs` | 注册 `trajectory_upload` 模块并导出 |
| `uenv-worker/src/runtime_gateway/mod.rs` | `create_session` 读取 `X-UEnv-Run-Id` 头并绑定到 session |
| `uenv-worker/src/episode/executor.rs` | native 路径 episode 结束时构造 bundle、seal 并上传，回填 `trajectory_id`/`storage_url` |
| `uenv-worker/src/config/mod.rs` | 新增 `TrajectoryUploadConfig`，支持 YAML 配置并导出为环境变量（env > yaml） |
| `uenv-worker/src/wal/mod.rs` | 测试结构体补 `..Default::default()`（适配 proto 新字段） |
| `uenv-worker/tests/trajectory_upload_e2e.rs`（新增，212 行） | 端到端测试：worker seal→上传→server 落库→GET 回读校验 |
| `uenv-worker/Cargo.toml` | 新增 flate2、sha2 等依赖 |

### 其他
| 文件 | 改动 |
|------|------|
| `uenv-bridge/core/src/main.rs` | 启动时按配置拉起轨迹 HTTP 服务（:8077）并注入到 ServerState |
| `Cargo.toml` / `Cargo.lock` | workspace 注册新 crate，锁定新依赖 |
| `uenv-server/src/trajectory.rs.bak_feat`、`uenv-worker/src/swe/trajectory.rs.bak_v22` | 改动前的备份文件（非生产代码） |

---

## 设计上的几个关键取舍

1. **上传绝不影响 reward**：同步路径只写本地 marker，所有网络 IO 都在后台线程，失败也只是堆积在 spool，训练不受影响。
2. **可靠性优先**：server 写盘"先 blob 后 DB"、SQLite WAL、幂等去重 + 409 冲突检测，配套压测脚本验证零丢失。
3. **配置双通道**：YAML 适合静态部署，环境变量适合临时覆盖（如不把真实 token 写进 yaml），env 优先。
4. **两条上传路径统一**：gateway（多步 session）和 native（单步 episode）共用同一套 seal + 上传逻辑。
