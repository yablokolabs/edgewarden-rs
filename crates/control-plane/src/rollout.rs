//! Canary rollout orchestration with health gates.
//!
//! Rings (default: internal -> 0.1% -> 1% -> 5% -> 25% -> 100%) are
//! configurable. Promotion requires every gate to pass on the current ring;
//! a bad ring PAUSES the rollout — it never auto-continues past failing
//! thresholds.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ring {
    pub name: String,
    /// Fraction of fleet in this ring, 0.0..=1.0 (cumulative target).
    pub fraction: f64,
}

impl Ring {
    pub fn new(name: impl Into<String>, fraction: f64) -> Self {
        Self {
            name: name.into(),
            fraction,
        }
    }
}

pub fn default_rings() -> Vec<Ring> {
    vec![
        Ring::new("internal", 0.0),
        Ring::new("canary-0.1pct", 0.001),
        Ring::new("canary-1pct", 0.01),
        Ring::new("canary-5pct", 0.05),
        Ring::new("canary-25pct", 0.25),
        Ring::new("stable-100pct", 1.0),
    ]
}

#[derive(Debug, Clone, Default)]
pub struct HealthSnapshot {
    pub crash_rate: f64,
    pub proxy_error_rate: f64,
    pub connection_success_rate: f64,
    pub cpu_p95: f64,
    pub mem_bytes_p95: u64,
    pub mem_limit_bytes: u64,
    pub health_check_failures: u64,
    pub sample_size: u64,
}

#[derive(Debug, Clone)]
pub struct GateThresholds {
    pub max_crash_rate: f64,
    pub max_proxy_error_rate: f64,
    pub min_connection_success_rate: f64,
    pub max_cpu_p95: f64,
    pub max_health_check_failures: u64,
    pub min_sample_size: u64,
}

impl Default for GateThresholds {
    fn default() -> Self {
        Self {
            max_crash_rate: 0.01,
            max_proxy_error_rate: 0.02,
            min_connection_success_rate: 0.99,
            max_cpu_p95: 0.85,
            max_health_check_failures: 0,
            min_sample_size: 5,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RolloutDecision {
    /// All gates pass; caller may promote to the next ring.
    Promote { from: String, to: String },
    /// Hold at the current ring (gates pass but soak time remains, or no
    /// next ring). Distinct from Pause: nothing is wrong.
    Hold { ring: String },
    /// A gate failed: rollout is frozen and requires operator action.
    /// Never auto-resume.
    Paused { ring: String, reason: String },
}

#[derive(Debug, Clone)]
pub struct RolloutManager {
    pub rings: Vec<Ring>,
    pub current: usize,
    pub paused: bool,
    pub pause_reason: Option<String>,
    pub thresholds: GateThresholds,
}

impl RolloutManager {
    pub fn new(rings: Vec<Ring>, thresholds: GateThresholds) -> Self {
        Self {
            rings,
            current: 0,
            paused: false,
            pause_reason: None,
            thresholds,
        }
    }

    pub fn current_ring(&self) -> &Ring {
        &self.rings[self.current.min(self.rings.len() - 1)]
    }

    pub fn pause(&mut self, reason: impl Into<String>) {
        self.paused = true;
        self.pause_reason = Some(reason.into());
    }

    pub fn resume(&mut self) {
        self.paused = false;
        self.pause_reason = None;
    }

    /// Evaluate gates for the current ring. On failure the manager latches
    /// `paused=true` and returns `Paused`; operator must call `resume()`.
    pub fn evaluate(&mut self, snap: &HealthSnapshot) -> RolloutDecision {
        let ring = self.current_ring().clone();
        if self.paused {
            return RolloutDecision::Paused {
                ring: ring.name,
                reason: self
                    .pause_reason
                    .clone()
                    .unwrap_or_else(|| "manually paused".into()),
            };
        }
        if snap.sample_size < self.thresholds.min_sample_size {
            return RolloutDecision::Hold { ring: ring.name };
        }
        let t = &self.thresholds;
        if snap.crash_rate > t.max_crash_rate {
            let r = format!(
                "crash_rate {:.4} > {:.4}",
                snap.crash_rate, t.max_crash_rate
            );
            self.pause(r.clone());
            return RolloutDecision::Paused {
                ring: ring.name,
                reason: r,
            };
        }
        if snap.proxy_error_rate > t.max_proxy_error_rate {
            let r = format!(
                "proxy_error_rate {:.4} > {:.4}",
                snap.proxy_error_rate, t.max_proxy_error_rate
            );
            self.pause(r.clone());
            return RolloutDecision::Paused {
                ring: ring.name,
                reason: r,
            };
        }
        if snap.connection_success_rate < t.min_connection_success_rate {
            let r = format!(
                "connection_success_rate {:.4} < {:.4}",
                snap.connection_success_rate, t.min_connection_success_rate
            );
            self.pause(r.clone());
            return RolloutDecision::Paused {
                ring: ring.name,
                reason: r,
            };
        }
        if snap.cpu_p95 > t.max_cpu_p95 {
            let r = format!("cpu_p95 {:.3} > {:.3}", snap.cpu_p95, t.max_cpu_p95);
            self.pause(r.clone());
            return RolloutDecision::Paused {
                ring: ring.name,
                reason: r,
            };
        }
        if snap.health_check_failures > t.max_health_check_failures {
            let r = format!(
                "health_check_failures {} > {}",
                snap.health_check_failures, t.max_health_check_failures
            );
            self.pause(r.clone());
            return RolloutDecision::Paused {
                ring: ring.name,
                reason: r,
            };
        }
        if self.current + 1 < self.rings.len() {
            let to = self.rings[self.current + 1].name.clone();
            RolloutDecision::Promote {
                from: ring.name,
                to,
            }
        } else {
            RolloutDecision::Hold { ring: ring.name }
        }
    }

    pub fn promote(&mut self) {
        if !self.paused && self.current + 1 < self.rings.len() {
            self.current += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn healthy() -> HealthSnapshot {
        HealthSnapshot {
            crash_rate: 0.0,
            proxy_error_rate: 0.001,
            connection_success_rate: 0.999,
            cpu_p95: 0.4,
            mem_bytes_p95: 100,
            mem_limit_bytes: 1000,
            health_check_failures: 0,
            sample_size: 100,
        }
    }

    #[test]
    fn good_ring_promotes() {
        let mut m = RolloutManager::new(default_rings(), GateThresholds::default());
        let d = m.evaluate(&healthy());
        assert!(matches!(d, RolloutDecision::Promote { .. }));
        m.promote();
        assert_eq!(m.current_ring().name, "canary-0.1pct");
    }

    #[test]
    fn bad_ring_pauses_and_stays_paused() {
        let mut m = RolloutManager::new(default_rings(), GateThresholds::default());
        let mut bad = healthy();
        bad.proxy_error_rate = 0.5;
        let d = m.evaluate(&bad);
        assert!(matches!(d, RolloutDecision::Paused { .. }));
        assert!(m.paused);
        // Even with healthy data afterwards, stays paused until operator resumes.
        let d2 = m.evaluate(&healthy());
        assert!(matches!(d2, RolloutDecision::Paused { .. }));
        m.resume();
        let d3 = m.evaluate(&healthy());
        assert!(matches!(d3, RolloutDecision::Promote { .. }));
    }

    #[test]
    fn small_sample_holds_without_pausing() {
        let mut m = RolloutManager::new(default_rings(), GateThresholds::default());
        let mut s = healthy();
        s.sample_size = 1;
        let d = m.evaluate(&s);
        assert!(matches!(d, RolloutDecision::Hold { .. }));
        assert!(!m.paused);
    }
}
