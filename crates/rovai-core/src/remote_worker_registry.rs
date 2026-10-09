//! Host-side state for the remote-machine Worker registration protocol.
//!
//! The wire types in [`crate::remote_worker`] are intentionally transport
//! agnostic.  This module is the small stateful seam used by a web handler (or
//! another Host transport) to apply registration and heartbeat messages.  It
//! does not persist credentials or spawn processes; callers persist the
//! snapshot and perform authentication before invoking the mutating methods.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::remote_worker::{
    validate_heartbeat, validate_registration, validate_task as validate_task_request,
    RemoteTaskPermission, RemoteTaskRequest, WorkerHeartbeat, WorkerRegistration, WorkerStatus,
    REMOTE_WORKER_PROTOCOL_VERSION,
};

/// The default period used by the web API when asking a Worker to heartbeat.
/// A caller can override this per deployment; it is part of the response so a
/// Worker does not need to guess the server's liveness policy.
pub const DEFAULT_HEARTBEAT_INTERVAL_MS: u64 = 15_000;

/// Workers are considered offline after this many milliseconds without a
/// heartbeat unless a caller supplies a different timeout to
/// [`WorkerRegistry::expire_stale`].
pub const DEFAULT_HEARTBEAT_TIMEOUT_MS: u64 = 60_000;

/// Stable paths shared by the Host web layer and a Worker client.  Keeping the
/// paths in Core prevents the HTTP adapter and a client from drifting apart.
pub const WORKER_REGISTRATION_ENDPOINT: &str = "/v1/workers/register";
pub const WORKER_HEARTBEAT_ENDPOINT_PREFIX: &str = "/v1/workers";
pub const WORKER_PAIRING_ENDPOINT: &str = "/v1/workers/pair";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerRecord {
    pub registration: WorkerRegistration,
    pub status: WorkerStatus,
    pub running_task_ids: Vec<String>,
    pub load_percent: u8,
    pub registered_at_ms: u64,
    pub last_heartbeat_at_ms: u64,
    pub heartbeat_count: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerRegistrySnapshot {
    pub protocol_version: u32,
    pub workers: Vec<WorkerRecord>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkerRegistrationOutcome {
    /// The registration created a new record.
    Registered,
    /// The exact same registration was retried and was accepted idempotently.
    AlreadyRegistered,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkerRegistryError {
    InvalidRegistration(&'static str),
    InvalidHeartbeat(&'static str),
    WorkerAlreadyRegistered,
    RegistrationNonceMismatch,
    WorkerNotFound,
    WorkerIdMismatch,
    HeartbeatOutOfOrder,
    InvalidTask(&'static str),
    UnknownAgent,
    UnknownWorkspace,
    WorkspaceReadOnly,
}

impl std::fmt::Display for WorkerRegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRegistration(reason) => write!(f, "invalid_registration:{reason}"),
            Self::InvalidHeartbeat(reason) => write!(f, "invalid_heartbeat:{reason}"),
            Self::WorkerAlreadyRegistered => f.write_str("worker_already_registered"),
            Self::RegistrationNonceMismatch => f.write_str("registration_nonce_mismatch"),
            Self::WorkerNotFound => f.write_str("worker_not_found"),
            Self::WorkerIdMismatch => f.write_str("worker_id_mismatch"),
            Self::HeartbeatOutOfOrder => f.write_str("heartbeat_out_of_order"),
            Self::InvalidTask(reason) => write!(f, "invalid_task:{reason}"),
            Self::UnknownAgent => f.write_str("unknown_agent"),
            Self::UnknownWorkspace => f.write_str("unknown_workspace"),
            Self::WorkspaceReadOnly => f.write_str("workspace_read_only"),
        }
    }
}

impl std::error::Error for WorkerRegistryError {}

/// In-memory projection of the Host's machine registry.
///
/// The struct is deliberately not internally synchronized: the web layer can
/// put it behind its existing database transaction or async mutex, avoiding a
/// second lock with a different lifecycle.  `snapshot`/`from_snapshot` make
/// persistence explicit and deterministic.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WorkerRegistry {
    workers: BTreeMap<String, WorkerRecord>,
}

impl WorkerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_snapshot(snapshot: WorkerRegistrySnapshot) -> Result<Self, WorkerRegistryError> {
        if snapshot.protocol_version != REMOTE_WORKER_PROTOCOL_VERSION {
            return Err(WorkerRegistryError::InvalidRegistration(
                "protocol_incompatible",
            ));
        }

        let mut workers = BTreeMap::new();
        for record in snapshot.workers {
            validate_registration(&record.registration)
                .map_err(WorkerRegistryError::InvalidRegistration)?;
            if record.registered_at_ms == 0 || record.last_heartbeat_at_ms == 0 {
                return Err(WorkerRegistryError::InvalidRegistration(
                    "registry_timestamp_required",
                ));
            }
            if record.last_heartbeat_at_ms < record.registered_at_ms {
                return Err(WorkerRegistryError::InvalidRegistration(
                    "registry_timestamp_out_of_order",
                ));
            }
            validate_heartbeat_for_record(
                &record.registration.worker_id,
                &record.registration,
                &WorkerHeartbeat {
                    worker_id: record.registration.worker_id.clone(),
                    status: record.status.clone(),
                    running_task_ids: record.running_task_ids.clone(),
                    load_percent: record.load_percent,
                    observed_at_ms: record.last_heartbeat_at_ms,
                },
            )
            .map_err(WorkerRegistryError::InvalidHeartbeat)?;
            if workers
                .insert(record.registration.worker_id.clone(), record)
                .is_some()
            {
                return Err(WorkerRegistryError::InvalidRegistration(
                    "duplicate_worker_id",
                ));
            }
        }
        Ok(Self { workers })
    }

    pub fn snapshot(&self) -> WorkerRegistrySnapshot {
        WorkerRegistrySnapshot {
            protocol_version: REMOTE_WORKER_PROTOCOL_VERSION,
            workers: self.workers.values().cloned().collect(),
        }
    }

    pub fn get(&self, worker_id: &str) -> Option<&WorkerRecord> {
        self.workers.get(worker_id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &WorkerRecord> {
        self.workers.values()
    }

    pub fn len(&self) -> usize {
        self.workers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.workers.is_empty()
    }

    /// Apply a registration from the one-time pairing flow.
    ///
    /// A byte-for-byte retry is safe and idempotent.  Once a worker ID has
    /// been paired, a different nonce cannot replace it; replacement is an
    /// explicit Host action so a stolen pairing request cannot evict a live
    /// worker.
    pub fn register(
        &mut self,
        registration: WorkerRegistration,
        registered_at_ms: u64,
    ) -> Result<WorkerRegistrationOutcome, WorkerRegistryError> {
        validate_registration(&registration).map_err(WorkerRegistryError::InvalidRegistration)?;
        if registered_at_ms == 0 {
            return Err(WorkerRegistryError::InvalidRegistration(
                "registration_timestamp_required",
            ));
        }

        if let Some(existing) = self.workers.get(&registration.worker_id) {
            if existing.registration == registration {
                return Ok(WorkerRegistrationOutcome::AlreadyRegistered);
            }
            if existing.registration.registration_nonce != registration.registration_nonce {
                return Err(WorkerRegistryError::RegistrationNonceMismatch);
            }
            return Err(WorkerRegistryError::WorkerAlreadyRegistered);
        }

        let worker_id = registration.worker_id.clone();
        self.workers.insert(
            worker_id,
            WorkerRecord {
                registration,
                status: WorkerStatus::Online,
                running_task_ids: Vec::new(),
                load_percent: 0,
                registered_at_ms,
                last_heartbeat_at_ms: registered_at_ms,
                heartbeat_count: 0,
            },
        );
        Ok(WorkerRegistrationOutcome::Registered)
    }

    /// Apply a heartbeat after the web layer has authenticated the Worker.
    pub fn heartbeat(
        &mut self,
        worker_id: &str,
        heartbeat: WorkerHeartbeat,
    ) -> Result<WorkerHeartbeatAck, WorkerRegistryError> {
        if heartbeat.worker_id != worker_id {
            return Err(WorkerRegistryError::WorkerIdMismatch);
        }
        let record = self
            .workers
            .get_mut(worker_id)
            .ok_or(WorkerRegistryError::WorkerNotFound)?;
        validate_heartbeat_for_record(worker_id, &record.registration, &heartbeat)
            .map_err(WorkerRegistryError::InvalidHeartbeat)?;
        if heartbeat.observed_at_ms < record.last_heartbeat_at_ms {
            return Err(WorkerRegistryError::HeartbeatOutOfOrder);
        }

        record.status = heartbeat.status;
        record.running_task_ids = heartbeat.running_task_ids;
        record.load_percent = heartbeat.load_percent;
        record.last_heartbeat_at_ms = heartbeat.observed_at_ms;
        record.heartbeat_count = record.heartbeat_count.saturating_add(1);

        Ok(WorkerHeartbeatAck {
            worker_id: worker_id.to_owned(),
            status: record.status.clone(),
            server_time_ms: record.last_heartbeat_at_ms,
            heartbeat_count: record.heartbeat_count,
            heartbeat_interval_ms: DEFAULT_HEARTBEAT_INTERVAL_MS,
        })
    }

    /// Mark workers with no recent heartbeat as offline and return their IDs.
    pub fn expire_stale(&mut self, now_ms: u64, timeout_ms: u64) -> Vec<String> {
        if timeout_ms == 0 {
            return Vec::new();
        }
        let mut expired = Vec::new();
        for (worker_id, record) in &mut self.workers {
            let age = now_ms.saturating_sub(record.last_heartbeat_at_ms);
            if age >= timeout_ms && record.status != WorkerStatus::Offline {
                record.status = WorkerStatus::Offline;
                expired.push(worker_id.clone());
            }
        }
        expired
    }

    /// Explicitly remove a worker after revocation/decommissioning.
    pub fn remove(&mut self, worker_id: &str) -> Option<WorkerRecord> {
        self.workers.remove(worker_id)
    }

    /// Validate a task against the capabilities and current lease projection
    /// advertised by its target Worker. The same checks should be repeated by
    /// the durable queue at claim time because capabilities can change after
    /// admission.
    pub fn validate_task(&self, task: &RemoteTaskRequest) -> Result<(), WorkerRegistryError> {
        validate_task_request(task).map_err(WorkerRegistryError::InvalidTask)?;
        let record = self
            .workers
            .get(&task.worker_id)
            .ok_or(WorkerRegistryError::WorkerNotFound)?;
        if !record
            .registration
            .capabilities
            .agents
            .iter()
            .any(|agent| agent.kind == task.agent_kind)
        {
            return Err(WorkerRegistryError::UnknownAgent);
        }
        let workspace = record
            .registration
            .capabilities
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == task.workspace_id)
            .ok_or(WorkerRegistryError::UnknownWorkspace)?;
        if workspace.read_only && !matches!(task.permission, RemoteTaskPermission::ReadOnly) {
            return Err(WorkerRegistryError::WorkspaceReadOnly);
        }
        if record.running_task_ids.len()
            >= record.registration.capabilities.max_concurrent_tasks as usize
        {
            return Err(WorkerRegistryError::InvalidTask("worker_capacity_exceeded"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerRegistrationResponse {
    pub worker_id: String,
    pub accepted: bool,
    pub server_time_ms: u64,
    pub heartbeat_interval_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerHeartbeatAck {
    pub worker_id: String,
    pub status: WorkerStatus,
    pub server_time_ms: u64,
    pub heartbeat_count: u64,
    pub heartbeat_interval_ms: u64,
}

fn validate_heartbeat_for_record(
    worker_id: &str,
    registration: &WorkerRegistration,
    heartbeat: &WorkerHeartbeat,
) -> Result<(), &'static str> {
    validate_heartbeat(heartbeat)?;
    if heartbeat.worker_id != worker_id {
        return Err("worker_id_mismatch");
    }
    if heartbeat.running_task_ids.len() > registration.capabilities.max_concurrent_tasks as usize {
        return Err("worker_capacity_exceeded");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_worker::{AgentCapability, WorkerCapabilities, WorkspaceCapability};

    fn registration(nonce: &str) -> WorkerRegistration {
        WorkerRegistration {
            protocol_version: REMOTE_WORKER_PROTOCOL_VERSION,
            worker_id: "worker-1".into(),
            machine_name: "build-box".into(),
            capabilities: WorkerCapabilities {
                platform: "linux".into(),
                architecture: "x86_64".into(),
                max_concurrent_tasks: 2,
                agents: vec![AgentCapability {
                    kind: "codex_cli".into(),
                    display_name: "Codex CLI".into(),
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
            labels: vec!["gpu".into()],
            registration_nonce: nonce.into(),
        }
    }

    #[test]
    fn registration_is_idempotent_and_nonce_cannot_replace_worker() {
        let mut registry = WorkerRegistry::new();
        let first = registration("nonce-1");
        assert_eq!(
            registry.register(first.clone(), 100),
            Ok(WorkerRegistrationOutcome::Registered)
        );
        assert_eq!(
            registry.register(first, 101),
            Ok(WorkerRegistrationOutcome::AlreadyRegistered)
        );
        assert_eq!(
            registry.register(registration("nonce-2"), 102),
            Err(WorkerRegistryError::RegistrationNonceMismatch)
        );
    }

    #[test]
    fn heartbeat_updates_projection_and_rejects_replay() {
        let mut registry = WorkerRegistry::new();
        registry.register(registration("nonce"), 100).unwrap();
        let heartbeat = WorkerHeartbeat {
            worker_id: "worker-1".into(),
            status: WorkerStatus::Busy,
            running_task_ids: vec!["task-1".into()],
            load_percent: 50,
            observed_at_ms: 200,
        };
        let ack = registry.heartbeat("worker-1", heartbeat.clone()).unwrap();
        assert_eq!(ack.heartbeat_count, 1);
        assert_eq!(registry.get("worker-1").unwrap().status, WorkerStatus::Busy);
        assert_eq!(
            registry.heartbeat("worker-1", heartbeat),
            Ok(WorkerHeartbeatAck {
                worker_id: "worker-1".into(),
                status: WorkerStatus::Busy,
                server_time_ms: 200,
                heartbeat_count: 2,
                heartbeat_interval_ms: DEFAULT_HEARTBEAT_INTERVAL_MS,
            })
        );
        let replay = WorkerHeartbeat {
            worker_id: "worker-1".into(),
            status: WorkerStatus::Busy,
            running_task_ids: vec![],
            load_percent: 0,
            observed_at_ms: 199,
        };
        assert_eq!(
            registry.heartbeat("worker-1", replay),
            Err(WorkerRegistryError::HeartbeatOutOfOrder)
        );
    }

    #[test]
    fn stale_workers_are_marked_offline_and_snapshots_round_trip() {
        let mut registry = WorkerRegistry::new();
        registry.register(registration("nonce"), 100).unwrap();
        assert_eq!(registry.expire_stale(1_000, 100), vec!["worker-1"]);
        assert_eq!(
            registry.get("worker-1").unwrap().status,
            WorkerStatus::Offline
        );
        let snapshot = registry.snapshot();
        let restored = WorkerRegistry::from_snapshot(snapshot).unwrap();
        assert_eq!(restored, registry);
    }

    #[test]
    fn task_admission_checks_worker_capabilities() {
        let mut registry = WorkerRegistry::new();
        registry.register(registration("nonce"), 100).unwrap();
        let task = RemoteTaskRequest {
            task_id: "task-1".into(),
            worker_id: "worker-1".into(),
            agent_kind: "codex_cli".into(),
            workspace_id: "main".into(),
            prompt: "read project".into(),
            permission: RemoteTaskPermission::ReadOnly,
            timeout_seconds: 60,
            attempt: 1,
        };
        registry.validate_task(&task).unwrap();
        let mut unknown_agent = task.clone();
        unknown_agent.agent_kind = "missing".into();
        assert_eq!(
            registry.validate_task(&unknown_agent),
            Err(WorkerRegistryError::UnknownAgent)
        );
        let mut unknown_workspace = task.clone();
        unknown_workspace.workspace_id = "missing".into();
        assert_eq!(
            registry.validate_task(&unknown_workspace),
            Err(WorkerRegistryError::UnknownWorkspace)
        );
        registry
            .heartbeat(
                "worker-1",
                WorkerHeartbeat {
                    worker_id: "worker-1".into(),
                    status: WorkerStatus::Busy,
                    running_task_ids: vec!["running-1".into(), "running-2".into()],
                    load_percent: 100,
                    observed_at_ms: 200,
                },
            )
            .unwrap();
        assert_eq!(
            registry.validate_task(&task),
            Err(WorkerRegistryError::InvalidTask("worker_capacity_exceeded"))
        );
    }
}
