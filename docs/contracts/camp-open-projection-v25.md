---
document_type: protocol-contract
contract: camp-open-projection-v25
authority: camp-open-member-creation-receipts
status: accepted
version: 25
source_version: v1.72
last_updated: 2026-10-02
---

# Camp Open Projection v25

Inherits [v24](camp-open-projection-v24.md), including Open schema 8, bounded Run/message windows, Run input summary,
empty Execution Evidence, independent watermarks and zero-write reads. Adds `memberCreations` to both Snapshot and Open.
Older projections without this additive field render no joined cards.

[Member Creation Flow v1](member-creation-flow-v1.md) owns receipt fields, immutability, transaction, retention and
invalidation. The read selects only the requested Thread's indexed `member_creation` rows, in `(created_at, creation_id)`
order, without profile joins, event-log reconstruction or filesystem access. Receipt count follows that Thread's successful
AI creations and does not expand message, Run or Evidence windows. Unrelated Threads' history cannot increase this read's
scan range. Existing Snapshot/Open schema numbers and model-facing tool responses remain unchanged.
