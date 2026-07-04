// build.rs：Rust 项目的构建脚本，在编译主程序之前由 cargo 自动运行。
//
// 本脚本的作用是：把 .proto 文件（接口定义文件）编译成 Rust 代码。
// .proto 文件定义了服务器和客户端之间通过 gRPC 通信的数据结构和接口。
// tonic_prost_build 是专门用于把 .proto 文件转成 Rust 代码的工具。

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // proto 文件的根目录，位于上级目录的 proto 文件夹
    let proto_root = "../proto";
    // worker 端的 proto 文件单独放在 uenv-worker/proto 目录下
    let worker_proto = "../uenv-worker/proto";

    tonic_prost_build::configure()
        .build_server(true)   // 生成服务端代码（Server trait 和相关结构体）
        .build_client(true)   // 生成客户端代码（Client 结构体，用于调用其他服务）
        .compile_protos(
            &[
                // 主服务的 proto：定义客户端提交 episode 的接口
                format!("{proto_root}/uenv/v1/server.proto"),
                // 调度器/控制平面的 proto：定义 worker 注册、心跳、上报结果的接口
                format!("{proto_root}/uenv/v1/scheduler.proto"),
                // Agent 池控制面 proto：AgentJob、RegisterAgent、PollAgentJob、CompleteAgentJob
                format!("{proto_root}/uenv/v1/agent.proto"),
                // worker gRPC 服务的 proto：定义服务器下发 episode 给 worker 的接口
                format!("{worker_proto}/worker_service.proto"),
            ],
            // proto 文件的搜索路径（用于解析 .proto 文件内的 import 语句）
            &[proto_root.to_string(), worker_proto.to_string()],
        )?;
    Ok(())
}
