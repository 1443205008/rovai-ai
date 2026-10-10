//! Worker-side async execution boundary.
//!
//! This module is intentionally a narrow seam.  It turns the portable
//! [`RemoteTaskRequest`](crate::remote_worker::RemoteTaskRequest) into a typed
//! read-only request, resolves a worker-local workspace binding, and invokes
//! an asynchronous Runtime port.  It does not contain transport code, shell
//! text, process spawning, or filesystem paths.  A future Runtime integration
//! can implement [`WorkerRuntimePort`] without changing the protocol or the
//! execution state machine.

use std::{collections::BTreeMap, future::Future, pin::Pin};

use crate::{
    agent_profile::AdapterKind,
    remote_worker::{RemoteTaskPermission, RemoteTaskRequest, validate_task},
    remote_worker_runtime::{
        RemoteTaskExecutionEvent, RemoteTaskExecutionState, RuntimeDispatchReceipt,
        transition_execution_state,
    },
};

/// The only Agent kind admitted by this first Worker-side bridge.
///
/// The wire protocol still carries a string for compatibility with existing
/// registration records.  Parsing at this boundary prevents a Worker from
/// dispatching an arbitrary provider or command name to a local Runtime.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CanonicalWorkerAgentKind {
    CodexCli,
}

impl CanonicalWorkerAgentKind {
    pub const CODEX_CLI: &'static str = "codex_cli";

    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::CodexCli => Self::CODEX_CLI,
        }
    }

    pub fn parse(value: &str) -> Result<Self, WorkerBridgeError<()>> {
        match value {
            Self::CODEX_CLI => Ok(Self::CodexCli),
            _ => Err(WorkerBridgeError::UnsupportedAgentKind),
        }
    }
}

/// Opaque Worker-local workspace identity.
///
/// This is deliberately an ID and never a `Path`.  The binder implementation
/// owns the mapping from this ID to local state; Core/transport code cannot
/// smuggle an absolute path through this bridge.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WorkerWorkspaceId(String);

impl WorkerWorkspaceId {
    pub const MAX_LEN: usize = 128;

    pub fn parse(value: &str) -> Result<Self, WorkerBridgeError<()>> {
        if value.trim().is_empty()
            || value.trim() != value
            || value.len() > Self::MAX_LEN
            || value.chars().any(char::is_control)
            || value.contains('/')
            || value.contains('\\')
            || matches!(value, "." | "..")
        {
            return Err(WorkerBridgeError::InvalidWorkspaceId);
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Typed task accepted by the Worker-side bridge.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerExecutionRequest {
    pub task_id: String,
    pub worker_id: String,
    pub agent_kind: CanonicalWorkerAgentKind,
    pub workspace_id: WorkerWorkspaceId,
    pub prompt: String,
    pub timeout_seconds: u32,
    pub attempt: u32,
}

impl TryFrom<RemoteTaskRequest> for WorkerExecutionRequest {
    type Error = WorkerBridgeError<()>;

    fn try_from(value: RemoteTaskRequest) -> Result<Self, Self::Error> {
        validate_task(&value).map_err(WorkerBridgeError::InvalidTask)?;
        if !matches!(value.permission, RemoteTaskPermission::ReadOnly) {
            return Err(WorkerBridgeError::PermissionNotAdmitted);
        }
        Ok(Self {
            task_id: value.task_id,
            worker_id: value.worker_id,
            agent_kind: CanonicalWorkerAgentKind::parse(&value.agent_kind)?,
            workspace_id: WorkerWorkspaceId::parse(&value.workspace_id)?,
            prompt: value.prompt,
            timeout_seconds: value.timeout_seconds,
            attempt: value.attempt,
        })
    }
}

impl WorkerExecutionRequest {
    /// Reconstruct the protocol request without exposing any local binding.
    pub fn as_remote_task(&self) -> RemoteTaskRequest {
        RemoteTaskRequest {
            task_id: self.task_id.clone(),
            worker_id: self.worker_id.clone(),
            agent_kind: self.agent_kind.wire_name().to_owned(),
            workspace_id: self.workspace_id.as_str().to_owned(),
            prompt: self.prompt.clone(),
            permission: RemoteTaskPermission::ReadOnly,
            timeout_seconds: self.timeout_seconds,
            attempt: self.attempt,
        }
    }

    /// Bind a validated Worker request to an already-admitted Core execution.
    ///
    /// The wire request intentionally does not carry a Core `AgentRun` lease,
    /// compatibility digest, or a machine-local path.  A Worker may only
    /// enter the Runtime Fleet after the trusted lease/dispatch path supplies
    /// those values.  Keeping this conversion explicit prevents callers from
    /// deriving a Fleet lease from `task_id`, `attempt`, or an untrusted
    /// Worker payload.
    pub fn bind_to_fleet(
        &self,
        context: WorkerFleetExecutionContext,
    ) -> Result<WorkerFleetExecutionRequest, WorkerFleetBindingError> {
        context.validate()?;
        Ok(WorkerFleetExecutionRequest {
            task: self.clone(),
            agent_run_id: context.agent_run_id,
            execution_epoch: context.execution_epoch,
            camp_id: context.camp_id,
            agent_id: context.agent_id,
            workspace_key: context.workspace_key,
            runtime_compatibility_digest: context.runtime_compatibility_digest,
            lease_token: context.lease_token,
        })
    }
}

/// Trusted Core-side execution identity needed before a Worker can acquire an
/// existing Runtime Fleet lease.  None of these fields are accepted from a
/// remote task payload; they come from the Core command/lease transaction.
/// In particular, `workspace_key` is an opaque identifier, never a path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerFleetExecutionContext {
    pub agent_run_id: String,
    pub execution_epoch: i64,
    pub camp_id: String,
    pub agent_id: String,
    pub workspace_key: WorkerWorkspaceId,
    pub runtime_compatibility_digest: String,
    pub lease_token: String,
}

impl WorkerFleetExecutionContext {
    fn validate(&self) -> Result<(), WorkerFleetBindingError> {
        if self.execution_epoch <= 0 {
            return Err(WorkerFleetBindingError::ExecutionEpochInvalid);
        }
        validate_opaque_identity(&self.agent_run_id, 256)
            .then_some(())
            .ok_or(WorkerFleetBindingError::AgentRunIdentityInvalid)?;
        validate_opaque_identity(&self.camp_id, 256)
            .then_some(())
            .ok_or(WorkerFleetBindingError::CampIdentityInvalid)?;
        validate_opaque_identity(&self.agent_id, 256)
            .then_some(())
            .ok_or(WorkerFleetBindingError::AgentIdentityInvalid)?;
        validate_opaque_identity(&self.runtime_compatibility_digest, 512)
            .then_some(())
            .ok_or(WorkerFleetBindingError::CompatibilityDigestInvalid)?;
        validate_opaque_identity(&self.lease_token, 512)
            .then_some(())
            .ok_or(WorkerFleetBindingError::LeaseTokenInvalid)?;
        Ok(())
    }
}

/// The minimum typed request a future Worker Runtime adapter must hand to the
/// existing Core Runtime Fleet.  It has no executable path, shell text, or
/// mutable workspace root.  The Fleet integration is deliberately a separate
/// port because constructing a real Host also needs the local, admitted
/// runtime configuration and Core-owned event/receipt sinks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerFleetExecutionRequest {
    pub task: WorkerExecutionRequest,
    pub agent_run_id: String,
    pub execution_epoch: i64,
    pub camp_id: String,
    pub agent_id: String,
    pub workspace_key: WorkerWorkspaceId,
    pub runtime_compatibility_digest: String,
    pub lease_token: String,
}

impl WorkerFleetExecutionRequest {
    pub fn adapter_kind(&self) -> AdapterKind {
        match self.task.agent_kind {
            CanonicalWorkerAgentKind::CodexCli => AdapterKind::CodexCli,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkerFleetBindingError {
    AgentRunIdentityInvalid,
    ExecutionEpochInvalid,
    CampIdentityInvalid,
    AgentIdentityInvalid,
    CompatibilityDigestInvalid,
    LeaseTokenInvalid,
}

impl std::fmt::Display for WorkerFleetBindingError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::AgentRunIdentityInvalid => "agent_run_identity_invalid",
            Self::ExecutionEpochInvalid => "execution_epoch_invalid",
            Self::CampIdentityInvalid => "camp_identity_invalid",
            Self::AgentIdentityInvalid => "agent_identity_invalid",
            Self::CompatibilityDigestInvalid => "runtime_compatibility_digest_invalid",
            Self::LeaseTokenInvalid => "lease_token_invalid",
        })
    }
}

impl std::error::Error for WorkerFleetBindingError {}

fn validate_opaque_identity(value: &str, max_len: usize) -> bool {
    !value.trim().is_empty()
        && value.trim() == value
        && value.len() <= max_len
        && !value.contains('/')
        && !value.contains('\\')
        && !value.chars().any(char::is_control)
}

/// Port implemented by the Worker process once it has a local Runtime Fleet
/// owner and the Core lease/event sinks.  This intentionally carries a typed
/// Fleet request rather than a `Command`, executable path, or cwd.  The
/// current repository has no Worker process entrypoint that can satisfy all
/// of those inputs, so no implementation is provided here yet.
pub trait WorkerRuntimeFleetPort {
    type WorkspaceBinding: Clone + Send + Sync + 'static;
    type Error;

    fn execute_fleet<'a>(
        &'a mut self,
        request: &'a WorkerFleetExecutionRequest,
        binding: Self::WorkspaceBinding,
    ) -> WorkerRuntimeFuture<'a, Result<(), Self::Error>>;

    fn cancel_fleet<'a>(
        &'a mut self,
        task_id: &'a str,
    ) -> WorkerRuntimeFuture<'a, Result<(), Self::Error>>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkerBridgeError<E> {
    InvalidTask(&'static str),
    PermissionNotAdmitted,
    UnsupportedAgentKind,
    InvalidWorkspaceId,
    WorkspaceNotBound,
    TaskNotFound,
    TaskNotCancellable,
    InvalidState,
    Runtime(E),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerWorkspaceBindingError(pub &'static str);

/// Resolve a protocol workspace ID to a local, opaque binding.
///
/// The associated type is intentionally owned by the Worker implementation;
/// this trait has no path-returning method and is therefore safe to use from a
/// transport-facing boundary.
pub trait WorkerWorkspaceBinder {
    type Binding: Clone + Send + Sync + 'static;

    fn bind(
        &self,
        workspace_id: &WorkerWorkspaceId,
    ) -> Result<Self::Binding, WorkerWorkspaceBindingError>;
}

pub type WorkerRuntimeFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Async local Runtime seam.  Implementations remain responsible for their
/// own process/admission policy; this interface receives only a typed request
/// and an opaque workspace binding.
pub trait WorkerRuntimePort {
    type WorkspaceBinding: Clone + Send + Sync + 'static;
    type Error;

    fn execute<'a>(
        &'a mut self,
        request: &'a WorkerExecutionRequest,
        binding: Self::WorkspaceBinding,
    ) -> WorkerRuntimeFuture<'a, Result<(), Self::Error>>;

    fn cancel<'a>(
        &'a mut self,
        task_id: &'a str,
    ) -> WorkerRuntimeFuture<'a, Result<(), Self::Error>>;
}

/// Worker-side async bridge with explicit lifecycle projection.
///
/// `execute` leases and starts a task before awaiting the Runtime, then uses
/// the shared `remote_worker_runtime` transition function to record the
/// observed terminal outcome.  It never interprets a disconnect as success.
pub struct WorkerExecutionBridge<R, B>
where
    R: WorkerRuntimePort,
    B: WorkerWorkspaceBinder<Binding = R::WorkspaceBinding>,
{
    runtime: R,
    binder: B,
    states: BTreeMap<String, (String, RemoteTaskExecutionState)>,
}

impl<R, B> WorkerExecutionBridge<R, B>
where
    R: WorkerRuntimePort,
    B: WorkerWorkspaceBinder<Binding = R::WorkspaceBinding>,
{
    pub fn new(runtime: R, binder: B) -> Self {
        Self {
            runtime,
            binder,
            states: BTreeMap::new(),
        }
    }

    pub fn state(&self, task_id: &str) -> Option<RemoteTaskExecutionState> {
        self.states.get(task_id).map(|(_, state)| *state)
    }

    pub async fn execute(
        &mut self,
        raw: RemoteTaskRequest,
    ) -> Result<RuntimeDispatchReceipt, WorkerBridgeError<R::Error>> {
        let request =
            WorkerExecutionRequest::try_from(raw).map_err(|error| map_unit_error(error))?;
        let task_id = request.task_id.clone();
        if self.states.contains_key(&task_id) {
            return Err(WorkerBridgeError::InvalidState);
        }
        let binding = self
            .binder
            .bind(&request.workspace_id)
            .map_err(|_| WorkerBridgeError::WorkspaceNotBound)?;
        let worker_id = request.worker_id.clone();
        let mut state = RemoteTaskExecutionState::Queued;
        state = transition_execution_state(state, RemoteTaskExecutionEvent::Lease)
            .map_err(|_| WorkerBridgeError::InvalidState)?;
        state = transition_execution_state(state, RemoteTaskExecutionEvent::Start)
            .map_err(|_| WorkerBridgeError::InvalidState)?;
        self.states
            .insert(task_id.clone(), (worker_id.clone(), state));

        let result = self.runtime.execute(&request, binding).await;
        let event = if result.is_ok() {
            RemoteTaskExecutionEvent::Complete
        } else {
            RemoteTaskExecutionEvent::Fail
        };
        state = transition_execution_state(state, event)
            .map_err(|_| WorkerBridgeError::InvalidState)?;
        self.states
            .insert(task_id.clone(), (worker_id.clone(), state));
        match result {
            Ok(()) => Ok(RuntimeDispatchReceipt {
                task_id,
                worker_id,
                status: state,
                fixture: false,
            }),
            Err(error) => Err(WorkerBridgeError::Runtime(error)),
        }
    }

    pub async fn cancel(
        &mut self,
        task_id: &str,
    ) -> Result<RuntimeDispatchReceipt, WorkerBridgeError<R::Error>> {
        let (worker_id, state) = self
            .states
            .get(task_id)
            .cloned()
            .ok_or(WorkerBridgeError::TaskNotFound)?;
        if !state.can_request_cancel() && state != RemoteTaskExecutionState::CancelRequested {
            return Err(WorkerBridgeError::TaskNotCancellable);
        }
        let state = transition_execution_state(state, RemoteTaskExecutionEvent::RequestCancel)
            .map_err(|_| WorkerBridgeError::InvalidState)?;
        self.states
            .insert(task_id.to_owned(), (worker_id.clone(), state));
        self.runtime
            .cancel(task_id)
            .await
            .map_err(WorkerBridgeError::Runtime)?;
        Ok(RuntimeDispatchReceipt {
            task_id: task_id.to_owned(),
            worker_id,
            status: state,
            fixture: false,
        })
    }

    pub fn disconnect(&mut self) -> Vec<RuntimeDispatchReceipt> {
        let mut receipts = Vec::new();
        for (task_id, (worker_id, state)) in &mut self.states {
            if !state.can_mark_lost() {
                continue;
            }
            if let Ok(next) = transition_execution_state(*state, RemoteTaskExecutionEvent::MarkLost)
            {
                *state = next;
                receipts.push(RuntimeDispatchReceipt {
                    task_id: task_id.clone(),
                    worker_id: worker_id.clone(),
                    status: next,
                    fixture: false,
                });
            }
        }
        receipts
    }
}

fn map_unit_error<E>(error: WorkerBridgeError<()>) -> WorkerBridgeError<E> {
    match error {
        WorkerBridgeError::InvalidTask(reason) => WorkerBridgeError::InvalidTask(reason),
        WorkerBridgeError::PermissionNotAdmitted => WorkerBridgeError::PermissionNotAdmitted,
        WorkerBridgeError::UnsupportedAgentKind => WorkerBridgeError::UnsupportedAgentKind,
        WorkerBridgeError::InvalidWorkspaceId => WorkerBridgeError::InvalidWorkspaceId,
        WorkerBridgeError::WorkspaceNotBound => WorkerBridgeError::WorkspaceNotBound,
        WorkerBridgeError::TaskNotFound => WorkerBridgeError::TaskNotFound,
        WorkerBridgeError::TaskNotCancellable => WorkerBridgeError::TaskNotCancellable,
        WorkerBridgeError::InvalidState => WorkerBridgeError::InvalidState,
        WorkerBridgeError::Runtime(()) => {
            unreachable!("unit validation cannot produce runtime error")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_worker::RemoteTaskPermission;
    use std::future;

    #[derive(Clone, Debug)]
    struct Binder;

    impl WorkerWorkspaceBinder for Binder {
        type Binding = u8;

        fn bind(
            &self,
            workspace_id: &WorkerWorkspaceId,
        ) -> Result<Self::Binding, WorkerWorkspaceBindingError> {
            (workspace_id.as_str() == "main")
                .then_some(7)
                .ok_or(WorkerWorkspaceBindingError("unknown_workspace"))
        }
    }

    #[derive(Debug, Default)]
    struct Runtime {
        fail: bool,
        binding: Option<u8>,
    }

    impl WorkerRuntimePort for Runtime {
        type WorkspaceBinding = u8;
        type Error = &'static str;

        fn execute<'a>(
            &'a mut self,
            _request: &'a WorkerExecutionRequest,
            binding: Self::WorkspaceBinding,
        ) -> WorkerRuntimeFuture<'a, Result<(), Self::Error>> {
            self.binding = Some(binding);
            let fail = self.fail;
            Box::pin(future::ready(if fail {
                Err("runtime_failed")
            } else {
                Ok(())
            }))
        }

        fn cancel<'a>(
            &'a mut self,
            _task_id: &'a str,
        ) -> WorkerRuntimeFuture<'a, Result<(), Self::Error>> {
            Box::pin(future::ready(Ok(())))
        }
    }

    fn task() -> RemoteTaskRequest {
        RemoteTaskRequest {
            task_id: "task-1".into(),
            worker_id: "worker-1".into(),
            agent_kind: CanonicalWorkerAgentKind::CODEX_CLI.into(),
            workspace_id: "main".into(),
            prompt: "read-only request".into(),
            permission: RemoteTaskPermission::ReadOnly,
            timeout_seconds: 30,
            attempt: 1,
        }
    }

    fn fleet_context() -> WorkerFleetExecutionContext {
        WorkerFleetExecutionContext {
            agent_run_id: "run-1".into(),
            execution_epoch: 7,
            camp_id: "camp-1".into(),
            agent_id: "agent-1".into(),
            workspace_key: WorkerWorkspaceId::parse("main").unwrap(),
            runtime_compatibility_digest: "sha256:fixture".into(),
            lease_token: "lease-1".into(),
        }
    }

    #[test]
    fn typed_request_rejects_writes_unknown_agents_and_path_like_workspace_ids() {
        let mut write = task();
        write.permission = RemoteTaskPermission::WorkspaceWrite;
        assert_eq!(
            WorkerExecutionRequest::try_from(write),
            Err(WorkerBridgeError::PermissionNotAdmitted)
        );
        let mut unknown = task();
        unknown.agent_kind = "arbitrary-shell".into();
        assert_eq!(
            WorkerExecutionRequest::try_from(unknown),
            Err(WorkerBridgeError::UnsupportedAgentKind)
        );
        let mut path = task();
        path.workspace_id = "../main".into();
        assert_eq!(
            WorkerExecutionRequest::try_from(path),
            Err(WorkerBridgeError::InvalidWorkspaceId)
        );
    }

    #[test]
    fn fleet_binding_requires_trusted_opaque_execution_context() {
        let request = WorkerExecutionRequest::try_from(task()).unwrap();
        let bound = request.bind_to_fleet(fleet_context()).unwrap();
        assert_eq!(bound.adapter_kind(), AdapterKind::CodexCli);
        assert_eq!(bound.agent_run_id, "run-1");
        assert_eq!(bound.execution_epoch, 7);
        assert_eq!(bound.workspace_key.as_str(), "main");

        let mut invalid = fleet_context();
        invalid.lease_token = "/tmp/lease".into();
        assert_eq!(
            request.bind_to_fleet(invalid),
            Err(WorkerFleetBindingError::LeaseTokenInvalid)
        );
        let mut invalid = fleet_context();
        invalid.execution_epoch = 0;
        assert_eq!(
            request.bind_to_fleet(invalid),
            Err(WorkerFleetBindingError::ExecutionEpochInvalid)
        );
    }

    #[tokio::test]
    async fn bridge_binds_opaque_workspace_and_projects_terminal_state() {
        let mut bridge = WorkerExecutionBridge::new(Runtime::default(), Binder);
        let receipt = bridge.execute(task()).await.unwrap();
        assert_eq!(receipt.status, RemoteTaskExecutionState::Completed);
        assert_eq!(
            bridge.state("task-1"),
            Some(RemoteTaskExecutionState::Completed)
        );
        assert!(!receipt.fixture);
    }

    #[tokio::test]
    async fn runtime_failure_is_not_reported_as_completion() {
        let mut bridge = WorkerExecutionBridge::new(
            Runtime {
                fail: true,
                binding: None,
            },
            Binder,
        );
        assert_eq!(
            bridge.execute(task()).await,
            Err(WorkerBridgeError::Runtime("runtime_failed"))
        );
        assert_eq!(
            bridge.state("task-1"),
            Some(RemoteTaskExecutionState::Failed)
        );
    }

    #[test]
    fn disconnect_projects_lost_for_running_tasks() {
        let mut bridge = WorkerExecutionBridge::new(Runtime::default(), Binder);
        bridge.states.insert(
            "task-1".into(),
            ("worker-1".into(), RemoteTaskExecutionState::Running),
        );
        assert_eq!(
            bridge.disconnect()[0].status,
            RemoteTaskExecutionState::Lost
        );
    }
}
