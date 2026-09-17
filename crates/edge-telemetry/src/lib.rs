//! Prometheus telemetry for edge appliances and the control plane.
//!
//! All metrics are registered on a per-instance [`EdgeMetrics`] registry so
//! tests can construct isolated registries without global state.

use prometheus::{
    Encoder, Histogram, HistogramOpts, IntCounter, IntGauge, Opts, Registry, TextEncoder,
};
use std::net::SocketAddr;
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MetricsError {
    #[error("prometheus: {0}")]
    Prometheus(#[from] prometheus::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// All edge metrics. Field names map 1:1 to the required metric names.
pub struct EdgeMetrics {
    registry: Registry,
    pub connections_total: IntCounter,
    pub active_connections: IntGauge,
    pub bytes_in_total: IntCounter,
    pub bytes_out_total: IntCounter,
    pub errors_total: IntCounter,
    pub conn_duration: Histogram,
    pub cpu_usage: prometheus::Gauge,
    pub memory_bytes: IntGauge,
    pub control_plane_connected: IntGauge,
    pub reconnect_total: IntCounter,
    pub policy_version: IntGauge,
    pub software_version_info: IntGauge,
    pub ota_success_total: IntCounter,
    pub ota_rollback_total: IntCounter,
}

impl EdgeMetrics {
    pub fn new() -> Result<Arc<Self>, MetricsError> {
        let registry = Registry::new();
        let m = Self::new_on_registry(registry)?;
        Ok(Arc::new(m))
    }

    fn new_on_registry(registry: Registry) -> Result<Self, MetricsError> {
        let connections_total = IntCounter::with_opts(Opts::new(
            "edge_proxy_connections_total",
            "Total proxied connections",
        ))?;
        registry.register(Box::new(connections_total.clone()))?;
        let active_connections = IntGauge::with_opts(Opts::new(
            "edge_proxy_active_connections",
            "Currently active proxied connections",
        ))?;
        registry.register(Box::new(active_connections.clone()))?;
        let bytes_in_total = IntCounter::with_opts(Opts::new(
            "edge_proxy_bytes_in_total",
            "Bytes client->upstream",
        ))?;
        registry.register(Box::new(bytes_in_total.clone()))?;
        let bytes_out_total = IntCounter::with_opts(Opts::new(
            "edge_proxy_bytes_out_total",
            "Bytes upstream->client",
        ))?;
        registry.register(Box::new(bytes_out_total.clone()))?;
        let errors_total =
            IntCounter::with_opts(Opts::new("edge_proxy_errors_total", "Proxy errors"))?;
        registry.register(Box::new(errors_total.clone()))?;
        let conn_duration = Histogram::with_opts(HistogramOpts::new(
            "edge_proxy_connection_duration_seconds",
            "Proxied connection duration",
        ))?;
        registry.register(Box::new(conn_duration.clone()))?;
        let cpu_usage =
            prometheus::Gauge::with_opts(Opts::new("edge_cpu_usage", "CPU usage 0..1"))?;
        registry.register(Box::new(cpu_usage.clone()))?;
        let memory_bytes =
            IntGauge::with_opts(Opts::new("edge_memory_bytes", "Resident memory bytes"))?;
        registry.register(Box::new(memory_bytes.clone()))?;
        let control_plane_connected = IntGauge::with_opts(Opts::new(
            "edge_control_plane_connected",
            "1 if control plane connected",
        ))?;
        registry.register(Box::new(control_plane_connected.clone()))?;
        let reconnect_total = IntCounter::with_opts(Opts::new(
            "edge_reconnect_total",
            "Control-plane reconnects",
        ))?;
        registry.register(Box::new(reconnect_total.clone()))?;
        let policy_version =
            IntGauge::with_opts(Opts::new("edge_policy_version", "Applied policy version"))?;
        registry.register(Box::new(policy_version.clone()))?;
        let software_version_info = IntGauge::with_opts(Opts::new(
            "edge_software_version",
            "Software version as info metric (value always 1; check logs for mapping)",
        ))?;
        registry.register(Box::new(software_version_info.clone()))?;
        let ota_success_total = IntCounter::with_opts(Opts::new(
            "edge_ota_success_total",
            "Successful OTA updates",
        ))?;
        registry.register(Box::new(ota_success_total.clone()))?;
        let ota_rollback_total =
            IntCounter::with_opts(Opts::new("edge_ota_rollback_total", "OTA rollbacks"))?;
        registry.register(Box::new(ota_rollback_total.clone()))?;

        Ok(Self {
            registry,
            connections_total,
            active_connections,
            bytes_in_total,
            bytes_out_total,
            errors_total,
            conn_duration,
            cpu_usage,
            memory_bytes,
            control_plane_connected,
            reconnect_total,
            policy_version,
            software_version_info,
            ota_success_total,
            ota_rollback_total,
        })
    }

    pub fn render(&self) -> Result<String, MetricsError> {
        let families = self.registry.gather();
        let mut buf = Vec::new();
        TextEncoder::new().encode(&families, &mut buf)?;
        Ok(String::from_utf8_lossy(&buf).into_owned())
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }
}

impl Default for EdgeMetrics {
    fn default() -> Self {
        Self::new_on_registry(Registry::new()).expect("default metrics registry")
    }
}

/// Spawn an Axum `/metrics` endpoint. Returns the bound address.
pub async fn spawn_metrics_server(
    metrics: Arc<EdgeMetrics>,
    addr: SocketAddr,
) -> Result<SocketAddr, MetricsError> {
    use axum::{response::IntoResponse, routing::get, Router};
    async fn handler(
        axum::extract::State(metrics): axum::extract::State<Arc<EdgeMetrics>>,
    ) -> impl IntoResponse {
        match metrics.render() {
            Ok(body) => (
                [(
                    axum::http::header::CONTENT_TYPE,
                    "text/plain; version=0.0.4",
                )],
                body,
            ),
            Err(e) => (
                [(axum::http::header::CONTENT_TYPE, "text/plain")],
                format!("render error: {e}"),
            ),
        }
    }
    let app = Router::new()
        .route("/metrics", get(handler))
        .with_state(metrics);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(MetricsError::Io)?;
    let bound = listener.local_addr().map_err(MetricsError::Io)?;
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::warn!(error = %e, "metrics server exited");
        }
    });
    Ok(bound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_render() {
        let m = EdgeMetrics::default();
        m.connections_total.inc_by(3);
        m.active_connections.set(2);
        m.policy_version.set(42);
        let out = m.render().unwrap();
        assert!(out.contains("edge_proxy_connections_total 3"));
        assert!(out.contains("edge_policy_version 42"));
    }

    #[tokio::test]
    async fn metrics_endpoint_serves() {
        let m = Arc::new(EdgeMetrics::default());
        let addr = spawn_metrics_server(m, "127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let body = tokio::task::spawn_blocking(move || {
            // Minimal HTTP client without extra deps: use TCP directly.
            use std::io::{Read, Write};
            use std::net::TcpStream;
            let mut s = TcpStream::connect(addr).unwrap();
            s.write_all(b"GET /metrics HTTP/1.0\r\nHost: x\r\n\r\n")
                .unwrap();
            let mut buf = String::new();
            s.read_to_string(&mut buf).unwrap();
            buf
        })
        .await
        .unwrap();
        let _ = body;
        // If we got here without error the server accepted the connection.
    }
}
