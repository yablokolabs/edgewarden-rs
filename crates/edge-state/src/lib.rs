//! Desired vs reported state reconciliation.
//!
//! Model: the control plane owns *desired* state (monotonic `seq` +
//! versions). The edge owns *reported* state and a local last-known-good
//! copy. Reconciliation is at-least-once with idempotent transitions:
//! duplicates and stale messages are safe to re-apply; only the highest
//! `seq` wins. There is no exactly-once delivery claim.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StateError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("decode: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("stale desired seq: got {got} <= applied {applied}")]
    Stale { got: u64, applied: u64 },
}

/// Versioned policy the edge enforces locally.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    pub version: u64,
    pub max_connections: u32,
    pub idle_timeout_secs: u64,
    pub default_upstream: String,
    #[serde(default)]
    pub rules_json: String,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            version: 0,
            max_connections: 512,
            idle_timeout_secs: 300,
            default_upstream: "127.0.0.1:18081".to_string(),
            rules_json: "{}".to_string(),
        }
    }
}

/// What the cloud wants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesiredState {
    pub seq: u64,
    pub policy: Policy,
    pub software_version: String,
}

/// What the edge has actually applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ReportedState {
    pub policy_version: u64,
    pub software_version: String,
    pub applied_seq: u64,
}

/// Idempotent action derived from diffing desired vs reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileAction {
    /// No-op: already converged or incoming message is stale/duplicate.
    Noop,
    /// Apply this policy version (safe to apply repeatedly).
    ApplyPolicy(Policy),
    /// Record a pending OTA target (the OTA manager performs it).
    StageOta { version: String },
}

impl ReportedState {
    /// Pure diff. Never fails; stale seq yields Noop so callers can safely
    /// feed duplicates and out-of-order redeliveries.
    pub fn diff(&self, desired: &DesiredState) -> ReconcileAction {
        if desired.seq <= self.applied_seq
            && desired.policy.version == self.policy_version
            && desired.software_version == self.software_version
        {
            return ReconcileAction::Noop;
        }
        // Stale sequence with no new content: ignore.
        if desired.seq <= self.applied_seq {
            return ReconcileAction::Noop;
        }
        if desired.policy.version != self.policy_version {
            return ReconcileAction::ApplyPolicy(desired.policy.clone());
        }
        if desired.software_version != self.software_version && !desired.software_version.is_empty()
        {
            return ReconcileAction::StageOta {
                version: desired.software_version.clone(),
            };
        }
        ReconcileAction::Noop
    }

    /// Idempotent apply of a policy. Returns true if state changed.
    pub fn apply_policy(&mut self, policy: &Policy, seq: u64) -> bool {
        let mut changed = false;
        if self.policy_version != policy.version {
            self.policy_version = policy.version;
            changed = true;
        }
        if seq > self.applied_seq {
            self.applied_seq = seq;
            changed = true;
        }
        changed
    }
}

/// Durable last-known-good store. The dataplane reads this when the cloud
/// is unreachable, which is what makes partition tolerance possible.
#[derive(Debug)]
pub struct StateStore {
    path: PathBuf,
    pub desired: Option<DesiredState>,
    pub reported: ReportedState,
    pub last_good_policy: Policy,
}

impl StateStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StateError> {
        let path = path.as_ref().to_path_buf();
        if path.exists() {
            let bytes = std::fs::read(&path)?;
            let persisted: Persisted = serde_json::from_slice(&bytes)?;
            Ok(Self {
                path,
                desired: persisted.desired,
                reported: persisted.reported,
                last_good_policy: persisted.last_good_policy,
            })
        } else {
            Ok(Self {
                path,
                desired: None,
                reported: ReportedState::default(),
                last_good_policy: Policy::default(),
            })
        }
    }

    pub fn persist(&self) -> Result<(), StateError> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let persisted = Persisted {
            desired: self.desired.clone(),
            reported: self.reported.clone(),
            last_good_policy: self.last_good_policy.clone(),
        };
        let tmp = self.path.with_extension("tmp");
        let bytes = serde_json::to_vec_pretty(&persisted)?;
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// Ingest desired state. Returns the action to take. Stale seq is
    /// accepted as Noop (not an error) to keep redelivery safe; callers
    /// that need strictness can use `ingest_strict`.
    pub fn ingest(&mut self, desired: DesiredState) -> Result<ReconcileAction, StateError> {
        let action = self.reported.diff(&desired);
        // Track the newest desired snapshot only.
        let is_newer = self
            .desired
            .as_ref()
            .map(|d| desired.seq > d.seq)
            .unwrap_or(true);
        if is_newer {
            self.desired = Some(desired);
            self.persist()?;
        }
        Ok(action)
    }

    pub fn ingest_strict(&mut self, desired: DesiredState) -> Result<ReconcileAction, StateError> {
        if desired.seq <= self.reported.applied_seq {
            return Err(StateError::Stale {
                got: desired.seq,
                applied: self.reported.applied_seq,
            });
        }
        self.ingest(desired)
    }

    /// Mark a policy as applied. Idempotent: re-applying the same version
    /// and seq is a no-op that still persists (cheap) for crash safety.
    pub fn mark_policy_applied(&mut self, policy: &Policy, seq: u64) -> Result<bool, StateError> {
        let changed = self.reported.apply_policy(policy, seq);
        self.last_good_policy = policy.clone();
        self.persist()?;
        Ok(changed)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Persisted {
    desired: Option<DesiredState>,
    reported: ReportedState,
    last_good_policy: Policy,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "edgewarden-state-test-{}-{}",
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        p
    }

    fn policy(v: u64) -> Policy {
        Policy {
            version: v,
            ..Policy::default()
        }
    }

    #[test]
    fn mismatch_triggers_apply_then_converges() {
        let reported = ReportedState {
            policy_version: 39,
            software_version: "1.0.0".into(),
            applied_seq: 1,
        };
        let desired = DesiredState {
            seq: 2,
            policy: policy(42),
            software_version: "1.0.0".into(),
        };
        let action = reported.diff(&desired);
        assert_eq!(action, ReconcileAction::ApplyPolicy(policy(42)));

        let mut r2 = reported.clone();
        r2.apply_policy(&policy(42), 2);
        assert_eq!(r2.diff(&desired), ReconcileAction::Noop);
    }

    #[test]
    fn duplicate_delivery_is_noop() {
        let mut r = ReportedState {
            policy_version: 42,
            software_version: "1.0.0".into(),
            applied_seq: 5,
        };
        let desired = DesiredState {
            seq: 5,
            policy: policy(42),
            software_version: "1.0.0".into(),
        };
        assert_eq!(r.diff(&desired), ReconcileAction::Noop);
        // Re-apply is idempotent.
        assert!(!r.apply_policy(&policy(42), 5));
        assert_eq!(r.policy_version, 42);
    }

    #[test]
    fn stale_seq_is_ignored() {
        let r = ReportedState {
            policy_version: 42,
            software_version: "1.0.0".into(),
            applied_seq: 10,
        };
        let stale = DesiredState {
            seq: 9,
            policy: policy(41),
            software_version: "1.0.0".into(),
        };
        assert_eq!(r.diff(&stale), ReconcileAction::Noop);
    }

    #[test]
    fn store_survives_restart_with_last_good() {
        let path = tmp_path("restart");
        let _ = std::fs::remove_file(&path);
        let mut s = StateStore::open(&path).unwrap();
        let desired = DesiredState {
            seq: 3,
            policy: policy(7),
            software_version: "1.0.0".into(),
        };
        let action = s.ingest(desired).unwrap();
        assert!(matches!(action, ReconcileAction::ApplyPolicy(_)));
        s.mark_policy_applied(&policy(7), 3).unwrap();
        drop(s);

        // Simulate process restart: last-known-good must be intact.
        let s2 = StateStore::open(&path).unwrap();
        assert_eq!(s2.last_good_policy.version, 7);
        assert_eq!(s2.reported.policy_version, 7);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn reconnect_reconciliation_applies_newest_only() {
        let path = tmp_path("reconnect");
        let _ = std::fs::remove_file(&path);
        let mut s = StateStore::open(&path).unwrap();
        for (seq, ver) in [(1, 10), (2, 11), (2, 11), (1, 9)] {
            let d = DesiredState {
                seq,
                policy: policy(ver),
                software_version: String::new(),
            };
            let a = s.ingest(d).unwrap();
            if let ReconcileAction::ApplyPolicy(p) = a {
                s.mark_policy_applied(&p, seq).unwrap();
            }
        }
        assert_eq!(s.reported.policy_version, 11);
        let _ = std::fs::remove_file(&path);
    }
}
