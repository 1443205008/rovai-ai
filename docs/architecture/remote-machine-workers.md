---
document_type: architecture
architecture: remote-machine-workers
authority: remote-worker-protocol-and-rollout
status: proposed
last_updated: 2026-10-09
---

# Remote Machine Workers

## Goal

Keep the existing Rovai Host/Core/Runtime Fleet as the execution engine while
adding a machine-level Worker that can run on another computer. The Worker is
the only component allowed to translate a remote task into a local Agent
Runtime process. The central Host never receives arbitrary shell commands or a
remote filesystem path.

## Boundaries

- `Host/Core` owns Camps, AgentRuns, scheduling, authorization, task identity,
  event ordering, and the machine registry.
- `Worker` owns local Agent discovery, workspace allowlists, process lifecycle,
  local approvals, and redaction of machine-local details.
- `Runtime Adapter` remains local to a Worker. Existing Codex, Claude, Pi, ACP,
  and other adapters are not changed to know about SSH or another machine.
- `Tailscale`, mTLS, or a reverse proxy provides transport security; it is not
  itself a task protocol.

## Protocol v1

The portable wire types live in `rovai_core::remote_worker`.

- Registration advertises a stable `worker_id`, machine name, labels, platform,
  capacity, installed Agents, models, and named workspace allowlists.
- Heartbeats carry status, running task IDs, load, and an observation timestamp.
- Task requests carry an idempotent `task_id`, target Worker, Agent kind,
  workspace ID, prompt, permission tier, timeout, and attempt number.
- Worker events carry a monotonic sequence and message ID. The central side
  deduplicates by `(worker_id, sequence)` and never replays a task solely
  because a connection was lost.
- Approval, file-change, completion, failure, and cancellation are explicit
  events. Raw credentials, arbitrary absolute paths, and shell text are not
  protocol fields.

## Current implementation status

The durable queue module (`remote_task_queue`) is currently a queue-owned persistence
seam and test fixture only. It has no enrolled Core migration, authenticated Worker
transport, or correlated Domain Command handler. Its terminal observations therefore
remain in the queue’s own tables; they must not be treated as `AgentRun` state, and
no queue event is allowed to best-effort overwrite `agent_run` or `event_log`. A
future integration must bind `task_id + attempt + execution_epoch + lease_token`
through the existing Core command transaction before claiming the M5/M6 rollout
steps below.

## Rollout

1. M1: ship the versioned types, validation, and registry persistence.
2. M2: add authenticated Worker registration and heartbeat endpoints.
3. M3: add a Worker poll/stream loop and a read-only task execution fixture.
4. M4: bind real local Runtime Adapters, stream output, cancel, and recover.
5. M5: route existing AgentRuns through local or remote Workers using the same
   state machine and audit records.
6. M6: add UI for machine capabilities, target selection, health, approvals,
   and task history.

## Security gates

- Pairing is one-time; the Worker then uses a machine credential that can be
  revoked independently of the Owner login token.
- Workspace IDs are allowlisted on the Worker; the central side never submits
  a path that bypasses that mapping.
- Every task and event is authenticated, bounded, idempotent, and auditable.
- Disconnects move a Worker to `offline`; they do not imply task completion.
- Elevated permission requires an explicit approval event and is disabled by
  default.
