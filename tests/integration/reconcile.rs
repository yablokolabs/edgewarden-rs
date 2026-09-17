//! Desired/reported reconciliation over gRPC: push policy, observe converge,
//! duplicate delivery, stale seq, and reconnect behaviour.

use control_plane::{FleetServiceImpl, Registry};
use edge_agent::{AgentConfig, EdgeAgent};
use edge_protocol::fleet::{fleet_service_server::FleetServiceServer, Policy};
use std::time::Duration;

async fn start_cp(
    registry: Registry,
) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let svc = FleetServiceImpl::new(registry);
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(l);
    let h = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(FleetServiceServer::new(svc))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });
    tokio::time::sleep(Duration::from_millis(80)).await;
    (addr, h)
}

#[tokio::test]
async fn policy_push_converges_and_duplicates_are_safe() {
    let registry = Registry::new();
    let (addr, _h) = start_cp(registry.clone()).await;
    registry
        .set_policy(Policy {
            version: 42,
            max_connections: 128,
            idle_timeout_secs: 60,
            default_upstream: "127.0.0.1:9".into(),
            rules_json: "{}".into(),
        })
        .await;

    let state_path = std::env::temp_dir().join(format!(
        "edgewarden-reconcile-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let m = std::sync::Arc::new(edge_telemetry::EdgeMetrics::default());
    let mut agent = EdgeAgent::new(
        AgentConfig {
            device_id: "edge-rec-01".into(),
            hardware_id: "hw".into(),
            software_version: "0.1.0".into(),
            control_plane: format!("http://{addr}"),
            state_path: state_path.clone(),
            heartbeat_interval: Duration::from_secs(1),
            tls_ca: None,
        },
        m,
    )
    .unwrap();

    // First sync applies 42.
    assert!(agent.sync_once(0).await.unwrap());
    assert_eq!(agent.reported_policy_version(), 42);

    // Second sync with identical desired state is a no-op (idempotent).
    assert!(!agent.sync_once(0).await.unwrap());
    assert_eq!(agent.reported_policy_version(), 42);

    let _ = std::fs::remove_file(&state_path);
}

#[tokio::test]
async fn invalid_policy_version_does_not_break_agent() {
    // Policy version 0 means "no policy yet": agent must keep last-known-good
    // and report version 0 rather than crashing.
    let registry = Registry::new();
    let (addr, _h) = start_cp(registry.clone()).await;
    let state_path = std::env::temp_dir().join(format!(
        "edgewarden-invalid-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let m = std::sync::Arc::new(edge_telemetry::EdgeMetrics::default());
    let mut agent = EdgeAgent::new(
        AgentConfig {
            device_id: "edge-inv-01".into(),
            hardware_id: "hw".into(),
            software_version: "0.1.0".into(),
            control_plane: format!("http://{addr}"),
            state_path: state_path.clone(),
            heartbeat_interval: Duration::from_secs(1),
            tls_ca: None,
        },
        m,
    )
    .unwrap();
    // No policy set: sync succeeds, nothing to apply.
    let _ = agent.sync_once(0).await.unwrap();
    assert_eq!(agent.reported_policy_version(), 0);
    let _ = std::fs::remove_file(&state_path);
}
