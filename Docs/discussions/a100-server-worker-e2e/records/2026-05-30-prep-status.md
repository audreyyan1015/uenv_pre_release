# 2026-05-30 实机联调准备状态

> **最后更新**：2026-05-30  
> **阶段**：环境 bootstrap 完成，待初始化联调目录并同步代码

---

## 机器概况

| 项 | 机器 A (7143) — Server | 机器 B (7142) — Worker |
|----|------------------------|------------------------|
| SSH | `219.147.100.43:7143` | `219.147.100.43:7142` |
| 内网 IP | **10.10.20.143** | **10.10.20.142** |
| hostname | user | user |
| GPU | 2× A100-80GB | 2× A100-80GB |
| 磁盘 `/` | 850G，可用 ~309G (64%) | 850G，可用 ~325G (62%) |
| grpcurl / protoc / Rust | ✅ 均已就绪 | ✅ 均已就绪 |
| `/var/log/uenv` | ✅ | ✅ |
| `/tmp/uenv/wal` | ✅ | ✅ |
| 50051/50052/19090 | ❌ 未监听 | ❌ 未监听 |

---

## `/root` 现有目录（2026-05-30 实勘）

两台机器 hostname 均为 `user`，home 下已有较多实验/训练项目。**本次联调单独占用 `/root/UEnv`，不改动现有目录。**

### 机器 A (7143)

```
agentsi_bench  evorl_train  evorl_train_latest.tar.gz
Applications   outputs      Source/          ← 空目录
log  log.lammps             yanziyi → /data/yanziyi
launch_cos_memory_full_qwen14_7143.sh
```

| 路径 | 说明 |
|------|------|
| `~/Source/` | 空，**不用于 UEnv** |
| `~/yanziyi` | 符号链接 → `/data/yanziyi`（含旧版 `/data/yanziyi/uenv`，proto 过时） |
| `~/evorl_train*` | 训练相关，与本次联调无关 |
| `launch_cos_memory_full_qwen14_7143.sh` | 本机启动脚本 |

### 机器 B (7142)

```
agentsi_bench  Case01Math  Case02MCP  Case03Lammps  caseb_awm
codes/         datasets/   models/    miniconda3/   Tools/
evorl_train*   Source/shy_op/          tmp/  stopagent
launch_cos_memory_full_qwen14_7142.sh
```

| 路径 | 说明 |
|------|------|
| `~/Source/shy_op/` | 已有第三方项目，**不混放 UEnv** |
| `~/codes/` | OpenEnv、agent-world-model 等，与 UEnv 无关 |
| `~/Case*` / `caseb_awm` | 历史 case 实验 |
| `/data/ronghao/uenv` | 旧版 uenv（无 release 二进制），**不复用** |

---

## 联调目录规划（新建）

### 决策

| 选项 | 结论 |
|------|------|
| `/root/Source/UEnv` | ❌ B 的 Source 已有 shy_op；A 虽空但不统一 |
| `/data/*/uenv` | ❌ 他人旧代码，proto 未对齐 |
| **`/root/UEnv`** | ✅ **采用**：独立、两台一致、与 sync 脚本默认路径相同 |

### 目标布局

```
/root/UEnv/                    ← 代码仓库根（sync / scp 目标）
  ├── uenv-server/
  ├── uenv-worker/
  ├── plugins/gsm8k/           ← Worker 插件（B 侧必需）
  ├── proto/
  ├── config/
  ├── fixtures/gsm8k/
  └── target/release/          ← cargo build --release 产出

/var/log/uenv/
  ├── server.log               ← A: uenv-server
  └── worker.log               ← B: uenv-worker

/tmp/uenv/wal/                 ← B: Worker WAL
```

### 初始化命令（两台机器各执行一次）

```bash
# 方式 1：脚本（推荐，sync 前可先跑）
bash /tmp/init-e2e-layout.sh   # 由 sync-from-dev.ps1 一并上传，或 scp 后执行

# 方式 2：手工
mkdir -p /root/UEnv /var/log/uenv /tmp/uenv/wal
```

### 代码同步（A100 无法直接 git clone，需从开发机推送）

```powershell
# Windows 开发机
cd d:\code\UEnv
.\Docs\discussions\a100-server-worker-e2e\scripts\sync-from-dev.ps1 -Target Both
```

同步完成后两台机器验证：

```bash
ls /root/UEnv/{Cargo.toml,uenv-server,uenv-worker,plugins/gsm8k,proto}
```

---

## 已完成准备工作

| # | 项 | 状态 |
|---|-----|------|
| 1 | SSH 连通 | ✅ |
| 2 | Windows 私钥权限 | ✅ |
| 3 | 工具链 bootstrap | ✅ |
| 4 | 联调脚本 | ✅ `../scripts/` |
| 5 | `/root` 目录实勘 | ✅ 见上节 |
| 6 | 联调目录规划 | ✅ `/root/UEnv` |
| 7 | 代码同步至 `/root/UEnv` | ⏳ 待执行 |
| 8 | release 编译 | ⏳ 待执行 |

---

## Bootstrap 执行记录

### 机器 A (7143)

- `prep-bootstrap.sh`：grpcurl ✅；rustup 下载中断（已有 Rust 1.95）
- 验证：`rustc 1.95.0` / `grpcurl v1.9.3` / `protoc 3.21.12`

### 机器 B (7142)

- 首次 bootstrap：protoc ✅，grpcurl GitHub 下载失败
- `install-grpcurl.sh` 补救 ✅ → `grpcurl v1.9.3`

---

## 网络探测

| 方向 | 结果 | 说明 |
|------|------|------|
| A → B:50052 | Connection refused | 服务未启动 |
| B → A:50051 | Connection refused | 服务未启动 |
| 内网 10.10.x | 路由可达 | 启动服务后复测 |

---

## 本地仓库

- HEAD：`a4f6e5a update docs`
- remote：`http://8.130.179.41:3000/pku-team/uenv.git`（A100 需认证，**不可直接 clone**）

---

## Worker 配置要点

代码同步至 `/root/UEnv` 后：

| 配置项 | 值 |
|--------|-----|
| `config/uenv-worker.yaml` → `server.endpoint` | `10.10.20.143:50051` |
| Worker 注册 `endpoint` | `10.10.20.142:50052` |

```bash
cd /root/UEnv
source Docs/discussions/a100-server-worker-e2e/scripts/machine-env.sh worker
```

---

## 联调脚本清单

| 脚本 | 用途 |
|------|------|
| `scripts/init-e2e-layout.sh` | 创建 `/root/UEnv` + 运行时目录 |
| `scripts/sync-from-dev.ps1` | Windows → 两台 `/root/UEnv` |
| `scripts/ssh-connect.ps1` | 快捷 SSH（`A` / `B`） |
| `scripts/machine-env.sh` | Server / Worker 环境变量 |
| `scripts/submit-episode-grpcurl.sh` | Bridge Mock 验收 |
| `scripts/prep-bootstrap.sh` | 工具链初始化（已完成） |

---

## 待执行 P0 步骤

1. [x] ~~两台机器 bootstrap~~
2. [x] ~~`/root` 目录实勘与联调路径规划~~
3. [ ] 两台机器执行 `init-e2e-layout.sh`（创建 `/root/UEnv`）
4. [ ] `sync-from-dev.ps1 -Target Both` 同步代码
5. [ ] 两台：`cd /root/UEnv && make proto && cargo build -p uenv-server -p uenv-worker --release`
6. [ ] **A**：`./target/release/uenv-server -b 0.0.0.0:50051`
7. [ ] **B**：`source machine-env.sh worker && ./target/release/uenv-worker serve --config config/uenv-worker.yaml`
8. [ ] **A**：`submit-episode-grpcurl.sh` 验收
9. [ ] 保存日志至本目录

---

## SSH 连接（Windows）

```powershell
ssh -i secrets\9aa460dab6678381f86a1022b8a54c9f_32e42d1c7902ce68ba6719d551645e02_8.143 -p 7143 root@219.147.100.43
ssh -i secrets\2a9f778a35e7d08c738c79493ba643ef_65c3b455afbe3c81a8a757c01b0faae8_8.142 -p 7142 root@219.147.100.43
```

或：`.\Docs\discussions\a100-server-worker-e2e\scripts\ssh-connect.ps1 A|B`

---

## 备注

- 联调范围：[../README.md](../README.md) — **uenv-server + uenv-worker**，Bridge/Hub 用 grpcurl Mock。
- 旧路径 `/data/yanziyi/uenv`、`/data/ronghao/uenv` 仅作历史参考，**不参与本次联调**。
