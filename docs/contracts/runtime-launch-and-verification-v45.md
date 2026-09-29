---
document_type: contract
name: Runtime Launch and Verification
version: v45
status: accepted
source_version: v1.72
last_updated: 2026-09-29
---

# Runtime Launch and Verification v45

继承 [v44](runtime-launch-and-verification-v44.md) 的 Runtime 启动、检查、恢复、权限、证据和失败边界。
本版仅增加 Claude Code `--print` 执行中的用户审批回调，不改变其他 Runtime、Claude 的流式终态或
Built-in CLI 业务目录。

## Claude Code 权限请求

Claude Code 继续以 `--print --output-format stream-json` 启动，保留冻结的原生 `permission_mode`。
有当前 Run 的 Built-in CLI transport 且原生模式可能询问用户时，Adapter 在本轮私有 `--settings`
文件中注册 `PermissionRequest` command hook：`rovai __claude-permission-hook`。`bypassPermissions`、
`dontAsk` 与旧 `core_enforced_v1` 只读 Run 不注册；`acceptEdits` 自动许可范围仍由 Claude 原生规则决定。
Hook 是私有权限运输，不出现在 Agent 可调用的 Built-in operation catalog，也不添加 MCP 业务桥接。

Hook 输入至多 1 MiB，必须是对象。Core 验证进程及当前 Run lease、Adapter kind、Run/epoch、
Claude Native Session、事件名、工具名与对象输入。Claude 流中的完整 `assistant.tool_use` 输入只保留在
本轮内存中；Hook 的工具名和输入必须唯一匹配尚未领取的原生 Tool ID。缺失、重复或无法唯一关联时拒绝，
不创建可被误认为已授权的 Action。完整工具输入不因这项关联进入公开 Runtime Evidence。

Core 将合格请求准备为现有 `Intercepted` Action 和 Approval，展示原生命令、文件、MCP 或一般权限
摘要；选项只有「拒绝」和「允许一次」。用户允许只回填原 `tool_input`，不保存 Claude permission rule，
也不能越过 Claude 自己的 deny/ask 规则。用户拒绝、输入非法、Core 不可用、Hook 断线、Run/epoch
失效或取消时均返回或落实拒绝；不会把该失败解释为用户同意。Core 在权限响应写入并 flush 到 Hook
后才 ACK Runtime Delivery；交付失败保留现有 unknown/reconciliation 语义。

允许后的 Action 与同一原生 Tool ID 的流式 `tool_result` 结算；明确错误记为 failed，完成记为
succeeded，不从授权回填推断工具已执行。Run 终态仍受未决 Approval、Action、Runtime Delivery 的
原有安全阻断约束。进程退出、重启或取消不会重放已接受的输入或旧审批。

## 验证边界

确定性测试验证 Session/Tool ID 关联、重复输入拒绝、原始输入原样回填、响应 flush ACK 和 Run
清理。真实 Claude Smoke 需在隔离数据目录与 Skill Library 中分别观察允许、拒绝、取消、
`acceptEdits` 的 Bash 权限请求及 Action 结果结算；单元测试不能替代上游 Hook 实际触发证据。
