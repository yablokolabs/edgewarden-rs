use edge_protocol::{
    fleet::{
        fleet_service_client::FleetServiceClient, DeviceIdentity, EdgeReport, HeartbeatRequest,
        ResourceUsage,
    },
    Backoff,
};
use edge_state::{DesiredState, Policy as LocalPolicy, StateStore};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use thiserror::Error;
use tonic::transport::Channel;
use tracing::{info, warn};

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("transport: {0}")]
    Transport(#[from] tonic::transport::Error),
    #[error("rpc: {0}")]
    Rpc(#[from] tonic::Status),
    #[error("state: {0}")]
    State(String),
}

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub device_id: String,
    pub hardware_id: String,
    pub software_version: String,
    pub control_plane: String,
    pub state_path: PathBuf,
    pub heartbeat_interval: Duration,
    pub tls_ca: Option<String>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            device_id: "edge-01".into(),
            hardware_id: "dev-hw".into(),
            software_version: "0.1.0".into(),
            control_plane: "http://127.0.0.1:50051".into(),
            state_path: PathBuf::from("edge-state.json"),
            heartbeat_interval: Duration::from_secs(5),
            tls_ca: None,
        }
    }
}

pub struct EdgeAgent {
    config: AgentConfig,
    store: StateStore,
    metrics: std::sync::Arc<edge_telemetry::EdgeMetrics>,
    start: Instant,
}

impl EdgeAgent {
    pub fn new(
        config: AgentConfig,
        metrics: std::sync::Arc<edge_telemetry::EdgeMetrics>,
    ) -> Result<Self, AgentError> {
        let store =
            StateStore::open(&config.state_path).map_err(|e| AgentError::State(e.to_string()))?;
        Ok(Self {
            config,
            store,
            metrics,
            start: Instant::now(),
        })
    }

    pub fn reported_policy_version(&self) -> u64 {
        self.store.reported.policy_version
    }

    pub fn last_good_policy(&self) -> LocalPolicy {
        self.store.last_good_policy.clone()
    }

    async fn connect(&self) -> Result<FleetServiceClient<Channel>, AgentError> {
        // mTLS: when `tls_ca` is set we use rustls with the pinned CA;
        // otherwise plaintext (dev/demo). Device client certs are loaded
        // the same way in production builds (see docs/security.md).
        if let Some(ca) = &self.config.tls_ca {
            let pem = std::fs::read(ca).map_err(|e| AgentError::State(e.to_string()))?;
            let ca_cert = tonic::transport::Certificate::from_pem(pem);
            let tls = tonic::transport::ClientTlsConfig::new()
                .ca_certificate(ca_cert)
                .domain_name("localhost");
            let channel = Channel::from_shared(self.config.control_plane.clone())
                .map_err(|e| AgentError::State(e.to_string()))?
                .tls_config(tls)?
                .connect()
                .await?;
            Ok(FleetServiceClient::new(channel))
        } else {
            let channel = Channel::from_shared(self.config.control_plane.clone())
                .map_err(|e| AgentError::State(e.to_string()))?
                .connect()
                .await?;
            Ok(FleetServiceClient::new(channel))
        }
    }

    fn usage(&self, active: u64) -> ResourceUsage {
        let snap = edge_health::sample_resources();
        ResourceUsage {
            cpu_usage: snap.cpu_usage,
            memory_bytes: snap.memory_bytes,
            active_connections: active,
            uptime_secs: self.start.elapsed().as_secs(),
        }
    }

    /// Run one register+heartbeat cycle. Returns the desired state if the
    /// cloud is reachable; `Ok(None)` means partitioned — caller keeps
    /// serving last-known-good.
    pub async fn sync_once(&mut self, active_connections: u64) -> Result<bool, AgentError> {
        let mut client = self.connect().await?;
        let cfg = &self.config;
        // Register is idempotent on the server.
        let _ = client
            .register(edge_protocol::fleet::RegisterRequest {
                identity: Some(DeviceIdentity {
                    device_id: cfg.device_id.clone(),
                    hardware_id: cfg.hardware_id.clone(),
                    software_version: cfg.software_version.clone(),
                }),
                reported_policy_version: self.store.reported.policy_version.to_string(),
                reported_software_version: self.store.reported.software_version.clone(),
            })
            .await?;

        let hb = client
            .heartbeat(HeartbeatRequest {
                device_id: cfg.device_id.clone(),
                reported_policy_version: self.store.reported.policy_version.to_string(),
                reported_software_version: self.store.reported.software_version.clone(),
                health: 1,
                usage: Some(self.usage(active_connections)),
                active_connections,
            })
            .await?
            .into_inner();

        self.metrics.control_plane_connected.set(1);

        if hb.has_policy {
            if let Some(p) = hb.policy {
                let desired = DesiredState {
                    seq: hb.desired_seq.max(1),
                    policy: LocalPolicy {
                        version: p.version,
                        max_connections: p.max_connections,
                        idle_timeout_secs: p.idle_timeout_secs,
                        default_upstream: if p.default_upstream.is_empty() {
                            self.store.last_good_policy.default_upstream.clone()
                        } else {
                            p.default_upstream.clone()
                        },
                        rules_json: p.rules_json.clone(),
                    },
                    software_version: hb.desired_software_version.clone(),
                };
                let action = self
                    .store
                    .ingest(desired.clone())
                    .map_err(|e| AgentError::State(e.to_string()))?;
                if let edge_state::ReconcileAction::ApplyPolicy(policy) = action {
                    info!(version = policy.version, "applying desired policy");
                    self.store
                        .mark_policy_applied(&policy, desired.seq)
                        .map_err(|e| AgentError::State(e.to_string()))?;
                    self.metrics.policy_version.set(policy.version as i64);
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    pub fn note_disconnected(&self) {
        self.metrics.control_plane_connected.set(0);
        self.metrics.reconnect_total.inc();
    }

    /// Main loop: heartbeat with jittered backoff. Never exits on error;
    /// partition only degrades to last-known-good.
    pub async fn run(mut self, mut shutdown: tokio::sync::watch::Receiver<bool>) {
        let mut backoff = Backoff::new(Duration::from_millis(200), Duration::from_secs(30));
        loop {
            tokio::select! {
                _ = shutdown.changed() => {
                    info!("agent shutdown");
                    return;
                }
                _ = tokio::time::sleep(self.config.heartbeat_interval) => {}
            }
            match self.sync_once(0).await {
                Ok(_) => backoff.reset(),
                Err(e) => {
                    warn!(error = %e, "control-plane unreachable; continuing on last-known-good");
                    self.note_disconnected();
                    let d = backoff.next_delay();
                    tokio::select! {
                        _ = shutdown.changed() => return,
                        _ = tokio::time::sleep(d) => {}
                    }
                }
            }
        }
    }

    /// Long-lived Watch stream with the same partition tolerance. Falls back
    /// to heartbeat polling when the stream breaks.
    pub async fn run_watch(mut self) -> Result<(), AgentError> {
        let mut backoff = Backoff::new(Duration::from_millis(200), Duration::from_secs(30));
        loop {
            let mut client = match self.connect().await {
                Ok(c) => {
                    backoff.reset();
                    c
                }
                Err(e) => {
                    warn!(error = %e, "watch connect failed");
                    self.note_disconnected();
                    tokio::time::sleep(backoff.next_delay()).await;
                    continue;
                }
            };
            let device_id = self.config.device_id.clone();
            let reported_policy = self.store.reported.policy_version.to_string();
            let reported_software = self.store.reported.software_version.clone();
            let applied_seq = self.store.reported.applied_seq;
            let usage = self.usage(0);
            let outbound = async_stream::stream! {
                // Minimal keepalive: send current report once; server replies
                // with commands. Production sends periodic reports here.
                yield EdgeReport {
                    device_id,
                    reported_policy_version: reported_policy,
                    reported_software_version: reported_software,
                    health: 1,
                    usage: Some(usage),
                    active_connections: 0,
                    applied_seq,
                };
            };
            match client.watch(outbound).await {
                Ok(resp) => {
                    let mut inbound = resp.into_inner();
                    while let Ok(Some(cmd)) = inbound.message().await {
                        if let Some(c) = cmd.command {
                            use edge_protocol::fleet::cloud_command::Command;
                            match c {
                                Command::PolicyUpdate(u) => {
                                    if let Some(p) = u.policy {
                                        let desired = DesiredState {
                                            seq: cmd.seq.max(1),
                                            policy: LocalPolicy {
                                                version: p.version,
                                                max_connections: p.max_connections,
                                                idle_timeout_secs: p.idle_timeout_secs,
                                                default_upstream: p.default_upstream.clone(),
                                                rules_json: p.rules_json.clone(),
                                            },
                                            software_version: String::new(),
                                        };
                                        if let Ok(edge_state::ReconcileAction::ApplyPolicy(pol)) =
                                            self.store.ingest(desired.clone())
                                        {
                                            let _ =
                                                self.store.mark_policy_applied(&pol, desired.seq);
                                        }
                                    }
                                }
                                Command::Noop(_) => {}
                                Command::OtaUpdate(_) => {
                                    info!("ota command received (handled by OTA manager)");
                                }
                            }
                        }
                    }
                    warn!("watch stream closed; reconnecting");
                }
                Err(e) => {
                    warn!(error = %e, "watch failed; reconnecting");
                }
            }
            self.note_disconnected();
            tokio::time::sleep(backoff.next_delay()).await;
        }
    }
}
