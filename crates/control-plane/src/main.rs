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
    // mTLS is supported via tonic TLS config; for the local demo these are
    // optional and plaintext is used with a warning. See docs/security.md.
    #[arg(long, env = "CONTROL_TLS_CERT")]
    tls_cert: Option<String>,
    #[arg(long, env = "CONTROL_TLS_KEY")]
    tls_key: Option<String>,
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
    let registry = Registry::new();
    let svc = FleetServiceImpl::new(registry.clone());

    let grpc_addr = args.grpc_addr;
    let grpc = tokio::spawn(async move {
        let mut builder = tonic::transport::Server::builder();
        // mTLS: if cert+key are provided, terminate TLS here. Client CA
        // verification should be added for full mTLS in production.
        if let (Some(cert), Some(key)) = (args_tls_cert(), args_tls_key()) {
            let cert = std::fs::read(&cert)?;
            let key = std::fs::read(&key)?;
            let identity = tonic::transport::Identity::from_pem(cert, key);
            builder =
                builder.tls_config(tonic::transport::ServerTlsConfig::new().identity(identity))?;
        } else {
            tracing::warn!("starting gRPC WITHOUT TLS (dev/demo mode); use CONTROL_TLS_* for mTLS");
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

fn args_tls_cert() -> Option<String> {
    std::env::var("CONTROL_TLS_CERT").ok()
}
fn args_tls_key() -> Option<String> {
    std::env::var("CONTROL_TLS_KEY").ok()
}
