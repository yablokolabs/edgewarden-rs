//! In-memory device registry with desired-state tracking.

use edge_protocol::fleet::Policy;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct DeviceRecord {
    pub device_id: String,
    pub hardware_id: String,
    pub software_version: String,
    pub reported_policy_version: String,
    pub reported_software_version: String,
    pub desired_policy_version: u64,
    pub desired_software_version: String,
    pub desired_seq: u64,
    pub health: i32,
    pub last_seen_secs: u64,
    pub uptime_secs: u64,
    pub cpu_usage: f64,
    pub memory_bytes: u64,
    pub active_connections: u64,
}

impl DeviceRecord {
    pub fn last_seen_age_secs(&self) -> u64 {
        now_secs().saturating_sub(self.last_seen_secs)
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Debug, Default)]
struct Inner {
    devices: HashMap<String, DeviceRecord>,
    policy: Option<Policy>,
    policy_seq: u64,
    desired_software: String,
}

/// Heartbeat fields from the edge. Grouped to keep the API stable.
#[derive(Debug, Clone)]
pub struct HeartbeatUpdate<'a> {
    pub device_id: &'a str,
    pub reported_policy: &'a str,
    pub reported_software: &'a str,
    pub health: i32,
    pub cpu: f64,
    pub mem: u64,
    pub active: u64,
    pub uptime: u64,
}

/// Thread-safe registry. AllWebsite mutations are O(1) and bounded by the
/// number of registered devices; no unbounded channels or global state.
#[derive(Debug, Clone, Default)]
pub struct Registry {
    inner: Arc<RwLock<Inner>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn register(
        &self,
        device_id: String,
        hardware_id: String,
        software_version: String,
        reported_policy: String,
        reported_software: String,
    ) -> DeviceRecord {
        let mut inner = self.inner.write().await;
        let desired_policy_version = inner.policy.as_ref().map(|p| p.version).unwrap_or(0);
        let desired_seq = inner.policy_seq;
        let rec = DeviceRecord {
            device_id: device_id.clone(),
            hardware_id,
            software_version: software_version.clone(),
            reported_policy_version: reported_policy,
            reported_software_version: reported_software.clone(),
            desired_policy_version,
            desired_software_version: inner.desired_software.clone(),
            desired_seq,
            health: 1,
            last_seen_secs: now_secs(),
            uptime_secs: 0,
            cpu_usage: 0.0,
            memory_bytes: 0,
            active_connections: 0,
        };
        inner.devices.insert(device_id, rec.clone());
        rec
    }

    /// Compatibility wrapper used by older call sites and tests.
    #[allow(clippy::too_many_arguments)]
    pub async fn heartbeat(
        &self,
        device_id: &str,
        reported_policy: &str,
        reported_software: &str,
        health: i32,
        cpu: f64,
        mem: u64,
        active: u64,
        uptime: u64,
    ) -> Option<(u64, Option<Policy>, String, u64)> {
        self.heartbeat_update(HeartbeatUpdate {
            device_id,
            reported_policy,
            reported_software,
            health,
            cpu,
            mem,
            active,
            uptime,
        })
        .await
    }

    pub async fn heartbeat_update(
        &self,
        u: HeartbeatUpdate<'_>,
    ) -> Option<(u64, Option<Policy>, String, u64)> {
        let mut inner = self.inner.write().await;
        let desired_policy_version = inner.policy.as_ref().map(|p| p.version).unwrap_or(0);
        let desired_software = inner.desired_software.clone();
        let desired_seq = inner.policy_seq;
        let policy_clone = inner.policy.clone();
        let rec = inner.devices.get_mut(u.device_id)?;
        rec.reported_policy_version = u.reported_policy.to_string();
        rec.reported_software_version = u.reported_software.to_string();
        rec.health = u.health;
        rec.cpu_usage = u.cpu;
        rec.memory_bytes = u.mem;
        rec.active_connections = u.active;
        rec.uptime_secs = u.uptime;
        rec.last_seen_secs = now_secs();
        rec.desired_policy_version = desired_policy_version;
        rec.desired_software_version = desired_software.clone();
        rec.desired_seq = desired_seq;
        Some((
            desired_policy_version,
            policy_clone,
            desired_software,
            desired_seq,
        ))
    }

    /// Operator pushes a new desired policy. Monotonic seq; idempotent on
    /// identical version (no seq bump, no duplicate rollout).
    pub async fn set_policy(&self, policy: Policy) -> u64 {
        let mut inner = self.inner.write().await;
        if inner.policy.as_ref().map(|p| p.version) == Some(policy.version) {
            return inner.policy_seq;
        }
        inner.policy_seq += 1;
        inner.policy = Some(policy);
        inner.policy_seq
    }

    pub async fn set_desired_software(&self, version: String) -> u64 {
        let mut inner = self.inner.write().await;
        if inner.desired_software == version {
            return inner.policy_seq;
        }
        inner.policy_seq += 1;
        inner.desired_software = version;
        inner.policy_seq
    }

    pub async fn current_policy(&self) -> (Option<Policy>, u64) {
        let inner = self.inner.read().await;
        (inner.policy.clone(), inner.policy_seq)
    }

    pub async fn devices(&self) -> Vec<DeviceRecord> {
        let inner = self.inner.read().await;
        inner.devices.values().cloned().collect()
    }

    pub async fn get(&self, device_id: &str) -> Option<DeviceRecord> {
        let inner = self.inner.read().await;
        inner.devices.get(device_id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn register_then_heartbeat_converges() {
        let r = Registry::new();
        r.set_policy(Policy {
            version: 42,
            max_connections: 100,
            idle_timeout_secs: 60,
            default_upstream: "127.0.0.1:9".into(),
            rules_json: "{}".into(),
        })
        .await;
        let rec = r
            .register(
                "d1".into(),
                "hw1".into(),
                "1.0.0".into(),
                "39".into(),
                "1.0.0".into(),
            )
            .await;
        assert_eq!(rec.desired_policy_version, 42);
        let hb = r
            .heartbeat("d1", "42", "1.0.0", 1, 0.1, 100, 5, 60)
            .await
            .unwrap();
        assert_eq!(hb.0, 42);
    }

    #[tokio::test]
    async fn duplicate_policy_push_does_not_bump_seq() {
        let r = Registry::new();
        let p = Policy {
            version: 7,
            max_connections: 10,
            idle_timeout_secs: 10,
            default_upstream: "x".into(),
            rules_json: "{}".into(),
        };
        let s1 = r.set_policy(p.clone()).await;
        let s2 = r.set_policy(p).await;
        assert_eq!(s1, s2);
    }
}
