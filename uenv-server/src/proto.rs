// 文件职责：把 build.rs 生成的 protobuf Rust 代码引入 uenv-server 命名空间。
// 主要功能：include v1、scheduler.v1、worker.v1 等 proto 包，供 service/control_plane/scheduler 共享类型。
// 大致工作流：编译时 tonic_prost_build 输出 OUT_DIR 文件；本模块通过 include_proto! 在源码中暴露生成类型。

// proto.rs：把编译好的 protobuf 代码引入 Rust 命名空间。
//
// .proto 文件在编译时（build.rs）被 tonic_prost_build 转换成 Rust 代码，
// 并放在编译输出目录（OUT_DIR）下。
// tonic::include_proto!("包名") 宏等价于：
//   include!(concat!(env!("OUT_DIR"), "/包名.rs"));
// 即把生成的 Rust 源代码直接嵌入到当前模块中。

/// uenv.v1 包：Episode 数据类型与管理接口的 trait。
/// 对应 proto/uenv/v1/server.proto 中 package uenv.v1 的内容。
/// 包含 EpisodeRequest、EpisodeResult、AdminService 等。
pub mod v1 {
    tonic::include_proto!("uenv.v1");
}

/// uenv.scheduler.v1 包：控制平面接口（供 worker 使用）的数据类型与 trait。
/// 包含 RegisterWorkerRequest、HeartbeatRequest、ReportResultRequest 等。
pub mod scheduler {
    pub mod v1 {
        tonic::include_proto!("uenv.scheduler.v1");
    }
}

/// uenv.worker.v1 包：服务器主动下发 episode 给 worker 时使用的数据类型与客户端 stub。
/// 服务器在 dispatch_to_worker() 中使用 WorkerGrpcServiceClient 主动连接 worker。
pub mod worker {
    pub mod v1 {
        tonic::include_proto!("uenv.worker.v1");
    }
}
