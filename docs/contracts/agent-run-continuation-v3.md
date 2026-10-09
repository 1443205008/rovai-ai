---
document_type: protocol-contract
contract: agent-run-continuation-v3
authority: user-authorized-independent-run-continuation
status: accepted
version: 3
source_version: v1.72
last_updated: 2026-10-09
---

# AgentRun Continuation v3

继承 [v2](agent-run-continuation-v2.md) 的授权、幂等、FIFO、cleanup、输入范围和自动会话选择，
替代 v1 中公开系统操作消息的要求。User 点击继续只产生内部授权和 waiting Delivery。

## 内部操作与公开读取

新请求不发布 `camp_message.sent`、不生成系统文案或消息变更通知。
现有 Delivery 非空 message 外键仍引用内部载体：空 body、空 structured content、
system/run-continuation、origin_kind=system，创建时即设置 tombstoned_at，不进入 FTS 或引用索引。
该载体的 ID 仍保留在原命令回执中，仅用于内部定位，不是可读取的公开消息。

Desktop/Web 消息列表、分页、定位、会话查找、Agent 的 thread.read（timeline/item/replyChain）、
thread.search、history.search 和 historyHint 额外消息判断均沿既有 tombstone 规则排除这些记录。
既有其他 system 消息不受影响；waiting Delivery 和新 Run 仍在执行区独立可见。

队列读取仅对已具备 camp_run_continuation 来源事实的请求允许隐藏载体，
并照常重验原 Run 的业务输入、Task 和成员资格。新 Run 只领取原业务输入；载体永不成为 RUN_INPUT。
内部序号维持原 FIFO。新 claim 的公开边界使用仍可见的消息尾及已接受历史边界的较大值，
避免内部载体推进新公共边界，并保持已接受边界单调。

## 历史记录与证据

按 User 最新要求，仅改变新请求；已生成的公开记录保留原样，不批量删除、隐藏或改写索引。
不增加 migration 或 schema 版本。原事件、队列／来源外键、旧 Run、冻结 Manifest 和工具回执保留原字节。
未领取的旧续做请求继续排队，不重新授权、不重复发布。
模型模板、字段、预算和版本轴均不变，不清空原生会话；历史中已经实际投递的内容不能追溯撤回。
范围与前后对照见 [r2](../versions/v1.72/model-context-change-quiet-continuation.md)。
