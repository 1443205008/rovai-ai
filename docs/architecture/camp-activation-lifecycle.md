---
document_type: architecture
authority: camp-activation-component-boundary
status: accepted
last_updated: 2026-09-18
---

# Camp Activation Lifecycle

## Component authority

| Component | Responsibility |
| --- | --- |
| Rust Host / Core | Stores the shared one-click preference and saved default member/Lead configuration under the instance data root; see [Host Web v2](../contracts/host-web-v2.md#shared-creation-preferences). |
| Electron Main | Imports legacy creation preferences once through Core and adapts local presentation preferences; it does not store Camp activation or public Composer content. |
| Renderer | Chooses `pending` for a valid one-click entry and `active` for the explicit Dialog. A pending Camp's input stays Renderer-local; AI member-creation drafts may survive surface switches in the same window. |
| Core collaboration service | Validates creation structure, persists Camp activation, guards pre-activation mutation/discard, and activates a Pending Camp in the first accepted public-message transaction. |
| Navigation / Read Model | Lists Active Camps. Pending Camps have no Core navigation/restoration row. Renderer may overlay meaningful AI member-creation drafts within one window. |
| SQLite startup recovery | Removes Pending Camps that still satisfy the empty initial-state predicate. It never reconstructs public Composer input. |

## State flow

```text
one-click entry
  -> camps.create(pending)
  -> current Renderer opens an empty local Composer
     -> send rejected/failed: local input remains in that mounted Renderer
     -> send accepted transaction:
          Active + camp.activated + CampMessage + waiting Deliveries
     -> switch / refresh / close before send:
          local input is not persisted or restored
          empty Pending Camp remains hidden and is eligible for guarded cleanup

explicit Dialog
  -> camps.create(active)
  -> durable zero-message Camp
```

The first accepted send is the only Pending-to-Active transition. It validates the one frozen local send snapshot and,
in one Core transaction, activates the Camp, publishes the message and creates target Deliveries. A rejection or
rollback leaves the Camp Pending and does not clear the mounted Renderer input.

Renderer navigation may wait for an already-started local quote or source-attachment operation to settle before
unmounting. That wait is not autosave, a durable leave guard or a restoration promise. Public Composer state is never
written to Core merely because the user switches Camps, refreshes, closes a window or exits the App.

## Invariants

- A Pending Camp cannot have an accepted public message; its first accepted public send activates it atomically.
- Active never transitions back to Pending.
- Unsent Renderer content never makes a Pending Camp durably navigable or restorable; the AI member-creation overlay is window-local.
- `camps.discardPending` and startup cleanup may delete only an otherwise untouched empty Pending Camp.
- Single Chat's private Draft/Pending lifecycle is independent and unchanged.

## References

- [Camp lifecycle invariants](foundational-invariants.md#camp-lifecycle)
- [Pending Camp Activation v3](../contracts/pending-camp-activation-v3.md)
- [Camp Composer Draft v15](../contracts/camp-composer-draft-v15.md)

## AI 队员创建

名册入口选择一位当前可用协助者后复用普通 Pending Thread。BusinessApp 持有这个入口创建的窗口内草稿 map；
有输入时仅合并到 Renderer 的侧栏投影，同窗口切换可恢复，刷新与退出不恢复。上方普通一键入口的切换规则不变。
Core 仍只导航 Active Thread，首条接受的发送仍是唯一激活事务。选择与状态边界见
[Member Creation Flow v1](../contracts/member-creation-flow-v1.md)及[Pending Camp Activation v3](../contracts/pending-camp-activation-v3.md)。

Member Studio 调用现有 AgentProfile Gateway，在同一创建事务中保存静态入队回执和最近成功协助者。
ReadModel 只读业务回执表，Renderer 只展示创建时身份；当前 Runtime、Presence 与资料存续由跳转后的队员页处理。
回执不成为消息或模型上下文，也不引入另一条身份创建权限路径。
