//! Authenticated HTTP adapter for the remote Worker registration protocol.
//!
//! `rovai_core::remote_worker_api::RemoteWorkerApi` owns the state transition
//! and validation.  This module only handles JSON extraction, HTTP status
//! mapping, and the shared lock.  Machine credentials and one-time pairing are
//! intentionally not invented here: the routes stay behind the existing web
//! session middleware until that authentication contract is available.

use crate::{WebState, error};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use rovai_core::remote_worker::{
    REMOTE_WORKER_PROTOCOL_VERSION, RemoteTaskRequest, WorkerEnvelope, WorkerStatus,
};
use rovai_core::remote_worker_api::{
    WorkerHeartbeatRequest, WorkerPollRequest, WorkerRegistrationRequest, WorkerTaskAckRequest,
    WorkerTaskQueueSnapshot,
};
use rovai_core::remote_worker_registry::{WorkerRecord, WorkerRegistryError};
use serde_json::{Value, json};

/// M3 is a transport fixture.  It accepts and queues protocol messages, but
/// no Runtime Adapter is connected, so a read-only view must say so plainly.
const EXECUTION_AVAILABLE: bool = false;
const FIXTURE_ONLY: bool = true;

fn status_value(status: &WorkerStatus) -> Value {
    serde_json::to_value(status).unwrap_or_else(|_| Value::String("unknown".into()))
}

fn worker_view(
    record: &WorkerRecord,
    queue: Option<WorkerTaskQueueSnapshot>,
    now_ms: u64,
    timeout_ms: u64,
) -> Value {
    let heartbeat_age_ms = now_ms.saturating_sub(record.last_heartbeat_at_ms);
    let stale = heartbeat_age_ms >= timeout_ms;
    let status = if stale {
        Value::String("offline".into())
    } else {
        status_value(&record.status)
    };
    let queue = queue.unwrap_or_default();
    json!({
        "workerId": record.registration.worker_id,
        "machineName": record.registration.machine_name,
        "protocolVersion": record.registration.protocol_version,
        "labels": record.registration.labels,
        "capabilities": record.registration.capabilities,
        "health": {
            "status": status,
            "reportedStatus": status_value(&record.status),
            "stale": stale,
            "loadPercent": record.load_percent,
            "runningTaskIds": record.running_task_ids,
            "registeredAtMs": record.registered_at_ms,
            "lastHeartbeatAtMs": record.last_heartbeat_at_ms,
            "heartbeatAgeMs": heartbeat_age_ms,
            "heartbeatTimeoutMs": timeout_ms,
            "heartbeatCount": record.heartbeat_count,
        },
        "queue": {
            "pending": queue.pending,
            "leased": queue.leased,
            "acknowledged": queue.acknowledged,
            "retainedEvents": queue.retained_events,
            "truncated": queue.truncated,
        },
        "executionAvailable": EXECUTION_AVAILABLE,
        "fixtureOnly": FIXTURE_ONLY,
    })
}

/// List machine capabilities and a liveness projection. This is intentionally
/// read-only; stale Workers are reported offline without mutating the registry.
pub(crate) async fn list(State(state): State<WebState>) -> Response {
    let api = state.worker_api.lock().await;
    let now_ms = state.sessions.now();
    let workers = api
        .registry()
        .iter()
        .map(|record| {
            worker_view(
                record,
                api.task_queue_snapshot(&record.registration.worker_id),
                now_ms,
                api.heartbeat_timeout_ms(),
            )
        })
        .collect::<Vec<_>>();
    Json(json!({
        "protocolVersion": REMOTE_WORKER_PROTOCOL_VERSION,
        "workers": workers,
        "executionAvailable": EXECUTION_AVAILABLE,
        "fixtureOnly": FIXTURE_ONLY,
    }))
    .into_response()
}

/// Return one machine's capabilities and health projection.
pub(crate) async fn show(State(state): State<WebState>, Path(worker_id): Path<String>) -> Response {
    let api = state.worker_api.lock().await;
    let Some(record) = api.worker(&worker_id) else {
        return registry_error(WorkerRegistryError::WorkerNotFound);
    };
    let view = worker_view(
        record,
        api.task_queue_snapshot(&worker_id),
        state.sessions.now(),
        api.heartbeat_timeout_ms(),
    );
    Json(view).into_response()
}

/// Return bounded, transport-fixture task history. Prompt text and terminal
/// output are intentionally absent: this queue has no real execution yet.
pub(crate) async fn history(
    State(state): State<WebState>,
    Path(worker_id): Path<String>,
) -> Response {
    let api = state.worker_api.lock().await;
    if api.worker(&worker_id).is_none() {
        return registry_error(WorkerRegistryError::WorkerNotFound);
    }
    let snapshot = api.task_queue_snapshot(&worker_id).unwrap_or_default();
    let truncated = snapshot.truncated;
    let tasks = snapshot.tasks;
    Json(json!({
        "workerId": worker_id,
        "tasks": tasks,
        "executionAvailable": EXECUTION_AVAILABLE,
        "fixtureOnly": FIXTURE_ONLY,
        "durable": false,
        "truncated": truncated,
    }))
    .into_response()
}

fn registry_error(error: WorkerRegistryError) -> Response {
    let status = match error {
        WorkerRegistryError::InvalidRegistration(_)
        | WorkerRegistryError::InvalidHeartbeat(_)
        | WorkerRegistryError::InvalidTask(_)
        | WorkerRegistryError::InvalidEvent(_)
        | WorkerRegistryError::UnknownAgent
        | WorkerRegistryError::UnknownWorkspace
        | WorkerRegistryError::WorkspaceReadOnly => StatusCode::UNPROCESSABLE_ENTITY,
        WorkerRegistryError::WorkerNotFound => StatusCode::NOT_FOUND,
        WorkerRegistryError::WorkerOffline => StatusCode::CONFLICT,
        WorkerRegistryError::WorkerAlreadyRegistered
        | WorkerRegistryError::RegistrationNonceMismatch
        | WorkerRegistryError::WorkerIdMismatch
        | WorkerRegistryError::HeartbeatOutOfOrder
        | WorkerRegistryError::EventOutOfOrder => StatusCode::CONFLICT,
        WorkerRegistryError::TaskNotFound => StatusCode::NOT_FOUND,
    };
    (status, Json(json!({"error": {"code": error.to_string()}}))).into_response()
}

/// Register a Worker through the authenticated internal transport adapter.
pub(crate) async fn register(
    State(state): State<WebState>,
    body: Result<Json<WorkerRegistrationRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(body)) = body else {
        return error(StatusCode::BAD_REQUEST, "invalid_worker_registration");
    };
    let server_time_ms = state.sessions.now();
    let mut api = state.worker_api.lock().await;
    match api.register_with_outcome(body, server_time_ms) {
        Ok((response, outcome)) => {
            let status = match outcome {
                rovai_core::remote_worker_registry::WorkerRegistrationOutcome::Registered => {
                    StatusCode::CREATED
                }
                rovai_core::remote_worker_registry::WorkerRegistrationOutcome::AlreadyRegistered => {
                    StatusCode::OK
                }
            };
            (status, Json(response)).into_response()
        }
        Err(error) => registry_error(error),
    }
}

/// Record a Worker heartbeat through the authenticated internal transport
/// adapter.  The path identity is checked by Core against the body identity;
/// a body cannot update a different Worker record.
pub(crate) async fn heartbeat(
    State(state): State<WebState>,
    Path(worker_id): Path<String>,
    body: Result<Json<WorkerHeartbeatRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(body)) = body else {
        return error(StatusCode::BAD_REQUEST, "invalid_worker_heartbeat");
    };
    let mut api = state.worker_api.lock().await;
    match api.heartbeat(&worker_id, body) {
        Ok(response) => Json(response).into_response(),
        Err(error) => registry_error(error),
    }
}

/// Queue a task for a registered Worker.  The Core API performs capability,
/// workspace, permission and queue-capacity validation before admitting it.
pub(crate) async fn dispatch(
    State(state): State<WebState>,
    body: Result<Json<RemoteTaskRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(body)) = body else {
        return error(StatusCode::BAD_REQUEST, "invalid_worker_task");
    };
    let mut api = state.worker_api.lock().await;
    match api.dispatch_task(body) {
        Ok(outcome) => Json(json!({
            "accepted": true,
            "outcome": match outcome {
                rovai_core::remote_worker_api::WorkerTaskQueueOutcome::Queued => "queued",
                rovai_core::remote_worker_api::WorkerTaskQueueOutcome::AlreadyQueued => "already_queued",
                rovai_core::remote_worker_api::WorkerTaskQueueOutcome::AlreadyLeased => "already_leased",
            }
        }))
        .into_response(),
        Err(error) => registry_error(error),
    }
}

/// Lease pending tasks for a Worker.  Leases remain in Core until acked.
pub(crate) async fn poll(
    State(state): State<WebState>,
    Path(worker_id): Path<String>,
    body: Result<Json<WorkerPollRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(body)) = body else {
        return error(StatusCode::BAD_REQUEST, "invalid_worker_poll");
    };
    let mut api = state.worker_api.lock().await;
    match api.poll_tasks(&worker_id, body.limit) {
        Ok(response) => Json(response).into_response(),
        Err(error) => registry_error(error),
    }
}

/// Acknowledge a previously leased task.  The path and body Worker IDs are
/// compared before any state change, preventing cross-Worker acknowledgements.
pub(crate) async fn acknowledge(
    State(state): State<WebState>,
    Path((worker_id, task_id)): Path<(String, String)>,
    body: Result<Json<WorkerTaskAckRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(body)) = body else {
        return error(StatusCode::BAD_REQUEST, "invalid_worker_task_ack");
    };
    if body.task_id != task_id {
        return registry_error(WorkerRegistryError::InvalidTask("task_id_mismatch"));
    }
    let mut api = state.worker_api.lock().await;
    match api.acknowledge_task(&worker_id, body) {
        Ok(response) => Json(response).into_response(),
        Err(error) => registry_error(error),
    }
}

/// Accept a strictly ordered Worker event.  M3 stores bounded envelopes for a
/// later Core projector; it never executes an event's payload as a command.
pub(crate) async fn event(
    State(state): State<WebState>,
    Path(worker_id): Path<String>,
    body: Result<Json<WorkerEnvelope<Value>>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(body)) = body else {
        return error(StatusCode::BAD_REQUEST, "invalid_worker_event");
    };
    let mut api = state.worker_api.lock().await;
    match api.accept_event(&worker_id, body) {
        Ok(response) => Json(response).into_response(),
        Err(error) => registry_error(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rovai_core::remote_worker::{
        AgentCapability, REMOTE_WORKER_PROTOCOL_VERSION, WorkerCapabilities, WorkerHeartbeat,
        WorkerRegistration, WorkerStatus, WorkspaceCapability,
    };
    use rovai_core::remote_worker_api::RemoteWorkerApi;
    use rovai_core::remote_worker_registry::WorkerRegistrationOutcome;

    fn registration() -> WorkerRegistration {
        WorkerRegistration {
            protocol_version: REMOTE_WORKER_PROTOCOL_VERSION,
            worker_id: "worker-http".into(),
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
            registration_nonce: "nonce".into(),
        }
    }

    #[test]
    fn registration_is_created_then_idempotent() {
        let mut api = RemoteWorkerApi::default();
        let first = api.register_with_outcome(registration(), 1).unwrap();
        assert_eq!(first.0.worker_id, "worker-http");
        assert_eq!(first.1, WorkerRegistrationOutcome::Registered);
        let retry = api.register_with_outcome(registration(), 2).unwrap();
        assert_eq!(retry.1, WorkerRegistrationOutcome::AlreadyRegistered);
    }

    #[test]
    fn heartbeat_requires_matching_path_identity() {
        let mut api = RemoteWorkerApi::default();
        api.register(registration(), 1).unwrap();
        let error = api
            .heartbeat(
                "other-worker",
                WorkerHeartbeat {
                    worker_id: "worker-http".into(),
                    status: WorkerStatus::Online,
                    running_task_ids: vec![],
                    load_percent: 0,
                    observed_at_ms: 2,
                },
            )
            .unwrap_err();
        assert_eq!(error, WorkerRegistryError::WorkerIdMismatch);
        let response = registry_error(error);
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn malformed_json_is_a_client_error() {
        let response = error(StatusCode::BAD_REQUEST, "invalid_worker_registration");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn read_projection_exposes_health_and_fixture_boundary_without_pairing_secret() {
        let mut api = RemoteWorkerApi::default();
        api.register(registration(), 100).unwrap();
        let record = api.worker("worker-http").unwrap();
        let view = worker_view(
            record,
            api.task_queue_snapshot("worker-http"),
            60_100,
            60_000,
        );
        assert_eq!(view["health"]["status"], "offline");
        assert_eq!(view["health"]["reportedStatus"], "online");
        assert_eq!(view["executionAvailable"], false);
        assert_eq!(view["fixtureOnly"], true);
        assert!(view.get("registrationNonce").is_none());
        assert!(view["capabilities"]["agents"].is_array());
    }

    #[test]
    fn task_history_projection_contains_queue_identity_only() {
        let mut api = RemoteWorkerApi::default();
        api.register(registration(), 100).unwrap();
        api.dispatch_task(RemoteTaskRequest {
            task_id: "task-http".into(),
            worker_id: "worker-http".into(),
            agent_kind: "codex_cli".into(),
            workspace_id: "main".into(),
            prompt: "private prompt must not be projected".into(),
            permission: rovai_core::remote_worker::RemoteTaskPermission::ReadOnly,
            timeout_seconds: 30,
            attempt: 1,
        })
        .unwrap();
        let snapshot = api.task_queue_snapshot("worker-http").unwrap();
        assert_eq!(snapshot.tasks.len(), 1);
        let task = serde_json::to_value(&snapshot.tasks[0]).unwrap();
        assert_eq!(task["taskId"], "task-http");
        assert!(task.get("prompt").is_none());
        assert_eq!(task["state"], "queued");
    }
}
