fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_root = "../proto";
    let worker_proto = "../uenv-worker/proto";

    tonic_prost_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(
            &[
                format!("{proto_root}/uenv/v1/server.proto"),
                format!("{proto_root}/uenv/v1/scheduler.proto"),
                format!("{worker_proto}/worker_service.proto"),
            ],
            &[proto_root.to_string(), worker_proto.to_string()],
        )?;
    Ok(())
}
