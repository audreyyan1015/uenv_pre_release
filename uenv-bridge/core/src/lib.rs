// =============================================================================
// uenv-adapter-core 库的根模块
//
// 这个 crate 同时作为库（lib）和可执行文件（bin）使用：
//   - 作为库：对外暴露 AdapterCore 和相关类型，供测试和集成使用
//   - 作为可执行文件：main.rs 调用这里导出的类型启动 gRPC 服务器
//
// 模块结构：
//   core        —— 核心处理逻辑，SampleEnvelope 转换和 EpisodeService 调用
//   protocol    —— 内部数据结构定义（batch 和 sample 级别的请求/响应类型）
//   server_api  —— EpisodeService trait 的重导出（定义在 uenv-server crate 中）
//   service     —— AdapterCoreService 的 gRPC 服务端实现
//   pb          —— adapter_core.proto 生成的 Rust 代码（gRPC 消息类型和服务 trait）
// =============================================================================

pub mod core;
pub mod protocol;
pub mod server_api;
pub mod service;

// 对外暴露最常用的类型，调用方可以直接写 uenv_adapter_core::AdapterCore
// 而不需要写完整路径 uenv_adapter_core::core::AdapterCore。
pub use core::AdapterCore;
pub use protocol::{
    CoreError, ExecuteBatchRequest, ExecuteBatchResponse, SampleEnvelope, SampleResult,
};
pub use server_api::EpisodeService;
pub use service::AdapterCoreServiceImpl;

// pb 模块包含由 build.rs 在编译期从 adapter_core.proto 生成的 Rust 代码。
// include_proto! 宏会把 OUT_DIR 里的生成文件内联到这个模块中。
// "uenv.bridge.v1" 是 proto 文件中声明的 package 名称。
pub mod pb {
    tonic::include_proto!("uenv.bridge.v1");
}
