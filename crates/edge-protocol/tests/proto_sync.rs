//! Guards the dual-location IDL: the published crate builds from
//! `crates/edge-protocol/proto/fleet.proto`, while the workspace documents
//! `proto/fleet.proto`. Both must stay byte-identical.

#[test]
fn workspace_proto_mirror_is_in_sync() {
    let local =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/proto/fleet.proto"))
            .expect("crate-local proto");
    let workspace = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../proto/fleet.proto"
    ))
    .expect("workspace-root proto");
    assert_eq!(local, workspace, "proto drift: copy the change to both files");
}
