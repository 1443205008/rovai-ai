---
document_type: version-decisions
version: v1.72
authority: decision-rationale
lifecycle: current
last_updated: 2026-09-27
---

# v1.72 版本决定

<a id="v1-72-d01"></a>
## V1.72-D01：Lark 作为独立 provider，克隆表族并参数化飞书实现

- 状态：accepted
- 日期：2026-09-24
- 当前权威：Lark Channel v1、Feishu Channel v17 与 Lark 渠道架构

### 背景

飞书渠道从设计之初就在 Core、可信域、发布器和合同中预留了 `brand=lark`，但登录入口固定为飞书账号站，运行期
长连接也没有按品牌传入 SDK 域，Lark 从未真正可用。`feishu_account` 上的 connected 唯一索引使同一时间只能连接一个
开发者账号，只要飞书与 Lark 共用该表族，两者就必然互斥。Principal 明确要求飞书与 Lark 作为两个独立渠道同时使用。

### 选择

新增 provider `lark`，拥有 5 张与飞书表结构等价的 `lark_*` 表、独立 Host 组件、8 个领域命令类型和 20 个初始请求名（D03 扩至 21 个）；
可信域、登录配置与 SDK 域按 provider 分离，飞书 provider 收窄为只接受飞书品牌。实现上不复制飞书代码：Core 领域逻辑
以 `ChannelProviderSpec` 参数化表名与 provider 常量，Main 以 Provider Profile 创建同一渠道服务类的第二个实例。
Core 只从请求名推导 Host actor。

### 后果

- 需要一次行为不变的参数化重构，覆盖飞书领域 SQL 中的表名字面量与 provider 常量；重构先于 Lark 接入独立合入。
- 三张在 CHECK 中写死 provider 值域的中立表必须重建；两个目录视图增加 Lark 分支。
- 同一队员可以同时拥有飞书与 Lark Bot；两家账号、Bot、会话和失败互不影响。
- 飞书 provider 不再接受 `larksuite.com`，理论上的历史 `brand=lark` 飞书行只保留可读，不自动迁移。
- 登录与控制台协议在 Lark 站点的一致性只能由真实租户证明，验收前产品不宣称支持 Lark。

### 未选择方案

- **在飞书 provider 内增加品牌选项**：改动最小，但受单 connected 账号约束，飞书与 Lark 只能二选一，不满足要求。
- **飞书表族增加 provider 列并改为按 provider 唯一**：迁移较小，但每条既有 SQL 都要补 provider 谓词，遗漏一处就会
  跨 provider 读写，隔离从结构保证退化为逐条人工保证。
- **复制飞书 Core 与 Main 代码并改名**：隔离清晰，但产生上万行重复实现，两家行为会逐步漂移。
- **中立请求增加 provider 参数代替独立请求名**：请求数更少，但会改变飞书既有请求合同，并让 actor 由 payload
  决定；按请求名推导 actor 与钉钉现有模式一致，请求面保持封闭可审计。

<a id="v1-72-d02"></a>
## V1.72-D02：导航摘要放入 Camp，范围失效配合完整快照恢复

- 状态：accepted
- 日期：2026-09-27
- 当前权威：[侧栏刷新架构](../../architecture/desktop-navigation-refresh.md)、[Navigation Read v1](../../contracts/navigation-read-v1.md)

### 背景

切换会话触发重复全局读取，导航仍聚合历史事件、全量载入会话后截取。单纯延迟刷新不能消除队列等待。
Principal 接受按会话/分组缩小范围，同时明确禁止新增持久化业务表和复杂同步机制。主线的通知与 Lark 迁移已占用
175 和 176；导航摘要独立顺延为 Migration 177。

### 选择

复用 camp 保存必要摘要、camp_view_state 保存已读、Core 事务保证一致性。在线提示声明行或分组；缺少可信
基础/变化范围时从摘要重新读取完整快照。活动序号按首次发布的用户消息更新，未读回复序号按首次发布、未撤回的
Agent 消息更新；Run 终态本身不产生新回复标记。

### 后果与替代方案

需要一次历史回填和摘要写入维护；正常读取不再付出历史聚合成本。不采用独立导航/分组变化表、删除补齐记录、
保留期或通用增量同步，避免双份状态与恢复协议。保留现有聚焦/低频完整性兜底；不引入优先级队列或独立数据库连接。

<a id="v1-72-d03"></a>
## V1.72-D03：Lark 入站附件复用持久下载队列并保留独立 Host 完成请求

- 状态：accepted
- 日期：2026-09-27
- 当前权威：[Lark Channel v1](../../contracts/lark-channel-v1.md#入站附件)、
  [Channel Message Bridge v1](../../contracts/channel-message-bridge-v1.md#inbound-attachments)与
  [Lark 渠道架构](../../architecture/lark-channel.md#共享而不复制)

### 背景

Lark 已有独立 Host 和 `Domain.Lark` 的 SDK 客户端，但入站观察仅保存附件摘要。飞书与钉钉后来新增的持久下载
链路不能直接复用飞书完成请求名：该请求名会被赋予飞书 Host 身份，Core 会拒绝 Lark 的待下载 Request。
因此只在 Host 打开资源描述会让下载长期停在队列，无法把文件交给 Agent。

### 选择

Lark 复用现有资源提取、下载器、等待队列、Source Ref 和失败提示；仅增加 Lark 专属完成请求名。
Core 从该封闭请求名赋予 `lark-channel-host`，继续以 provider、App、绑定和尝试代数校验完成结果。
Lark 文件导入 Camp 的 `lark/` 子目录，消息就绪后才可派发。

### 后果

- Lark 入站图片与普通文件由摘要升级为可读取的本地 Source Ref；文件夹与贴纸仍走整条消息失败提示。
- Lark Host 与飞书 Host 的下载候选、完成权限和文件目录保持隔离。
- 真实 Lark 租户的资源权限与客户端文件行为仍需单独验收；自动化测试不能解除该能力 gate。

### 未选择方案

- **复用飞书完成请求名**：请求名决定飞书 Host 身份，会破坏 provider 隔离并导致 Lark Request 无法完成。
- **另建 Lark 下载表和调度器**：可隔离状态，但重复了已有的 provider 分区、重试和 FIFO 语义，增加两套状态漂移风险。

<a id="v1-72-d04"></a>
## V1.72-D04：钉钉入站按平台签名链接下载，终态失败撤回排队卡

- 状态：accepted
- 日期：2026-09-27
- 当前权威：[Channel Message Bridge v1](../../contracts/channel-message-bridge-v1.md#inbound-attachments)

### 背景

真实钉钉租户的两条图片消息已经进入 Rovai，但下载三次失败，Agent 未执行，用户仍看到两张“进行中”排队卡。
对同一条失败请求进行只读核对：下载码兑换成功，平台返回 HTTP 签名 OSS 链接，图片内容可读取；
Host 的 HTTPS 限制在请求文件前拒绝该链接。原终态失败只删除尚未发送的排队确认，已发送卡片没有撤回任务。

### 选择

钉钉入站使用下载接口实际返回的 HTTP 或 HTTPS 签名链接，不改写协议；App 访问令牌仍仅用于兑换接口，
不转发给文件存储。所有 queued Request 的终态失败共用排队确认收口：未发出的确认删除，已发出的确认排队撤回，
再发送可见失败提示；钉钉失败提示卡使用终态失败状态。飞书、Lark 的成功准入和失败收口保持同一语义。

### 后果与替代方案

钉钉返回 HTTP 链接时文件下载使用 HTTP；这是本次真实租户可用性取舍。未采用仅把链接升级到 HTTPS：
当前 OSS 样本升级后可访问，但平台没有保证所有签名链接和存储域在改写协议后均有效。旧的两条请求已终态失败，
代码修复不会自动重新执行，需要用户发送新消息验收。
