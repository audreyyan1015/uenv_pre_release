# Hub 待调整事宜：qa 制品、Rubric 契约与 Agent 侧分发

> 日期：2026-07-25
> 背景：本轮将 `math` 收敛为 `qa`，并引入 **verifiers Rubric 金标对齐** 与 **ToolEnv Agent 编排**。
> 判断：Hub 侧需要同步更新「提供的制品」与「注册方式」——下文逐项确认需求、现状与建议动作。
> 总报告：[综合报告-验证型环境与ToolEnv-B3联调](./综合报告-验证型环境与ToolEnv-B3联调.md)

---

## 1. 结论先行

**确认：Hub 需要更新。** 不止版本号 bump，还包括：

1. **环境身份与兼容策略**（`qa` 正式、`math` deprecate）；
2. **Rubric / 判分契约进入制品元数据**（否则 Agent / 训练侧无法声明「对齐哪一版金标」）；
3. **Agent 机沙箱制品分发**（ToolEnv 与 Worker 官方 env 应对齐同一 digest，不能长期靠手工 scp）；
4. **注册标签扩展**（Worker `env.types` vs Agent `agent_bridge` 的双轨注册）。

当前实机：Hub `8.130.95.176` 已有 `qa@0.2.0`（由 `math` 镜像发布），**尚未**携带 Rubric 契约字段，也**尚未**提供面向 Agent 机的独立制品通道。

---

## 2. 环境身份：`qa` 正式化与 `math` 退役

| 项 | 现状 | Hub 建议动作 |
|----|------|----------------|
| `qa` | 实机已 `publish` `qa@0.2.0`；Worker 已注册 `qa` | 将 `qa` 标为 **canonical**；seed/template 与仓库本地一致（本地 templates 已 4→5） |
| `math` | Worker 已不注册；Hub 仍可查到历史版本 | 标 **deprecated**，保留 yank/兼容别名一段时间；文档写明「新流量一律 `qa`」 |
| 拉取行为 | Worker `serve` 对 Hub 404 **硬失败** | Hub 保证 `qa/versions/latest` 稳定；deprecate 的 `math` 可 410 或仍 200 但带 warning header |

**制品内容**：当前 `qa` 实质复用 math 插件判分（加法式）。Hub 元数据应写清：

- `env_type: qa`
- `runtime_plugin: uenv-math-plugin`（或后续拆出的 `uenv-qa-plugin`）
- `supported_datasets: [gsm8k, pubmedqa, scitab, olympmath, …]`
- `compat_aliases: [math]`（过渡期）

---

## 3. Rubric 撰写方式与金标契约（新增需求，确认成立）

### 3.1 为什么 Hub 要管

本轮金标结论：

- 生产判分（Rust `score_action`）对齐 `verifiers` + `math_verify`：**96.55%**，**过宽 0**；
- olympmath 曾存在「子串包含 → 空输出/错误答案满分」洞，已修。

若 Hub 只发「能跑的 plugin 二进制/镜像」，训练与评测无法声明：

> 本次 run 对齐的是哪一版 Rubric、哪一版对齐语料、是否包含 olympmath 修复。

### 3.2 建议 Hub 制品增补的字段（契约草案）

在 env version manifest / package metadata 中增加（命名可调整，语义需稳定）：

```yaml
rubric:
  schema_version: "1"
  backend: "verifiers+math_verify"   # 金标参考实现
  production_scorer: "uenv-math-plugin/score_action"
  alignment:
    corpus_id: "qa_rubric_corpus@2026-07-25"
    corpus_digest: "<sha256 of data/alignment/qa_rubric_corpus.jsonl>"
    report_digest: "<sha256 of temp/alignment/qa_rubric/report.json>"
    metrics:
      agreement: 0.9655
      too_lenient: 0
      too_strict: 2
  datasets:
    gsm8k: { scorer: "gsm8k", notes: "#### 抽取" }
    pubmedqa: { scorer: "pubmedqa" }
    scitab: { scorer: "scitab" }
    olympmath: { scorer: "olympmath", notes: "no substring contains; numeric_equivalent" }
  known_gaps:
    - id: "natural_language_without_hash"
      severity: "too_strict"
    - id: "long_lhs_assignment_rejected"
      severity: "intentional"
```

### 3.3 Hub 注册 / 发布流程建议

| 步骤 | 说明 |
|------|------|
| 1. 上传对齐语料与报告 | 作为 env version 的 **附属 artifact**（或独立 `rubric` package） |
| 2. 发布时强制校验 | `too_lenient == 0` 否则拒绝 promote 到 `latest`（可配置） |
| 3. API | `GET /envs/qa/versions/{ver}` 返回上述 `rubric` 块；Worker/Bridge 可记录到 trajectory metadata |
| 4. Rubric 变更流程 | 改判分 → 跑 `verify_qa_rubric_alignment.py` → 更新 corpus/report → **新 version**，禁止 silent overwrite |

**确认**：用户判断「Rubric 撰写方式需要 Hub 侧更新注册」——**成立**。本地已有工具链，缺的是 Hub 的制品模型与发布闸门。

---

## 4. Agent 侧制品分发（ToolEnv / DSCodeBench）

### 4.1 问题

- Worker 上的官方 `code`/`dscodebench` env 与 208.77 沙箱依赖（numpy/pandas/torch/…）必须一致，否则 Agent 迭代通过、官方 harness 失败。
- 当前：bootstrap + 手工同步 `DSCodeBench.json`（md5 对齐）+ sandbox-venv heavy requirements。
- **Hub 尚未**向 Agent 机提供「与 Worker 同 digest 的 code 环境包」。

### 4.2 建议

| 方案 | 描述 | 推荐 |
|------|------|------|
| A. 同一 env package 双消费者 | Hub 的 `code`/`dscodebench` version 同时声明 `consumers: [worker, toolenv-agent]`，Agent bootstrap 按 digest 拉取数据与 requirements lock | ✅ 推荐 |
| B. 独立 `toolenv-sandbox` package | 仅含 venv lock + 数据集指针，env_type 仍走 Worker 判分 | 可作补充 |
| C. 继续手工 | 仅适合联调 | ❌ 不可作为生产 |

Agent 注册（已落地）使用 `agent_bridge_id=uenv-agent-toolenv`；Hub 宜增加：

- AgentBridge 包注册（版本、入口、所需 env digest）；
- 与 `RegisterAgent.synced_agent_bridges` 字段对齐的查询 API。

---

## 5. 注册方式变更清单（给 Hub 实现同学）

1. **Seed**：`qa` 正式 seed；`math` deprecated。
2. **Template**：控制台/CLI 创建环境默认 `qa`。
3. **Publish API**：支持 `rubric` metadata + 附属 artifact 上传。
4. **Promote 闸门**：金标 `too_lenient=0`（可开关）。
5. **AgentBridge 目录**：注册 `uenv-agent-toolenv` / `uenv-agent-openhands` 及兼容的 env digest。
6. **文档**：对外说明「验证型单轮用 `qa`；code Agent 编排用 `execution_mode=agent` + ToolEnv bridge」。
7. **Yank 策略**：错误 Rubric 版本可 yank，但保留 digest 可追溯。

---

## 6. 本轮已在仓库落地、等待 Hub 吸收的内容

| 仓库产物 | Hub 应对应动作 |
|----------|----------------|
| `plugins/qa/` | 发布/维护 `qa` 包时引用该插件清单 |
| `uenv-bridge/scripts/hub_publish_qa_env.py` | 可扩展为带 rubric 字段的正式发布器 |
| `data/alignment/qa_rubric_corpus.jsonl` | 作为附属 artifact |
| `uenv-bridge/scripts/verify_qa_rubric_alignment.py` | CI / publish pre-check |
| `scripts/toolenv/requirements-sandbox*.txt` | 与 code env 的 lock 对齐后由 Hub 下发 |
| `AgentJob.task_payload_json` | Hub 文档说明 Agent 载荷形态（非 Hub 存储，但是契约的一部分） |

---

## 7. 优先级建议

| P0 | 把 `qa` 标 canonical、`math` deprecate；保证 latest 拉取稳定 |
| P0 | Rubric metadata 进入 `qa` 下一 version（即使先只贴对齐率与 corpus digest） |
| P1 | Agent 拉取与 Worker 同 digest 的 DSCodeBench 数据 + sandbox lock |
| P1 | Publish 闸门：禁止 too_lenient>0 的版本成为 latest |
| P2 | AgentBridge 目录与 UI |

---

## 8. 不在 Hub 范围（避免误派）

- Server `CodeAgentBackend` / poller 常驻 —— 属 Server + Agent 机。
- olympmath Rust 判分修复 —— 属 Worker 插件；Hub 只负责**声明**该修复落在哪个 version。
- 7142 临时 vLLM —— 已释放，与 Hub 无关。
