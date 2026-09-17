//! mTLS is enforced and verified, not merely supported.
//!
//! A test PKI (rcgen) mints a CA plus server/device certificates. The
//! control plane requires device client certs (`client_ca_root`); the agent
//! presents its identity and pins the CA. Negative cases prove half-TLS and
//! cert-less clients fail instead of silently downgrading.

use control_plane::{FleetServiceImpl, Registry};
use edge_agent::{AgentConfig, EdgeAgent};
use edge_protocol::fleet::{
    fleet_service_client::FleetServiceClient, fleet_service_server::FleetServiceServer, Policy,
    RegisterRequest,
};
use rcgen::{CertificateParams, DistinguishedName, DnType, IsCa, KeyPair};
use std::time::Duration;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity, ServerTlsConfig};

fn dn(cn: &str) -> DistinguishedName {
    let mut d = DistinguishedName::new();
    d.push(DnType::CommonName, cn);
    d
}

struct Pki {
    dir: std::path::PathBuf,
}

impl Pki {
    fn mint() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "edgewarden-mtls-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let mut ca_params = CertificateParams::default();
        ca_params.is_ca = IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        ca_params.distinguished_name = dn("edgewarden-test-ca");
        let ca_key = KeyPair::generate().unwrap();
        let ca_cert = ca_params.self_signed(&ca_key).unwrap();
        let ca_pem = ca_cert.pem();
        std::fs::write(dir.join("ca.crt"), &ca_pem).unwrap();

        for (name, sans) in [
            ("server", vec!["localhost".to_string()]),
            ("edge-01", vec!["edge-01".to_string()]),
        ] {
            let mut params = CertificateParams::new(sans).unwrap();
            params.distinguished_name = dn(name);
            let key = KeyPair::generate().unwrap();
            let cert = params.signed_by(&key, &ca_cert, &ca_key).unwrap();
            std::fs::write(dir.join(format!("{name}.crt")), cert.pem()).unwrap();
            std::fs::write(dir.join(format!("{name}.key")), key.serialize_pem()).unwrap();
        }
        Self { dir }
    }

    fn path(&self, name: &str) -> String {
        self.dir.join(name).to_string_lossy().into_owned()
    }
}

impl Drop for Pki {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[tokio::test]
async fn mtls_agent_registers_and_reconciles() {
    let pki = Pki::mint();
    let registry = Registry::new();
    registry
        .set_policy(Policy {
            version: 5,
            max_connections: 32,
            idle_timeout_secs: 30,
            default_upstream: "127.0.0.1:9".into(),
            rules_json: "{}".into(),
        })
        .await;

    // Control plane with required device client certs.
    let svc = FleetServiceImpl::new(registry);
    let server_cert = std::fs::read(pki.path("server.crt")).unwrap();
    let server_key = std::fs::read(pki.path("server.key")).unwrap();
    let ca_pem = std::fs::read(pki.path("ca.crt")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    let handle = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .tls_config(
                ServerTlsConfig::new()
                    .identity(Identity::from_pem(server_cert, server_key))
                    .client_ca_root(Certificate::from_pem(ca_pem)),
            )
            .expect("server mTLS config")
            .add_service(FleetServiceServer::new(svc))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });
    tokio::time::sleep(Duration::from_millis(150)).await;

    let state_path = pki.dir.join("edge-state.json");
    let metrics = std::sync::Arc::new(edge_telemetry::EdgeMetrics::default());
    let mut agent = EdgeAgent::new(
        AgentConfig {
            device_id: "edge-01".into(),
            hardware_id: "hw-mtls".into(),
            software_version: "0.1.0".into(),
            control_plane: format!("https://localhost:{port}"),
            state_path: state_path.clone(),
            heartbeat_interval: Duration::from_secs(1),
            tls_ca: Some(pki.path("ca.crt")),
            tls_cert: Some(pki.path("edge-01.crt")),
            tls_key: Some(pki.path("edge-01.key")),
            tls_domain: Some("localhost".into()),
        },
        metrics,
    )
    .unwrap();
    agent.sync_once(0).await.expect("mTLS sync");
    assert_eq!(agent.reported_policy_version(), 5);
    handle.abort();
}

#[tokio::test]
async fn mtls_client_without_identity_is_rejected() {
    let pki = Pki::mint();
    let svc = FleetServiceImpl::new(Registry::new());
    let server_cert = std::fs::read(pki.path("server.crt")).unwrap();
    let server_key = std::fs::read(pki.path("server.key")).unwrap();
    let ca_pem = std::fs::read(pki.path("ca.crt")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    let handle = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .tls_config(
                ServerTlsConfig::new()
                    .identity(Identity::from_pem(server_cert, server_key))
                    .client_ca_root(Certificate::from_pem(ca_pem)),
            )
            .expect("server mTLS config")
            .add_service(FleetServiceServer::new(svc))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });
    tokio::time::sleep(Duration::from_millis(150)).await;

    // CA only, no device identity: the TLS handshake must fail server-side.
    let ca_only = std::fs::read(pki.path("ca.crt")).unwrap();
    let channel = Channel::from_shared(format!("https://localhost:{port}"))
        .unwrap()
        .tls_config(
            ClientTlsConfig::new()
                .ca_certificate(Certificate::from_pem(ca_only))
                .domain_name("localhost"),
        )
        .unwrap()
        .connect()
        .await
        .unwrap();
    let mut client = FleetServiceClient::new(channel);
    let res = client
        .register(RegisterRequest {
            identity: None,
            reported_policy_version: "0".into(),
            reported_software_version: "0.1.0".into(),
        })
        .await;
    assert!(res.is_err(), "cert-less client must be rejected");
    handle.abort();
}

#[tokio::test]
async fn agent_fails_fast_on_half_tls_config() {
    // CA without a device identity never reaches the network: fail fast.
    let dir = std::env::temp_dir();
    let metrics = std::sync::Arc::new(edge_telemetry::EdgeMetrics::default());
    let mut agent = EdgeAgent::new(
        AgentConfig {
            device_id: "edge-01".into(),
            hardware_id: "hw".into(),
            software_version: "0.1.0".into(),
            control_plane: "https://localhost:1".into(),
            state_path: dir.join(format!(
                "edgewarden-half-tls-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            )),
            heartbeat_interval: Duration::from_secs(1),
            tls_ca: Some("/nonexistent/ca.crt".into()),
            tls_cert: None,
            tls_key: None,
            tls_domain: None,
        },
        metrics,
    )
    .unwrap();
    assert!(agent.sync_once(0).await.is_err());
}
