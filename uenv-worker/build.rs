fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_root = "../proto";
    let worker_proto = "proto";
    let plugin_proto = "../plugin_proto";

    tonic_prost_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(
            &[
                format!("{proto_root}/uenv/v1/adapter_core.proto"),
                format!("{proto_root}/uenv/v1/agent.proto"),
                format!("{proto_root}/uenv/v1/common.proto"),
                format!("{proto_root}/uenv/v1/episode.proto"),
                format!("{proto_root}/uenv/v1/scheduler.proto"),
                format!("{proto_root}/uenv/v1/server.proto"),
                format!("{proto_root}/uenv/v1/wal.proto"),
                format!("{worker_proto}/worker_service.proto"),
                format!("{plugin_proto}/uenv/plugin/v1/plugin.proto"),
            ],
            &[
                proto_root.to_string(),
                worker_proto.to_string(),
                plugin_proto.to_string(),
            ],
        )?;
    Ok(())
}
