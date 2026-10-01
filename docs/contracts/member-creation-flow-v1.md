---
document_type: protocol-contract
contract: member-creation-flow-v1
authority: member-creation-entry-and-presentation-receipt
status: accepted
version: 1
source_version: v1.72
last_updated: 2026-10-02
---

# Member Creation Flow v1

## Entry and helper

The roster's primary Add action reads `threads.creationPreflight` and shared general preferences. Select exactly one
present member with a saved Runtime and readiness `ready` or `light_ready`, in this order:

1. `lastMemberCreationHelperAgentId`, if available;
2. the saved new-conversation Default Lead, if available;
3. the first available member in stable `memberOrder` order.

Availability is checked before priority. With no available member, open the existing manual creation form. A transport
failure reports an error rather than claiming no member is available. The split button's manual action opens that form
directly. Each AI entry creates a new pending quick-chat Thread containing only the helper, who is also its Default Lead.
It uses ordinary conversation controls, delivery and permissions. It does not resume an earlier creation Thread, inject
hidden instructions, change model context, configure Runtime, or invite the created member into this Thread.

The three localized starters fill editable user text without sending. Their scenarios are adapting a favorite character,
defining a work partner by responsibilities and collaboration style, and exploring an original companion.
[Pending Camp Activation v3](pending-camp-activation-v3.md) owns the window-local draft and first-send boundary.

## Immutable success receipt

Only an applied authenticated `member.create` records a receipt. Profile creation, the receipt, the last-successful-helper
preference and the existing command result commit in the same Domain Command Gateway transaction. Failure rolls them
back together; idempotent replay returns the original result without another receipt or preference update. Manual
`members.create` does not manufacture an AI receipt. Existing confirmation, authorization and creation-key checks remain.

`MemberCreationView` contains `creationId`, `agentId`, `displayName`, `avatarRef`, `teamRole`,
`professionalResponsibilities`, `personalityTraits`, `creatorAgentId`, `creatorDisplayName` and `createdAt`.
Identity and helper name are snapshots at creation time, not joins to current profiles. This is presentation data;
it adds no CampMessage, Thread member, tool-output field, ContextManifest field or bootstrap instruction.

Migration 179 admits exactly v1.72/schema 128 and atomically advances to schema 129. `member_creation` stores one JSON
snapshot per command identity with a Thread foreign key and `(camp_id, created_at, creation_id)` index. Thread deletion
cascades its receipts. `member_creation_preference` is an instance-wide singleton helper ID, retained independently of
Thread deletion; a deleted or unavailable helper is skipped by entry preflight. Existing profiles are unchanged and
historical tool evidence is not backfilled into cards.

## Read and navigation

Snapshot and Open expose `memberCreations`, ordered by timestamp and creation ID, via the indexed business table in
the same read transaction. They do not scan `event_log`, inspect current member status or write during reads. See
[Camp Open Projection v25](camp-open-projection-v25.md).

After an applied creation, Core emits the existing `members.invalidated` signal and `thread.memberCreated` with
`threadId`. Desktop refreshes the matching active Thread through its existing coalesced reader; Web uses the ordinary
invalidation stream. Reopening always loads the receipt. The event itself is not a second durable source.

The joined card remains unchanged after rename, Runtime setup, departure or removal. Its sole action opens the existing
member Runtime section by `agentId`. The destination loads the roster before resolving an unknown cached selection;
away profiles remain accessible and removed profiles follow existing destination behavior. The card never polls member
status and never becomes a start-conversation action.

## Verification owners

- Existing `team_tool` member-create transaction test owns rollback, direct-user authorization, idempotent replay,
  no added public message/membership, immutable snapshots and Open projection.
- `db_member_creation` owns the new schema 128 to 129 boundary, rollback and profile preservation.
- Renderer helper/navigation tests own deterministic selection and local draft overlay.
- `pnpm test:member-creation` exercises production conversation/member surfaces with isolated transport and native input;
  it does not qualify a real model or physical mobile device.
