//! `edge-agent` binary: proxy dataplane + fleet sync + health + metrics.

use anyhow::Context;
use clap::Parser;
use edge_agent::{AgentConfig, EdgeAgent};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::info;

#[derive(Debug, Parser)]
#[command(name = "edge-agent")]
struct Args {
    #[arg(long, env = "EDGE_DEVICE_ID", default_value = "edge-01")]
    device_id: String,
    #[arg(long, env = "EDGE_HARDWARE_ID", default_value = "hw-01")]
    hardware_id: String,
    #[arg(long, env = "EDGE_SOFTWARE_VERSION", default_value = "0.1.0")]
    software_version: String,
    #[arg(
        long,
        env = "EDGE_CONTROL_PLANE",
        default_value = "http://127.0.0.1:50051"
    )]
    control_plane: String,
    #[arg(long, env = "EDGE_STATE_PATH", default_value = "edge-state.json")]
    state_path: PathBuf,
    #[arg(long, env = "EDGE_LISTEN", default_value = "127.0.0.1:18080")]
    listen: SocketAddr,
    #[arg(long, env = "EDGE_UPSTREAM", default_value = "127.0.0.1:18081")]
    upstream: SocketAddr,
    #[arg(long, env = "EDGE_METRICS_ADDR", default_value = "127.0.0.1:19091")]
    metrics_addr: SocketAddr,
    #[arg(long, env = "EDGE_TLS_CA")]
    tls_ca: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args = Args::parse();
    let metrics = Arc::new(edge_telemetry::EdgeMetrics::default());
    edge_telemetry::spawn_metrics_server(metrics.clone(), args.metrics_addr)
        .await
        .context("metrics bind")?;

    // Start the dataplane FIRST on last-known-good so a cloud outage at boot
    // still leaves forwarding working.
    let agent_cfg = AgentConfig {
        device_id: args.device_id.clone(),
        hardware_id: args.hardware_id.clone(),
        software_version: args.software_version.clone(),
        control_plane: args.control_plane.clone(),
        state_path: args.state_path.clone(),
        heartbeat_interval: Duration::from_secs(5),
        tls_ca: args.tls_ca.clone(),
    };
    let probe = EdgeAgent::new(agent_cfg.clone(), metrics.clone()).context("open state store")?;
    let last_good = probe.last_good_policy();
    drop(probe);

    let proxy_cfg = edge_proxy::ProxyConfig {
        listen_addr: args.listen,
        upstream_addr: if last_good.default_upstream.is_empty() {
            args.upstream
        } else {
            last_good.default_upstream.parse().unwrap_or(args.upstream)
        },
        max_connections: last_good.max_connections as usize,
        connect_timeout_secs: 5,
        idle_timeout_secs: last_good.idle_timeout_secs,
        transparent: false,
    };
    let proxy = edge_proxy::ProxyServer::new(proxy_cfg);
    let proxy_counters = proxy.counters();
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let proxy_handle = tokio::spawn(async move { proxy.serve_until(shutdown_rx).await });

    let agent = EdgeAgent::new(agent_cfg, metrics.clone()).context("open state store")?;
    let (_a_tx, a_rx) = tokio::sync::watch::channel(false);
    let agent_handle = tokio::spawn(async move { agent.run(a_rx).await });

    // Bridge proxy counters into Prometheus.
    let m2 = metrics.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        loop {
            tick.tick().await;
            m2.active_connections.set(
                proxy_counters
                    .active
                    .load(std::sync::atomic::Ordering::Relaxed) as i64,
            );
        }
    });

    info!(device = %args.device_id, "edge agent up (dataplane independent of cloud)");

    tokio::signal::ctrl_c().await.context("signal")?;
    let _ = shutdown_tx.send(true);
    agent_handle.abort();
    tokio::time::sleep(Duration::from_secs(1)).await;
    proxy_handle.abort();
    Ok(())
}
