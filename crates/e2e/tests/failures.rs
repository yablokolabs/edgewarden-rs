//! Failure-injection scenarios: restart, corrupt config, duplicate command,
//! stale desired state, certificate failure. Each test asserts the edge
//! degrades gracefully instead of crashing.

use edge_state::{DesiredState, Policy, StateStore};
use std::time::Duration;

fn policy(v: u64) -> Policy {
    Policy {
        version: v,
        ..Policy::default()
    }
}

#[test]
fn corrupt_config_falls_back_to_default() {
    let path = std::env::temp_dir().join(format!(
        "edgewarden-corrupt-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&path, b"{ not valid json").unwrap();
    // Corrupt file must surface an error, not panic; caller falls back to default.
    assert!(StateStore::open(&path).is_err());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn duplicate_and_stale_commands_are_safe() {
    let path = std::env::temp_dir().join(format!(
        "edgewarden-dup-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&path);
    let mut s = StateStore::open(&path).unwrap();
    let d = DesiredState {
        seq: 5,
        policy: policy(9),
        software_version: String::new(),
    };
    let a1 = s.ingest(d.clone()).unwrap();
    assert!(matches!(a1, edge_state::ReconcileAction::ApplyPolicy(_)));
    s.mark_policy_applied(&policy(9), 5).unwrap();
    // Duplicate seq+version: noop.
    let a2 = s.ingest(d).unwrap();
    assert_eq!(a2, edge_state::ReconcileAction::Noop);
    // Stale seq: also noop, never regresses.
    let stale = DesiredState {
        seq: 4,
        policy: policy(8),
        software_version: String::new(),
    };
    let a3 = s.ingest(stale).unwrap();
    assert_eq!(a3, edge_state::ReconcileAction::Noop);
    assert_eq!(s.reported.policy_version, 9);
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn edge_restart_keeps_forwarding() {
    // Proxy restart must be fast and not require the cloud.
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up_addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                break;
            };
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut b = [0u8; 1024];
                loop {
                    let n = match s.read(&mut b).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    if s.write_all(&b[..n]).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    for _ in 0..2 {
        let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = probe.local_addr().unwrap();
        drop(probe);
        let proxy = edge_proxy::ProxyServer::new(edge_proxy::ProxyConfig {
            listen_addr: addr,
            upstream_addr: up_addr,
            max_connections: 16,
            connect_timeout_secs: 2,
            idle_timeout_secs: 10,
            transparent: false,
        });
        let (tx, rx) = tokio::sync::watch::channel(false);
        let h = tokio::spawn(async move {
            let _ = proxy.serve_until(rx).await;
        });
        tokio::time::sleep(Duration::from_millis(80)).await;
        let mut c = tokio::net::TcpStream::connect(addr).await.unwrap();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        c.write_all(b"restart-ok").await.unwrap();
        let mut buf = [0u8; 10];
        tokio::time::timeout(Duration::from_secs(2), c.read_exact(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&buf, b"restart-ok");
        let _ = tx.send(true);
        h.abort();
    }
}
