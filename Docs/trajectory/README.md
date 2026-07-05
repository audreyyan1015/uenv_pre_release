# UEnv 轨迹（Trajectory）文档

本目录收录 **Worker / Server 侧 SWE 轨迹保存** 的冻结规范与字段说明。

| 文档 | 说明 |
|------|------|
| [frozen-spec-v2.2.md](./frozen-spec-v2.2.md) | **当前生效**的 v2.2 冻结规范：目录布局、JSON 字段、索引、上传 API；§8 说明**当前不必改代码**与**后续扩展**时的改码范围 |
| [trajectory-bundle.example.json](./trajectory-bundle.example.json) | 符合规范的 `TrajectoryBundle` 示例（仅供阅读，非 JSON Schema） |

**上游来源**（历史与设计背景，仍有效）：

- `Docs/260625-trajectory-server-migration-evaluation.md` — v2.2 原始冻结决策全文
- `Docs/trajectory_v2.2_changes_summary.md` — 实现改动与验证记录
- 代码契约：`uenv-common/src/trajectory.rs`、`uenv-worker/src/swe/trajectory.rs`

**版本**：v2.2（2026-06-25 冻结，2026-07-05 实机全链路仍在使用）
