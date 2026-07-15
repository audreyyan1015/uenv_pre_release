// 文件职责：作为 service 模块入口，把拆分后的 episode 编排、admin、RPC 和测试分片组合起来。
// 主要功能：保持 crate::service::* 外部 API 不变，同时把原 service.rs 拆成更容易阅读的职责片段。
// 大致工作流：编译时按 include! 顺序拼入分片文件，各分片共享原 service 模块作用域和私有 helper。

// service module is split by orchestration path and RPC surface while preserving the original module API.
include!("prelude_and_guards.rs");
include!("episode.rs");
include!("support.rs");
include!("admin.rs");
include!("rpc.rs");
include!("tests.rs");
