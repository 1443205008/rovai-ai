//! Versioned protocol primitives for dispatching AgentRuns to another machine.
//!
//! This module deliberately contains only portable, authenticated-message data.
//! Transport, persistence, and process supervision remain owned by Host/Core.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const REMOTE_WORKER_PROTOCOL_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerStatus {
    Online,
    Busy,
    Draining,
    Offline,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerEventKind {
    Registered,
    Heartbeat,
    TaskAccepted,
    TaskStarted,
    TextDelta,
    ToolCall,
    ApprovalRequired,
    FileChange,
    TaskCompleted,
    TaskFailed,
    TaskCancelled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerEnvelope<T> {
    pub protocol_version: u32,
    pub worker_id: String,
    pub message_id: String,
    pub sequence: u64,
    pub sent_at_ms: u64,
    pub kind: WorkerEventKind,
    pub payload: T,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCapability {
    pub kind: String,
    pub display_name: String,
    pub version: Option<String>,
    pub models: Vec<String>,
    pub supports_streaming: bool,
    pub supports_cancel: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceCapability {
    pub workspace_id: String,
    pub display_name: String,
    pub root: String,
    pub read_only: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerCapabilities {
    pub platform: String,
    pub architecture: String,
    pub max_concurrent_tasks: u32,
    pub agents: Vec<AgentCapability>,
    pub workspaces: Vec<WorkspaceCapability>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerRegistration {
    pub protocol_version: u32,
    pub worker_id: String,
    pub machine_name: String,
    pub capabilities: WorkerCapabilities,
    pub labels: Vec<String>,
    pub registration_nonce: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerHeartbeat {
    pub worker_id: String,
    pub status: WorkerStatus,
    pub running_task_ids: Vec<String>,
    pub load_percent: u8,
    pub observed_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerRecord {
    pub registration: WorkerRegistration,
    pub status: WorkerStatus,
    pub last_heartbeat_ms: u64,
    pub running_task_ids: Vec<String>,
    pub load_percent: u8,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WorkerRegistry {
    workers: BTreeMap<String, WorkerRecord>,
}

impl WorkerRegistry {
    pub fn register(&mut self, registration: WorkerRegistration, now_ms: u64) -> Result<(), &'static str> {
        validate_registration(&registration)?;
        let worker_id = registration.worker_id.clone();
        self.workers.insert(
            worker_id,
            WorkerRecord {
                registration,
                status: WorkerStatus::Online,
                last_heartbeat_ms: now_ms,
                running_task_ids: Vec::new(),
                load_percent: 0,
            },
        );
        Ok(())
    }

    pub fn heartbeat(&mut self, heartbeat: WorkerHeartbeat) -> Result<(), &'static str> {
        let Some(record) = self.workers.get_mut(&heartbeat.worker_id) else {
            return Err("worker_not_registered");
        };
        if heartbeat.load_percent > 100 {
            return Err("worker_load_invalid");
        }
        record.status = heartbeat.status;
        record.last_heartbeat_ms = heartbeat.observed_at_ms;
        record.running_task_ids = heartbeat.running_task_ids;
        record.load_percent = heartbeat.load_percent;
        Ok(())
    }

    pub fn get(&self, worker_id: &str) -> Option<&WorkerRecord> {
        self.workers.get(worker_id)
    }

    pub fn records(&self) -> impl Iterator<Item = &WorkerRecord> {
        self.workers.values()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteTaskPermission {
    ReadOnly,
    WorkspaceWrite,
    Elevated,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteTaskRequest {
    pub task_id: String,
    pub worker_id: String,
    pub agent_kind: String,
    pub workspace_id: String,
    pub prompt: String,
    pub permission: RemoteTaskPermission,
    pub timeout_seconds: u32,
    pub attempt: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteTaskCancel {
    pub task_id: String,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventCursor {
    worker_id: String,
    next_sequence: u64,
}

impl EventCursor {
    pub fn new(worker_id: impl Into<String>) -> Self {
        Self {
            worker_id: worker_id.into(),
            next_sequence: 1,
        }
    }

    pub fn accept<T>(&mut self, event: &WorkerEnvelope<T>) -> bool {
        if event.worker_id != self.worker_id || event.sequence != self.next_sequence {
            return false;
        }
        self.next_sequence = self.next_sequence.saturating_add(1);
        true
    }

    pub fn next_sequence(&self) -> u64 {
        self.next_sequence
    }
}

pub fn validate_registration(value: &WorkerRegistration) -> Result<(), &'static str> {
    if value.protocol_version != REMOTE_WORKER_PROTOCOL_VERSION {
        return Err("protocol_incompatible");
    }
    if value.worker_id.trim().is_empty() || value.machine_name.trim().is_empty() {
        return Err("worker_identity_required");
    }
    if value.registration_nonce.trim().is_empty() {
        return Err("registration_nonce_required");
    }
    if value.capabilities.max_concurrent_tasks == 0 {
        return Err("worker_capacity_required");
    }
    Ok(())
}

pub fn validate_task(value: &RemoteTaskRequest) -> Result<(), &'static str> {
    if value.task_id.trim().is_empty() || value.worker_id.trim().is_empty() {
        return Err("task_identity_required");
    }
    if value.agent_kind.trim().is_empty() || value.workspace_id.trim().is_empty() {
        return Err("task_target_required");
    }
    if value.prompt.trim().is_empty() {
        return Err("task_prompt_required");
    }
    if value.timeout_seconds == 0 || value.timeout_seconds > 86_400 {
        return Err("task_timeout_invalid");
    }
    if value.attempt == 0 {
        return Err("task_attempt_invalid");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capabilities() -> WorkerCapabilities {
        WorkerCapabilities {
            platform: "linux".into(),
            architecture: "x86_64".into(),
            max_concurrent_tasks: 2,
            agents: vec![AgentCapability {
                kind: "codex_cli".into(),
                display_name: "Codex CLI".into(),
                version: Some("0.1".into()),
                models: vec!["default".into()],
                supports_streaming: true,
                supports_cancel: true,
            }],
            workspaces: vec![WorkspaceCapability {
                workspace_id: "main".into(),
                display_name: "Main".into(),
                root: "/srv/projects/main".into(),
                read_only: false,
            }],
        }
    }

    #[test]
    fn registration_round_trips_and_validates() {
        let registration = WorkerRegistration {
            protocol_version: REMOTE_WORKER_PROTOCOL_VERSION,
            worker_id: "worker-1".into(),
            machine_name: "build-box".into(),
            capabilities: capabilities(),
            labels: vec!["gpu".into()],
            registration_nonce: "nonce".into(),
        };
        validate_registration(&registration).unwrap();
        let encoded = serde_json::to_string(&registration).unwrap();
        let decoded: WorkerRegistration = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, registration);
    }

    #[test]
    fn task_validation_rejects_unbounded_or_empty_requests() {
        let mut task = RemoteTaskRequest {
            task_id: "task-1".into(),
            worker_id: "worker-1".into(),
            agent_kind: "codex_cli".into(),
            workspace_id: "main".into(),
            prompt: "inspect the project".into(),
            permission: RemoteTaskPermission::ReadOnly,
            timeout_seconds: 60,
            attempt: 1,
        };
        validate_task(&task).unwrap();
        task.timeout_seconds = 86_401;
        assert_eq!(validate_task(&task), Err("task_timeout_invalid"));
        task.timeout_seconds = 60;
        task.prompt.clear();
        assert_eq!(validate_task(&task), Err("task_prompt_required"));
    }

    #[test]
    fn event_cursor_rejects_duplicates_gaps_and_other_workers() {
        let payload = WorkerHeartbeat {
            worker_id: "worker-1".into(),
            status: WorkerStatus::Online,
            running_task_ids: vec![],
            load_percent: 0,
            observed_at_ms: 1,
        };
        let event = |worker_id: &str, sequence| WorkerEnvelope {
            protocol_version: REMOTE_WORKER_PROTOCOL_VERSION,
            worker_id: worker_id.into(),
            message_id: format!("message-{sequence}"),
            sequence,
            sent_at_ms: 1,
            kind: WorkerEventKind::Heartbeat,
            payload: payload.clone(),
        };
        let mut cursor = EventCursor::new("worker-1");
        assert!(cursor.accept(&event("worker-1", 1)));
        assert!(!cursor.accept(&event("worker-1", 1)));
        assert!(!cursor.accept(&event("worker-1", 3)));
        assert!(!cursor.accept(&event("worker-2", 2)));
        assert!(cursor.accept(&event("worker-1", 2)));
        assert_eq!(cursor.next_sequence(), 3);
    }

    #[test]
    fn registry_tracks_registration_and_heartbeat() {
        let registration = WorkerRegistration {
            protocol_version: REMOTE_WORKER_PROTOCOL_VERSION,
            worker_id: "worker-1".into(),
            machine_name: "build-box".into(),
            capabilities: capabilities(),
            labels: vec![],
            registration_nonce: "nonce".into(),
        };
        let mut registry = WorkerRegistry::default();
        registry.register(registration.clone(), 10).unwrap();
        assert_eq!(registry.get("worker-1").unwrap().last_heartbeat_ms, 10);
        registry
            .heartbeat(WorkerHeartbeat {
                worker_id: "worker-1".into(),
                status: WorkerStatus::Busy,
                running_task_ids: vec!["task-1".into()],
                load_percent: 50,
                observed_at_ms: 20,
            })
            .unwrap();
        let record = registry.get("worker-1").unwrap();
        assert_eq!(record.status, WorkerStatus::Busy);
        assert_eq!(record.running_task_ids, vec!["task-1"]);
        assert_eq!(registry.records().count(), 1);
    }
}
