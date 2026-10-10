//! Durable Host-side queue and terminal observation store for remote Worker tasks.
//!
//! The M3 API keeps a bounded in-memory queue for transport tests.  This
//! module is the small persistence seam: task intent, leases, event cursor
//! and terminal observations are written in SQLite before a response is
//! acknowledged.  It intentionally does not start a process or resolve a
//! machine-local path.

use std::fmt;

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::remote_worker::{
    validate_envelope, validate_task, RemoteTaskPermission, RemoteTaskRequest, WorkerEnvelope,
    WorkerEventKind,
};

pub const MAX_LEASE_MS: u64 = 86_400_000;
pub const MAX_EVENT_PAYLOAD_BYTES: usize = 1_048_576;
const MAX_SQLITE_MS: u64 = i64::MAX as u64;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS remote_worker_task (
    task_id TEXT PRIMARY KEY,
    worker_id TEXT NOT NULL,
    agent_kind TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    prompt TEXT NOT NULL,
    permission TEXT NOT NULL CHECK(permission IN ('read_only','workspace_write','elevated')),
    timeout_seconds INTEGER NOT NULL,
    attempt INTEGER NOT NULL,
    state TEXT NOT NULL CHECK(state IN (
        'queued','leased','accepted','started','cancel_requested',
        'completed','failed','cancelled','lost')),
    lease_token TEXT,
    execution_epoch INTEGER NOT NULL DEFAULT 0,
    lease_expires_at_ms INTEGER,
    last_event_sequence INTEGER NOT NULL DEFAULT 0,
    cancel_requested_at_ms INTEGER,
    terminal_payload_json TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS remote_worker_task_claim_idx
    ON remote_worker_task(worker_id, state, created_at_ms, task_id);
CREATE TABLE IF NOT EXISTS remote_worker_event_cursor (
    worker_id TEXT PRIMARY KEY,
    next_sequence INTEGER NOT NULL CHECK(next_sequence > 0)
);
CREATE TABLE IF NOT EXISTS remote_worker_event_dedupe (
    worker_id TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    message_id TEXT NOT NULL,
    task_id TEXT,
    event_kind TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    sent_at_ms INTEGER NOT NULL,
    received_at_ms INTEGER NOT NULL,
    PRIMARY KEY(worker_id, sequence),
    UNIQUE(worker_id, message_id)
);
"#;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteTaskState {
    Queued,
    Leased,
    Accepted,
    Started,
    CancelRequested,
    Completed,
    Failed,
    Cancelled,
    Lost,
}

impl RemoteTaskState {
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Lost
        )
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Leased => "leased",
            Self::Accepted => "accepted",
            Self::Started => "started",
            Self::CancelRequested => "cancel_requested",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Lost => "lost",
        }
    }
}

fn state_from_str(value: &str) -> Result<RemoteTaskState, QueueError> {
    match value {
        "queued" => Ok(RemoteTaskState::Queued),
        "leased" => Ok(RemoteTaskState::Leased),
        "accepted" => Ok(RemoteTaskState::Accepted),
        "started" => Ok(RemoteTaskState::Started),
        "cancel_requested" => Ok(RemoteTaskState::CancelRequested),
        "completed" => Ok(RemoteTaskState::Completed),
        "failed" => Ok(RemoteTaskState::Failed),
        "cancelled" => Ok(RemoteTaskState::Cancelled),
        "lost" => Ok(RemoteTaskState::Lost),
        _ => Err(QueueError::InvalidState),
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteTaskRecord {
    pub task_id: String,
    pub worker_id: String,
    pub agent_kind: String,
    pub workspace_id: String,
    pub prompt: String,
    pub permission: RemoteTaskPermission,
    pub timeout_seconds: u32,
    pub attempt: u32,
    pub state: RemoteTaskState,
    pub lease_token: Option<String>,
    /// Monotonically increasing claim fence. Worker events must echo it.
    pub execution_epoch: u64,
    pub lease_expires_at_ms: Option<u64>,
    pub last_event_sequence: u64,
    pub cancel_requested_at_ms: Option<u64>,
    pub terminal_payload: Option<Value>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteTaskLease {
    pub task: RemoteTaskRecord,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnqueueOutcome {
    Enqueued,
    AlreadyPresent,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EventProjectionOutcome {
    Applied {
        task_id: Option<String>,
        state: Option<RemoteTaskState>,
    },
    Duplicate,
    Ignored,
}

#[derive(Debug)]
pub enum QueueError {
    Sqlite(rusqlite::Error),
    InvalidTask(&'static str),
    InvalidEvent(&'static str),
    InvalidState,
    TaskNotFound,
    LeaseNotFound,
    LeaseExpired,
    LeaseOwnerMismatch,
    EventOutOfOrder,
    EventTaskMismatch,
    EventFenceMismatch,
    EventConflict,
}

impl fmt::Display for QueueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => write!(f, "sqlite:{error}"),
            Self::InvalidTask(reason) => write!(f, "invalid_task:{reason}"),
            Self::InvalidEvent(reason) => write!(f, "invalid_event:{reason}"),
            Self::InvalidState => f.write_str("invalid_state"),
            Self::TaskNotFound => f.write_str("task_not_found"),
            Self::LeaseNotFound => f.write_str("lease_not_found"),
            Self::LeaseExpired => f.write_str("lease_expired"),
            Self::LeaseOwnerMismatch => f.write_str("lease_owner_mismatch"),
            Self::EventOutOfOrder => f.write_str("event_out_of_order"),
            Self::EventTaskMismatch => f.write_str("event_task_mismatch"),
            Self::EventFenceMismatch => f.write_str("event_fence_mismatch"),
            Self::EventConflict => f.write_str("event_conflict"),
        }
    }
}

impl std::error::Error for QueueError {}

impl From<rusqlite::Error> for QueueError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Sqlite(value)
    }
}

/// SQLite repository for remote Worker tasks. All mutating methods are
/// transactional and safe to call again after a lost HTTP response.
pub struct RemoteTaskQueue;

impl RemoteTaskQueue {
    pub fn ensure_schema(connection: &mut Connection) -> Result<(), QueueError> {
        // This is a queue-owned schema seam. Core must enroll it in its normal
        // migration transaction before exposing remote-worker APIs; it is not
        // permission to create or mutate domain tables. The immediate
        // transaction also makes the restart/upgrade check race-free.
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(SCHEMA)?;
        let has_epoch: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('remote_worker_task') WHERE name='execution_epoch')",
            [], |row| row.get(0),
        )?;
        if !has_epoch {
            tx.execute(
                "ALTER TABLE remote_worker_task ADD COLUMN execution_epoch INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn enqueue(
        connection: &mut Connection,
        request: &RemoteTaskRequest,
        now_ms: u64,
    ) -> Result<EnqueueOutcome, QueueError> {
        validate_task(request).map_err(QueueError::InvalidTask)?;
        if now_ms == 0 || now_ms > MAX_SQLITE_MS {
            return Err(QueueError::InvalidTask("timestamp_required"));
        }
        Self::ensure_schema(connection)?;
        let permission = permission_str(&request.permission);
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: Option<(String, String, String, String, String, i64, i64, i64)> = tx
            .query_row(
                "SELECT worker_id, agent_kind, workspace_id, prompt, permission,
                        timeout_seconds, attempt, created_at_ms
                 FROM remote_worker_task WHERE task_id=?1",
                [&request.task_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )
            .optional()?;
        if let Some((worker, agent, workspace, prompt, existing_permission, timeout, attempt, _)) =
            existing
        {
            let same = worker == request.worker_id
                && agent == request.agent_kind
                && workspace == request.workspace_id
                && prompt == request.prompt
                && existing_permission == permission
                && timeout == i64::from(request.timeout_seconds)
                && attempt == i64::from(request.attempt);
            if !same {
                return Err(QueueError::InvalidTask("task_id_reused"));
            }
            tx.commit()?;
            return Ok(EnqueueOutcome::AlreadyPresent);
        }
        tx.execute(
            "INSERT INTO remote_worker_task(
                task_id, worker_id, agent_kind, workspace_id, prompt, permission,
                timeout_seconds, attempt, state, created_at_ms, updated_at_ms)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'queued',?9,?9)",
            params![
                request.task_id,
                request.worker_id,
                request.agent_kind,
                request.workspace_id,
                request.prompt,
                permission,
                request.timeout_seconds,
                request.attempt,
                now_ms as i64,
            ],
        )?;
        tx.commit()?;
        Ok(EnqueueOutcome::Enqueued)
    }

    /// Atomically claim the oldest queued task for a Worker. An expired lease
    /// is fenced as `lost`; disconnect is never interpreted as success or as
    /// permission to replay unknown work.
    pub fn claim(
        connection: &mut Connection,
        worker_id: &str,
        now_ms: u64,
        lease_ms: u64,
    ) -> Result<Option<RemoteTaskLease>, QueueError> {
        if worker_id.trim().is_empty() {
            return Err(QueueError::InvalidTask("worker_identity_required"));
        }
        if now_ms == 0
            || now_ms > MAX_SQLITE_MS
            || lease_ms == 0
            || lease_ms > MAX_LEASE_MS
            || lease_ms > MAX_SQLITE_MS - now_ms
        {
            return Err(QueueError::InvalidTask("lease_invalid"));
        }
        Self::ensure_schema(connection)?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE remote_worker_task
             SET state='lost', lease_token=NULL, lease_expires_at_ms=NULL,
                 terminal_payload_json=COALESCE(terminal_payload_json, '{\"reason\":\"lease_expired\"}'),
                 updated_at_ms=?1
             WHERE state='leased' AND lease_expires_at_ms IS NOT NULL AND lease_expires_at_ms <= ?1",
            [now_ms as i64],
        )?;
        let task_id: Option<String> = tx
            .query_row(
                "SELECT task_id FROM remote_worker_task
                 WHERE worker_id=?1 AND state='queued'
                 ORDER BY created_at_ms, task_id LIMIT 1",
                [worker_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(task_id) = task_id else {
            tx.commit()?;
            return Ok(None);
        };
        let token = Uuid::new_v4().to_string();
        let expires = now_ms.saturating_add(lease_ms);
        tx.execute(
            "UPDATE remote_worker_task SET state='leased', lease_token=?2,
                execution_epoch=execution_epoch+1, lease_expires_at_ms=?3, updated_at_ms=?4
             WHERE task_id=?1 AND state='queued'",
            params![task_id, token, expires as i64, now_ms as i64],
        )?;
        let task = load_task(&tx, &task_id)?.ok_or(QueueError::TaskNotFound)?;
        tx.commit()?;
        Ok(Some(RemoteTaskLease { task }))
    }

    pub fn acknowledge(
        connection: &mut Connection,
        task_id: &str,
        lease_token: &str,
        accepted: bool,
        now_ms: u64,
    ) -> Result<RemoteTaskRecord, QueueError> {
        if now_ms == 0 || now_ms > MAX_SQLITE_MS {
            return Err(QueueError::InvalidTask("timestamp_required"));
        }
        Self::ensure_schema(connection)?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task = load_task(&tx, task_id)?.ok_or(QueueError::TaskNotFound)?;
        match task.state {
            RemoteTaskState::Leased | RemoteTaskState::Accepted => {
                if task.lease_token.as_deref() != Some(lease_token) {
                    return Err(QueueError::LeaseOwnerMismatch);
                }
                if task.state == RemoteTaskState::Leased {
                    if task
                        .lease_expires_at_ms
                        .is_some_and(|expires| expires <= now_ms)
                    {
                        return Err(QueueError::LeaseExpired);
                    }
                    let state = if accepted {
                        RemoteTaskState::Accepted
                    } else {
                        RemoteTaskState::Queued
                    };
                    tx.execute(
                        "UPDATE remote_worker_task SET state=?2, lease_token=CASE WHEN ?3 THEN lease_token ELSE NULL END,
                            lease_expires_at_ms=CASE WHEN ?3 THEN lease_expires_at_ms ELSE NULL END,
                            updated_at_ms=?4 WHERE task_id=?1",
                        params![task_id, state.as_str(), accepted, now_ms as i64],
                    )?;
                } else if !accepted {
                    return Err(QueueError::InvalidState);
                }
            }
            _ => return Err(QueueError::InvalidState),
        }
        let result = load_task(&tx, task_id)?.ok_or(QueueError::TaskNotFound)?;
        tx.commit()?;
        Ok(result)
    }

    pub fn request_cancel(
        connection: &mut Connection,
        task_id: &str,
        now_ms: u64,
    ) -> Result<RemoteTaskRecord, QueueError> {
        if now_ms == 0 || now_ms > MAX_SQLITE_MS {
            return Err(QueueError::InvalidTask("timestamp_required"));
        }
        Self::ensure_schema(connection)?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task = load_task(&tx, task_id)?.ok_or(QueueError::TaskNotFound)?;
        if task.state.is_terminal() {
            tx.commit()?;
            return Ok(task);
        }
        tx.execute(
            "UPDATE remote_worker_task SET state='cancel_requested',
                cancel_requested_at_ms=?2, lease_expires_at_ms=NULL,
                updated_at_ms=?2 WHERE task_id=?1",
            params![task_id, now_ms as i64],
        )?;
        let result = load_task(&tx, task_id)?.ok_or(QueueError::TaskNotFound)?;
        tx.commit()?;
        Ok(result)
    }

    /// Apply an ordered Worker event and project queue state. Transport
    /// authentication is owned by the caller; this method only validates the
    /// envelope and the task execution fence. Duplicate `(worker_id, sequence)`
    /// events are harmless.
    pub fn project_event(
        connection: &mut Connection,
        event: &WorkerEnvelope<Value>,
        received_at_ms: u64,
    ) -> Result<EventProjectionOutcome, QueueError> {
        validate_envelope(event).map_err(QueueError::InvalidEvent)?;
        if received_at_ms == 0 || received_at_ms > MAX_SQLITE_MS || event.sent_at_ms > MAX_SQLITE_MS
        {
            return Err(QueueError::InvalidEvent("timestamp_required"));
        }
        let payload_json = serde_json::to_string(&event.payload)
            .map_err(|_| QueueError::InvalidEvent("payload_invalid"))?;
        let event_kind_json = serde_json::to_string(&event.kind)
            .map_err(|_| QueueError::InvalidEvent("kind_invalid"))?;
        if payload_json.len() > MAX_EVENT_PAYLOAD_BYTES {
            return Err(QueueError::InvalidEvent("payload_too_large"));
        }
        Self::ensure_schema(connection)?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let prior: Option<(String, String, String)> = tx
            .query_row(
                "SELECT message_id, event_kind, payload_json FROM remote_worker_event_dedupe
             WHERE worker_id=?1 AND sequence=?2",
                params![event.worker_id, event.sequence as i64],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        if let Some((message_id, event_kind, prior_payload)) = prior {
            let kind = serde_json::to_string(&event.kind)
                .map_err(|_| QueueError::InvalidEvent("kind_invalid"))?;
            if message_id != event.message_id || event_kind != kind || prior_payload != payload_json
            {
                return Err(QueueError::EventConflict);
            }
            tx.commit()?;
            return Ok(EventProjectionOutcome::Duplicate);
        }
        let message_duplicate: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM remote_worker_event_dedupe WHERE worker_id=?1 AND message_id=?2)",
            params![event.worker_id, event.message_id], |row| row.get(0))?;
        if message_duplicate {
            return Err(QueueError::EventConflict);
        }
        let next: Option<u64> = tx
            .query_row(
                "SELECT next_sequence FROM remote_worker_event_cursor WHERE worker_id=?1",
                [&event.worker_id],
                |row| row.get::<_, i64>(0).map(|value| value as u64),
            )
            .optional()?;
        if next.unwrap_or(1) != event.sequence {
            return Err(QueueError::EventOutOfOrder);
        }
        let task_id = event
            .payload
            .get("taskId")
            .or_else(|| event.payload.get("task_id"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let state = event_state(&event.kind);
        let mut fenced_out = false;
        if let Some(task_id) = task_id.as_deref() {
            let task = load_task(&tx, task_id)?.ok_or(QueueError::TaskNotFound)?;
            if task.worker_id != event.worker_id {
                return Err(QueueError::EventTaskMismatch);
            }
            let fence = event_fence(&event.payload)?;
            if task.execution_epoch != fence.execution_epoch
                || task.attempt != fence.attempt
                || task.lease_token.as_deref() != Some(fence.lease_token.as_str())
            {
                // Consume a stale event's global sequence without allowing it
                // to mutate this task. Otherwise one late event would wedge
                // every later event from this authenticated Worker forever.
                fenced_out = true;
            } else if let Some(next_state) = state {
                if !valid_event_transition(task.state, next_state) {
                    return Err(QueueError::InvalidState);
                }
                let terminal_payload = next_state.is_terminal().then_some(payload_json.clone());
                tx.execute(
                    "UPDATE remote_worker_task SET state=?2, last_event_sequence=?3,
                        terminal_payload_json=COALESCE(?4, terminal_payload_json),
                        lease_token=CASE WHEN ?6 THEN NULL ELSE lease_token END,
                        lease_expires_at_ms=NULL, updated_at_ms=?5
                     WHERE task_id=?1",
                    params![
                        task_id,
                        next_state.as_str(),
                        event.sequence as i64,
                        terminal_payload,
                        received_at_ms as i64,
                        next_state.is_terminal()
                    ],
                )?;
            } else {
                tx.execute(
                    "UPDATE remote_worker_task SET last_event_sequence=?2, updated_at_ms=?3
                     WHERE task_id=?1",
                    params![task_id, event.sequence as i64, received_at_ms as i64],
                )?;
            }
        } else if state.is_some() {
            return Err(QueueError::InvalidEvent("task_identity_required"));
        }
        tx.execute(
            "INSERT INTO remote_worker_event_dedupe(
                worker_id, sequence, message_id, task_id, event_kind,
                payload_json, sent_at_ms, received_at_ms)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                event.worker_id,
                event.sequence as i64,
                event.message_id,
                task_id,
                event_kind_json,
                payload_json,
                event.sent_at_ms as i64,
                received_at_ms as i64,
            ],
        )?;
        // The queue cannot authenticate a correlated AgentRun projection yet.
        // Keep the durable worker event here; Core's domain command handler must
        // later consume it through an explicit execution-epoch fence.
        tx.execute(
            "INSERT INTO remote_worker_event_cursor(worker_id,next_sequence) VALUES(?1,?2)
             ON CONFLICT(worker_id) DO UPDATE SET next_sequence=excluded.next_sequence",
            params![event.worker_id, (event.sequence + 1) as i64],
        )?;
        tx.commit()?;
        Ok(if state.is_some() && !fenced_out {
            EventProjectionOutcome::Applied { task_id, state }
        } else {
            EventProjectionOutcome::Ignored
        })
    }

    pub fn get(
        connection: &Connection,
        task_id: &str,
    ) -> Result<Option<RemoteTaskRecord>, QueueError> {
        // Reads are safe on read-only Core connections. A database that has
        // not yet admitted the remote-worker migration simply has no tasks.
        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='remote_worker_task')",
            [],
            |row| row.get(0),
        )?;
        if !exists {
            return Ok(None);
        }
        Ok(load_task(connection, task_id)?)
    }

    pub fn recover_expired(connection: &mut Connection, now_ms: u64) -> Result<usize, QueueError> {
        if now_ms == 0 || now_ms > MAX_SQLITE_MS {
            return Err(QueueError::InvalidTask("timestamp_required"));
        }
        Self::ensure_schema(connection)?;
        Ok(connection.execute(
            "UPDATE remote_worker_task SET state='lost', lease_token=NULL,
                lease_expires_at_ms=NULL,
                terminal_payload_json=COALESCE(terminal_payload_json, '{\"reason\":\"lease_expired\"}'),
                updated_at_ms=?1
             WHERE state='leased' AND lease_expires_at_ms IS NOT NULL AND lease_expires_at_ms <= ?1",
            [now_ms as i64],
        )?)
    }
}

fn permission_str(permission: &RemoteTaskPermission) -> &'static str {
    match permission {
        RemoteTaskPermission::ReadOnly => "read_only",
        RemoteTaskPermission::WorkspaceWrite => "workspace_write",
        RemoteTaskPermission::Elevated => "elevated",
    }
}

fn permission_from_str(value: &str) -> Result<RemoteTaskPermission, QueueError> {
    match value {
        "read_only" => Ok(RemoteTaskPermission::ReadOnly),
        "workspace_write" => Ok(RemoteTaskPermission::WorkspaceWrite),
        "elevated" => Ok(RemoteTaskPermission::Elevated),
        _ => Err(QueueError::InvalidTask("permission_invalid")),
    }
}

fn load_task(
    connection: &Connection,
    task_id: &str,
) -> Result<Option<RemoteTaskRecord>, QueueError> {
    let mut statement = connection.prepare(
        "SELECT task_id, worker_id, agent_kind, workspace_id, prompt, permission,
                timeout_seconds, attempt, state, lease_token, execution_epoch, lease_expires_at_ms,
                last_event_sequence, cancel_requested_at_ms, terminal_payload_json,
                created_at_ms, updated_at_ms
         FROM remote_worker_task WHERE task_id=?1",
    )?;
    statement
        .query_row([task_id], |row| {
            let payload: Option<String> = row.get(14)?;
            Ok(RemoteTaskRecord {
                task_id: row.get(0)?,
                worker_id: row.get(1)?,
                agent_kind: row.get(2)?,
                workspace_id: row.get(3)?,
                prompt: row.get(4)?,
                permission: permission_from_str(&row.get::<_, String>(5)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                timeout_seconds: row.get::<_, i64>(6)? as u32,
                attempt: row.get::<_, i64>(7)? as u32,
                state: state_from_str(&row.get::<_, String>(8)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                lease_token: row.get(9)?,
                execution_epoch: row.get::<_, i64>(10)? as u64,
                lease_expires_at_ms: row.get::<_, Option<i64>>(11)?.map(|value| value as u64),
                last_event_sequence: row.get::<_, i64>(12)? as u64,
                cancel_requested_at_ms: row.get::<_, Option<i64>>(13)?.map(|value| value as u64),
                terminal_payload: payload
                    .map(|value| {
                        serde_json::from_str(&value).map_err(|_| rusqlite::Error::InvalidQuery)
                    })
                    .transpose()?,
                created_at_ms: row.get::<_, i64>(15)? as u64,
                updated_at_ms: row.get::<_, i64>(16)? as u64,
            })
        })
        .optional()
        .map_err(QueueError::from)
}

struct EventFence {
    execution_epoch: u64,
    attempt: u32,
    lease_token: String,
}

fn event_fence(payload: &Value) -> Result<EventFence, QueueError> {
    let execution_epoch = payload
        .get("executionEpoch")
        .or_else(|| payload.get("execution_epoch"))
        .and_then(Value::as_u64)
        .ok_or(QueueError::InvalidEvent("execution_epoch_required"))?;
    let attempt = payload
        .get("attempt")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .ok_or(QueueError::InvalidEvent("attempt_required"))?;
    let lease_token = payload
        .get("leaseToken")
        .or_else(|| payload.get("lease_token"))
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .ok_or(QueueError::InvalidEvent("lease_token_required"))?
        .to_owned();
    Ok(EventFence {
        execution_epoch,
        attempt,
        lease_token,
    })
}

fn event_state(kind: &WorkerEventKind) -> Option<RemoteTaskState> {
    match kind {
        WorkerEventKind::TaskAccepted => Some(RemoteTaskState::Accepted),
        WorkerEventKind::TaskStarted => Some(RemoteTaskState::Started),
        WorkerEventKind::TaskCompleted => Some(RemoteTaskState::Completed),
        WorkerEventKind::TaskFailed => Some(RemoteTaskState::Failed),
        WorkerEventKind::TaskCancelled => Some(RemoteTaskState::Cancelled),
        _ => None,
    }
}

fn valid_event_transition(current: RemoteTaskState, next: RemoteTaskState) -> bool {
    // The transport ACK and the explicit TaskAccepted event can race. Treat a
    // repeated non-terminal observation as idempotent; terminal repeats are
    // fenced out once the terminal event clears the lease token.
    if current == next {
        return true;
    }
    matches!(
        (current, next),
        (RemoteTaskState::Leased, RemoteTaskState::Accepted)
            | (RemoteTaskState::Leased, RemoteTaskState::Started)
            | (RemoteTaskState::Leased, RemoteTaskState::Completed)
            | (RemoteTaskState::Leased, RemoteTaskState::Failed)
            | (RemoteTaskState::Leased, RemoteTaskState::Cancelled)
            | (RemoteTaskState::Accepted, RemoteTaskState::Started)
            | (RemoteTaskState::Accepted, RemoteTaskState::Completed)
            | (RemoteTaskState::Accepted, RemoteTaskState::Failed)
            | (RemoteTaskState::Accepted, RemoteTaskState::Cancelled)
            | (RemoteTaskState::Started, RemoteTaskState::Completed)
            | (RemoteTaskState::Started, RemoteTaskState::Failed)
            | (RemoteTaskState::Started, RemoteTaskState::Cancelled)
            | (RemoteTaskState::CancelRequested, RemoteTaskState::Completed)
            | (RemoteTaskState::CancelRequested, RemoteTaskState::Failed)
            | (RemoteTaskState::CancelRequested, RemoteTaskState::Cancelled)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_worker::{WorkerEventKind, REMOTE_WORKER_PROTOCOL_VERSION};

    fn request(id: &str) -> RemoteTaskRequest {
        RemoteTaskRequest {
            task_id: id.into(),
            worker_id: "worker-1".into(),
            agent_kind: "codex_cli".into(),
            workspace_id: "main".into(),
            prompt: "inspect fixture".into(),
            permission: RemoteTaskPermission::ReadOnly,
            timeout_seconds: 30,
            attempt: 1,
        }
    }

    fn event(
        kind: WorkerEventKind,
        sequence: u64,
        task_id: &str,
        lease: &RemoteTaskLease,
    ) -> WorkerEnvelope<Value> {
        WorkerEnvelope {
            protocol_version: REMOTE_WORKER_PROTOCOL_VERSION,
            worker_id: "worker-1".into(),
            message_id: format!("message-{sequence}"),
            sequence,
            sent_at_ms: 100 + sequence,
            kind,
            payload: serde_json::json!({
                "taskId": task_id,
                "executionEpoch": lease.task.execution_epoch,
                "attempt": lease.task.attempt,
                "leaseToken": lease.task.lease_token.as_deref().unwrap(),
            }),
        }
    }

    #[test]
    fn expired_lease_is_lost_and_not_reclaimed() {
        let mut connection = Connection::open_in_memory().unwrap();
        RemoteTaskQueue::enqueue(&mut connection, &request("task-1"), 10).unwrap();
        let lease = RemoteTaskQueue::claim(&mut connection, "worker-1", 20, 5)
            .unwrap()
            .unwrap();
        assert_eq!(lease.task.execution_epoch, 1);
        assert!(RemoteTaskQueue::claim(&mut connection, "worker-1", 26, 5)
            .unwrap()
            .is_none());
        assert_eq!(
            RemoteTaskQueue::get(&connection, "task-1")
                .unwrap()
                .unwrap()
                .state,
            RemoteTaskState::Lost
        );
    }

    #[test]
    fn legacy_queue_schema_is_upgraded_with_execution_epoch() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE remote_worker_task(
                    task_id TEXT PRIMARY KEY, worker_id TEXT NOT NULL,
                    agent_kind TEXT NOT NULL, workspace_id TEXT NOT NULL,
                    prompt TEXT NOT NULL, permission TEXT NOT NULL,
                    timeout_seconds INTEGER NOT NULL, attempt INTEGER NOT NULL,
                    state TEXT NOT NULL, lease_token TEXT,
                    lease_expires_at_ms INTEGER, last_event_sequence INTEGER NOT NULL DEFAULT 0,
                    cancel_requested_at_ms INTEGER, terminal_payload_json TEXT,
                    created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL
                )",
            )
            .unwrap();
        RemoteTaskQueue::ensure_schema(&mut connection).unwrap();
        let has_epoch: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('remote_worker_task') WHERE name='execution_epoch')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(has_epoch);
    }

    #[test]
    fn queue_reopens_with_lease_fence_and_event_cursor() {
        let path =
            std::env::temp_dir().join(format!("rovai-remote-queue-{}.sqlite", Uuid::new_v4()));
        {
            let mut connection = Connection::open(&path).unwrap();
            RemoteTaskQueue::enqueue(&mut connection, &request("task-1"), 10).unwrap();
            let lease = RemoteTaskQueue::claim(&mut connection, "worker-1", 20, 100)
                .unwrap()
                .unwrap();
            assert_eq!(lease.task.execution_epoch, 1);
            RemoteTaskQueue::acknowledge(
                &mut connection,
                "task-1",
                lease.task.lease_token.as_deref().unwrap(),
                true,
                21,
            )
            .unwrap();
            let event = event(WorkerEventKind::TaskStarted, 1, "task-1", &lease);
            RemoteTaskQueue::project_event(&mut connection, &event, 22).unwrap();
        }
        {
            let connection = Connection::open(&path).unwrap();
            let task = RemoteTaskQueue::get(&connection, "task-1")
                .unwrap()
                .unwrap();
            assert_eq!(task.state, RemoteTaskState::Started);
            assert_eq!(task.execution_epoch, 1);
            assert_eq!(task.last_event_sequence, 1);
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn terminal_projector_is_ordered_fenced_and_idempotent() {
        let mut connection = Connection::open_in_memory().unwrap();
        RemoteTaskQueue::enqueue(&mut connection, &request("task-1"), 10).unwrap();
        let lease = RemoteTaskQueue::claim(&mut connection, "worker-1", 20, 100)
            .unwrap()
            .unwrap();
        RemoteTaskQueue::acknowledge(
            &mut connection,
            "task-1",
            lease.task.lease_token.as_deref().unwrap(),
            true,
            21,
        )
        .unwrap();
        assert!(matches!(
            RemoteTaskQueue::project_event(
                &mut connection,
                &event(WorkerEventKind::TaskStarted, 1, "task-1", &lease),
                22
            )
            .unwrap(),
            EventProjectionOutcome::Applied {
                state: Some(RemoteTaskState::Started),
                ..
            }
        ));
        let completed = event(WorkerEventKind::TaskCompleted, 2, "task-1", &lease);
        assert!(matches!(
            RemoteTaskQueue::project_event(&mut connection, &completed, 23).unwrap(),
            EventProjectionOutcome::Applied {
                state: Some(RemoteTaskState::Completed),
                ..
            }
        ));
        assert_eq!(
            RemoteTaskQueue::project_event(&mut connection, &completed, 24).unwrap(),
            EventProjectionOutcome::Duplicate
        );
        let mut conflicting = completed.clone();
        conflicting.payload["output"] = Value::String("changed".into());
        assert!(matches!(
            RemoteTaskQueue::project_event(&mut connection, &conflicting, 25),
            Err(QueueError::EventConflict)
        ));
    }

    #[test]
    fn stale_fence_cannot_project_after_lease_expiry() {
        let mut connection = Connection::open_in_memory().unwrap();
        RemoteTaskQueue::enqueue(&mut connection, &request("task-1"), 10).unwrap();
        let lease = RemoteTaskQueue::claim(&mut connection, "worker-1", 20, 5)
            .unwrap()
            .unwrap();
        RemoteTaskQueue::recover_expired(&mut connection, 26).unwrap();
        let event = event(WorkerEventKind::TaskCompleted, 1, "task-1", &lease);
        assert_eq!(
            RemoteTaskQueue::project_event(&mut connection, &event, 27).unwrap(),
            EventProjectionOutcome::Ignored
        );
        assert_eq!(
            RemoteTaskQueue::get(&connection, "task-1")
                .unwrap()
                .unwrap()
                .state,
            RemoteTaskState::Lost
        );
        // The stale event was consumed at the Worker cursor, so a later
        // heartbeat is not rejected as an artificial sequence gap.
        let heartbeat = WorkerEnvelope {
            protocol_version: REMOTE_WORKER_PROTOCOL_VERSION,
            worker_id: "worker-1".into(),
            message_id: "message-2".into(),
            sequence: 2,
            sent_at_ms: 102,
            kind: WorkerEventKind::Heartbeat,
            payload: serde_json::json!({}),
        };
        assert_eq!(
            RemoteTaskQueue::project_event(&mut connection, &heartbeat, 28).unwrap(),
            EventProjectionOutcome::Ignored
        );
    }

    #[test]
    fn queue_never_writes_agent_run_projection() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute(
                "CREATE TABLE agent_run(id TEXT PRIMARY KEY, status TEXT NOT NULL, ended_at TEXT, updated_at TEXT)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO agent_run(id,status) VALUES('task-1','running')",
                [],
            )
            .unwrap();
        RemoteTaskQueue::enqueue(&mut connection, &request("task-1"), 10).unwrap();
        let lease = RemoteTaskQueue::claim(&mut connection, "worker-1", 20, 100)
            .unwrap()
            .unwrap();
        RemoteTaskQueue::acknowledge(
            &mut connection,
            "task-1",
            lease.task.lease_token.as_deref().unwrap(),
            true,
            21,
        )
        .unwrap();
        let event = event(WorkerEventKind::TaskCompleted, 1, "task-1", &lease);
        RemoteTaskQueue::project_event(&mut connection, &event, 22).unwrap();
        let status: String = connection
            .query_row(
                "SELECT status FROM agent_run WHERE id='task-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "running");
    }
}
