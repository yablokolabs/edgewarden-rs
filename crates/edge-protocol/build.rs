fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Canonical IDL lives here so the published crate is self-contained.
    // `proto/fleet.proto` at the workspace root mirrors this file; a sync
    // test (`tests/proto_sync.rs`) fails the build if they ever drift.
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&["proto/fleet.proto"], &["proto"])?;
    Ok(())
}
