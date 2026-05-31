fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bridge_proto_root = "../proto";
    let l1_proto_root = "../../proto";

    tonic_prost_build::configure()
        .build_server(true)
        .build_client(false)
        .compile_protos(&["../proto/adapter_core.proto"], &[bridge_proto_root])?;
    println!("cargo:rerun-if-changed=../proto/adapter_core.proto");

    tonic_prost_build::configure()
        .build_server(false)
        .build_client(true)
        .compile_protos(
            &[
                format!("{l1_proto_root}/uenv/v1/server.proto"),
                format!("{l1_proto_root}/uenv/v1/scheduler.proto"),
            ],
            &[l1_proto_root.to_string()],
        )?;
    println!("cargo:rerun-if-changed={l1_proto_root}/uenv/v1/server.proto");
    println!("cargo:rerun-if-changed={l1_proto_root}/uenv/v1/scheduler.proto");
    println!("cargo:rerun-if-changed={l1_proto_root}/uenv/v1/episode.proto");
    println!("cargo:rerun-if-changed={l1_proto_root}/uenv/v1/common.proto");
    Ok(())
}
