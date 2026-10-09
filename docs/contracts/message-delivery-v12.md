---
document_type: protocol-contract
contract: message-delivery
version: 12
status: accepted
authority: public-message-delivery-route-reconciliation
source_version: v1.72
last_updated: 2026-10-09
---

# Message Delivery v12

继承 [v11](message-delivery-v11.md) 的单一 FIFO、独立续做批次、原输入集合、路由与事件唤醒。
按 [Continuation v3](agent-run-continuation-v3.md)，续做的 message 外键是不可公开读取的空载体，
不再是公开系统消息。原 queue_sequence 保持排队顺序，来源记录识别此例外。
载体被隐藏不取消请求；原业务输入被删除／撤回、成员或 Task 失效仍按原准入取消。
普通 Delivery 仍排除 tombstone，读模型保留合法续做的 waiting 状态。
