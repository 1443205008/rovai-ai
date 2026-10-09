//! Transport-neutral web API seam for remote Worker registration and
//! heartbeats.
//!
//! An HTTP adapter can deserialize request JSON into the protocol types, do
//! mTLS or machine-credential authentication, and then delegate to this type.
//! Keeping the state transition here makes the HTTP, WebSocket and test
//! transports follow the same idempotency and ordering rules.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    mem,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::remote_worker::{
    EventCursor, RemoteTaskPermission, RemoteTaskRequest, WorkerEnvelope, WorkerHeartbeat,
    WorkerRegistration, validate_envelope,
};
use crate::remote_worker_registry::{
    DEFAULT_HEARTBEAT_INTERVAL_MS, DEFAULT_HEARTBEAT_TIMEOUT_MS, WorkerHeartbeatAck, WorkerRecord,
    WorkerRegistrationOutcome, WorkerRegistrationResponse, WorkerRegistry, WorkerRegistryError,
};

pub use crate::remote_worker_registry::{
    WORKER_HEARTBEAT_ENDPOINT_PREFIX, WORKER_PAIRING_ENDPOINT, WORKER_REGISTRATION_ENDPOINT,
};

/// Request payload for `POST /v1/workers/register`.
///
/// The alias keeps the wire shape identical to `WorkerRegistration`; it is
/// named separately so web handlers can document request/response boundaries.
pub type WorkerRegistrationRequest = WorkerRegistration;

/// Request payload for `POST /v1/workers/{worker_id}/heartbeat`.
pub type WorkerHeartbeatRequest = WorkerHeartbeat;

/// Maximum number of tasks retained for a Worker before it polls.  The queue
/// is deliberately bounded because this M3 slice is an in-memory transport
/// adapter; durable scheduling remains a later Core milestone.
pub const MAX_QUEUED_TASKS_PER_WORKER: usize = 256;
pub const MAX_POLL_BATCH: usize = 64;
pub const MAX_RETAINED_EVENTS_PER_WORKER: usize = 1024;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerPollRequest {
    #[serde(default = "default_poll_limit")]
    pub limit: usize,
}

fn default_poll_limit() -> usize {
    16
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerPollResponse {
    pub worker_id: String,
    pub tasks: Vec<RemoteTaskRequest>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerTaskAckRequest {
    pub worker_id: String,
    pub task_id: String,
    pub accepted: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerTaskAckResponse {
    pub worker_id: String,
    pub task_id: String,
    pub accepted: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerEventResponse {
    pub worker_id: String,
    pub accepted: bool,
    pub sequence: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkerTaskQueueOutcome {
    Queued,
    AlreadyQueued,
    AlreadyLeased,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct WorkerQueue {
    pending: VecDeque<RemoteTaskRequest>,
    leased: BTreeMap<String, RemoteTaskRequest>,
    acknowledged: BTreeSet<String>,
    cursor: EventCursor,
    events: VecDeque<WorkerEnvelope<Value>>,
}

impl WorkerQueue {
    fn new(worker_id: &str) -> Self {
        Self {
            pending: VecDeque::new(),
            leased: BTreeMap::new(),
            acknowledged: BTreeSet::new(),
            cursor: EventCursor::new(worker_id),
            events: VecDeque::new(),
        }
    }
}

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
    queues: BTreeMap<String, WorkerQueue>,
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
            queues: BTreeMap::new(),
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
            queues: BTreeMap::new(),
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
    ) -> Result<(RegistrationApiResponse, WorkerRegistrationOutcome), WorkerRegistryError> {
        let worker_id = request.worker_id.clone();
        let outcome = self.registry.register(request, server_time_ms)?;
        self.queues
            .entry(worker_id.clone())
            .or_insert_with(|| WorkerQueue::new(&worker_id));
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

    /// Queue a validated task for a registered Worker.  Exact retries are
    /// idempotent; a different request reusing an in-flight task ID is
    /// rejected by the registry error returned from this transport seam.
    pub fn dispatch_task(
        &mut self,
        request: RemoteTaskRequest,
    ) -> Result<WorkerTaskQueueOutcome, WorkerRegistryError> {
        self.registry.validate_task(&request)?;
        if matches!(request.permission, RemoteTaskPermission::Elevated) {
            return Err(WorkerRegistryError::InvalidTask(
                "elevated_permission_requires_approval",
            ));
        }
        let worker_id = request.worker_id.clone();
        let task_id = request.task_id.clone();
        let queue = self
            .queues
            .entry(worker_id.clone())
            .or_insert_with(|| WorkerQueue::new(&worker_id));
        if let Some(existing) = queue.pending.iter().find(|task| task.task_id == task_id) {
            if existing == &request {
                return Ok(WorkerTaskQueueOutcome::AlreadyQueued);
            }
            return Err(WorkerRegistryError::InvalidTask("task_id_reused"));
        }
        if let Some(existing) = queue.leased.get(&task_id) {
            if existing == &request {
                return Ok(WorkerTaskQueueOutcome::AlreadyLeased);
            }
            return Err(WorkerRegistryError::InvalidTask("task_id_reused"));
        }
        if queue.acknowledged.contains(&task_id) {
            return Err(WorkerRegistryError::InvalidTask(
                "task_already_acknowledged",
            ));
        }
        if queue.pending.len() >= MAX_QUEUED_TASKS_PER_WORKER {
            return Err(WorkerRegistryError::InvalidTask("worker_queue_full"));
        }
        queue.pending.push_back(request);
        Ok(WorkerTaskQueueOutcome::Queued)
    }

    /// Lease up to `limit` tasks for a Worker.  Leased tasks remain until the
    /// Worker acknowledges them, so a lost poll response cannot silently
    /// duplicate execution.
    pub fn poll_tasks(
        &mut self,
        worker_id: &str,
        limit: usize,
    ) -> Result<WorkerPollResponse, WorkerRegistryError> {
        if self.registry.get(worker_id).is_none() {
            return Err(WorkerRegistryError::WorkerNotFound);
        }
        if limit == 0 || limit > MAX_POLL_BATCH {
            return Err(WorkerRegistryError::InvalidTask("poll_limit_invalid"));
        }
        let queue = self
            .queues
            .entry(worker_id.to_owned())
            .or_insert_with(|| WorkerQueue::new(worker_id));
        let count = limit.min(queue.pending.len());
        let mut tasks = Vec::with_capacity(count);
        for _ in 0..count {
            if let Some(task) = queue.pending.pop_front() {
                queue.leased.insert(task.task_id.clone(), task.clone());
                tasks.push(task);
            }
        }
        Ok(WorkerPollResponse {
            worker_id: worker_id.to_owned(),
            tasks,
        })
    }

    /// Acknowledge a leased task.  Repeating the exact acknowledgement is
    /// idempotent; a different Worker identity cannot acknowledge it.
    pub fn acknowledge_task(
        &mut self,
        path_worker_id: &str,
        request: WorkerTaskAckRequest,
    ) -> Result<WorkerTaskAckResponse, WorkerRegistryError> {
        if request.worker_id != path_worker_id {
            return Err(WorkerRegistryError::WorkerIdMismatch);
        }
        if self.registry.get(path_worker_id).is_none() {
            return Err(WorkerRegistryError::WorkerNotFound);
        }
        let queue = self
            .queues
            .entry(path_worker_id.to_owned())
            .or_insert_with(|| WorkerQueue::new(path_worker_id));
        if queue.leased.remove(&request.task_id).is_some() {
            queue.acknowledged.insert(request.task_id.clone());
            return Ok(WorkerTaskAckResponse {
                worker_id: path_worker_id.to_owned(),
                task_id: request.task_id,
                accepted: request.accepted,
            });
        }
        if queue.acknowledged.contains(&request.task_id) {
            return Ok(WorkerTaskAckResponse {
                worker_id: path_worker_id.to_owned(),
                task_id: request.task_id,
                accepted: request.accepted,
            });
        }
        Err(WorkerRegistryError::TaskNotFound)
    }

    /// Accept an ordered event from a Worker.  `EventCursor` rejects gaps,
    /// duplicates, malformed metadata, and cross-Worker identities before the
    /// event is retained.  Payload dispatch is intentionally deferred to M4.
    pub fn accept_event(
        &mut self,
        path_worker_id: &str,
        event: WorkerEnvelope<Value>,
    ) -> Result<WorkerEventResponse, WorkerRegistryError> {
        if event.worker_id != path_worker_id {
            return Err(WorkerRegistryError::WorkerIdMismatch);
        }
        if self.registry.get(path_worker_id).is_none() {
            return Err(WorkerRegistryError::WorkerNotFound);
        }
        validate_envelope(&event).map_err(WorkerRegistryError::InvalidEvent)?;
        let queue = self
            .queues
            .entry(path_worker_id.to_owned())
            .or_insert_with(|| WorkerQueue::new(path_worker_id));
        if queue.events.len() >= MAX_RETAINED_EVENTS_PER_WORKER {
            return Err(WorkerRegistryError::InvalidEvent("event_queue_full"));
        }
        if !queue.cursor.accept(&event) {
            return Err(WorkerRegistryError::EventOutOfOrder);
        }
        let sequence = event.sequence;
        queue.events.push_back(event);
        Ok(WorkerEventResponse {
            worker_id: path_worker_id.to_owned(),
            accepted: true,
            sequence,
        })
    }

    /// Drain retained events for a host-side projector.  This keeps the M3
    /// transport bounded while making the accepted payloads observable in
    /// tests and future Core integration.
    pub fn drain_events(&mut self, worker_id: &str) -> Vec<WorkerEnvelope<Value>> {
        let Some(queue) = self.queues.get_mut(worker_id) else {
            return Vec::new();
        };
        mem::take(&mut queue.events).into_iter().collect()
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
        AgentCapability, REMOTE_WORKER_PROTOCOL_VERSION, WorkerCapabilities, WorkerStatus,
        WorkspaceCapability,
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
        let first = api.register_with_outcome(registration(), 100).unwrap();
        assert_eq!(first.1, WorkerRegistrationOutcome::Registered);
        let retry = api.register_with_outcome(registration(), 101).unwrap();
        assert_eq!(retry.1, WorkerRegistrationOutcome::AlreadyRegistered);
    }

    fn task(task_id: &str) -> RemoteTaskRequest {
        RemoteTaskRequest {
            task_id: task_id.into(),
            worker_id: "worker-api".into(),
            agent_kind: "codex_cli".into(),
            workspace_id: "main".into(),
            prompt: "read the fixture".into(),
            permission: RemoteTaskPermission::ReadOnly,
            timeout_seconds: 30,
            attempt: 1,
        }
    }

    #[test]
    fn task_queue_is_bounded_by_lease_and_idempotent_ack() {
        let mut api = RemoteWorkerApi::default();
        api.register(registration(), 100).unwrap();
        assert_eq!(
            api.dispatch_task(task("task-1")).unwrap(),
            WorkerTaskQueueOutcome::Queued
        );
        assert_eq!(
            api.dispatch_task(task("task-1")).unwrap(),
            WorkerTaskQueueOutcome::AlreadyQueued
        );
        let poll = api.poll_tasks("worker-api", 1).unwrap();
        assert_eq!(poll.tasks, vec![task("task-1")]);
        assert!(api.poll_tasks("worker-api", 1).unwrap().tasks.is_empty());
        let ack = WorkerTaskAckRequest {
            worker_id: "worker-api".into(),
            task_id: "task-1".into(),
            accepted: true,
        };
        assert_eq!(
            api.acknowledge_task("worker-api", ack.clone())
                .unwrap()
                .accepted,
            true
        );
        assert_eq!(
            api.acknowledge_task("worker-api", ack).unwrap().task_id,
            "task-1"
        );
        let reused = task("task-1");
        assert!(matches!(
            api.dispatch_task(reused),
            Err(WorkerRegistryError::InvalidTask(
                "task_already_acknowledged"
            ))
        ));
    }

    #[test]
    fn event_cursor_rejects_duplicates_and_gaps() {
        let mut api = RemoteWorkerApi::default();
        api.register(registration(), 100).unwrap();
        let event = |sequence| WorkerEnvelope {
            protocol_version: REMOTE_WORKER_PROTOCOL_VERSION,
            worker_id: "worker-api".into(),
            message_id: format!("message-{sequence}"),
            sequence,
            sent_at_ms: 100 + sequence,
            kind: crate::remote_worker::WorkerEventKind::TaskStarted,
            payload: serde_json::json!({"taskId":"task-1"}),
        };
        assert_eq!(
            api.accept_event("worker-api", event(1)).unwrap().sequence,
            1
        );
        assert_eq!(
            api.accept_event("worker-api", event(1)).unwrap_err(),
            WorkerRegistryError::EventOutOfOrder
        );
        assert_eq!(
            api.accept_event("worker-api", event(3)).unwrap_err(),
            WorkerRegistryError::EventOutOfOrder
        );
        assert!(api.accept_event("worker-api", event(2)).is_ok());
        assert_eq!(api.drain_events("worker-api").len(), 2);
    }
}
