//! Transport-neutral web API seam for remote Worker registration and
//! heartbeats.
//!
//! An HTTP adapter can deserialize request JSON into the protocol types, do
//! mTLS or machine-credential authentication, and then delegate to this type.
//! Keeping the state transition here makes the HTTP, WebSocket and test
//! transports follow the same idempotency and ordering rules.

use serde::{Deserialize, Serialize};

use crate::remote_worker::{WorkerHeartbeat, WorkerRegistration};
use crate::remote_worker_registry::{
    WorkerHeartbeatAck, WorkerRecord, WorkerRegistrationOutcome, WorkerRegistrationResponse,
    WorkerRegistry, WorkerRegistryError, DEFAULT_HEARTBEAT_INTERVAL_MS,
    DEFAULT_HEARTBEAT_TIMEOUT_MS,
};

pub use crate::remote_worker_registry::{
    WORKER_HEARTBEAT_ENDPOINT_PREFIX, WORKER_PAIRING_ENDPOINT,
    WORKER_REGISTRATION_ENDPOINT,
};

/// Request payload for `POST /v1/workers/register`.
///
/// The alias keeps the wire shape identical to `WorkerRegistration`; it is
/// named separately so web handlers can document request/response boundaries.
pub type WorkerRegistrationRequest = WorkerRegistration;

/// Request payload for `POST /v1/workers/{worker_id}/heartbeat`.
pub type WorkerHeartbeatRequest = WorkerHeartbeat;

/// A compact response returned after a successful registration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegistrationApiResponse {
    pub worker_id: String,
    pub accepted: bool,
    pub server_time_ms: u64,
    pub heartbeat_interval_ms: u64,
}

/// Error shape suitable for a JSON HTTP response body.  The adapter chooses
/// status codes (401/404/409/422) based on the error; Core never assumes a
/// particular web framework.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerApiErrorBody {
    pub code: String,
}

impl From<&WorkerRegistryError> for WorkerApiErrorBody {
    fn from(error: &WorkerRegistryError) -> Self {
        Self {
            code: error.to_string(),
        }
    }
}

/// Host-owned API state.  Put this value behind the web server's existing
/// transaction/async lock.  Authentication is intentionally performed by the
/// caller before `register`/`heartbeat`; credentials never enter Core state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteWorkerApi {
    registry: WorkerRegistry,
    heartbeat_interval_ms: u64,
    heartbeat_timeout_ms: u64,
}

impl Default for RemoteWorkerApi {
    fn default() -> Self {
        Self::new(DEFAULT_HEARTBEAT_INTERVAL_MS, DEFAULT_HEARTBEAT_TIMEOUT_MS)
    }
}

impl RemoteWorkerApi {
    pub fn new(heartbeat_interval_ms: u64, heartbeat_timeout_ms: u64) -> Self {
        Self {
            registry: WorkerRegistry::new(),
            heartbeat_interval_ms: heartbeat_interval_ms.max(1),
            heartbeat_timeout_ms: heartbeat_timeout_ms.max(1),
        }
    }

    pub fn from_registry(
        registry: WorkerRegistry,
        heartbeat_interval_ms: u64,
        heartbeat_timeout_ms: u64,
    ) -> Self {
        Self {
            registry,
            heartbeat_interval_ms: heartbeat_interval_ms.max(1),
            heartbeat_timeout_ms: heartbeat_timeout_ms.max(1),
        }
    }

    pub fn registry(&self) -> &WorkerRegistry {
        &self.registry
    }

    pub fn registry_mut(&mut self) -> &mut WorkerRegistry {
        &mut self.registry
    }

    pub fn heartbeat_interval_ms(&self) -> u64 {
        self.heartbeat_interval_ms
    }

    pub fn heartbeat_timeout_ms(&self) -> u64 {
        self.heartbeat_timeout_ms
    }

    /// Implements the registration state transition for the web handler.
    pub fn register(
        &mut self,
        request: WorkerRegistrationRequest,
        server_time_ms: u64,
    ) -> Result<RegistrationApiResponse, WorkerRegistryError> {
        self.register_with_outcome(request, server_time_ms)
            .map(|(response, _)| response)
    }

    /// Register and expose whether this was a new Worker or an exact retry.
    /// HTTP adapters that need to choose 201 versus 200 can use this method;
    /// the compact [`register`] helper keeps the original response shape.
    pub fn register_with_outcome(
        &mut self,
        request: WorkerRegistrationRequest,
        server_time_ms: u64,
    ) -> Result<
        (RegistrationApiResponse, WorkerRegistrationOutcome),
        WorkerRegistryError,
    > {
        let worker_id = request.worker_id.clone();
        let outcome = self.registry.register(request, server_time_ms)?;
        Ok((
            RegistrationApiResponse {
                worker_id,
                accepted: true,
                server_time_ms,
                heartbeat_interval_ms: self.heartbeat_interval_ms,
            },
            outcome,
        ))
    }

    /// Implements the heartbeat state transition for the web handler.  The
    /// path parameter must be passed separately so a malformed body cannot
    /// update another Worker.
    pub fn heartbeat(
        &mut self,
        path_worker_id: &str,
        request: WorkerHeartbeatRequest,
    ) -> Result<WorkerHeartbeatAck, WorkerRegistryError> {
        let mut ack = self.registry.heartbeat(path_worker_id, request)?;
        ack.heartbeat_interval_ms = self.heartbeat_interval_ms;
        Ok(ack)
    }

    /// Run from a periodic Host maintenance task.  The returned IDs can be
    /// used to append an offline transition to the existing audit/event log.
    pub fn expire_stale(&mut self, now_ms: u64) -> Vec<String> {
        self.registry
            .expire_stale(now_ms, self.heartbeat_timeout_ms)
    }

    pub fn worker(&self, worker_id: &str) -> Option<&WorkerRecord> {
        self.registry.get(worker_id)
    }

    pub fn registration_endpoint() -> &'static str {
        WORKER_REGISTRATION_ENDPOINT
    }

    pub fn heartbeat_endpoint(worker_id: &str) -> String {
        format!("{WORKER_HEARTBEAT_ENDPOINT_PREFIX}/{worker_id}/heartbeat")
    }
}

// Compile-time check that the response type remains wire-compatible with the
// registry response.  This is intentionally a function rather than a type
// alias so API docs keep exposing the web-oriented name above.
#[allow(dead_code)]
fn _response_shape(response: WorkerRegistrationResponse) -> RegistrationApiResponse {
    RegistrationApiResponse {
        worker_id: response.worker_id,
        accepted: response.accepted,
        server_time_ms: response.server_time_ms,
        heartbeat_interval_ms: response.heartbeat_interval_ms,
    }
}

// Keep this alias available to adapters that historically imported the
// registry's acknowledgement name from the API module.
pub type HeartbeatApiResponse = WorkerHeartbeatAck;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_worker::{
        AgentCapability, WorkspaceCapability, WorkerCapabilities,
        WorkerStatus, REMOTE_WORKER_PROTOCOL_VERSION,
    };

    fn registration() -> WorkerRegistration {
        WorkerRegistration {
            protocol_version: REMOTE_WORKER_PROTOCOL_VERSION,
            worker_id: "worker-api".into(),
            machine_name: "test-box".into(),
            capabilities: WorkerCapabilities {
                platform: "linux".into(),
                architecture: "x86_64".into(),
                max_concurrent_tasks: 1,
                agents: vec![AgentCapability {
                    kind: "codex_cli".into(),
                    display_name: "Codex".into(),
                    version: None,
                    models: vec!["default".into()],
                    supports_streaming: true,
                    supports_cancel: true,
                }],
                workspaces: vec![WorkspaceCapability {
                    workspace_id: "main".into(),
                    display_name: "Main".into(),
                    root: "/srv/main".into(),
                    read_only: false,
                }],
            },
            labels: vec![],
            registration_nonce: "pairing".into(),
        }
    }

    #[test]
    fn api_routes_registration_and_heartbeat_through_registry() {
        let mut api = RemoteWorkerApi::new(5_000, 20_000);
        let response = api.register(registration(), 100).unwrap();
        assert_eq!(response.worker_id, "worker-api");
        assert_eq!(response.heartbeat_interval_ms, 5_000);
        assert_eq!(
            RemoteWorkerApi::heartbeat_endpoint("worker-api"),
            "/v1/workers/worker-api/heartbeat"
        );

        let ack = api
            .heartbeat(
                "worker-api",
                WorkerHeartbeat {
                    worker_id: "worker-api".into(),
                    status: WorkerStatus::Online,
                    running_task_ids: vec![],
                    load_percent: 0,
                    observed_at_ms: 200,
                },
            )
            .unwrap();
        assert_eq!(ack.heartbeat_interval_ms, 5_000);
        assert!(api.worker("worker-api").is_some());
        assert!(api.expire_stale(20_199).is_empty());
        assert_eq!(api.expire_stale(20_200), vec!["worker-api"]);
    }

    #[test]
    fn path_and_body_worker_ids_cannot_be_crossed() {
        let mut api = RemoteWorkerApi::default();
        api.register(registration(), 100).unwrap();
        let error = api
            .heartbeat(
                "other-worker",
                WorkerHeartbeat {
                    worker_id: "worker-api".into(),
                    status: WorkerStatus::Online,
                    running_task_ids: vec![],
                    load_percent: 0,
                    observed_at_ms: 101,
                },
            )
            .unwrap_err();
        assert_eq!(error, WorkerRegistryError::WorkerIdMismatch);
    }

    #[test]
    fn registration_exposes_new_vs_idempotent_outcome() {
        let mut api = RemoteWorkerApi::default();
        let first = api
            .register_with_outcome(registration(), 100)
            .unwrap();
        assert_eq!(first.1, WorkerRegistrationOutcome::Registered);
        let retry = api
            .register_with_outcome(registration(), 101)
            .unwrap();
        assert_eq!(retry.1, WorkerRegistrationOutcome::AlreadyRegistered);
    }
}
