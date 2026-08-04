# SWE-smith 全量 catalog 补齐与 Worker 重启报告

> 日期：2026-08-04  
> 主机：A100 **7143**（`uenv-worker`）  
> 关联：
> - [Adapter 覆盖核验](../../adapter/20260804-SWE-smith-Worker数据集覆盖核验说明.md)
> - [SWE-smith 变更与联调报告](../260801/SWE-smith变更与联调报告.md)
> - 拓扑：[secrets/README.md](../../../secrets/README.md)

---

## 1. 结论

| 项 | 结果 |
|---|---|
| 问题 | 训练侧大量 `not in catalog (size=736)`；Smith 实际只有 **5** 条 smoke |
| 处置 | 用 HF `SWE-bench/SWE-smith` ∩ 本机 **222** 个 `jyangballin/swesmith.*` 镜像导出全量可跑 catalog |
| Smith catalog | **59136**（镜像缺 0） |
| Gateway 合并 catalog | **59867**（Pro **731** + Smith **59136**） |
| Worker 重启 | ✅ health `ok`；`:28888` / `:28777` / `:28097` 监听正常；heartbeat 持续 |

**一句话**：Worker 已从 Smith smoke（5）切换为本地镜像可覆盖的全量 catalog（59136），服务重启后加载与探活正常。

---

## 2. 背景

Adapter 训练 `verl_swesmith_grpo_train_20260804_102356` 中报：

```text
swe instance_id ... not in catalog (size=736)
```

核验确认：`736 = Pro 731 + Smith smoke 5`。当时 EnvPackage：

```text
/var/lib/uenv/envs/swe-bench-smith/0.1.0-local/
bundle_digest = local-smith-smoke
catalog.json  = 5 条 oauthlib instance
```

7143 上虽已有约 222 个 Smith Docker 镜像，但 catalog / `images.manifest.json` 未挂入，训练查表仍只能命中 smoke 子集。

---

## 3. 操作步骤（7143）

1. **依赖**：在 `/data/uenv/tools/swesmith-export-venv` 安装 `pyarrow`。
2. **数据源**：本机 HF cache parquet  
   `/root/.cache/huggingface/hub/datasets--SWE-bench--SWE-smith/snapshots/*/data/*.parquet`（59136 行）。
3. **过滤**：`--only-local-images` 语义 —— `image_name` 必须已在 `docker images` 中；实机匹配 **59136 / 59136**，缺镜像 **0**。
4. **写出包**（compact JSON，避免 indent 膨胀到 ~6GiB）：

```text
/data/uenv/envs/swe-bench-smith/0.1.0-local/
  catalog.json            ~4.84 GiB（compact）
  images.manifest.json    59136 条 → 222 unique images
  manifest.json
  worker.overlay.yaml     variant=smith, local_only, grader=swesmith
  eval_spec.json
  .synced                 bundle_digest=local-smith-full-59136
```

5. **路径兼容**：系统盘 `/` 当时约 **94%** 占用，包落在 `/data`；软链：

```text
/var/lib/uenv/envs/swe-bench-smith/0.1.0-local
  -> /data/uenv/envs/swe-bench-smith/0.1.0-local
```

6. **备份**：原 smoke 包保留为  
   `/var/lib/uenv/envs/swe-bench-smith/0.1.0-local.smoke-bak-20260804`
7. **重启**：`SKIP_REBUILD=1 bash scripts/restart-worker-gateway-28097-7143.sh`  
   配置仍为 `config/uenv-worker.deploy-7143-swe-pro.yaml`（`variants: [pro, smith]`）。

---

## 4. 规模与资产

| 指标 | 数值 |
|---|---:|
| HF / parquet 总行 | 59136 |
| 导出 catalog instance | 59136 |
| unique images | 222 |
| `problem_statement` 非空 | 41103 |
| `problem_statement` 为空 | 18033 |
| 包目录体积 | ~4.9 GiB |
| Docker 镜像缺口 | 0 |

说明：一条镜像可对应多条 instance；「222 镜像」≠「222 样本」。

---

## 5. 重启验收（2026-08-04）

### 5.1 进程与端口

| 检查 | 结果 |
|---|---|
| 进程 | `uenv-worker --config config/uenv-worker.deploy-7143-swe-pro.yaml serve`（pid 重启后存活） |
| gRPC | `0.0.0.0:28888` LISTEN |
| health | `0.0.0.0:28777`；`GET /health` → `ok` |
| Runtime Gateway | `0.0.0.0:28097` LISTEN；无 key 访问返回 **401**（鉴权正常） |
| ControlPlane | 持续 `heartbeat` → Server `8.130.75.157:8088` |

### 5.2 加载日志（摘录）

```text
swe_catalog_loaded_from_env_package package_id=swe-bench-pro   count=731
swe_catalog_loaded_from_env_package package_id=swe-bench-smith count=59136
runtime_gateway_start catalog=59867
grpc_server_start / observability_server_start
```

### 5.3 已知非阻塞告警

- Hub `GET .../envs/swe/versions/latest` **404**：沿用本地 manifest（既有行为，Smith 包仍为 local-only）。
- 重启瞬间旧进程 `transport error` panic：停服预期现象，新进程已正常 serve。

---

## 6. 对 Adapter / 训练的影响

| 之前 | 之后 |
|---|---|
| Smith 可查表 **5** 条；合并 catalog **736** | Smith **59136**；合并 **59867** |
| 正式训练集大量 `not in catalog` | 只要 `instance_id` 在 HF 全量内即可查表 |
| 仅宜用 `swesmith_train_smoke_catalog_intersection` | 可按正式 parquet 训练；建议仍过滤空 `problem_statement`（约 4.1 万有效） |

训练侧无需改 Worker endpoint；确认 Episode 使用 `benchmark_variant=smith` / `env_package_id=swe-bench-smith` 即可。

---

## 7. 回滚

若需回到 smoke：

```bash
# 7143
rm -f /var/lib/uenv/envs/swe-bench-smith/0.1.0-local
cp -a /var/lib/uenv/envs/swe-bench-smith/0.1.0-local.smoke-bak-20260804 \
      /var/lib/uenv/envs/swe-bench-smith/0.1.0-local
SKIP_REBUILD=1 bash /root/UEnv/scripts/restart-worker-gateway-28097-7143.sh
# 期望日志：swe-bench-smith count=5，gateway catalog=736
```

---

## 8. 后续建议

1. Adapter 正式训练优先使用 `problem_statement` 非空子集，避免空题面跳过浪费 episode。
2. catalog ~5GiB 常驻内存（RSS 约数 GiB 量级）；系统盘仍紧，包继续放 `/data`，勿拷回 `/`。
3. Hub 正式注册 `swe-bench-smith` + `image_tar` 分发仍后置；当前依赖本机镜像 + `local_only`。
4. 若训练只要 Python 子集，可用导出脚本 `--prefer-python` 再出一版瘦包，降低 Gateway 查表与判分 P2P 成本。
