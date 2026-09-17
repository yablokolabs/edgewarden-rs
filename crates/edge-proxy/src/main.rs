//! `edge-proxy` binary: portable TCP forwarder with metrics + graceful shutdown.

use anyhow::Context;
use clap::Parser;
use edge_proxy::{ProxyConfig, ProxyServer};
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::info;

#[derive(Debug, Parser)]
#[command(name = "edge-proxy", about = "EdgeWarden TCP proxy dataplane")]
struct Args {
    #[arg(long, env = "EDGE_LISTEN", default_value = "127.0.0.1:18080")]
    listen: SocketAddr,
    /// Upstream as `IP:port` or DNS `host:port` (resolved once at startup;
    /// container/K8s service names work here).
    #[arg(long, env = "EDGE_UPSTREAM", default_value = "127.0.0.1:18081")]
    upstream: String,
    #[arg(long, env = "EDGE_MAX_CONN", default_value_t = 512)]
    max_connections: usize,
    #[arg(long, env = "EDGE_CONNECT_TIMEOUT_SECS", default_value_t = 5)]
    connect_timeout_secs: u64,
    #[arg(long, env = "EDGE_IDLE_TIMEOUT_SECS", default_value_t = 300)]
    idle_timeout_secs: u64,
    #[arg(long, env = "EDGE_METRICS_ADDR", default_value = "127.0.0.1:19090")]
    metrics_addr: SocketAddr,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args = Args::parse();
    let upstream = resolve_upstream(&args.upstream)
        .await
        .with_context(|| format!("resolving upstream {}", args.upstream))?;
    let config = ProxyConfig {
        listen_addr: args.listen,
        upstream_addr: upstream,
        max_connections: args.max_connections,
        connect_timeout_secs: args.connect_timeout_secs,
        idle_timeout_secs: args.idle_timeout_secs,
        transparent: false,
    };
    let metrics = Arc::new(edge_telemetry::EdgeMetrics::default());
    let metrics_clone = metrics.clone();
    let proxy_metrics = metrics.clone();

    // Metrics endpoint (best effort; proxy works even if it fails).
    match edge_telemetry::spawn_metrics_server(metrics_clone, args.metrics_addr).await {
        Ok(bound) => info!(%bound, "metrics listening"),
        Err(e) => tracing::warn!(error = %e, "metrics server failed to start"),
    }

    let server = ProxyServer::new(config);
    let counters = server.counters();

    // Bridge proxy counters into Prometheus every 5s.
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            tick.tick().await;
            proxy_metrics
                .active_connections
                .set(counters.active.load(std::sync::atomic::Ordering::Relaxed) as i64);
            proxy_metrics.connections_total.inc_by(
                counters
                    .total
                    .load(std::sync::atomic::Ordering::Relaxed)
                    .saturating_sub(proxy_metrics.connections_total.get()),
            );
        }
    });

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let serve = tokio::spawn(async move { server.serve_until(shutdown_rx).await });

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("SIGINT received, draining");
        }
        r = serve => {
            let inner = r.context("proxy task panicked")?;
            inner.context("proxy serve failed")?;
            return Ok(());
        }
    }
    let _ = shutdown_tx.send(true);
    // Give in-flight connections a moment; idle timeout bounds the wait.
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    info!("shutdown complete");
    Ok(())
}

/// Resolve `IP:port` or DNS `host:port` to a socket address (first result).
/// DNS is resolved once at startup and logged, so operators can see exactly
/// which backend the dataplane pinned.
pub async fn resolve_upstream(spec: &str) -> anyhow::Result<SocketAddr> {
    if let Ok(addr) = spec.parse::<SocketAddr>() {
        return Ok(addr);
    }
    let mut addrs = tokio::net::lookup_host(spec)
        .await
        .with_context(|| format!("DNS lookup for upstream {spec}"))?;
    addrs
        .next()
        .with_context(|| format!("DNS for upstream {spec} returned no addresses"))
}
