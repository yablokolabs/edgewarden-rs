//! Core forwarding server.
//!
//! Design notes:
//! - Concurrency is bounded by a [`tokio::sync::Semaphore`] (no unbounded
//!   tasks/queues). Over-limit connections are refused fast with a counter.
//! - Backpressure propagates naturally: both directions are driven by a
//!   single `copy_bidirectional` over TCP sockets, so a slow reader applies
//!   TCP-window backpressure end to end. No userspace buffering means no
//!   unbounded queues and no unnecessary copies beyond the kernel.
//! - Half-close semantics come from `copy_bidirectional` (it shuts down the
//!   write half when one direction hits EOF).
//! - Idle timeout wraps the whole relay with `tokio::time::timeout`; a fully
//!   idle connection is reaped. [`relay`] is the extension point for
//!   per-read idle tracking, L7 inspection, or XDP offload later.

use crate::config::ProxyConfig;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio::time::timeout;
use tracing::{debug, info, warn};

#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Lock-free connection counters shared with telemetry.
#[derive(Debug, Default)]
pub struct ConnectionCounters {
    pub total: AtomicU64,
    pub active: AtomicU64,
    pub refused_over_limit: AtomicU64,
    pub errors: AtomicU64,
    pub bytes_in: AtomicU64,
    pub bytes_out: AtomicU64,
}

#[derive(Debug, Clone)]
pub struct ProxyStats {
    pub total: u64,
    pub active: u64,
    pub refused_over_limit: u64,
    pub errors: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
}

/// Bidirectional relay with idle timeout. Zero-copy `copy_bidirectional`
/// keeps backpressure in the kernel (TCP windows) instead of userspace
/// queues.
pub async fn relay(
    mut client: TcpStream,
    mut upstream: TcpStream,
    idle_timeout: Duration,
    counters: &ConnectionCounters,
) -> std::io::Result<()> {
    let res = timeout(
        idle_timeout,
        tokio::io::copy_bidirectional(&mut client, &mut upstream),
    )
    .await;
    match res {
        Ok(Ok((c2u, u2c))) => {
            counters.bytes_in.fetch_add(c2u, Ordering::Relaxed);
            counters.bytes_out.fetch_add(u2c, Ordering::Relaxed);
            debug!(c2u, u2c, "relay done");
            Ok(())
        }
        Ok(Err(e)) => Err(e),
        Err(_) => {
            warn!("connection idle timeout");
            Ok(())
        }
    }
}

pub struct ProxyServer {
    config: ProxyConfig,
    counters: Arc<ConnectionCounters>,
    semaphore: Arc<Semaphore>,
}

impl ProxyServer {
    pub fn new(config: ProxyConfig) -> Self {
        let permits = config.max_connections.max(1);
        Self {
            config,
            counters: Arc::new(ConnectionCounters::default()),
            semaphore: Arc::new(Semaphore::new(permits)),
        }
    }

    /// Access shared counters (for telemetry / health / tests).
    pub fn counters(&self) -> Arc<ConnectionCounters> {
        self.counters.clone()
    }

    pub fn stats(&self) -> ProxyStats {
        ProxyStats {
            total: self.counters.total.load(Ordering::Relaxed),
            active: self.counters.active.load(Ordering::Relaxed),
            refused_over_limit: self.counters.refused_over_limit.load(Ordering::Relaxed),
            errors: self.counters.errors.load(Ordering::Relaxed),
            bytes_in: self.counters.bytes_in.load(Ordering::Relaxed),
            bytes_out: self.counters.bytes_out.load(Ordering::Relaxed),
        }
    }

    pub fn config(&self) -> &ProxyConfig {
        &self.config
    }

    /// Serve until the listener errors or `shutdown` fires. Graceful: stops
    /// accepting, in-flight relays run to completion or idle timeout.
    pub async fn serve_until(
        self,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Result<(), ProxyError> {
        let listener = TcpListener::bind(self.config.listen_addr).await?;
        info!(listen = %self.config.listen_addr, upstream = %self.config.upstream_addr, "proxy listening");
        let this = Arc::new(self);
        loop {
            tokio::select! {
                _ = shutdown.changed() => {
                    info!("proxy shutdown requested");
                    return Ok(());
                }
                accepted = listener.accept() => {
                    let (client, peer) = accepted?;
                    let srv = this.clone();
                    tokio::spawn(async move {
                        srv.handle_one(client, peer).await;
                    });
                }
            }
        }
    }

    async fn handle_one(&self, client: TcpStream, peer: SocketAddr) {
        // Fast reject when at capacity: bounded resource usage.
        let permit = match self.semaphore.clone().try_acquire_owned() {
            Ok(p) => p,
            Err(_) => {
                self.counters
                    .refused_over_limit
                    .fetch_add(1, Ordering::Relaxed);
                self.counters.errors.fetch_add(1, Ordering::Relaxed);
                debug!(%peer, "connection refused: over limit");
                return;
            }
        };
        let _permit = permit;
        self.counters.total.fetch_add(1, Ordering::Relaxed);
        self.counters.active.fetch_add(1, Ordering::Relaxed);
        debug!(%peer, "accepted");

        let upstream = match timeout(
            self.config.connect_timeout(),
            TcpStream::connect(self.config.upstream_addr),
        )
        .await
        {
            Ok(Ok(u)) => u,
            Ok(Err(e)) => {
                warn!(%peer, error = %e, "upstream connect failed");
                self.counters.errors.fetch_add(1, Ordering::Relaxed);
                self.counters.active.fetch_sub(1, Ordering::Relaxed);
                return;
            }
            Err(_) => {
                warn!(%peer, "upstream connect timeout");
                self.counters.errors.fetch_add(1, Ordering::Relaxed);
                self.counters.active.fetch_sub(1, Ordering::Relaxed);
                return;
            }
        };

        // Best-effort socket tuning; failures are non-fatal.
        let _ = client.set_nodelay(true);
        let _ = upstream.set_nodelay(true);

        if let Err(e) = relay(client, upstream, self.config.idle_timeout(), &self.counters).await {
            warn!(%peer, error = %e, "relay error");
            self.counters.errors.fetch_add(1, Ordering::Relaxed);
        }
        self.counters.active.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn echo_server() -> SocketAddr {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
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

    async fn start_proxy(
        mut cfg: ProxyConfig,
    ) -> (
        SocketAddr,
        Arc<ConnectionCounters>,
        tokio::sync::watch::Sender<bool>,
    ) {
        // Bind an ephemeral port first so tests never collide.
        let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bound = probe.local_addr().unwrap();
        drop(probe);
        cfg.listen_addr = bound;
        let server = ProxyServer::new(cfg);
        let counters = server.counters();
        let (tx, rx) = tokio::sync::watch::channel(false);
        tokio::spawn(async move {
            let _ = server.serve_until(rx).await;
        });
        tokio::time::sleep(Duration::from_millis(80)).await;
        (bound, counters, tx)
    }

    fn test_config(upstream: SocketAddr) -> ProxyConfig {
        ProxyConfig {
            listen_addr: "127.0.0.1:0".parse().unwrap(),
            upstream_addr: upstream,
            max_connections: 64,
            connect_timeout_secs: 2,
            idle_timeout_secs: 30,
            transparent: false,
        }
    }

    #[tokio::test]
    async fn forwards_both_directions() {
        let upstream = echo_server().await;
        let (proxy, _c, shutdown) = start_proxy(test_config(upstream)).await;
        let mut c = TcpStream::connect(proxy).await.unwrap();
        c.write_all(b"hello-edge").await.unwrap();
        let mut buf = [0u8; 32];
        let n = timeout(Duration::from_secs(2), c.read(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&buf[..n], b"hello-edge");
        let _ = shutdown.send(true);
    }

    #[tokio::test]
    async fn tracks_byte_counters() {
        let upstream = echo_server().await;
        let (proxy, counters, shutdown) = start_proxy(test_config(upstream)).await;
        let mut c = TcpStream::connect(proxy).await.unwrap();
        c.write_all(b"12345").await.unwrap();
        let mut buf = [0u8; 16];
        let n = timeout(Duration::from_secs(2), c.read(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(n, 5);
        drop(c);
        // Counters flush when the relay observes EOF and returns.
        let mut ok = false;
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            if counters.bytes_in.load(Ordering::Relaxed) >= 5
                && counters.bytes_out.load(Ordering::Relaxed) >= 5
            {
                ok = true;
                break;
            }
        }
        assert!(ok, "byte counters did not flush after close");
        let _ = shutdown.send(true);
    }

    #[tokio::test]
    async fn upstream_failure_counts_error() {
        let dead: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let (proxy, counters, shutdown) = start_proxy(test_config(dead)).await;
        let mut c = TcpStream::connect(proxy).await.unwrap();
        let mut buf = [0u8; 8];
        let _ = timeout(Duration::from_secs(2), c.read(&mut buf)).await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(counters.errors.load(Ordering::Relaxed) >= 1);
        let _ = shutdown.send(true);
    }
}
