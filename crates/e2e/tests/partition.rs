//! Network-partition tolerance: the dataplane must survive cloud loss.
//!
//! Scenario:
//! 1. control plane up, appliance connected
//! 2. policy downloaded
//! 3. cloud disconnected (server stopped)
//! 4. proxy keeps forwarding on last-known-good
//! 5. cloud returns
//! 6. appliance reconnects
//! 7. state reconciles to the newest desired policy

use control_plane::{FleetServiceImpl, Registry};
use edge_agent::{AgentConfig, EdgeAgent};
use edge_protocol::fleet::{fleet_service_server::FleetServiceServer, Policy};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

async fn echo_upstream() -> std::net::SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                loop {
                    let n = match s.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    if s.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    addr
}

async fn proxy_roundtrip(proxy: std::net::SocketAddr, payload: &[u8]) -> Vec<u8> {
    let mut c = TcpStream::connect(proxy).await.expect("proxy dial");
    c.write_all(payload).await.expect("write");
    let mut buf = vec![0u8; payload.len()];
    tokio::time::timeout(Duration::from_secs(3), c.read_exact(&mut buf))
        .await
        .expect("read timeout")
        .expect("read");
    buf
}

async fn start_control_plane(
    registry: Registry,
) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let svc = FleetServiceImpl::new(registry);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    let h = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(FleetServiceServer::new(svc))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    (addr, h)
}

#[tokio::test]
async fn partition_proxy_survives_and_reconciles() {
    // --- control plane up ---
    let registry = Registry::new();
    registry
        .set_policy(Policy {
            version: 10,
            max_connections: 64,
            idle_timeout_secs: 30,
            default_upstream: "127.0.0.1:9".into(),
            rules_json: "{}".into(),
        })
        .await;
    let (cp_addr, cp_handle) = start_control_plane(registry.clone()).await;

    // --- edge proxy on last-known-good ---
    let upstream = echo_upstream().await;
    let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = probe.local_addr().unwrap();
    drop(probe);
    let proxy = edge_proxy::ProxyServer::new(edge_proxy::ProxyConfig {
        listen_addr: proxy_addr,
        upstream_addr: upstream,
        max_connections: 64,
        connect_timeout_secs: 2,
        idle_timeout_secs: 30,
        transparent: false,
    });
    let (shtx, shrx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        let _ = proxy.serve_until(shrx).await;
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    // --- 1-2. connected: download policy via agent sync ---
    let state_path = std::env::temp_dir().join(format!(
        "edgewarden-e2e-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let metrics = std::sync::Arc::new(edge_telemetry::EdgeMetrics::default());
    let mut agent = EdgeAgent::new(
        AgentConfig {
            device_id: "edge-e2e-01".into(),
            hardware_id: "hw-e2e".into(),
            software_version: "0.1.0".into(),
            control_plane: format!("http://{cp_addr}"),
            state_path: state_path.clone(),
            heartbeat_interval: Duration::from_secs(1),
            tls_ca: None,
            tls_cert: None,
            tls_key: None,
            tls_domain: None,
        },
        metrics,
    )
    .unwrap();
    agent.sync_once(0).await.expect("initial sync");
    assert_eq!(agent.reported_policy_version(), 10);

    // Proxy works while connected.
    assert_eq!(proxy_roundtrip(proxy_addr, b"ping-1").await, b"ping-1");

    // --- 3. cloud disappears ---
    cp_handle.abort();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Sync must now fail, but with the agent keeping last-known-good.
    let sync_result = agent.sync_once(0).await;
    assert!(
        sync_result.is_err(),
        "expected sync to fail while partitioned"
    );

    // --- 4. proxy STILL forwards on last-known-good ---
    assert_eq!(
        proxy_roundtrip(proxy_addr, b"ping-during-outage").await,
        b"ping-during-outage"
    );

    // --- operator pushes a newer policy while partitioned (queued in registry clone) ---
    registry
        .set_policy(Policy {
            version: 11,
            max_connections: 64,
            idle_timeout_secs: 30,
            default_upstream: "127.0.0.1:9".into(),
            rules_json: "{}".into(),
        })
        .await;

    // --- 5. cloud returns ---
    let (cp_addr2, _h2) = start_control_plane(registry.clone()).await;

    // --- 6-7. reconnect + reconcile to newest ---
    // Point the agent at the new address (DHCP/DNS change tolerated).
    let metrics2 = std::sync::Arc::new(edge_telemetry::EdgeMetrics::default());
    let mut agent2 = EdgeAgent::new(
        AgentConfig {
            device_id: "edge-e2e-01".into(),
            hardware_id: "hw-e2e".into(),
            software_version: "0.1.0".into(),
            control_plane: format!("http://{cp_addr2}"),
            state_path: state_path.clone(),
            heartbeat_interval: Duration::from_secs(1),
            tls_ca: None,
            tls_cert: None,
            tls_key: None,
            tls_domain: None,
        },
        metrics2,
    )
    .unwrap();
    agent2.sync_once(0).await.expect("reconnect sync");
    assert_eq!(agent2.reported_policy_version(), 11);

    let _ = shtx.send(true);
    let _ = std::fs::remove_file(&state_path);
}
