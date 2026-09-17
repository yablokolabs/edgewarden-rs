//! Cloud control plane: device registry, desired state, rollout manager.
//!
//! The cloud never dials into the edge. It serves gRPC (Register/Heartbeat/
//! Watch) plus an operator REST API, and evaluates canary health gates
//! before promoting rollouts.

pub mod registry;
pub mod rollout;
pub mod service;

pub use registry::{DeviceRecord, HeartbeatUpdate, Registry};
pub use rollout::{HealthSnapshot, Ring, RolloutDecision, RolloutManager};
pub use service::FleetServiceImpl;
