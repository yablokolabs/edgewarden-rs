//! `control-plane` binary: gRPC fleet service + operator REST + metrics.

use anyhow::Context;
use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use clap::Parser;
use control_plane::{FleetServiceImpl, Registry};
use edge_protocol::fleet::{fleet_service_server::FleetServiceServer, Policy};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

#[derive(Debug, Parser)]
#[command(name = "control-plane")]
struct Args {
    #[arg(long, env = "CONTROL_GRPC_ADDR", default_value = "0.0.0.0:50051")]
    grpc_addr: SocketAddr,
    #[arg(long, env = "CONTROL_HTTP_ADDR", default_value = "0.0.0.0:8080")]
    http_addr: SocketAddr,
    /// Server certificate + key for gRPC TLS. Production deployments must
    /// set these together with `tls_client_ca` (full mTLS). When absent,
    /// the server runs plaintext and logs a warning — local demo only.
    /// See docs/security.md.
    #[arg(long, env = "CONTROL_TLS_CERT")]
    tls_cert: Option<String>,
    #[arg(long, env = "CONTROL_TLS_KEY")]
    tls_key: Option<String>,
    /// CA certificate used to verify edge device client certificates.
    /// REQUIRED whenever `tls_cert`/`tls_key` are set: the server refuses
    /// to start with one-way TLS so a misconfigured pilot cannot silently
    /// run without device authentication.
    #[arg(long, env = "CONTROL_TLS_CLIENT_CA")]
    tls_client_ca: Option<String>,
    /// Path for the crash-safe registry snapshot (created if absent).
    /// Without it the registry is memory-only and fleet state is lost on
    /// restart — not production-acceptable.
    #[arg(long, env = "CONTROL_PERSIST_PATH")]
    persist_path: Option<String>,
}

#[derive(Debug, Clone)]
struct HttpState {
    registry: Registry,
}

#[derive(Debug, Serialize)]
struct DeviceView {
    device_id: String,
    reported_policy_version: String,
    desired_policy_version: u64,
    health: i32,
    last_seen_age_secs: u64,
    active_connections: u64,
}

#[derive(Debug, Deserialize)]
struct PolicyBody {
    version: u64,
    #[serde(default = "default_max_conn")]
    max_connections: u32,
    #[serde(default = "default_idle")]
    idle_timeout_secs: u64,
    #[serde(default = "default_upstream")]
    default_upstream: String,
}
fn default_max_conn() -> u32 {
    512
}
fn default_idle() -> u64 {
    300
}
fn default_upstream() -> String {
    "upstream:18081".into()
}

async fn list_devices(State(s): State<HttpState>) -> Json<Vec<DeviceView>> {
    let devs = s.registry.devices().await;
    Json(
        devs.into_iter()
            .map(|d| {
                let age = d.last_seen_age_secs();
                DeviceView {
                    device_id: d.device_id,
                    reported_policy_version: d.reported_policy_version,
                    desired_policy_version: d.desired_policy_version,
                    health: d.health,
                    last_seen_age_secs: age,
                    active_connections: d.active_connections,
                }
            })
            .collect(),
    )
}

async fn push_policy(
    State(s): State<HttpState>,
    Json(b): Json<PolicyBody>,
) -> Json<serde_json::Value> {
    let seq = s
        .registry
        .set_policy(Policy {
            version: b.version,
            max_connections: b.max_connections,
            idle_timeout_secs: b.idle_timeout_secs,
            default_upstream: b.default_upstream,
            rules_json: "{}".into(),
        })
        .await;
    Json(serde_json::json!({"seq": seq, "version": b.version}))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args = Args::parse();
    let registry = match &args.persist_path {
        Some(p) => Registry::load(p).await.context("load registry snapshot")?,
        None => {
            tracing::warn!(
                "no CONTROL_PERSIST_PATH set: registry is memory-only and fleet state will be lost on restart"
            );
            Registry::new()
        }
    };
    let svc = FleetServiceImpl::new(registry.clone());

    let grpc_addr = args.grpc_addr;
    let tls_cert = args.tls_cert.clone();
    let tls_key = args.tls_key.clone();
    let tls_client_ca = args.tls_client_ca.clone();
    let grpc = tokio::spawn(async move {
        let mut builder = tonic::transport::Server::builder();
        match (tls_cert, tls_key, tls_client_ca) {
            (Some(cert_path), Some(key_path), Some(ca_path)) => {
                let cert = std::fs::read(&cert_path)?;
                let key = std::fs::read(&key_path)?;
                let ca = std::fs::read(&ca_path)?;
                let identity = tonic::transport::Identity::from_pem(cert, key);
                let client_ca = tonic::transport::Certificate::from_pem(ca);
                builder = builder.tls_config(
                    tonic::transport::ServerTlsConfig::new()
                        .identity(identity)
                        .client_ca_root(client_ca),
                )?;
                tracing::info!("gRPC mTLS enabled (device client certs required)");
            }
            (Some(_), Some(_), None) => {
                anyhow::bail!(
                    "CONTROL_TLS_CLIENT_CA is required with CONTROL_TLS_CERT/KEY: \
                     refusing one-way TLS without device authentication"
                );
            }
            (None, None, _) => {
                tracing::warn!(
                    "starting gRPC WITHOUT TLS (local demo only); set CONTROL_TLS_* for mTLS"
                );
            }
            _ => {
                anyhow::bail!("set both CONTROL_TLS_CERT and CONTROL_TLS_KEY, or neither");
            }
        }
        builder
            .add_service(FleetServiceServer::new(svc))
            .serve(grpc_addr)
            .await?;
        Ok::<(), anyhow::Error>(())
    });

    let http_state = HttpState { registry };
    let app = Router::new()
        .route("/devices", get(list_devices))
        .route("/policy", post(push_policy))
        .route("/healthz", get(|| async { "ok" }))
        .route(
            "/readyz",
            get(|State(s): State<HttpState>| async move {
                // Ready when the registry is reachable; extend with fleet
                // quorum checks as needed.
                let _ = s.registry.devices().await;
                "ready"
            }),
        )
        .with_state(http_state);
    let listener = tokio::net::TcpListener::bind(args.http_addr)
        .await
        .context("bind http")?;
    tracing::info!(grpc = %args.grpc_addr, http = %args.http_addr, "control plane up");
    let http = tokio::spawn(async move { axum::serve(listener, app).await });

    let (g, h) = tokio::join!(grpc, http);
    g??;
    h??;
    Ok(())
}
