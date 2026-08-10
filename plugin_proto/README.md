# L2 插件 IPC — 与 L1 控制面严格分离

本目录定义 Worker ↔ 插件子进程（Protobuf over UDS）的 **L2** 契约。

- **禁止** 被 `uenv-server`、`uenv-mock-scheduler`、`uenv-bridge` 引用
- **禁止** import 进 `proto/` 下的 L1 定义

正式内置环境入口为 `qa` 和 `code`；`math` 仅作为 `qa` 的历史兼容别名。

## Reset 配置 sidecar

`ResetRequest` 保持精简，不在每次增加环境参数时修改 protobuf。Worker 会在
插件 UDS 旁写入 `<uds-path>.episode.json`，插件在处理 `Reset` 时读取它：

- 旧插件继续读取顶层 `question`、`dataset`、`target` 等兼容字段；
- 任意 `payload.env_config` 字段会在不覆盖旧顶层值的前提下展平；
- `_uenv.payload` 保存完整原始 Episode payload；
- `_uenv.reward_config` 保存完整原始 reward config；
- `_uenv.seed` 与 protobuf 中的 seed 一致。

新环境请从 `templates/process-plugin/` 开始，实现 `proto-uds` 服务并使用
`_uenv` 中的完整上下文；不要为环境私有参数修改本协议。
