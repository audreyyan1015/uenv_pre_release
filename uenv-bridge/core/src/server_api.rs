// =============================================================================
// EpisodeService trait 的重导出
//
// EpisodeService 是 adapter core 和 episode 执行后端之间的函数调用边界。
// 它定义在 uenv-server crate 中，因为其参数类型直接使用 server.proto 生成的
// EpisodeRequest / EpisodeResult，而这两个类型属于 uenv-server。
//
// adapter core 通过这个 trait 调用后端，不关心后端的具体实现细节：
//   - 在生产环境中，后端是 UEnvEpisodeService，它把请求分发给真实的 Worker
//   - 在单元测试中，后端可以是任何实现了这个 trait 的测试替身
//
// 将 trait 定义在 uenv-server 中而不是 adapter core 中，是为了避免循环依赖：
//   - adapter core 依赖 uenv-server（使用其 proto 类型）
//   - 如果 uenv-server 也依赖 adapter core（为了实现 trait），就形成了循环
//   - 现在 uenv-server 自己定义 trait 并实现它，adapter core 只依赖 uenv-server
// =============================================================================

// 直接从 uenv_server crate 重导出，调用方可以写 crate::server_api::EpisodeService
// 而不需要写 uenv_server::EpisodeService。
pub use uenv_server::{EpisodeService, EpisodeServiceError};
