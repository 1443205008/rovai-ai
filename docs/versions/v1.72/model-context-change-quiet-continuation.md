---
document_type: model-context-change
version: v1.72
revision: 2
confirmation_status: confirmed
confirmed_revision: 2
confirmed_by: local_user
confirmed_at: 2026-10-10
confirmation_message_id: d37611df-603b-4a2e-9050-e5c6183b8305
last_updated: 2026-10-10
---

# 续做操作退出公开消息 r2

本说明记录 Thread 中已逐项讨论并授权的边界：User 要求界面和消息历史均移除续做系统消息；
答复明确提出内部授权／排队／审计保留、新请求不进入历史与搜索、historyHint 不受操作记录影响。
User 已授权实施、PR 合入 main 并安装本机，并进一步明确“已生成的记录不需要批量删除，功能还没发布版本”。
r2 因此收窄为仅处理新请求；本文是该已确认范围的技术展开，不声称 User 此前阅读过此文件。

## 变更前

点击继续会保存并发布 system/run-continuation 消息，正文为
`你继续了爱丽丝的执行。`（强制新会话为 `你使用新会话继续了爱丽丝的执行。`）。
Desktop/Web 历史、thread.read 的 timeline/item/replyChain、thread.search 与 history.search 可见。
该消息不属于 RUN_INPUT，但会令 claim 的额外公共消息 EXISTS 为真。

只有原业务消息 m1 和续做记录时，本轮输入为：

```text
[RUN_FACTS]
{"attachmentOutputRoot":"/workspace/attachments/thread-example","historyHint":"The latest public message before your last recorded run in this Thread had sequence 1. As of this run's start, there are additional visible messages after that sequence beyond RUN_INPUT and messages written by you."}
[/RUN_FACTS]
[RUN_INPUT]
{"messages":[{"body":"检查导出流程","messageId":"m1","senderId":"local_user","senderType":"user","sequence":1,"mentions":[]}]}
[/RUN_INPUT]
```

## 变更后

相同输入及条件的完整两个 section：

```text
[RUN_FACTS]
{"attachmentOutputRoot":"/workspace/attachments/thread-example","historyHint":"The latest public message before your last recorded run in this Thread had sequence 1. As of this run's start, all visible messages after that sequence are already in RUN_INPUT or were written by you."}
[/RUN_FACTS]
[RUN_INPUT]
{"messages":[{"body":"检查导出流程","messageId":"m1","senderId":"local_user","senderType":"user","sequence":1,"mentions":[]}]}
[/RUN_INPUT]
```

不发布新的续做公屏消息，不向模型附加“继续”。新请求的内部续做记录不参与界面消息、
消息分页／定位／搜索、Agent 历史读取／搜索或额外消息判断。仍有其他可见消息时继续使用原来的 true 提示。
消息返回 shape、分页预算、withdrawn 占位与其他 system 消息均保留；已生成的公开续做记录也保留原样。
排队和新 Run 仍从原执行区观察；内部操作记录不作为业务输入或消息预览。

## 明确不变

Bootstrap、Charter、成员、Task、Skills、原业务输入集合、消息字段、预算和序列化模板保持。
Formatter/Manifest 33、Run Facts 9、Profile 10、Agent Output 10、CLI/Transport 36 保持；
这是内部操作退出公共读取集合，未增加字段或改变序列化与证据编码。
既有冻结 Manifest、Runtime 输入、工具回执和 Native Session 历史不重写、不清空、不补发。
幂等、FIFO、cleanup、会话选择、取消响应和多次独立续做沿用原合同。

## 迁移与兼容

不增加 migration 或 schema 版本，不批量删除、隐藏已生成的记录或改写其索引。
新请求沿用 Delivery 的非空外键，
只创建空正文且生来不可见的内部载体，不产生 publication event、引用索引或消息变更通知。
保留唯一 lane 与原来源记录，领取与执行队列允许该内部载体，但仍重验原业务消息资格。
内部序号继续分配以保持 FIFO；新 claim 的公共边界排除隐藏载体，且不得回退到已经接受的历史边界之前。
未 claim 的旧请求继续可调度；沿用当前 schema 137；不执行 clean break。

## 二次确认

User 消息 5e2e02ed-f24d-4ccf-b690-78846df02fff 提出移除；答复
12dc0642-0270-4dd2-b53c-74fed06ba4ac 列明上下文边界后，User 在
35e8bd36-37f6-4e2e-9d54-82a6ca25db97 明确授权完整交付，再在
d37611df-603b-4a2e-9050-e5c6183b8305 明确已有记录无需批量处理。
实现遵循这次最新收窄，不要求 User 重复批准已授权的新请求变更。

## 验证

扩展现有 delivery_queue 事务 owner，验证公开历史、搜索、分页和额外消息判断不包含载体，
普通消息及普通 system 消息仍可见，源输入与证据保持，重复点击、幂等、FIFO、cleanup 和重启仍成立。
同一 owner 复用两条 waiting 请求，并验证已有公开续做记录在重启后正文、可见性、版本和搜索结果不变；
必须用隔离 SQLite，纯函数无法覆盖 FTS 触发器与队列事务。
最小命令为 cargo test -p rovai-core --features extended-tests --lib continuation。
同环境真实 Codex 续做专项验证记录为补充证据；通用语义 Judge Gate 的覆盖单独如实报告，
不能以确定性测试或专项 Runtime 测试冒充通用 Gate。
验证总预算 90 分钟；不改模型提示词、Case 评分或原生会话恢复规则。
