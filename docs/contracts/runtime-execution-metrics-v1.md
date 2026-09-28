---
document_type: contract
contract: runtime-execution-metrics
version: 1
status: accepted
source_version: v1.72
last_updated: 2026-09-28
---

# Runtime Execution Metrics v1

本合同把执行台的三种数据分开：当前速度属于 `AgentRun + executionEpoch` 的 Renderer 临时状态；四项原生用量属于逻辑 `AgentRun`，沿用 [Runtime Usage Monitoring v4](runtime-usage-monitoring-v4.md)；上下文属于当前原生 `Conversation / Binding / Session`。三者不互相填补。

## 读取与归属

`monitoring.execution({campId, agentRunIds})` 返回 schemaVersion 1、`runs[]` 与 `sessions[]`。最多读取 500 个指定 Run，Run 必须属于 Camp；返回的用量只取当前 Monitoring collection 的 `runtime_usage_run_summary`。每个 Run 行提供 `agentRunId`、`executionEpoch`、`promptInputTotalTokens`、`outputTokens`、`cacheReadTokens`、`cacheWriteTokens`、`finalizedAt`、`lastObservedAt`。缺失保持 `null`，运行中已到达的原生用量继续保存和读取。

`sessions[]` 只返回该 Camp 目前 `Conversation.native_binding_id + native_binding_generation + native_session_id` 均匹配的最新上下文。每行包含 `conversationId`、`agentId`、`sessionGeneration`、`runtimeKind`、`modelKey`、`usedTokens`、`windowTokens`、`source`、`dialectId`、`observedAt`。持久行还保存原生 Session ID、Runtime 版本、有效配置摘要、来源 Run/epoch。旧会话事件、旧绑定代次、较早观测不能覆盖当前值；后续 Run 的有效配置变化使旧观测在读取时失效。

只接受原生 Session gauge 的明确 `used` 或正数 `window`；只有窗口上限时 `used` 仍为空。Codex `modelContextWindow` 只提供窗口上限；`tokenUsage.last.inputTokens` 不自动当作上下文占用。只有整轮或整会话累计 token 也不能代替占用。`used > window` 的无效观测不进入当前上下文。当前绑定发生已确认压缩后，旧 gauge 在新 gauge 到达前不再显示。

## 显示

- 运行中在 Run 卡片显示不可点击的当前 `tok/s`，由公开根 Agent `agent.text.delta` 正文块的新增文本估算。首次看到的块只作基线；历史回放、重复快照、工具与私有思考不计数。单调时钟每 500 ms 采样，约 1.5 秒指数平滑，静默 3 秒后变未知；终态停止采样。该值不进入原生 Usage 或持久化。
- 终态卡片在原有耗时旁显示 `xxk` 用量入口。只有成功、Run summary 已结算、Input 与 Output 都已得到时，入口值为 `Input + Output`；Cache 是 Input 的子集，不再次相加。失败、取消及部分观测仍能在气泡查看已收到的字段，入口值为未知。
- 用量气泡仅显示 Input Token、Output Token、Cache Read、Cache Write 四行。上下文入口是弱化小圆环；气泡只显示 `used / window` 与比例。缺乏 used 或 window 不显示 `0%`，不从比例反推精确 used。

## 证据门槛

字段资格由 Runtime、版本及实际 wire 方言决定。Parser 命中、安装版本、端到端 Run、持久化和 UI 是不同证据阶段；未经过当前安装版本与真实调用核验的字段保持“未验证”。原生输入为缓存包含总量时直接使用原生总量；互斥桶只有齐全才合成 Input。Reasoning 已包含在 Output 时不重加。重复模型调用与终态统计按来源身份去重，恢复累计量由 checkpoint 建立基线；失败、取消和超时保留部分观测。

逐 Runtime 证据、版本与未决事项见[执行指标核验记录](../research/runtime-monitoring/execution-metrics-verification-2026-09-28.md)。
