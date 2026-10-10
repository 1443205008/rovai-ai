//! Core-owned Runtime context for remote Worker execution.
//!
//! This module is the admission seam between the Worker protocol and the
//! existing Runtime Fleet.  A protocol request is not a Runtime lease: Core
//! must first supply a frozen configuration, an execution epoch, a lease
//! fence, and an admitted workspace binding.  The public types below do not
//! serialize; a frozen config may retain Core's private path for a future
//! local adapter, but no path or shell text crosses this boundary.
//!
//! The Fleet request conversion is `pub(crate)` on purpose.  The Fleet keeps
//! its process and host types private to Core; a transport adapter can only
//! receive opaque IDs and a typed Core-owned binding.

use std::{collections::BTreeMap, fmt};

use crate::{
    agent_profile::{AdapterKind, FrozenAgentRuntimeConfig},
    planned_shutdown::RuntimeTerminalOutcome,
    remote_worker_bridge::WorkerWorkspaceId,
    runtime::AgentRunWorkspace,
    runtime_fleet::{FleetAcquireRequest, RuntimeCompatibilityKey},
};

const MAX_IDENTITY_LEN: usize = 256;
const MAX_FINGERPRINT_LEN: usize = 512;

/// Errors raised while Core admits the typed Runtime context.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuntimeContextAdmissionError {
    IdentityInvalid(&'static str),
    ExecutionEpochInvalid,
    LeaseFenceInvalid,
    FrozenRuntimeInvalid(&'static str),
    WorkspaceInvalid(&'static str),
    WorkspaceBindingMismatch,
}

impl fmt::Display for RuntimeContextAdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IdentityInvalid(field) => write!(formatter, "{field}_invalid"),
            Self::ExecutionEpochInvalid => formatter.write_str("execution_epoch_invalid"),
            Self::LeaseFenceInvalid => formatter.write_str("lease_fence_invalid"),
            Self::FrozenRuntimeInvalid(field) => {
                write!(formatter, "frozen_runtime_{field}_invalid")
            }
            Self::WorkspaceInvalid(field) => write!(formatter, "workspace_{field}_invalid"),
            Self::WorkspaceBindingMismatch => formatter.write_str("workspace_binding_mismatch"),
        }
    }
}

impl std::error::Error for RuntimeContextAdmissionError {}

fn valid_opaque(value: &str, max_len: usize) -> bool {
    !value.trim().is_empty()
        && value.trim() == value
        && value.len() <= max_len
        && !value.chars().any(char::is_control)
        && !value.contains('/')
        && !value.contains('\\')
}

/// Core's execution lease fence.  The token is intentionally not exposed in
/// a serializable form; it is only compared with a callback/event supplied to
/// the Core admission API.
#[derive(Clone, Eq, PartialEq)]
pub struct RuntimeLeaseFence {
    agent_run_id: String,
    execution_epoch: i64,
    lease_token: String,
}

impl fmt::Debug for RuntimeLeaseFence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeLeaseFence")
            .field("agent_run_id", &self.agent_run_id)
            .field("execution_epoch", &self.execution_epoch)
            .field("lease_token", &"<redacted>")
            .finish()
    }
}

impl RuntimeLeaseFence {
    pub fn new(
        agent_run_id: impl Into<String>,
        execution_epoch: i64,
        lease_token: impl Into<String>,
    ) -> Result<Self, RuntimeContextAdmissionError> {
        let agent_run_id = agent_run_id.into();
        let lease_token = lease_token.into();
        if !valid_opaque(&agent_run_id, MAX_IDENTITY_LEN) {
            return Err(RuntimeContextAdmissionError::IdentityInvalid("agent_run"));
        }
        if execution_epoch <= 0 {
            return Err(RuntimeContextAdmissionError::ExecutionEpochInvalid);
        }
        if !valid_opaque(&lease_token, MAX_FINGERPRINT_LEN) {
            return Err(RuntimeContextAdmissionError::LeaseFenceInvalid);
        }
        Ok(Self {
            agent_run_id,
            execution_epoch,
            lease_token,
        })
    }

    pub fn agent_run_id(&self) -> &str {
        &self.agent_run_id
    }

    /// Return the opaque lease token for Core-owned persistence adapters.
    ///
    /// The token is intentionally not part of any serialized DTO.  A queue
    /// adapter may persist it as part of its own fenced record, but callers
    /// still have to obtain the fence from a trusted Core admission first.
    pub fn lease_token(&self) -> &str {
        &self.lease_token
    }

    pub const fn execution_epoch(&self) -> i64 {
        self.execution_epoch
    }

    /// Compare a callback's complete fence.  Matching only the run ID is not
    /// sufficient: a late callback from an older epoch must be rejected.
    pub fn matches(&self, agent_run_id: &str, execution_epoch: i64, lease_token: &str) -> bool {
        self.agent_run_id == agent_run_id
            && self.execution_epoch == execution_epoch
            && self.lease_token == lease_token
    }
}

/// A frozen Runtime configuration owned by Core.  It is intentionally a
/// wrapper rather than a wire DTO: executable paths and provider-specific
/// options stay private behind the Core boundary.
#[derive(Clone, PartialEq)]
pub struct FrozenRuntimeConfig {
    inner: FrozenAgentRuntimeConfig,
}

impl fmt::Debug for FrozenRuntimeConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FrozenRuntimeConfig")
            .field("adapter_kind", &self.inner.adapter_kind)
            .field("installation_id", &self.inner.installation_id)
            .field("config_digest", &self.inner.config_digest)
            .field(
                "binding_compatibility_digest",
                &self.inner.binding_compatibility_digest,
            )
            .finish()
    }
}

impl FrozenRuntimeConfig {
    pub fn freeze(inner: FrozenAgentRuntimeConfig) -> Result<Self, RuntimeContextAdmissionError> {
        if !valid_opaque(&inner.installation_id, MAX_IDENTITY_LEN) {
            return Err(RuntimeContextAdmissionError::FrozenRuntimeInvalid(
                "installation_id",
            ));
        }
        if inner.executable_path.trim().is_empty() {
            return Err(RuntimeContextAdmissionError::FrozenRuntimeInvalid(
                "executable",
            ));
        }
        if !valid_opaque(&inner.binding_compatibility_digest, MAX_FINGERPRINT_LEN) {
            return Err(RuntimeContextAdmissionError::FrozenRuntimeInvalid(
                "binding_compatibility_digest",
            ));
        }
        if !valid_opaque(&inner.host_config_digest, MAX_FINGERPRINT_LEN) {
            return Err(RuntimeContextAdmissionError::FrozenRuntimeInvalid(
                "host_config_digest",
            ));
        }
        if !valid_opaque(&inner.config_digest, MAX_FINGERPRINT_LEN) {
            return Err(RuntimeContextAdmissionError::FrozenRuntimeInvalid(
                "config_digest",
            ));
        }
        Ok(Self { inner })
    }

    pub fn adapter_kind(&self) -> AdapterKind {
        self.inner.adapter_kind
    }

    pub fn installation_id(&self) -> &str {
        &self.inner.installation_id
    }

    pub fn config_digest(&self) -> &str {
        &self.inner.config_digest
    }

    pub fn binding_compatibility_digest(&self) -> &str {
        &self.inner.binding_compatibility_digest
    }

    pub fn host_config_digest(&self) -> &str {
        &self.inner.host_config_digest
    }

    #[allow(dead_code)]
    pub(crate) fn as_inner(&self) -> &FrozenAgentRuntimeConfig {
        &self.inner
    }
}

/// Access admitted for a remote Worker task.  The actual execution root is
/// kept in Core's AgentRun record and is never represented by this type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceAccess {
    ReadOnly,
    ReadWrite,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceIsolation {
    Shared,
    GitWorktree,
}

/// An opaque Core workspace admission.  `workspace_id` is a stable identity,
/// not a local path.  A future Worker binder maps it to a local binding after
/// its own admission checks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceAdmission {
    workspace_id: WorkerWorkspaceId,
    access: WorkspaceAccess,
    isolation: WorkspaceIsolation,
}

impl WorkspaceAdmission {
    pub fn admit(
        workspace_id: WorkerWorkspaceId,
        access: WorkspaceAccess,
        isolation: WorkspaceIsolation,
    ) -> Result<Self, RuntimeContextAdmissionError> {
        if workspace_id.as_str().is_empty() {
            return Err(RuntimeContextAdmissionError::WorkspaceInvalid("id"));
        }
        Ok(Self {
            workspace_id,
            access,
            isolation,
        })
    }

    pub fn from_worker_id(
        workspace_id: &str,
        access: WorkspaceAccess,
        isolation: WorkspaceIsolation,
    ) -> Result<Self, RuntimeContextAdmissionError> {
        let workspace_id = WorkerWorkspaceId::parse(workspace_id)
            .map_err(|_| RuntimeContextAdmissionError::WorkspaceInvalid("id"))?;
        Self::admit(workspace_id, access, isolation)
    }

    pub fn workspace_id(&self) -> &WorkerWorkspaceId {
        &self.workspace_id
    }

    pub const fn access(&self) -> WorkspaceAccess {
        self.access
    }

    pub const fn isolation(&self) -> WorkspaceIsolation {
        self.isolation
    }

    /// Bind a Worker-provided workspace identity to this Core admission.
    pub fn bind(
        &self,
        workspace_id: &WorkerWorkspaceId,
    ) -> Result<WorkspaceBinding, RuntimeContextAdmissionError> {
        if workspace_id != &self.workspace_id {
            return Err(RuntimeContextAdmissionError::WorkspaceBindingMismatch);
        }
        Ok(WorkspaceBinding {
            workspace_id: self.workspace_id.clone(),
            access: self.access,
            isolation: self.isolation,
        })
    }

    #[allow(dead_code)]
    pub(crate) fn from_agent_run(
        workspace: &AgentRunWorkspace,
        workspace_id: WorkerWorkspaceId,
    ) -> Result<Self, RuntimeContextAdmissionError> {
        workspace
            .validate()
            .map_err(|_| RuntimeContextAdmissionError::WorkspaceInvalid("record"))?;
        let access = match workspace.access.as_str() {
            "read_only" => WorkspaceAccess::ReadOnly,
            "write" => WorkspaceAccess::ReadWrite,
            _ => return Err(RuntimeContextAdmissionError::WorkspaceInvalid("access")),
        };
        let isolation = match workspace.isolation.as_str() {
            "shared" => WorkspaceIsolation::Shared,
            "git_worktree" => WorkspaceIsolation::GitWorktree,
            _ => return Err(RuntimeContextAdmissionError::WorkspaceInvalid("isolation")),
        };
        Self::admit(workspace_id, access, isolation)
    }
}

/// Allow the Core-owned admission to back the existing Worker bridge without
/// exposing its path-bearing AgentRun record.  The bridge only receives the
/// opaque binding returned here.
impl crate::remote_worker_bridge::WorkerWorkspaceBinder for WorkspaceAdmission {
    type Binding = WorkspaceBinding;

    fn bind(
        &self,
        workspace_id: &WorkerWorkspaceId,
    ) -> Result<Self::Binding, crate::remote_worker_bridge::WorkerWorkspaceBindingError> {
        WorkspaceAdmission::bind(self, workspace_id).map_err(|_| {
            crate::remote_worker_bridge::WorkerWorkspaceBindingError("workspace_binding_mismatch")
        })
    }
}

/// Opaque binding passed to a Runtime adapter.  No path or command field is
/// available, so an adapter cannot smuggle an unadmitted root through this
/// boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceBinding {
    workspace_id: WorkerWorkspaceId,
    access: WorkspaceAccess,
    isolation: WorkspaceIsolation,
}

impl WorkspaceBinding {
    pub fn workspace_id(&self) -> &WorkerWorkspaceId {
        &self.workspace_id
    }

    pub const fn access(&self) -> WorkspaceAccess {
        self.access
    }

    pub const fn isolation(&self) -> WorkspaceIsolation {
        self.isolation
    }
}

/// The complete Core-owned execution context required before dispatch.
#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeExecutionContext {
    fence: RuntimeLeaseFence,
    camp_id: String,
    agent_id: String,
    frozen_runtime: FrozenRuntimeConfig,
    workspace: WorkspaceAdmission,
    runtime_compatibility_digest: String,
}

impl RuntimeExecutionContext {
    pub fn admit(
        fence: RuntimeLeaseFence,
        camp_id: impl Into<String>,
        agent_id: impl Into<String>,
        frozen_runtime: FrozenRuntimeConfig,
        workspace: WorkspaceAdmission,
    ) -> Result<Self, RuntimeContextAdmissionError> {
        let camp_id = camp_id.into();
        let agent_id = agent_id.into();
        if !valid_opaque(&camp_id, MAX_IDENTITY_LEN) {
            return Err(RuntimeContextAdmissionError::IdentityInvalid("camp"));
        }
        if !valid_opaque(&agent_id, MAX_IDENTITY_LEN) {
            return Err(RuntimeContextAdmissionError::IdentityInvalid("agent"));
        }
        Ok(Self {
            fence,
            camp_id,
            agent_id,
            frozen_runtime,
            workspace,
            runtime_compatibility_digest: String::new(),
        })
    }

    /// Attach the Core-computed Fleet compatibility digest after workspace,
    /// permissions, and other process inputs have been projected.  The
    /// default context remains usable for admission tests; Fleet conversion
    /// falls back to the frozen binding digest until this is supplied.
    pub fn with_runtime_compatibility_digest(
        mut self,
        digest: impl Into<String>,
    ) -> Result<Self, RuntimeContextAdmissionError> {
        let digest = digest.into();
        if !valid_opaque(&digest, MAX_FINGERPRINT_LEN) {
            return Err(RuntimeContextAdmissionError::FrozenRuntimeInvalid(
                "runtime_compatibility_digest",
            ));
        }
        self.runtime_compatibility_digest = digest;
        Ok(self)
    }

    pub fn agent_run_id(&self) -> &str {
        self.fence.agent_run_id()
    }

    pub const fn execution_epoch(&self) -> i64 {
        self.fence.execution_epoch()
    }

    pub fn camp_id(&self) -> &str {
        &self.camp_id
    }

    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    pub fn adapter_kind(&self) -> AdapterKind {
        self.frozen_runtime.adapter_kind()
    }

    pub fn frozen_runtime(&self) -> &FrozenRuntimeConfig {
        &self.frozen_runtime
    }

    pub fn workspace(&self) -> &WorkspaceAdmission {
        &self.workspace
    }

    pub fn runtime_compatibility_digest(&self) -> &str {
        if self.runtime_compatibility_digest.is_empty() {
            self.frozen_runtime.binding_compatibility_digest()
        } else {
            &self.runtime_compatibility_digest
        }
    }

    pub fn lease_fence_matches(
        &self,
        agent_run_id: &str,
        execution_epoch: i64,
        lease_token: &str,
    ) -> bool {
        self.fence
            .matches(agent_run_id, execution_epoch, lease_token)
    }

    pub fn bind_workspace(
        &self,
        workspace_id: &WorkerWorkspaceId,
    ) -> Result<WorkspaceBinding, RuntimeContextAdmissionError> {
        self.workspace.bind(workspace_id)
    }

    /// Convert the admitted context to the existing Fleet request.  The Fleet
    /// process/Host types remain private and this method does not launch one.
    #[allow(dead_code)]
    pub(crate) fn fleet_acquire_request(&self) -> FleetAcquireRequest {
        FleetAcquireRequest {
            agent_run_id: self.agent_run_id().to_owned(),
            execution_epoch: self.execution_epoch(),
            adapter_kind: self.adapter_kind(),
            compatibility: RuntimeCompatibilityKey::workspace(
                self.camp_id.clone(),
                self.agent_id.clone(),
                self.workspace.workspace_id().as_str().to_owned(),
                self.runtime_compatibility_digest().to_owned(),
            ),
        }
    }

    /// Turn a terminal callback into a Core-owned observation only after the
    /// callback proves the complete lease fence.  The returned observation is
    /// not serializable and can only be settled through the sink below.
    pub fn observe_terminal(
        &self,
        agent_run_id: &str,
        execution_epoch: i64,
        lease_token: &str,
        outcome: RuntimeTerminalOutcome,
        fingerprint: impl Into<String>,
    ) -> Result<RuntimeTerminalObservation, TerminalSettlementError> {
        if !self
            .fence
            .matches(agent_run_id, execution_epoch, lease_token)
        {
            return Err(TerminalSettlementError::LeaseFenceMismatch);
        }
        let fingerprint = fingerprint.into();
        if !valid_opaque(&fingerprint, MAX_FINGERPRINT_LEN) {
            return Err(TerminalSettlementError::FingerprintInvalid);
        }
        Ok(RuntimeTerminalObservation {
            fence: self.fence.clone(),
            adapter_kind: self.adapter_kind(),
            outcome,
            fingerprint,
        })
    }
}

/// A terminal observation admitted by a matching Runtime execution fence.
/// Its fence is private so a caller cannot forge a late or successor epoch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeTerminalObservation {
    fence: RuntimeLeaseFence,
    adapter_kind: AdapterKind,
    outcome: RuntimeTerminalOutcome,
    fingerprint: String,
}

impl RuntimeTerminalObservation {
    pub fn agent_run_id(&self) -> &str {
        self.fence.agent_run_id()
    }

    pub const fn execution_epoch(&self) -> i64 {
        self.fence.execution_epoch()
    }

    pub const fn adapter_kind(&self) -> AdapterKind {
        self.adapter_kind
    }

    pub const fn outcome(&self) -> RuntimeTerminalOutcome {
        self.outcome
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// The complete lease token that admitted this terminal observation.
    /// Queue-owned persistence uses this to reject a late callback from a
    /// successor lease without exposing the token on a wire type.
    pub fn lease_token(&self) -> &str {
        self.fence.lease_token()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TerminalSettlementError {
    LeaseFenceMismatch,
    FingerprintInvalid,
    ConflictingTerminal,
}

impl fmt::Display for TerminalSettlementError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::LeaseFenceMismatch => "lease_fence_mismatch",
            Self::FingerprintInvalid => "terminal_fingerprint_invalid",
            Self::ConflictingTerminal => "conflicting_terminal",
        })
    }
}

impl std::error::Error for TerminalSettlementError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalSettlementReceipt {
    pub agent_run_id: String,
    pub execution_epoch: i64,
    pub adapter_kind: AdapterKind,
    pub outcome: RuntimeTerminalOutcome,
    pub idempotent: bool,
}

/// Core-owned terminal settlement interface.  The real implementation can
/// project the observation into the domain transaction; this in-memory owner
/// provides deterministic idempotency and conflict behavior for the seam.
pub trait TerminalSettlementSink {
    fn settle(
        &mut self,
        observation: RuntimeTerminalObservation,
    ) -> Result<TerminalSettlementReceipt, TerminalSettlementError>;
}

#[derive(Clone, Debug, Default)]
pub struct InMemoryTerminalSettlement {
    settled: BTreeMap<(String, i64, String), RuntimeTerminalObservation>,
}

impl InMemoryTerminalSettlement {
    pub fn is_settled(&self, agent_run_id: &str, execution_epoch: i64) -> bool {
        self.settled
            .keys()
            .any(|(run_id, epoch, _)| run_id == agent_run_id && *epoch == execution_epoch)
    }
}

impl TerminalSettlementSink for InMemoryTerminalSettlement {
    fn settle(
        &mut self,
        observation: RuntimeTerminalObservation,
    ) -> Result<TerminalSettlementReceipt, TerminalSettlementError> {
        let key = (
            observation.fence.agent_run_id.clone(),
            observation.fence.execution_epoch,
            observation.fence.lease_token.clone(),
        );
        // A lease token is part of the fence even when a buggy caller
        // reuses an execution epoch.  Never let a successor token settle a
        // second terminal outcome for that same epoch.
        if self.settled.keys().any(|existing_key| {
            existing_key.0 == key.0 && existing_key.1 == key.1 && existing_key.2 != key.2
        }) {
            return Err(TerminalSettlementError::LeaseFenceMismatch);
        }
        if let Some(existing) = self.settled.get(&key) {
            if existing.adapter_kind != observation.adapter_kind
                || existing.outcome != observation.outcome
                || existing.fingerprint != observation.fingerprint
            {
                return Err(TerminalSettlementError::ConflictingTerminal);
            }
            return Ok(TerminalSettlementReceipt {
                agent_run_id: existing.agent_run_id().to_owned(),
                execution_epoch: existing.execution_epoch(),
                adapter_kind: existing.adapter_kind,
                outcome: existing.outcome,
                idempotent: true,
            });
        }
        let receipt = TerminalSettlementReceipt {
            agent_run_id: observation.agent_run_id().to_owned(),
            execution_epoch: observation.execution_epoch(),
            adapter_kind: observation.adapter_kind,
            outcome: observation.outcome,
            idempotent: false,
        };
        self.settled.insert(key, observation);
        Ok(receipt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_profile::{AdapterPermissionConfig, ResolvedModelSelection};
    use serde_json::json;

    fn frozen_runtime(kind: AdapterKind) -> FrozenAgentRuntimeConfig {
        FrozenAgentRuntimeConfig {
            custom_api: None,
            camp_fast: None,
            adapter_kind: kind,
            installation_id: "install-1".into(),
            installation_generation: 1,
            search_environment_generation: 1,
            executable_path: "/private/runtime".into(),
            auth_scope: "account".into(),
            reported_version: Some("1.0.0".into()),
            executable_fingerprint: "sha256:exe".into(),
            capabilities: vec![],
            protocol_version: "1".into(),
            model: ResolvedModelSelection {
                source: "runtime_default".into(),
                model_id: "default".into(),
                options: json!({}),
            },
            permissions: AdapterPermissionConfig {
                adapter_kind: kind,
                schema_version: 1,
                values: json!({}),
            },
            native_session_compatibility_key: None,
            binding_compatibility_digest: "sha256:binding".into(),
            host_config_digest: "sha256:host".into(),
            config_digest: "sha256:config".into(),
        }
    }

    fn context() -> RuntimeExecutionContext {
        RuntimeExecutionContext::admit(
            RuntimeLeaseFence::new("run-1", 7, "lease-1").unwrap(),
            "camp-1",
            "agent-1",
            FrozenRuntimeConfig::freeze(frozen_runtime(AdapterKind::CodexCli)).unwrap(),
            WorkspaceAdmission::from_worker_id(
                "main",
                WorkspaceAccess::ReadOnly,
                WorkspaceIsolation::Shared,
            )
            .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn context_maps_epoch_fence_workspace_and_adapter_to_fleet_request() {
        let base = context();
        assert_eq!(base.execution_epoch(), 7);
        assert_eq!(base.adapter_kind(), AdapterKind::CodexCli);
        assert!(!format!("{:?}", base.frozen_runtime()).contains("/private/runtime"));
        let binding = base
            .bind_workspace(&WorkerWorkspaceId::parse("main").unwrap())
            .unwrap();
        assert_eq!(binding.workspace_id().as_str(), "main");
        assert_eq!(binding.access(), WorkspaceAccess::ReadOnly);
        let request = base.fleet_acquire_request();
        assert_eq!(request.agent_run_id, "run-1");
        assert_eq!(request.execution_epoch, 7);
        assert_eq!(request.adapter_kind, AdapterKind::CodexCli);
        assert_eq!(
            request.compatibility.runtime_compatibility_digest,
            "sha256:binding"
        );

        let context = context()
            .with_runtime_compatibility_digest("sha256:workspace")
            .unwrap();
        assert_eq!(
            context
                .fleet_acquire_request()
                .compatibility
                .runtime_compatibility_digest,
            "sha256:workspace"
        );
    }

    #[test]
    fn stale_epoch_or_workspace_cannot_bind() {
        let context = context();
        assert!(!context.lease_fence_matches("run-1", 6, "lease-1"));
        assert_eq!(
            context.bind_workspace(&WorkerWorkspaceId::parse("other").unwrap()),
            Err(RuntimeContextAdmissionError::WorkspaceBindingMismatch)
        );
    }

    #[test]
    fn terminal_settlement_is_fenced_idempotent_and_conflict_safe() {
        let context = context();
        let mut sink = InMemoryTerminalSettlement::default();
        let observation = context
            .observe_terminal(
                "run-1",
                7,
                "lease-1",
                RuntimeTerminalOutcome::Succeeded,
                "sha256:terminal",
            )
            .unwrap();
        let first = sink.settle(observation.clone()).unwrap();
        assert!(!first.idempotent);
        let replay = sink.settle(observation).unwrap();
        assert!(replay.idempotent);
        let conflicting = context
            .observe_terminal(
                "run-1",
                7,
                "lease-1",
                RuntimeTerminalOutcome::Failed,
                "sha256:terminal",
            )
            .unwrap();
        assert_eq!(
            sink.settle(conflicting),
            Err(TerminalSettlementError::ConflictingTerminal)
        );
        let successor_token = RuntimeLeaseFence::new("run-1", 7, "lease-2").unwrap();
        let successor = RuntimeTerminalObservation {
            fence: successor_token,
            adapter_kind: AdapterKind::CodexCli,
            outcome: RuntimeTerminalOutcome::Succeeded,
            fingerprint: "sha256:other".into(),
        };
        assert_eq!(
            sink.settle(successor),
            Err(TerminalSettlementError::LeaseFenceMismatch)
        );
        assert!(sink.is_settled("run-1", 7));
    }
}
