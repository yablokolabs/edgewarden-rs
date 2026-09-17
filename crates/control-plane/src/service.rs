//! Tonic service implementation + Axum operator API.

use crate::registry::Registry;
use edge_protocol::fleet::{
    fleet_service_server::FleetService, CloudCommand, EdgeReport, HeartbeatRequest,
    HeartbeatResponse, Noop, PolicyUpdate, RegisterRequest, RegisterResponse, TelemetryAck,
    TelemetryReport,
};
use std::pin::Pin;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

#[derive(Debug, Clone)]
pub struct FleetServiceImpl {
    pub registry: Registry,
}

impl FleetServiceImpl {
    pub fn new(registry: Registry) -> Self {
        Self { registry }
    }
}

#[tonic::async_trait]
impl FleetService for FleetServiceImpl {
    type WatchStream =
        Pin<Box<dyn tokio_stream::Stream<Item = Result<CloudCommand, Status>> + Send>>;

    async fn register(
        &self,
        req: Request<RegisterRequest>,
    ) -> Result<Response<RegisterResponse>, Status> {
        let r = req.into_inner();
        let id = r.identity.unwrap_or_default();
        let rec = self
            .registry
            .register(
                id.device_id.clone(),
                id.hardware_id.clone(),
                id.software_version.clone(),
                r.reported_policy_version.clone(),
                r.reported_software_version.clone(),
            )
            .await;
        let (policy, _seq) = self.registry.current_policy().await;
        Ok(Response::new(RegisterResponse {
            accepted: true,
            desired_policy_version: rec.desired_policy_version,
            policy,
            desired_software_version: rec.desired_software_version,
            pending_ota: None,
            has_pending_ota: false,
        }))
    }

    async fn heartbeat(
        &self,
        req: Request<HeartbeatRequest>,
    ) -> Result<Response<HeartbeatResponse>, Status> {
        let h = req.into_inner();
        let out = self
            .registry
            .heartbeat(
                &h.device_id,
                &h.reported_policy_version,
                &h.reported_software_version,
                h.health,
                h.usage.as_ref().map(|u| u.cpu_usage).unwrap_or(0.0),
                h.usage.as_ref().map(|u| u.memory_bytes).unwrap_or(0),
                h.active_connections,
                h.usage.as_ref().map(|u| u.uptime_secs).unwrap_or(0),
            )
            .await;
        match out {
            Some((ver, policy, sw, seq)) => Ok(Response::new(HeartbeatResponse {
                desired_policy_version: ver,
                policy,
                has_policy: true,
                desired_software_version: sw,
                pending_ota: None,
                has_pending_ota: false,
                desired_seq: seq,
            })),
            None => Err(Status::not_found("unknown device; register first")),
        }
    }

    async fn watch(
        &self,
        req: Request<tonic::Streaming<EdgeReport>>,
    ) -> Result<Response<Self::WatchStream>, Status> {
        let mut inbound = req.into_inner();
        let registry = self.registry.clone();
        let (tx, rx) = tokio::sync::mpsc::channel(32);
        tokio::spawn(async move {
            loop {
                let rep = match inbound.message().await {
                    Ok(Some(r)) => r,
                    Ok(None) | Err(_) => break,
                };
                let applied_hint = rep.applied_seq;
                let _ = registry
                    .heartbeat(
                        &rep.device_id,
                        &rep.reported_policy_version,
                        &rep.reported_software_version,
                        rep.health,
                        rep.usage.as_ref().map(|u| u.cpu_usage).unwrap_or(0.0),
                        rep.usage.as_ref().map(|u| u.memory_bytes).unwrap_or(0),
                        rep.active_connections,
                        rep.usage.as_ref().map(|u| u.uptime_secs).unwrap_or(0),
                    )
                    .await;
                let (policy, seq) = registry.current_policy().await;
                // Only push when there is newer desired state; otherwise
                // send a heartbeat noop so clients can detect liveness.
                let cmd = match policy {
                    Some(p)
                        if seq > applied_hint
                            && rep.reported_policy_version.parse::<u64>().unwrap_or(0)
                                != p.version =>
                    {
                        CloudCommand {
                            seq,
                            command: Some(
                                edge_protocol::fleet::cloud_command::Command::PolicyUpdate(
                                    PolicyUpdate { policy: Some(p) },
                                ),
                            ),
                        }
                    }
                    _ => CloudCommand {
                        seq: applied_hint,
                        command: Some(edge_protocol::fleet::cloud_command::Command::Noop(Noop {})),
                    },
                };
                if tx.send(Ok(cmd)).await.is_err() {
                    break;
                }
            }
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    async fn report_telemetry(
        &self,
        _req: Request<TelemetryReport>,
    ) -> Result<Response<TelemetryAck>, Status> {
        Ok(Response::new(TelemetryAck { ok: true }))
    }
}
