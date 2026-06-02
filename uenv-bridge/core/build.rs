fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_prost_build::configure()
        .build_server(true)
        .build_client(false)
        .compile_protos(
            &["../../proto/uenv/v1/adapter_core.proto"],
            &["../../proto"],
        )?;
    println!("cargo:rerun-if-changed=../../proto/uenv/v1/adapter_core.proto");
    Ok(())
}
