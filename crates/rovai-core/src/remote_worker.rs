//! Versioned protocol primitives for dispatching AgentRuns to another machine.
//!
//! This module deliberately contains only portable, authenticated-message data.
//! Transport, persistence, and process supervision remain owned by Host/Core.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

pub const REMOTE_WORKER_PROTOCOL_VERSION: u32 = 1;

// Keep protocol limits explicit and shared by every transport adapter.  These
// are deliberately conservative upper bounds: a Worker can advertise fewer
// capabilities, but no adapter should accept an unbounded JSON array/string
// before authentication and admission checks run.
pub const MAX_WORKER_ID_LEN: usize = 128;
pub const MAX_MACHINE_NAME_LEN: usize = 128;
pub const MAX_REGISTRATION_NONCE_LEN: usize = 512;
pub const MAX_LABELS: usize = 64;
pub const MAX_LABEL_LEN: usize = 64;
pub const MAX_AGENTS: usize = 64;
pub const MAX_WORKSPACES: usize = 128;
pub const MAX_CAPABILITY_ID_LEN: usize = 128;
pub const MAX_CAPABILITY_NAME_LEN: usize = 256;
pub const MAX_MODELS_PER_AGENT: usize = 64;
pub const MAX_TASK_IDS_PER_HEARTBEAT: usize = 4_096;
pub const MAX_TASK_ID_LEN: usize = 256;
pub const MAX_MESSAGE_ID_LEN: usize = 256;
pub const MAX_EVENT_SEQUENCE: u64 = 9_000_000_000_000_000_000;
pub const MAX_ATTEMPT: u32 = 1_000_000;

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

/// In-memory compatibility cursor for transports that do not yet have a
/// durable event cursor. The server should persist the next sequence (and a
/// worker boot/session epoch) before using this across reconnects.
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
        if validate_envelope(event).is_err()
            || event.worker_id != self.worker_id
            || event.sequence != self.next_sequence
        {
            return false;
        }
        self.next_sequence = self.next_sequence.saturating_add(1);
        true
    }

    pub fn next_sequence(&self) -> u64 {
        self.next_sequence
    }
}

/// Validate envelope metadata before handing the payload to an event
/// dispatcher.  Payload-specific validation belongs to the event kind's
/// handler; these checks prevent malformed metadata from poisoning a cursor.
pub fn validate_envelope<T>(value: &WorkerEnvelope<T>) -> Result<(), &'static str> {
    if value.protocol_version != REMOTE_WORKER_PROTOCOL_VERSION {
        return Err("protocol_incompatible");
    }
    if value.worker_id.trim().is_empty() {
        return Err("worker_identity_required");
    }
    if value.worker_id.len() > MAX_WORKER_ID_LEN
        || value.worker_id.contains('/')
        || value.worker_id.chars().any(char::is_control)
    {
        return Err("worker_identity_invalid");
    }
    if value.message_id.trim().is_empty()
        || value.message_id.len() > MAX_MESSAGE_ID_LEN
        || value.message_id.chars().any(char::is_control)
    {
        return Err("message_identity_invalid");
    }
    if value.sequence == 0 || value.sequence > MAX_EVENT_SEQUENCE {
        return Err("event_sequence_invalid");
    }
    if value.sent_at_ms == 0 {
        return Err("event_timestamp_required");
    }
    Ok(())
}

pub fn validate_registration(value: &WorkerRegistration) -> Result<(), &'static str> {
    if value.protocol_version != REMOTE_WORKER_PROTOCOL_VERSION {
        return Err("protocol_incompatible");
    }
    if value.worker_id.trim().is_empty() || value.machine_name.trim().is_empty() {
        return Err("worker_identity_required");
    }
    if value.worker_id.len() > MAX_WORKER_ID_LEN
        || value.worker_id.contains('/')
        || value.worker_id.chars().any(char::is_control)
    {
        return Err("worker_identity_invalid");
    }
    if value.machine_name.len() > MAX_MACHINE_NAME_LEN
        || value.machine_name.chars().any(char::is_control)
    {
        return Err("machine_name_invalid");
    }
    if value.registration_nonce.trim().is_empty() {
        return Err("registration_nonce_required");
    }
    if value.registration_nonce.len() > MAX_REGISTRATION_NONCE_LEN
        || value.registration_nonce.chars().any(char::is_control)
    {
        return Err("registration_nonce_invalid");
    }
    if value.capabilities.max_concurrent_tasks == 0 {
        return Err("worker_capacity_required");
    }
    if value.capabilities.max_concurrent_tasks > 4_096 {
        return Err("worker_capacity_invalid");
    }
    if value.capabilities.platform.trim().is_empty()
        || value.capabilities.architecture.trim().is_empty()
        || value.capabilities.platform.len() > MAX_CAPABILITY_ID_LEN
        || value.capabilities.architecture.len() > MAX_CAPABILITY_ID_LEN
        || value.capabilities.platform.chars().any(char::is_control)
        || value.capabilities.architecture.chars().any(char::is_control)
    {
        return Err("worker_platform_required");
    }
    if value.labels.len() > MAX_LABELS {
        return Err("labels_too_many");
    }
    let mut labels = BTreeSet::new();
    for label in &value.labels {
        if label.trim().is_empty()
            || label.len() > MAX_LABEL_LEN
            || label.chars().any(char::is_control)
            || !labels.insert(label)
        {
            return Err("label_invalid");
        }
    }
    if value.capabilities.agents.len() > MAX_AGENTS {
        return Err("agents_too_many");
    }
    let mut agents = BTreeSet::new();
    for agent in &value.capabilities.agents {
        if agent.kind.trim().is_empty()
            || agent.kind.len() > MAX_CAPABILITY_ID_LEN
            || agent.kind.chars().any(char::is_control)
            || agent.display_name.trim().is_empty()
            || agent.display_name.len() > MAX_CAPABILITY_NAME_LEN
            || agent.display_name.chars().any(char::is_control)
            || !agents.insert(&agent.kind)
            || agent.models.len() > MAX_MODELS_PER_AGENT
            || agent.models.iter().any(|model| {
                model.trim().is_empty()
                    || model.len() > MAX_CAPABILITY_ID_LEN
                    || model.chars().any(char::is_control)
            })
        {
            return Err("agent_capability_invalid");
        }
    }
    if value.capabilities.workspaces.len() > MAX_WORKSPACES {
        return Err("workspaces_too_many");
    }
    let mut workspaces = BTreeSet::new();
    for workspace in &value.capabilities.workspaces {
        if workspace.workspace_id.trim().is_empty()
            || workspace.workspace_id.len() > MAX_CAPABILITY_ID_LEN
            || workspace.workspace_id.chars().any(char::is_control)
            || workspace.display_name.trim().is_empty()
            || workspace.display_name.len() > MAX_CAPABILITY_NAME_LEN
            || workspace.display_name.chars().any(char::is_control)
            || workspace.root.len() > MAX_CAPABILITY_NAME_LEN
            || workspace.root.chars().any(char::is_control)
            || !workspaces.insert(&workspace.workspace_id)
        {
            return Err("workspace_capability_invalid");
        }
    }
    Ok(())
}

/// Validate the untrusted portion of a heartbeat before looking up the
/// registered Worker.  Capacity and monotonic ordering are registry concerns.
pub fn validate_heartbeat(value: &WorkerHeartbeat) -> Result<(), &'static str> {
    if value.worker_id.trim().is_empty() {
        return Err("worker_identity_required");
    }
    if value.observed_at_ms == 0 {
        return Err("heartbeat_timestamp_required");
    }
    if value.load_percent > 100 {
        return Err("heartbeat_load_invalid");
    }
    if value.worker_id.len() > MAX_WORKER_ID_LEN
        || value.worker_id.contains('/')
        || value.worker_id.chars().any(char::is_control)
    {
        return Err("worker_identity_invalid");
    }
    if value.running_task_ids.len() > MAX_TASK_IDS_PER_HEARTBEAT {
        return Err("task_count_too_many");
    }
    let mut seen = BTreeSet::new();
    for task_id in &value.running_task_ids {
        if task_id.trim().is_empty()
            || task_id.len() > MAX_TASK_ID_LEN
            || task_id.chars().any(char::is_control)
        {
            return Err("task_identity_required");
        }
        if !seen.insert(task_id) {
            return Err("duplicate_task_id");
        }
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
    if value.task_id.len() > MAX_TASK_ID_LEN
        || value.worker_id.len() > MAX_WORKER_ID_LEN
        || value.agent_kind.len() > MAX_CAPABILITY_ID_LEN
        || value.workspace_id.len() > MAX_CAPABILITY_ID_LEN
        || value
            .task_id
            .chars()
            .chain(value.worker_id.chars())
            .chain(value.agent_kind.chars())
            .chain(value.workspace_id.chars())
            .any(char::is_control)
    {
        return Err("task_identity_invalid");
    }
    if value.prompt.trim().is_empty() {
        return Err("task_prompt_required");
    }
    if value.prompt.len() > 1_048_576 {
        return Err("task_prompt_too_large");
    }
    if value.timeout_seconds == 0 || value.timeout_seconds > 86_400 {
        return Err("task_timeout_invalid");
    }
    if value.attempt == 0 || value.attempt > MAX_ATTEMPT {
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
    fn heartbeat_validation_rejects_bad_liveness_payloads() {
        let mut heartbeat = WorkerHeartbeat {
            worker_id: "worker-1".into(),
            status: WorkerStatus::Online,
            running_task_ids: vec!["task-1".into(), "task-1".into()],
            load_percent: 0,
            observed_at_ms: 1,
        };
        assert_eq!(validate_heartbeat(&heartbeat), Err("duplicate_task_id"));
        heartbeat.running_task_ids = vec![];
        heartbeat.load_percent = 101;
        assert_eq!(validate_heartbeat(&heartbeat), Err("heartbeat_load_invalid"));
    }

    #[test]
    fn registration_validation_rejects_path_injection_worker_ids() {
        let mut registration = WorkerRegistration {
            protocol_version: REMOTE_WORKER_PROTOCOL_VERSION,
            worker_id: "worker-1".into(),
            machine_name: "build-box".into(),
            capabilities: WorkerCapabilities {
                platform: "linux".into(),
                architecture: "x86_64".into(),
                max_concurrent_tasks: 1,
                agents: vec![],
                workspaces: vec![],
            },
            labels: vec![],
            registration_nonce: "nonce".into(),
        };
        registration.worker_id = "worker/1".into();
        assert_eq!(validate_registration(&registration), Err("worker_identity_invalid"));
    }

    #[test]
    fn event_cursor_rejects_duplicates_gaps_and_invalid_envelopes() {
        let payload = WorkerHeartbeat {
            worker_id: "worker-1".into(),
            status: WorkerStatus::Online,
            running_task_ids: vec![],
            load_percent: 0,
            observed_at_ms: 1,
        };
        let event = |sequence| WorkerEnvelope {
            protocol_version: REMOTE_WORKER_PROTOCOL_VERSION,
            worker_id: "worker-1".into(),
            message_id: format!("message-{sequence}"),
            sequence,
            sent_at_ms: 1,
            kind: WorkerEventKind::Heartbeat,
            payload: payload.clone(),
        };
        let mut cursor = EventCursor::new("worker-1");
        assert!(cursor.accept(&event(1)));
        assert!(!cursor.accept(&event(1)));
        assert!(!cursor.accept(&event(3)));
        assert!(cursor.accept(&event(2)));
        assert_eq!(cursor.next_sequence(), 3);
    }

    #[test]
    fn envelope_validation_rejects_bad_metadata_before_cursor_mutation() {
        let payload = WorkerHeartbeat {
            worker_id: "worker-1".into(),
            status: WorkerStatus::Online,
            running_task_ids: vec![],
            load_percent: 0,
            observed_at_ms: 1,
        };
        let mut cursor = EventCursor::new("worker-1");
        let mut event = WorkerEnvelope {
            protocol_version: REMOTE_WORKER_PROTOCOL_VERSION,
            worker_id: "worker-1".into(),
            message_id: "message-1".into(),
            sequence: 1,
            sent_at_ms: 1,
            kind: WorkerEventKind::Heartbeat,
            payload,
        };
        event.protocol_version = REMOTE_WORKER_PROTOCOL_VERSION + 1;
        assert_eq!(validate_envelope(&event), Err("protocol_incompatible"));
        assert!(!cursor.accept(&event));
        assert_eq!(cursor.next_sequence(), 1);
        event.protocol_version = REMOTE_WORKER_PROTOCOL_VERSION;
        event.message_id.clear();
        assert_eq!(validate_envelope(&event), Err("message_identity_invalid"));
        event.message_id = "message-1".into();
        event.sequence = 0;
        assert_eq!(validate_envelope(&event), Err("event_sequence_invalid"));
    }

    #[test]
    fn registration_validation_rejects_duplicate_and_unbounded_capabilities() {
        let mut registration = WorkerRegistration {
            protocol_version: REMOTE_WORKER_PROTOCOL_VERSION,
            worker_id: "worker-1".into(),
            machine_name: "build-box".into(),
            capabilities: capabilities(),
            labels: vec!["gpu".into(), "gpu".into()],
            registration_nonce: "nonce".into(),
        };
        assert_eq!(validate_registration(&registration), Err("label_invalid"));
        registration.labels = vec!["x".repeat(MAX_LABEL_LEN + 1)];
        assert_eq!(validate_registration(&registration), Err("label_invalid"));
        registration.labels.clear();
        registration.capabilities.agents[0].kind = "codex_cli".into();
        let duplicate_agent = registration.capabilities.agents[0].clone();
        registration.capabilities.agents.push(duplicate_agent);
        assert_eq!(validate_registration(&registration), Err("agent_capability_invalid"));
    }
}
