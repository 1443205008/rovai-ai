//! Runtime dispatch seam for remote Worker tasks.
//!
//! M4 deliberately stops at an in-process, read-only fixture.  A real
//! Runtime adapter can implement [`RemoteWorkerRuntimeAdapter`] later, after
//! local process admission, workspace roots, and approval policy are wired to
//! the existing Runtime Fleet.  No method in this module executes shell text
//! or accepts an arbitrary filesystem path.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::remote_worker::{RemoteTaskPermission, RemoteTaskRequest};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteTaskRuntimeStatus {
    Running,
    Completed,
    Cancelled,
    Disconnected,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeDispatchReceipt {
    pub task_id: String,
    pub worker_id: String,
    pub status: RemoteTaskRuntimeStatus,
    pub fixture: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuntimeDispatchError {
    PermissionNotAdmitted,
    TaskAlreadyRunning,
    TaskNotFound,
    TaskNotCancellable,
}

impl std::fmt::Display for RuntimeDispatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PermissionNotAdmitted => f.write_str("permission_not_admitted"),
            Self::TaskAlreadyRunning => f.write_str("task_already_running"),
            Self::TaskNotFound => f.write_str("task_not_found"),
            Self::TaskNotCancellable => f.write_str("task_not_cancellable"),
        }
    }
}

impl std::error::Error for RuntimeDispatchError {}

/// Minimal boundary that a real remote Runtime adapter must implement.
pub trait RemoteWorkerRuntimeAdapter {
    fn dispatch(
        &mut self,
        task: &RemoteTaskRequest,
    ) -> Result<RuntimeDispatchReceipt, RuntimeDispatchError>;

    fn cancel(&mut self, task_id: &str) -> Result<RuntimeDispatchReceipt, RuntimeDispatchError>;

    /// Mark tasks interrupted by a lost Worker connection.  The returned
    /// receipts are evidence for a caller to requeue or audit; this trait does
    /// not claim that a disconnected process completed.
    fn disconnect(&mut self) -> Vec<RuntimeDispatchReceipt>;
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct FixtureTask {
    task: RemoteTaskRequest,
    status: RemoteTaskRuntimeStatus,
}

/// A deterministic read-only adapter for transport and recovery tests.  It
/// stores task metadata only and never starts a local process.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReadOnlyFixtureRuntime {
    tasks: BTreeMap<String, FixtureTask>,
}

impl ReadOnlyFixtureRuntime {
    pub fn status(&self, task_id: &str) -> Option<RemoteTaskRuntimeStatus> {
        self.tasks.get(task_id).map(|task| task.status)
    }
}

impl RemoteWorkerRuntimeAdapter for ReadOnlyFixtureRuntime {
    fn dispatch(
        &mut self,
        task: &RemoteTaskRequest,
    ) -> Result<RuntimeDispatchReceipt, RuntimeDispatchError> {
        if !matches!(task.permission, RemoteTaskPermission::ReadOnly) {
            return Err(RuntimeDispatchError::PermissionNotAdmitted);
        }
        if self.tasks.contains_key(&task.task_id) {
            return Err(RuntimeDispatchError::TaskAlreadyRunning);
        }
        self.tasks.insert(
            task.task_id.clone(),
            FixtureTask {
                task: task.clone(),
                status: RemoteTaskRuntimeStatus::Running,
            },
        );
        Ok(RuntimeDispatchReceipt {
            task_id: task.task_id.clone(),
            worker_id: task.worker_id.clone(),
            status: RemoteTaskRuntimeStatus::Running,
            fixture: true,
        })
    }

    fn cancel(&mut self, task_id: &str) -> Result<RuntimeDispatchReceipt, RuntimeDispatchError> {
        let task = self
            .tasks
            .get_mut(task_id)
            .ok_or(RuntimeDispatchError::TaskNotFound)?;
        if !matches!(task.status, RemoteTaskRuntimeStatus::Running) {
            return Err(RuntimeDispatchError::TaskNotCancellable);
        }
        task.status = RemoteTaskRuntimeStatus::Cancelled;
        Ok(RuntimeDispatchReceipt {
            task_id: task.task.task_id.clone(),
            worker_id: task.task.worker_id.clone(),
            status: task.status,
            fixture: true,
        })
    }

    fn disconnect(&mut self) -> Vec<RuntimeDispatchReceipt> {
        self.tasks
            .values_mut()
            .filter_map(|task| {
                if !matches!(task.status, RemoteTaskRuntimeStatus::Running) {
                    return None;
                }
                task.status = RemoteTaskRuntimeStatus::Disconnected;
                Some(RuntimeDispatchReceipt {
                    task_id: task.task.task_id.clone(),
                    worker_id: task.task.worker_id.clone(),
                    status: task.status,
                    fixture: true,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_worker::{REMOTE_WORKER_PROTOCOL_VERSION, RemoteTaskPermission};

    fn task(permission: RemoteTaskPermission) -> RemoteTaskRequest {
        RemoteTaskRequest {
            task_id: "fixture-task".into(),
            worker_id: "worker-1".into(),
            agent_kind: "codex_cli".into(),
            workspace_id: "main".into(),
            prompt: "read-only fixture".into(),
            permission,
            timeout_seconds: 30,
            attempt: 1,
        }
    }

    #[test]
    fn fixture_never_admits_writes_or_shell_like_execution() {
        let mut runtime = ReadOnlyFixtureRuntime::default();
        assert_eq!(
            runtime.dispatch(&task(RemoteTaskPermission::WorkspaceWrite)),
            Err(RuntimeDispatchError::PermissionNotAdmitted)
        );
        let receipt = runtime
            .dispatch(&task(RemoteTaskPermission::ReadOnly))
            .unwrap();
        assert_eq!(receipt.status, RemoteTaskRuntimeStatus::Running);
        assert_eq!(
            runtime.status("fixture-task"),
            Some(RemoteTaskRuntimeStatus::Running)
        );
    }

    #[test]
    fn cancel_and_disconnect_are_explicit_non_completion_states() {
        let mut runtime = ReadOnlyFixtureRuntime::default();
        runtime
            .dispatch(&task(RemoteTaskPermission::ReadOnly))
            .unwrap();
        let cancelled = runtime.cancel("fixture-task").unwrap();
        assert_eq!(cancelled.status, RemoteTaskRuntimeStatus::Cancelled);

        let mut runtime = ReadOnlyFixtureRuntime::default();
        runtime
            .dispatch(&task(RemoteTaskPermission::ReadOnly))
            .unwrap();
        let disconnected = runtime.disconnect();
        assert_eq!(
            disconnected[0].status,
            RemoteTaskRuntimeStatus::Disconnected
        );
        assert_eq!(REMOTE_WORKER_PROTOCOL_VERSION, 1);
    }
}
