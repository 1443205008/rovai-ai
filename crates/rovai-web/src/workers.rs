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
use rovai_core::remote_worker_api::{WorkerHeartbeatRequest, WorkerRegistrationRequest};
use rovai_core::remote_worker_registry::WorkerRegistryError;
use serde_json::json;

fn registry_error(error: WorkerRegistryError) -> Response {
    let status = match error {
        WorkerRegistryError::InvalidRegistration(_)
        | WorkerRegistryError::InvalidHeartbeat(_)
        | WorkerRegistryError::InvalidTask(_)
        | WorkerRegistryError::UnknownAgent
        | WorkerRegistryError::UnknownWorkspace
        | WorkerRegistryError::WorkspaceReadOnly => StatusCode::UNPROCESSABLE_ENTITY,
        WorkerRegistryError::WorkerNotFound => StatusCode::NOT_FOUND,
        WorkerRegistryError::WorkerAlreadyRegistered
        | WorkerRegistryError::RegistrationNonceMismatch
        | WorkerRegistryError::WorkerIdMismatch
        | WorkerRegistryError::HeartbeatOutOfOrder => StatusCode::CONFLICT,
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
}
