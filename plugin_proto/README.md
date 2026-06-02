# L2 插件 IPC — 与 L1 控制面严格分离

本目录定义 Worker ↔ 插件子进程（Protobuf over UDS）的 **L2** 契约。

- **禁止** 被 `uenv-server`、`uenv-mock-scheduler`、`uenv-bridge` 引用
- **禁止** import 进 `proto/` 下的 L1 定义

MVP Phase 0 环境：`plugins/gsm8k/`（`env_type=gsm8k`, `ipc=proto-uds`）
