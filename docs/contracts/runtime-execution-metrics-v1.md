---
document_type: contract
contract: runtime-execution-metrics
version: 1
status: accepted
source_version: v1.72
last_updated: 2026-09-29
---

# Runtime Execution Metrics v1

本合同把执行台的三种数据分开：当前速度属于 `AgentRun + executionEpoch` 的 Renderer 临时状态；四项原生用量属于逻辑 `AgentRun`，沿用 [Runtime Usage Monitoring v4](runtime-usage-monitoring-v4.md)；上下文属于当前原生 `Conversation / Binding / Session`。三者不互相填补。

## 读取与归属

`monitoring.execution({campId, agentRunIds})` 返回 schemaVersion 1、`runs[]` 与 `sessions[]`。最多读取 500 个指定 Run，Run 必须属于 Camp；返回的用量只取当前 Monitoring collection 的 `runtime_usage_run_summary`。每个 Run 行提供 `agentRunId`、`executionEpoch`、`promptInputTotalTokens`、`outputTokens`、`cacheReadTokens`、`cacheWriteTokens`、`finalizedAt`、`lastObservedAt`。缺失保持 `null`，运行中已到达的原生用量继续保存和读取。

`sessions[]` 只返回该 Camp 目前 `Conversation.native_binding_id + native_binding_generation + native_session_id` 均匹配的最新上下文。每行包含 `conversationId`、`agentId`、`sessionGeneration`、`runtimeKind`、`modelKey`、`usedTokens`、`windowTokens`、`source`、`dialectId`、`observedAt`。持久行还保存原生 Session ID、Runtime 版本、有效配置摘要、来源 Run/epoch。旧会话事件、旧绑定代次、较早观测不能覆盖当前值；后续 Run 的有效配置变化使旧观测在读取时失效。

只接受同一原生 Session 观测的明确 `used` 或正数 `window`；只有窗口上限时 `used` 仍为空。Codex 当前已核验的通知把 `tokenUsage.last.totalTokens` 作为最近单次模型调用的 Context used 候选，与同一通知的 `modelContextWindow` 配对；它不是实时窗口同步值。`tokenUsage.total.totalTokens`、`last.inputTokens`、Run 累计与正文估算都不能代替 used。Context 的来源身份独立于 Run Usage 的累计来源身份：消耗总量未变但 `last` 或窗口变化时仍接收新观测。ACP 原生 Gauge 在提示结束后到达时，可在短暂的原 Session owner 保留期内按原 Run／代次落盘；新 owner 绑定后不沿用旧 owner。`used > window` 的无效观测不进入当前上下文。当前绑定发生已确认压缩后，旧 gauge 在新 gauge 到达前不再显示。

## 显示

- 当前 `tok/s` 在队员名称行右侧、上下文圆环左侧显示为不可点击的纯文字，不随所展开的历史 Run 卡片切换；没有有效流式采样时隐藏。运行中的 Run 卡片保留原有执行耗时。`visible-text-heuristic-v2` 只估算公开根 Agent `agent.text.delta` 正文块的新增文本：ASCII 每 Unicode 标量 0.25、Han 每标量 0.60、其他标量 1.00。它是显示粗估，不能称作跨模型准确计数。首次看到的块只作基线；历史回放、重复快照、工具与私有思考不计数。单调时钟每 500 ms 采样，实际间隔用于速率与 2.5 秒时间常数的指数平滑；首次有效输出后至少 1 秒才显示，屏幕最多每 1 秒发布一次，5 秒无正文后隐藏，重新输出重新预热；终态立即停止采样。该值不进入原生 Usage、费用、Context 或持久化。
- 终态卡片不直接显示耗时。收到任一有效原生用量字段时，显示 `xxk` 用量入口；只有成功、Run summary 已结算、Input 与 Output 都已得到时，入口值为 `Input + Output`；Cache 是 Input 的子集，不再次相加。失败、取消及部分观测仍能在气泡查看已收到的字段，入口值为未知。完全没有用量字段时保留时钟入口，点击查看执行耗时；迟到用量落盘后入口切换为 token 数字。
- 用量气泡显示 Input Token、Output Token、Cache Read、Cache Write 四项和分隔后的执行耗时，不加用量合计行、Run 编号或摘要。上下文入口是弱化小圆环，默认并排显示四舍五入的整数百分比，比例未知时显示 `—`；气泡只显示 `used / window` 与比例。缺乏 used 或 window 不显示 `0%`，不从比例反推精确 used。

## 证据门槛

字段资格由 Runtime、版本及实际 wire 方言决定。Parser 命中、安装版本、端到端 Run、持久化和 UI 是不同证据阶段；未经过当前安装版本与真实调用核验的字段保持“未验证”。原生输入为缓存包含总量时直接使用原生总量；互斥桶只有齐全才合成 Input。Reasoning 已包含在 Output 时不重加。重复模型调用与终态统计按来源身份去重，恢复累计量由 checkpoint 建立基线；失败、取消和超时保留部分观测。

逐 Runtime 证据、版本与未决事项见[第二轮执行指标核验记录](../research/runtime-monitoring/execution-metrics-verification-2026-09-29.md)。
