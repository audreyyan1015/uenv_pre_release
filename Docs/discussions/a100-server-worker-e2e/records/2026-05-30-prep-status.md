# 2026-05-30 实机联调准备状态

> **最后更新**：2026-05-30  
> **阶段**：✅ **Server-Worker 联调已通过** — 见 [2026-05-30-e2e-run.md](./2026-05-30-e2e-run.md)

---

## 联调结论

| 项 | 状态 |
|----|------|
| 代码部署 `/root/UEnv` | ✅ 两台已同步 |
| release 编译 | ✅ A: uenv-server / B: uenv-worker + uenv-gsm8k-plugin |
| Register → Dispatch → Execute → Report | ✅ 全链路 |
| grpcurl SubmitEpisode | ✅ status=completed, total_reward=1.0 |

---

## 机器概况

| 项 | 机器 A (7143) — Server | 机器 B (7142) — Worker |
|----|------------------------|------------------------|
| SSH | `219.147.100.43:7143` | `219.147.100.43:7142` |
| 内网 IP | **10.10.20.143** | **10.10.20.142** |
| 代码路径 | **`/root/UEnv`** | **`/root/UEnv`** |
| 二进制 | `uenv-server/target/release/uenv-server` | `target/release/uenv-worker` |
| 服务端口 | 50051 ✅ | 50052 / 19090 ✅ |

---

## 目录规划（已落地）

```
/root/UEnv/                    ← 独立联调目录（不影响 ~/codes、~/Source 等）
/var/log/uenv/server.log       ← A
/var/log/uenv/worker.log       ← B
/tmp/uenv/wal/                 ← B WAL
```

`/root` 现有项目（evorl_train、codes、Source/shy_op 等）**未改动**。

---

## P0 步骤

1. [x] bootstrap
2. [x] `/root` 实勘与路径规划
3. [x] `init-e2e-layout.sh` + 代码同步
4. [x] release 编译
5. [x] 启动 Server + Worker
6. [x] grpcurl 验收
7. [x] 联调记录 → `2026-05-30-e2e-run.md`

---

## 配置要点

| 配置项 | 值 |
|--------|-----|
| Worker yaml | `Docs/discussions/a100-server-worker-e2e/config/uenv-worker.e2e.yaml` |
| `server.endpoint` | `10.10.20.143:50051` |
| Worker 注册 endpoint | `10.10.20.142:50052` |
| `UENV_GSM8K_PLUGIN_BIN` | `/root/UEnv/target/release/uenv-gsm8k-plugin` |

---

## 代码修复（联调中发现）

- `uenv-worker/src/plugin/arpc/mod.rs`：UDS 连接须 `hyper_util::rt::TokioIo` 包装（已合入本地）
- `submit-episode-grpcurl.sh`：`payload`/`reward_config` 使用 base64

---

## SSH 连接

```powershell
ssh -i secrets\9aa460dab6678381f86a1022b8a54c9f_32e42d1c7902ce68ba6719d551645e02_8.143 -p 7143 root@219.147.100.43
ssh -i secrets\2a9f778a35e7d08c738c79493ba643ef_65c3b455afbe3c81a8a757c01b0faae8_8.142 -p 7142 root@219.147.100.43
```

---

## 脚本

| 脚本 | 用途 |
|------|------|
| `scripts/init-e2e-layout.sh` | 创建目录 |
| `scripts/sync-from-dev.ps1` | Windows 同步（建议 tar+scp） |
| `scripts/submit-episode-grpcurl.sh` | Bridge Mock 验收 |
| `config/uenv-worker.e2e.yaml` | 实机 Worker 配置 |
