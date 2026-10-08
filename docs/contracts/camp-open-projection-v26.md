---
document_type: protocol-contract
contract: camp-open-projection-v26
authority: thread-user-message-navigation
status: accepted
version: 26
source_version: v1.72
last_updated: 2026-10-08
---

# Camp Open Projection v26

继承 [v25](camp-open-projection-v25.md) 的 Open schema 8、创建回执、消息模型信息、正文分页和只读边界。
新增两个 Owner 授权的封闭读取方法，Desktop/Web 使用同一 Core 实现。旧 Snapshot、Open 和 around schema
不升版；不新增数据库表、持久导航副本、业务写入或已读确认。

## 全会话用户目录

`thread.messages.anchors({ threadId })` 返回：

```ts
interface ThreadUserAnchorIndex {
  schemaVersion: 1
  threadId: string
  throughGlobalSequence: number
  totalCount: number
  items: Array<{ messageId: string; sequence: number; title: string; messageVersion: number }>
}
```

覆盖该 Thread 所有可导航 `user` / `external_principal` 消息，排除撤回、tombstone、使命启动与非用户系统初始化。
按 `(sequence, messageId)` 升序，`totalCount` 是符合条件的用户消息数，不套用正文分页限制，不截断早期目录。
读取以当前 Thread 的消息业务表为入口，只选 ID、顺序、版本和标题所需用户字段。批量解析提及名称、附件名和引用；
不复用整套消息 hydration，不装配 Agent 回复、Run、执行记录或附件文件。

标题使用现有结构化纯文本格式化规则，空正文依次回退附件名称、引用文本和统一占位。统一空白，并沿用现有摘要的
240 Unicode scalar 传输预算，超长以末尾省略号收口。该预算不定义 UI 展示长度，界面仍单行省略。

## 首条关联回复

`thread.messages.anchorPreview({ threadId, messageId })` 返回：

```ts
interface ThreadUserAnchorPreview {
  schemaVersion: 1
  threadId: string
  messageId: string
  throughGlobalSequence: number
  sourceAvailable: boolean
  firstReply: { messageId: string; sequence: number; summary: string; messageVersion: number } | null
}
```

`sourceAvailable` 同时检查 Thread、用户作者、特殊消息和撤回／删除；false 表示问题已不可导航。
true 且 `firstReply: null` 表示此次权威读取确认没有可展示回复，独立于前端未请求、加载中、失败状态。

Core 从完整业务关联选择晚于问题、有效且最早的 Agent 消息。显式 reply 引用优先，并明确阻止其他引用通过 Run
关系再次关联。无显式引用时使用该 Run 的完整输入、anchor 和 Turn 的用户触发关系；没有 Run 时允许消息自身的
Turn 触发关系。存在但没有该输入的 Run 不退回消息自身 Turn。持续 Run 的较早输出不能回答后来才加入的用户输入。
不以时间相邻推断。先按关系与顺序确定一个回复 ID，再读取这条消息的摘要字段，沿用相同传输预算。

索引首开不计算任何回复预览。悬浮／键盘聚焦延迟 120ms 后读取，离开取消尚未启动的读取；同一目标在途合并，
只缓存访问过的结果。普通正文里碰巧出现的回复不用于认定首条回复。失败保留标题和定位能力，允许重试。

## 独立定位与正文连续区间

目标已加载且身份、版本、可导航状态仍有效时直接定位，否则使用 `thread.messages.around`。
around 的原有 `sourceAvailable` 语义不变，前端仍单独验证目标类型、撤回、删除、Thread 和精确 ID。
其新增 `nextMessageSequence: number | null` 是窗口末尾之后的下一条实际未 tombstone 消息顺序，用于判断两个读取
范围之间是否存在未加载正文；不能根据数值跳号推断缺口。

锚点只持有一份当前目标窗口，切换目标替换。正常分页的 coverage、loadedCount、complete、hasEarlier、边界和游标
只由正常连续区间维护。展示合并按消息 ID 去重，保留较新版本与已知撤回／删除状态；不改变已有引用、通知、查找窗口
的生命周期。分页按钮及用户输入驱动的自动加载都跟随正常区间入口，程序定位不启动连续翻页。有缺口时明确显示
中间消息未加载，不提供通用缺口补齐或双向无限分页。

请求绑定 Thread 和代次；只允许最后一次点击定位，切换会话、关闭或选择其他导航目标后忽略旧响应。目标不可用时
明确反馈，不定位其他消息。私有待发送输入不进入目录；公开发送回执确定 ID 和顺序后可以即时追加，再由 Core 目录替换。

## 刷新与资源边界

目录与首屏正文并行请求，缓存可先显示；局部正文不能冒充完整目录。缓存仅属于当前 Business surface/Host，最多保留
最近 8 个 Thread 的目录；退出 Thread 释放正文定位窗口与预览在途状态。

Domain Command Gateway 从本次提交新增的公开消息事件元数据产生 `thread.messages.changed`，通过现有 Host 输出链路
传递 `{ threadId, indexChanged, throughGlobalSequence, unavailableMessageIds }`。Web SSE 只投影这些封闭字段，重连或丢帧
仍使用已有 resync。只在 commit 后发出，回滚和幂等回放不重发；不新增全局失效总线、轮询或持久水位。

相关用户消息变化合并刷新目录，已知撤回／删除立即使相关导航失效；可见名称变化也刷新标题。普通回复只使当前
Thread 已访问的预览失效，包括已确认没有回复的结果，下次访问再读。不因执行状态或整个 Snapshot 更替重读目录，
`throughGlobalSequence` 只负责拒绝旧响应和入场读取水位保护。未加载历史锚点不挂载消息 DOM。
