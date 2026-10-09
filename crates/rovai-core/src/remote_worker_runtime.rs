//! Runtime dispatch seam for remote Worker tasks.
//!
//! M4 deliberately stops at an in-process, read-only fixture.  A real
//! Runtime adapter can implement [`RemoteWorkerRuntimeAdapter`] later, after
//! local process admission, workspace roots, and approval policy are wired to
//! the existing Runtime Fleet.  No method in this module executes shell text
//! or accepts an arbitrary filesystem path.
//!
//! The execution state machine in this module is intentionally independent of
//! transport and process supervision.  A Worker connection can be retried or
//! replaced without allowing a transport event to imply completion.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::remote_worker::{RemoteTaskPermission, RemoteTaskRequest};

/// Durable execution states for a task dispatched to a remote Worker.
///
/// `cancel_requested` is an intent, rather than a terminal outcome.  The
/// Worker may still report `completed`, `failed`, or `lost` after that intent
/// is recorded (for example, when cancellation races with a final output).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteTaskExecutionState {
    Queued,
    Leased,
    Running,
    CancelRequested,
    Completed,
    Failed,
    Lost,
}

impl RemoteTaskExecutionState {
    /// Compatibility spelling for the pre-M4 fixture status.  The canonical
    /// wire value is `cancel_requested`; cancellation is not completion.
    #[allow(non_upper_case_globals)]
    pub const Cancelled: Self = Self::CancelRequested;

    /// Compatibility spelling for the pre-M4 fixture status.  The canonical
    /// wire value is `lost`; a disconnect never claims success.
    #[allow(non_upper_case_globals)]
    pub const Disconnected: Self = Self::Lost;

    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Lost)
    }

    pub const fn can_request_cancel(self) -> bool {
        matches!(self, Self::Queued | Self::Leased | Self::Running)
    }

    pub const fn can_mark_lost(self) -> bool {
        matches!(self, Self::Leased | Self::Running | Self::CancelRequested)
    }
}

/// The old name remains available to M3 transport callers while all new
/// values use [`RemoteTaskExecutionState`] and its explicit seven-state
/// vocabulary.
pub type RemoteTaskRuntimeStatus = RemoteTaskExecutionState;

/// A pure input to [`transition_execution_state`].
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteTaskExecutionEvent {
    Lease,
    Start,
    RequestCancel,
    Complete,
    Fail,
    MarkLost,
}

impl RemoteTaskExecutionEvent {
    /// Short compatibility spelling for callers that model cancellation as a
    /// command rather than an event name.
    #[allow(non_upper_case_globals)]
    pub const Cancel: Self = Self::RequestCancel;

    /// A Worker disconnect is represented as an explicit lost observation.
    #[allow(non_upper_case_globals)]
    pub const Disconnect: Self = Self::MarkLost;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RemoteTaskExecutionTransitionError {
    pub state: RemoteTaskExecutionState,
    pub event: RemoteTaskExecutionEvent,
}

impl std::fmt::Display for RemoteTaskExecutionTransitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "invalid remote task transition: {:?} + {:?}",
            self.state, self.event
        )
    }
}

impl std::error::Error for RemoteTaskExecutionTransitionError {}

/// Apply one execution event to a state without mutating any runtime or
/// transport state.
pub const fn transition_execution_state(
    state: RemoteTaskExecutionState,
    event: RemoteTaskExecutionEvent,
) -> Result<RemoteTaskExecutionState, RemoteTaskExecutionTransitionError> {
    use RemoteTaskExecutionEvent::{Complete, Fail, Lease, MarkLost, RequestCancel, Start};
    use RemoteTaskExecutionState::{
        CancelRequested, Completed, Failed, Leased, Lost, Queued, Running,
    };

    let next = match (state, event) {
        (Queued, Lease) => Leased,
        (Queued, RequestCancel) => CancelRequested,
        (Leased, Start) => Running,
        (Leased, RequestCancel) => CancelRequested,
        (Leased, MarkLost) => Lost,
        (Running, RequestCancel) => CancelRequested,
        (Running, Complete) => Completed,
        (Running, Fail) => Failed,
        (Running, MarkLost) => Lost,
        // A cancellation request is an idempotent intent, so a retried
        // request does not create a second transition or error.
        (CancelRequested, RequestCancel) => CancelRequested,
        // Completion/failure can race with cancellation; the terminal event
        // is accepted as the observed outcome of that race.
        (CancelRequested, Complete) => Completed,
        (CancelRequested, Fail) => Failed,
        (CancelRequested, MarkLost) => Lost,
        // Lost is also idempotent for repeated disconnect notifications.
        (Lost, MarkLost) => Lost,
        _ => {
            return Err(RemoteTaskExecutionTransitionError { state, event });
        }
    };
    Ok(next)
}

/// Pure helper for recording a cancellation intent.
pub const fn request_cancel(
    state: RemoteTaskExecutionState,
) -> Result<RemoteTaskExecutionState, RemoteTaskExecutionTransitionError> {
    transition_execution_state(state, RemoteTaskExecutionEvent::RequestCancel)
}

/// Pure helper for recording that a Worker connection was lost while the task
/// was leased or running.  A queued task remains queued until a scheduler
/// explicitly decides whether to retry it.
pub const fn mark_lost(
    state: RemoteTaskExecutionState,
) -> Result<RemoteTaskExecutionState, RemoteTaskExecutionTransitionError> {
    transition_execution_state(state, RemoteTaskExecutionEvent::MarkLost)
}

/// Descriptive alias for callers that prefer “task state” terminology.
pub const fn transition_task_state(
    state: RemoteTaskExecutionState,
    event: RemoteTaskExecutionEvent,
) -> Result<RemoteTaskExecutionState, RemoteTaskExecutionTransitionError> {
    transition_execution_state(state, event)
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
    TaskInvalidState,
}

impl std::fmt::Display for RuntimeDispatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PermissionNotAdmitted => f.write_str("permission_not_admitted"),
            Self::TaskAlreadyRunning => f.write_str("task_already_running"),
            Self::TaskNotFound => f.write_str("task_not_found"),
            Self::TaskNotCancellable => f.write_str("task_not_cancellable"),
            Self::TaskInvalidState => f.write_str("task_invalid_state"),
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct FixtureTask {
    task: RemoteTaskRequest,
    state: RemoteTaskExecutionState,
}

/// A deterministic read-only adapter for transport and recovery tests.  It
/// stores task metadata only and never starts a local process.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReadOnlyFixtureRuntime {
    tasks: BTreeMap<String, FixtureTask>,
}

impl ReadOnlyFixtureRuntime {
    pub fn status(&self, task_id: &str) -> Option<RemoteTaskRuntimeStatus> {
        self.execution_state(task_id)
    }

    pub fn execution_state(&self, task_id: &str) -> Option<RemoteTaskExecutionState> {
        self.tasks.get(task_id).map(|task| task.state)
    }

    /// Add a read-only task in the queued state.
    pub fn queue(
        &mut self,
        task: &RemoteTaskRequest,
    ) -> Result<RuntimeDispatchReceipt, RuntimeDispatchError> {
        self.admit(task)?;
        self.tasks.insert(
            task.task_id.clone(),
            FixtureTask {
                task: task.clone(),
                state: RemoteTaskExecutionState::Queued,
            },
        );
        Ok(self.receipt(task.task_id.as_str()))
    }

    pub fn lease(&mut self, task_id: &str) -> Result<RuntimeDispatchReceipt, RuntimeDispatchError> {
        self.transition(task_id, RemoteTaskExecutionEvent::Lease)
    }

    pub fn start(&mut self, task_id: &str) -> Result<RuntimeDispatchReceipt, RuntimeDispatchError> {
        self.transition(task_id, RemoteTaskExecutionEvent::Start)
    }

    pub fn complete(
        &mut self,
        task_id: &str,
    ) -> Result<RuntimeDispatchReceipt, RuntimeDispatchError> {
        self.transition(task_id, RemoteTaskExecutionEvent::Complete)
    }

    pub fn fail(&mut self, task_id: &str) -> Result<RuntimeDispatchReceipt, RuntimeDispatchError> {
        self.transition(task_id, RemoteTaskExecutionEvent::Fail)
    }

    pub fn request_cancel(
        &mut self,
        task_id: &str,
    ) -> Result<RuntimeDispatchReceipt, RuntimeDispatchError> {
        self.transition(task_id, RemoteTaskExecutionEvent::RequestCancel)
    }

    pub fn mark_lost(
        &mut self,
        task_id: &str,
    ) -> Result<RuntimeDispatchReceipt, RuntimeDispatchError> {
        self.transition(task_id, RemoteTaskExecutionEvent::MarkLost)
    }

    fn admit(&self, task: &RemoteTaskRequest) -> Result<(), RuntimeDispatchError> {
        if !matches!(task.permission, RemoteTaskPermission::ReadOnly) {
            return Err(RuntimeDispatchError::PermissionNotAdmitted);
        }
        if self.tasks.contains_key(&task.task_id) {
            return Err(RuntimeDispatchError::TaskAlreadyRunning);
        }
        Ok(())
    }

    fn transition(
        &mut self,
        task_id: &str,
        event: RemoteTaskExecutionEvent,
    ) -> Result<RuntimeDispatchReceipt, RuntimeDispatchError> {
        let task = self
            .tasks
            .get_mut(task_id)
            .ok_or(RuntimeDispatchError::TaskNotFound)?;
        task.state = transition_execution_state(task.state, event)
            .map_err(|_| RuntimeDispatchError::TaskInvalidState)?;
        Ok(RuntimeDispatchReceipt {
            task_id: task.task.task_id.clone(),
            worker_id: task.task.worker_id.clone(),
            status: task.state,
            fixture: true,
        })
    }

    fn receipt(&self, task_id: &str) -> RuntimeDispatchReceipt {
        let task = self
            .tasks
            .get(task_id)
            .expect("fixture task inserted before receipt");
        RuntimeDispatchReceipt {
            task_id: task.task.task_id.clone(),
            worker_id: task.task.worker_id.clone(),
            status: task.state,
            fixture: true,
        }
    }
}

impl RemoteWorkerRuntimeAdapter for ReadOnlyFixtureRuntime {
    fn dispatch(
        &mut self,
        task: &RemoteTaskRequest,
    ) -> Result<RuntimeDispatchReceipt, RuntimeDispatchError> {
        // The fixture's dispatch boundary represents a Worker that has
        // already claimed the task.  Keep the M3 behavior (dispatch returns
        // running) while exercising the same pure queued -> leased -> running
        // transitions used by a real adapter.
        self.queue(task)?;
        self.lease(&task.task_id)?;
        self.start(&task.task_id)
    }

    fn cancel(&mut self, task_id: &str) -> Result<RuntimeDispatchReceipt, RuntimeDispatchError> {
        let state = self
            .execution_state(task_id)
            .ok_or(RuntimeDispatchError::TaskNotFound)?;
        if !state.can_request_cancel() && state != RemoteTaskExecutionState::CancelRequested {
            return Err(RuntimeDispatchError::TaskNotCancellable);
        }
        self.request_cancel(task_id)
    }

    fn disconnect(&mut self) -> Vec<RuntimeDispatchReceipt> {
        self.tasks
            .values_mut()
            .filter_map(|task| {
                if !task.state.can_mark_lost() {
                    return None;
                }
                task.state = RemoteTaskExecutionState::Lost;
                Some(RuntimeDispatchReceipt {
                    task_id: task.task.task_id.clone(),
                    worker_id: task.task.worker_id.clone(),
                    status: task.state,
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
    fn pure_state_machine_covers_safe_execution_lifecycle() {
        use RemoteTaskExecutionEvent::*;
        use RemoteTaskExecutionState::*;

        let state = transition_execution_state(Queued, Lease).unwrap();
        assert_eq!(state, Leased);
        let state = transition_execution_state(state, Start).unwrap();
        assert_eq!(state, Running);
        let state = transition_execution_state(state, RequestCancel).unwrap();
        assert_eq!(state, CancelRequested);
        assert_eq!(request_cancel(state), Ok(CancelRequested));
        assert_eq!(transition_execution_state(state, Complete), Ok(Completed));
        assert_eq!(transition_execution_state(state, Fail), Ok(Failed));
        assert_eq!(mark_lost(state), Ok(Lost));
    }

    #[test]
    fn pure_state_machine_rejects_unsafe_or_terminal_transitions() {
        use RemoteTaskExecutionEvent::*;
        use RemoteTaskExecutionState::*;

        assert!(transition_execution_state(Queued, Complete).is_err());
        assert!(transition_execution_state(Running, Lease).is_err());
        assert!(transition_execution_state(Completed, MarkLost).is_err());
        assert!(request_cancel(Failed).is_err());
        assert!(mark_lost(Queued).is_err());
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
    fn fixture_exposes_all_states_and_terminal_outcomes() {
        let mut runtime = ReadOnlyFixtureRuntime::default();
        let read_only = task(RemoteTaskPermission::ReadOnly);
        assert_eq!(
            runtime.queue(&read_only).unwrap().status,
            RemoteTaskExecutionState::Queued
        );
        assert_eq!(
            runtime.lease("fixture-task").unwrap().status,
            RemoteTaskExecutionState::Leased
        );
        assert_eq!(
            runtime.start("fixture-task").unwrap().status,
            RemoteTaskExecutionState::Running
        );
        assert_eq!(
            runtime.complete("fixture-task").unwrap().status,
            RemoteTaskExecutionState::Completed
        );

        let mut runtime = ReadOnlyFixtureRuntime::default();
        runtime.queue(&read_only).unwrap();
        assert_eq!(
            runtime.request_cancel("fixture-task").unwrap().status,
            RemoteTaskExecutionState::CancelRequested
        );
        assert_eq!(
            runtime.fail("fixture-task").unwrap().status,
            RemoteTaskExecutionState::Failed
        );
    }

    #[test]
    fn cancel_request_and_disconnect_are_non_completion_states() {
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
