fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto = "../../proto/pertisk/machine/v1alpha1/machine.proto";
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&[proto], &["../../proto"])?;
    Ok(())
}
