# UEnv clean proto

这个目录是一份未来替换用的干净版 proto。

相对当前正式 `proto/`，这里已经移除迁移期旧兼容入口，只保留类型化协议字段。删除过的字段号用 `reserved` 保留，避免未来误复用字段号。

## 目录结构

- `proto/uenv/v1/*.proto`：保持与仓库正式 proto 相同的相对路径，方便未来直接替换。

## 已移除的旧字段

- `uenv.bridge.v1.SampleEnvelope`
  - 删除字段号 `6`
  - 删除字段号 `7`
  - 删除字段号 `18`
  - 保留：`reserved 6, 7, 18;`
- `uenv.v1.CancelEpisodeResponse`
  - 删除字段号 `1`
  - 保留：`reserved 1;`

## 不删除但不再承载协议旧键的字段

- `EpisodeRequest.metadata`
- `EpisodeResult.metadata`
- `StepRecord.info`

这些字段仍保留为上下文或环境信息容器，但不再读取或写入 `parallel_mode`、rollout 版本、logprobs、时间统计、`response_ids`、`response_mask` 等协议字段。

## 替换方式

未来确认所有客户端完成迁移后，可以用本目录下的 `proto/` 覆盖正式 `proto/`，然后重新生成 protobuf 代码并跑 server、worker、adapter-core 的测试。
