//! Atomic A/B OTA update state machine.
//!
//! Slot model: two directory/slot records (`A` and `B`) stand in for real
//! disk partitions (see below on production mapping). Every transition is
//! persisted to a JSON journal (atomic rename + directory fsync) so a
//! process crash or power loss resumes correctly.
//!
//! Artifact integrity is SHA-256 plus a real Ed25519 signature over the
//! artifact bytes, verified against a baked-in release verifying key.
//! Production mapping: real bootloader slot selection (e.g. `rauc`,
//! `bootupd`, dm-verity) replaces the directory flip; key distribution and
//! revocation ride the existing fleet PKI (see `docs/security.md`). The
//! state machine shape stays the same.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum OtaError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("decode: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("hash mismatch: expected {expected}, got {got}")]
    HashMismatch { expected: String, got: String },
    #[error("bad signature")]
    BadSignature,
    #[error("invalid transition from {0:?}")]
    InvalidTransition(OtaState),
    #[error("health check failed: {0}")]
    HealthCheck(String),
    #[error("downgrade rejected: candidate {candidate} <= committed {committed}")]
    DowngradeRejected {
        candidate: String,
        committed: String,
    },
    #[error("bad key: {0}")]
    BadKey(String),
    #[error("{0}")]
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub enum Slot {
    #[default]
    A,
    B,
}

impl Slot {
    pub fn other(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum OtaState {
    #[default]
    Idle,
    Downloading,
    Verifying,
    Staged,
    RebootPending,
    BootingCandidate,
    HealthChecking,
    Committed,
    RollbackPending,
    RolledBack,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtaManifest {
    pub version: String,
    pub artifact_sha256_hex: String,
    pub signature_hex: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedOta {
    state: OtaState,
    active_slot: Slot,
    candidate_slot: Option<Slot>,
    candidate_version: Option<String>,
    last_error: Option<String>,
    /// Highest version ever committed on this device. Used for rollback
    /// protection: a signed-but-older artifact is rejected at
    /// `start_download` instead of being installed.
    #[serde(default)]
    highest_committed_version: Option<String>,
}

/// Compare dotted version strings numerically per component, falling back
/// to lexical ordering for non-numeric components. Returns Less / Equal /
/// Greater in the usual sense.
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let mut ai = a.split('.');
    let mut bi = b.split('.');
    loop {
        match (ai.next(), bi.next()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(nx), Ok(ny)) => nx.cmp(&ny),
                    _ => x.cmp(y),
                };
                if ord != Ordering::Equal {
                    return ord;
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct HealthGate {
    pub max_consecutive_failures: u32,
}

/// OTA manager. All mutating steps persist before returning so a crash at
/// any point can be recovered with [`OtaManager::recover`].
///
/// `verifying_key` is the 32-byte Ed25519 release verifying (public) key
/// baked into the device image. The corresponding signing key lives only in
/// release infrastructure — never on the device.
pub struct OtaManager {
    dir: PathBuf,
    state: OtaState,
    active_slot: Slot,
    candidate_slot: Option<Slot>,
    candidate_version: Option<String>,
    candidate_bytes: Option<Vec<u8>>,
    last_error: Option<String>,
    highest_committed_version: Option<String>,
    verifying_key: [u8; 32],
}

impl OtaManager {
    pub fn open(dir: impl AsRef<Path>, verifying_key: &[u8; 32]) -> Result<Self, OtaError> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)?;
        let persisted = Self::journal_path(&dir);
        if persisted.exists() {
            let bytes = std::fs::read(&persisted)?;
            let p: PersistedOta = serde_json::from_slice(&bytes)?;
            Ok(Self {
                dir,
                state: p.state,
                active_slot: p.active_slot,
                candidate_slot: p.candidate_slot,
                candidate_version: p.candidate_version,
                candidate_bytes: None, // payload itself is re-downloaded after crash
                last_error: p.last_error,
                highest_committed_version: p.highest_committed_version,
                verifying_key: *verifying_key,
            })
        } else {
            let m = Self {
                dir,
                state: OtaState::Idle,
                active_slot: Slot::A,
                candidate_slot: None,
                candidate_version: None,
                candidate_bytes: None,
                last_error: None,
                highest_committed_version: None,
                verifying_key: *verifying_key,
            };
            m.persist()?;
            Ok(m)
        }
    }

    fn journal_path(dir: &Path) -> PathBuf {
        dir.join("ota-journal.json")
    }

    fn persist(&self) -> Result<(), OtaError> {
        let p = PersistedOta {
            state: self.state,
            active_slot: self.active_slot,
            candidate_slot: self.candidate_slot,
            candidate_version: self.candidate_version.clone(),
            last_error: self.last_error.clone(),
            highest_committed_version: self.highest_committed_version.clone(),
        };
        let tmp = Self::journal_path(&self.dir).with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&p)?)?;
        // Best-effort durability: fsync the file, rename, then fsync the
        // directory so the rename survives a power loss.
        if let Ok(f) = std::fs::File::open(&tmp) {
            let _ = f.sync_all();
        }
        std::fs::rename(&tmp, Self::journal_path(&self.dir))?;
        if let Ok(d) = std::fs::File::open(&self.dir) {
            let _ = d.sync_all();
        }
        Ok(())
    }

    pub fn state(&self) -> OtaState {
        self.state
    }
    pub fn active_slot(&self) -> Slot {
        self.active_slot
    }
    pub fn candidate_version(&self) -> Option<&str> {
        self.candidate_version.as_deref()
    }

    fn set_state(&mut self, s: OtaState) -> Result<(), OtaError> {
        self.state = s;
        self.persist()
    }

    /// Begin an update. Idempotent: starting the same version twice while
    /// already staged/pending is a no-op success; a *different* version
    /// while busy is an error. Downgrades at or below the highest committed
    /// version are rejected (rollback-attack protection); genuine rollback
    /// is the slot-pointer flip in [`OtaManager::rollback`], not a reinstall.
    pub fn start_download(&mut self, manifest: &OtaManifest) -> Result<bool, OtaError> {
        if let Some(v) = &self.candidate_version {
            if v == &manifest.version
                && !matches!(
                    self.state,
                    OtaState::Idle | OtaState::Committed | OtaState::RolledBack | OtaState::Failed
                )
            {
                return Ok(false); // duplicate command
            }
            if !matches!(
                self.state,
                OtaState::Idle | OtaState::Committed | OtaState::RolledBack | OtaState::Failed
            ) {
                return Err(OtaError::Other(format!(
                    "busy in {:?}, cannot start {}",
                    self.state, manifest.version
                )));
            }
        }
        if let Some(committed) = &self.highest_committed_version {
            if compare_versions(&manifest.version, committed) != std::cmp::Ordering::Greater {
                return Err(OtaError::DowngradeRejected {
                    candidate: manifest.version.clone(),
                    committed: committed.clone(),
                });
            }
        }
        self.candidate_slot = Some(self.active_slot.other());
        self.candidate_version = Some(manifest.version.clone());
        self.candidate_bytes = None;
        self.last_error = None;
        self.set_state(OtaState::Downloading)?;
        Ok(true)
    }

    pub fn ingest_chunk(&mut self, chunk: &[u8], manifest: &OtaManifest) -> Result<(), OtaError> {
        if self.state != OtaState::Downloading {
            return Err(OtaError::InvalidTransition(self.state));
        }
        if chunk.len() as u64 > manifest.size_bytes + 1024 * 1024 {
            return Err(OtaError::Other("chunk exceeds declared size".into()));
        }
        self.candidate_bytes
            .get_or_insert_with(Vec::new)
            .extend_from_slice(chunk);
        Ok(())
    }

    /// Verify SHA-256 and the Ed25519 release signature over the staged bytes.
    pub fn verify(&mut self, manifest: &OtaManifest) -> Result<(), OtaError> {
        if self.state != OtaState::Downloading {
            return Err(OtaError::InvalidTransition(self.state));
        }
        self.set_state(OtaState::Verifying)?;
        let bytes = self.candidate_bytes.clone().unwrap_or_default();
        let digest = Sha256::digest(&bytes);
        let got = hex::encode(digest);
        if got != manifest.artifact_sha256_hex.to_lowercase() {
            self.last_error = Some("hash mismatch".into());
            self.set_state(OtaState::Failed)?;
            return Err(OtaError::HashMismatch {
                expected: manifest.artifact_sha256_hex.clone(),
                got,
            });
        }
        let sig_bytes =
            hex::decode(manifest.signature_hex.trim()).map_err(|_| OtaError::BadSignature)?;
        let sig_arr: [u8; 64] = sig_bytes.try_into().map_err(|_| OtaError::BadSignature)?;
        let signature = Signature::from_bytes(&sig_arr);
        let verifying = VerifyingKey::from_bytes(&self.verifying_key)
            .map_err(|e| OtaError::BadKey(e.to_string()))?;
        if verifying.verify(&bytes, &signature).is_err() {
            self.last_error = Some("bad signature".into());
            self.set_state(OtaState::Failed)?;
            return Err(OtaError::BadSignature);
        }
        // Persist staged artifact to the candidate slot dir (simulation).
        let slot_dir = self
            .dir
            .join(format!("slot-{:?}", self.candidate_slot.unwrap_or(Slot::B)));
        std::fs::create_dir_all(&slot_dir)?;
        std::fs::write(slot_dir.join("artifact.bin"), &bytes)?;
        std::fs::write(slot_dir.join("version"), manifest.version.as_bytes())?;
        self.set_state(OtaState::Staged)?;
        Ok(())
    }

    pub fn mark_reboot_pending(&mut self) -> Result<(), OtaError> {
        if self.state != OtaState::Staged {
            return Err(OtaError::InvalidTransition(self.state));
        }
        self.set_state(OtaState::RebootPending)
    }

    /// Simulate a reboot into the candidate. In production this would be a
    /// real reboot with bootloader slot selection; here we just flip the
    /// in-memory boot pointer and persist it first.
    pub fn simulate_reboot(&mut self) -> Result<(), OtaError> {
        if self.state != OtaState::RebootPending {
            return Err(OtaError::InvalidTransition(self.state));
        }
        self.set_state(OtaState::BootingCandidate)?;
        self.set_state(OtaState::HealthChecking)
    }

    /// Run health checks against the candidate. `healthy=false` triggers
    /// automatic rollback to the previous slot.
    pub fn health_check(&mut self, healthy: bool, detail: &str) -> Result<(), OtaError> {
        if self.state != OtaState::HealthChecking {
            return Err(OtaError::InvalidTransition(self.state));
        }
        if healthy {
            self.active_slot = self.candidate_slot.unwrap_or(self.active_slot.other());
            if let Some(v) = self.candidate_version.clone() {
                let dominated = match &self.highest_committed_version {
                    Some(h) => compare_versions(&v, h) == std::cmp::Ordering::Greater,
                    None => true,
                };
                if dominated {
                    self.highest_committed_version = Some(v);
                }
            }
            self.candidate_slot = None;
            self.candidate_version = None;
            self.candidate_bytes = None;
            self.set_state(OtaState::Committed)?;
            Ok(())
        } else {
            self.last_error = Some(detail.to_string());
            self.set_state(OtaState::RollbackPending)?;
            self.rollback()
        }
    }

    pub fn rollback(&mut self) -> Result<(), OtaError> {
        if !matches!(
            self.state,
            OtaState::RollbackPending | OtaState::HealthChecking | OtaState::Failed
        ) {
            return Err(OtaError::InvalidTransition(self.state));
        }
        // Active slot never flipped on failure, so "restore A" is just
        // dropping the candidate and recording the outcome.
        self.candidate_slot = None;
        self.candidate_version = None;
        self.candidate_bytes = None;
        self.set_state(OtaState::RolledBack)?;
        Ok(())
    }

    /// Recover after a crash/power loss. Returns the state to resume from.
    /// Anything before `Staged` restarts cleanly; `RebootPending` and later
    /// re-enter health checking so a bad candidate still rolls back.
    pub fn recover(&mut self) -> Result<OtaState, OtaError> {
        match self.state {
            OtaState::Downloading | OtaState::Verifying => {
                self.candidate_bytes = None;
                self.set_state(OtaState::Idle)?;
            }
            OtaState::RebootPending | OtaState::BootingCandidate | OtaState::HealthChecking => {
                self.set_state(OtaState::HealthChecking)?;
            }
            _ => {}
        }
        Ok(self.state)
    }
}

/// Release signing helpers. The signing key lives in release
/// infrastructure only; devices carry the verifying key. Tests mint a fixed
/// keypair from a seed — production loads keys from a KMS/HSM.
pub fn signing_key_from_seed(seed: &[u8; 32]) -> SigningKey {
    SigningKey::from_bytes(seed)
}

pub fn verifying_key_bytes(signing: &SigningKey) -> [u8; 32] {
    signing.verifying_key().to_bytes()
}

/// Sign an artifact: returns `(sha256_hex, ed25519_signature_hex)`.
pub fn sign_artifact(bytes: &[u8], signing: &SigningKey) -> (String, String) {
    let digest = Sha256::digest(bytes);
    let sig = signing.sign(bytes);
    (hex::encode(digest), hex::encode(sig.to_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED: [u8; 32] = *b"edgewarden-test-seed-00000000001";

    fn test_keys() -> (SigningKey, [u8; 32]) {
        let sk = signing_key_from_seed(&SEED);
        let vk = verifying_key_bytes(&sk);
        (sk, vk)
    }

    fn tmp_dir(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "edgewarden-ota-{}-{}",
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    fn manifest_for(payload: &[u8], version: &str) -> OtaManifest {
        let (sk, _) = test_keys();
        let (h, s) = sign_artifact(payload, &sk);
        OtaManifest {
            version: version.into(),
            artifact_sha256_hex: h,
            signature_hex: s,
            size_bytes: payload.len() as u64,
        }
    }

    fn open_test(dir: &Path) -> OtaManager {
        let (_, vk) = test_keys();
        OtaManager::open(dir, &vk).unwrap()
    }

    fn happy_path(dir: &Path, version: &str) -> OtaManager {
        let payload = format!("artifact-{version}").into_bytes();
        let m = manifest_for(&payload, version);
        let mut ota = open_test(dir);
        ota.start_download(&m).unwrap();
        ota.ingest_chunk(&payload, &m).unwrap();
        ota.verify(&m).unwrap();
        ota.mark_reboot_pending().unwrap();
        ota.simulate_reboot().unwrap();
        ota.health_check(true, "").unwrap();
        assert_eq!(ota.state(), OtaState::Committed);
        ota
    }

    #[test]
    fn successful_update_flips_slot() {
        let d = tmp_dir("ok");
        let ota = happy_path(&d, "2.0.0");
        assert_eq!(ota.active_slot(), Slot::B);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn corrupt_artifact_fails_verify() {
        let d = tmp_dir("corrupt");
        let payload = b"good-bytes";
        let m = manifest_for(payload, "2.0.0");
        let mut ota = open_test(&d);
        ota.start_download(&m).unwrap();
        ota.ingest_chunk(b"tampered-bytes", &m).unwrap();
        let r = ota.verify(&m);
        assert!(matches!(r, Err(OtaError::HashMismatch { .. })));
        assert_eq!(ota.state(), OtaState::Failed);
        assert_eq!(ota.active_slot(), Slot::A);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn bad_signature_fails_verify() {
        let d = tmp_dir("sig");
        let payload = b"payload";
        let (sk, _) = test_keys();
        // Sign *different* bytes so the signature is well-formed but wrong.
        let (_, wrong_sig) = sign_artifact(b"something-else", &sk);
        let digest = sha2::Sha256::digest(payload);
        let m = OtaManifest {
            version: "2.0.0".into(),
            artifact_sha256_hex: hex::encode(digest),
            signature_hex: wrong_sig,
            size_bytes: payload.len() as u64,
        };
        let mut ota = open_test(&d);
        ota.start_download(&m).unwrap();
        ota.ingest_chunk(payload, &m).unwrap();
        assert!(matches!(ota.verify(&m), Err(OtaError::BadSignature)));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn wrong_key_fails_verify() {
        // Artifact signed by an attacker's key must not verify against the
        // baked-in release key.
        let d = tmp_dir("wrongkey");
        let payload = b"payload";
        let attacker = signing_key_from_seed(b"attacker-seed-000000000000000001");
        let (h, s) = sign_artifact(payload, &attacker);
        let m = OtaManifest {
            version: "2.0.0".into(),
            artifact_sha256_hex: h,
            signature_hex: s,
            size_bytes: payload.len() as u64,
        };
        let mut ota = open_test(&d);
        ota.start_download(&m).unwrap();
        ota.ingest_chunk(payload, &m).unwrap();
        assert!(matches!(ota.verify(&m), Err(OtaError::BadSignature)));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn health_failure_rolls_back_to_a() {
        let d = tmp_dir("rollback");
        let payload = b"artifact-3".to_vec();
        let m = manifest_for(&payload, "3.0.0");
        let mut ota = open_test(&d);
        ota.start_download(&m).unwrap();
        ota.ingest_chunk(&payload, &m).unwrap();
        ota.verify(&m).unwrap();
        ota.mark_reboot_pending().unwrap();
        ota.simulate_reboot().unwrap();
        ota.health_check(false, "candidate 500s").unwrap();
        assert_eq!(ota.state(), OtaState::RolledBack);
        assert_eq!(ota.active_slot(), Slot::A);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn crash_mid_download_recovers_to_idle() {
        let d = tmp_dir("crash");
        let payload = b"artifact-4".to_vec();
        let m = manifest_for(&payload, "4.0.0");
        {
            let mut ota = open_test(&d);
            ota.start_download(&m).unwrap();
            ota.ingest_chunk(&payload[..4], &m).unwrap();
            // Drop without persisting further == crash.
        }
        let mut ota2 = open_test(&d);
        let resumed = ota2.recover().unwrap();
        assert_eq!(resumed, OtaState::Idle);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn power_loss_during_candidate_reenters_healthcheck() {
        let d = tmp_dir("power");
        let payload = b"artifact-5".to_vec();
        let m = manifest_for(&payload, "5.0.0");
        {
            let mut ota = open_test(&d);
            ota.start_download(&m).unwrap();
            ota.ingest_chunk(&payload, &m).unwrap();
            ota.verify(&m).unwrap();
            ota.mark_reboot_pending().unwrap();
            ota.simulate_reboot().unwrap();
            // Crash before health verdict.
        }
        let mut ota2 = open_test(&d);
        assert_eq!(ota2.recover().unwrap(), OtaState::HealthChecking);
        ota2.health_check(false, "post-power health fail").unwrap();
        assert_eq!(ota2.state(), OtaState::RolledBack);
        assert_eq!(ota2.active_slot(), Slot::A);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn duplicate_start_is_idempotent() {
        let d = tmp_dir("dup");
        let payload = b"artifact-6".to_vec();
        let m = manifest_for(&payload, "6.0.0");
        let mut ota = open_test(&d);
        assert!(ota.start_download(&m).unwrap());
        assert!(!ota.start_download(&m).unwrap());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn signed_downgrade_is_rejected() {
        // Even a correctly signed older artifact must not install over a
        // newer commit (rollback-attack protection).
        let d = tmp_dir("downgrade");
        let mut ota = happy_path(&d, "2.0.0");
        assert_eq!(ota.state(), OtaState::Committed);
        let old_payload = b"artifact-1.0.0".to_vec();
        let old = manifest_for(&old_payload, "1.0.0");
        let err = ota.start_download(&old).unwrap_err();
        assert!(
            matches!(err, OtaError::DowngradeRejected { .. }),
            "expected DowngradeRejected, got {err:?}"
        );
        // And the rejection survives a restart (journaled).
        drop(ota);
        let mut ota2 = open_test(&d);
        let err2 = ota2.start_download(&old).unwrap_err();
        assert!(matches!(err2, OtaError::DowngradeRejected { .. }));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn version_compare_orders_numerically() {
        use std::cmp::Ordering;
        assert_eq!(compare_versions("2.0.0", "10.0.0"), Ordering::Less);
        assert_eq!(compare_versions("10.0.0", "2.0.0"), Ordering::Greater);
        assert_eq!(compare_versions("1.2.3", "1.2.3"), Ordering::Equal);
        assert_eq!(compare_versions("1.2", "1.2.0"), Ordering::Less);
    }
}
