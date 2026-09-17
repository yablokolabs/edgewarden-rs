//! Health supervision: probes, watchdog, and fail-open bypass abstraction.
//!
//! Scope discipline: software cannot implement physical fail-to-wire. This
//! crate therefore exposes a [`BypassController`] trait. Real hardware
//! bypass NICs / relay boards implement it; tests and development use
//! [`MockBypassController`]. See `docs/architecture.md`.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::RwLock;

#[derive(Debug, Error)]
pub enum HealthError {
    #[error("probe failed: {0}")]
    Probe(String),
    #[error("bypass: {0}")]
    Bypass(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum HealthState {
    #[default]
    Unknown,
    Healthy,
    Degraded,
    Unhealthy,
}

impl HealthState {
    pub fn is_healthy(self) -> bool {
        matches!(self, Self::Healthy)
    }
}

/// Physical fail-open bypass. Software can only *request* bypass; the relay
/// itself must be hardware. Never claim otherwise.
#[async_trait::async_trait]
pub trait BypassController: Send + Sync + std::fmt::Debug {
    async fn enable_bypass(&self) -> Result<(), HealthError>;
    async fn disable_bypass(&self) -> Result<(), HealthError>;
    fn bypass_enabled(&self) -> bool;
}

#[derive(Debug, Default)]
pub struct MockBypassController {
    enabled: AtomicBool,
}

impl MockBypassController {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl BypassController for MockBypassController {
    async fn enable_bypass(&self) -> Result<(), HealthError> {
        self.enabled.store(true, Ordering::SeqCst);
        tracing::warn!("mock bypass ENABLED (no real hardware affected)");
        Ok(())
    }
    async fn disable_bypass(&self) -> Result<(), HealthError> {
        self.enabled.store(false, Ordering::SeqCst);
        Ok(())
    }
    fn bypass_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }
}

/// Point-in-time resource snapshot read from `/proc` (Linux) with graceful
/// fallback elsewhere.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceSnapshot {
    pub cpu_usage: f64,
    pub memory_bytes: u64,
    pub uptime_secs: u64,
}

pub fn sample_resources() -> ResourceSnapshot {
    ResourceSnapshot {
        cpu_usage: read_cpu_usage().unwrap_or(0.0),
        memory_bytes: read_memory_bytes().unwrap_or(0),
        uptime_secs: read_uptime().unwrap_or(0),
    }
}

fn read_cpu_usage() -> Option<f64> {
    // Cheap instantaneous estimate from /proc/stat is noisy; for a reference
    // implementation we report 0..1 based on 1-min loadavg / ncpu.
    let load = std::fs::read_to_string("/proc/loadavg")
        .ok()?
        .split_whitespace()
        .next()?
        .parse::<f64>()
        .ok()?;
    let ncpu = std::thread::available_parallelism().ok()?.get() as f64;
    Some((load / ncpu).clamp(0.0, 1.0))
}

fn read_memory_bytes() -> Option<u64> {
    let content = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kb * 1024);
        }
    }
    None
}

fn read_uptime() -> Option<u64> {
    let content = std::fs::read_to_string("/proc/uptime").ok()?;
    content
        .split_whitespace()
        .next()?
        .parse::<f64>()
        .ok()
        .map(|v| v as u64)
}

/// Watchdog: a supervised task must call `pet()` at least every `timeout`,
/// otherwise `is_expired()` returns true and the supervisor can act.
#[derive(Debug)]
pub struct Watchdog {
    timeout: Duration,
    last_pet: RwLock<Instant>,
    name: String,
}

impl Watchdog {
    pub fn new(name: impl Into<String>, timeout: Duration) -> Self {
        Self {
            timeout,
            last_pet: RwLock::new(Instant::now()),
            name: name.into(),
        }
    }

    pub async fn pet(&self) {
        *self.last_pet.write().await = Instant::now();
    }

    pub async fn is_expired(&self) -> bool {
        self.last_pet.read().await.elapsed() > self.timeout
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Supervisor input: everything health depends on.
#[derive(Debug, Clone, Default)]
pub struct SupervisorInput {
    pub proxy_alive: bool,
    pub control_plane_connected: bool,
    pub cpu_usage: f64,
    pub memory_bytes: u64,
    pub memory_limit_bytes: u64,
    pub active_connections: u64,
    pub software_version: String,
    pub policy_version: u64,
    pub consecutive_proxy_failures: u32,
}

/// Supervisor: pure evaluation + optional bypass actuation.
#[derive(Debug)]
pub struct Supervisor {
    bypass: Arc<dyn BypassController>,
    max_cpu: f64,
    watchdog_timeout: Duration,
}

impl Supervisor {
    pub fn new(
        bypass: Arc<dyn BypassController>,
        max_cpu: f64,
        watchdog_timeout: Duration,
    ) -> Self {
        Self {
            bypass,
            max_cpu,
            watchdog_timeout,
        }
    }

    pub fn evaluate(&self, input: &SupervisorInput) -> HealthState {
        if !input.proxy_alive || input.consecutive_proxy_failures >= 5 {
            return HealthState::Unhealthy;
        }
        if input.cpu_usage > self.max_cpu {
            return HealthState::Degraded;
        }
        if input.memory_limit_bytes > 0 && input.memory_bytes > input.memory_limit_bytes {
            return HealthState::Degraded;
        }
        HealthState::Healthy
    }

    /// Fail-open policy: only enable hardware bypass when the proxy itself
    /// is dead AND explicitly configured to do so. Control-plane loss alone
    /// must never trigger bypass because the dataplane keeps forwarding.
    pub async fn maybe_actuate(
        &self,
        state: HealthState,
        input: &SupervisorInput,
        fail_open_on_proxy_death: bool,
    ) -> Result<(), HealthError> {
        if state == HealthState::Unhealthy
            && !input.proxy_alive
            && fail_open_on_proxy_death
            && !self.bypass.bypass_enabled()
        {
            self.bypass.enable_bypass().await?;
        } else if state == HealthState::Healthy && self.bypass.bypass_enabled() {
            self.bypass.disable_bypass().await?;
        }
        Ok(())
    }

    pub fn watchdog_timeout(&self) -> Duration {
        self.watchdog_timeout
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn healthy_when_nominal() {
        let s = Supervisor::new(
            Arc::new(MockBypassController::new()),
            0.9,
            Duration::from_secs(5),
        );
        let st = s.evaluate(&SupervisorInput {
            proxy_alive: true,
            cpu_usage: 0.2,
            ..Default::default()
        });
        assert_eq!(st, HealthState::Healthy);
    }

    #[test]
    fn dead_proxy_is_unhealthy() {
        let s = Supervisor::new(
            Arc::new(MockBypassController::new()),
            0.9,
            Duration::from_secs(5),
        );
        let st = s.evaluate(&SupervisorInput {
            proxy_alive: false,
            consecutive_proxy_failures: 5,
            ..Default::default()
        });
        assert_eq!(st, HealthState::Unhealthy);
    }

    #[test]
    fn high_cpu_is_degraded_not_unhealthy() {
        let s = Supervisor::new(
            Arc::new(MockBypassController::new()),
            0.5,
            Duration::from_secs(5),
        );
        let st = s.evaluate(&SupervisorInput {
            proxy_alive: true,
            cpu_usage: 0.9,
            ..Default::default()
        });
        assert_eq!(st, HealthState::Degraded);
    }

    #[test]
    fn control_plane_loss_does_not_make_unhealthy() {
        // Critical invariant: cloud loss alone must not mark the edge unhealthy
        // because local forwarding continues on last-known-good policy.
        let s = Supervisor::new(
            Arc::new(MockBypassController::new()),
            0.9,
            Duration::from_secs(5),
        );
        let st = s.evaluate(&SupervisorInput {
            proxy_alive: true,
            control_plane_connected: false,
            cpu_usage: 0.1,
            ..Default::default()
        });
        assert_eq!(st, HealthState::Healthy);
    }

    #[tokio::test]
    async fn watchdog_expires() {
        let w = Watchdog::new("proxy", Duration::from_millis(30));
        assert!(!w.is_expired().await);
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(w.is_expired().await);
        w.pet().await;
        assert!(!w.is_expired().await);
    }

    #[tokio::test]
    async fn mock_bypass_toggles() {
        let b = MockBypassController::new();
        assert!(!b.bypass_enabled());
        b.enable_bypass().await.unwrap();
        assert!(b.bypass_enabled());
        b.disable_bypass().await.unwrap();
        assert!(!b.bypass_enabled());
    }

    #[test]
    fn resource_sample_does_not_panic() {
        let s = sample_resources();
        assert!(s.cpu_usage >= 0.0 && s.cpu_usage <= 1.0);
    }
}
