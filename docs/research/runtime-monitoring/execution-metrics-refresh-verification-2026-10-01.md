---
document_type: verification-report
status: verified-with-scoped-limitations
verified_at: 2026-10-01
baseline_ref: e3bd6b626d746f5773b334a4660756dfaaab1863
---

# 执行指标读取与引用复用验收

本轮只收口执行面板取数：可见性、请求范围、相同结果复用及迟到落盘通知。
保持既有速度／耗时／Run Card／Context／气泡布局和 v3 算法，不修改原生字段资格或数据库结构。
接口与生命周期由 [Runtime Execution Metrics v1](../../contracts/runtime-execution-metrics-v1.md#renderer-读取生命周期) 拥有。

## 改动与证据

| 要求 | 实现与 owner | 验收 |
| --- | --- | --- |
| 隐藏停止 UI 请求 | `useExecutionMetricsVisibility` 观察页面与实际裁切后的卡片；速度组件撤下，Core 继续计数 | Electron 中面板隐藏与页面隐藏分别等待 4.5s，没有新增 `monitoring.*` 请求；恢复立即重读且速度重新预热 |
| 缩小 Run 范围 | 当前活动＋视口卡片＋新展开；活动周期只读活动；后端参数化批量 SQL | 500 个合成 Run 中实际请求最多 7 个；滚到远端历史和展开缓存卡片都会重读，不扫稳定全部历史 |
| 相同结果不更新 state | 按完整 DTO 比较，保留对象／数组；只移除请求内缺失及无效代次 | 8 个确定性 Vitest owner 覆盖细微原始数值变化、null／0、Session 时间／模型／代次及 whole-view 删除 |
| 终态迟到 | 250ms／1s／4s 有限尾读，之后依赖已提交变更；新增周期 Flush 终态 batch 通知 | 真实 Renderer 尾读结束后再等 10.5s 请求数不变，再发送落盘通知后由时钟入口切换为 `1k`；气泡保留四项＋耗时 |
| 在途与恢复 | 一个在途请求、保留 trailing，隐藏／范围／epoch fencing，有限失败重试 | 最低层虚拟时钟覆盖通知突发、旧响应、重试停止和重新可见；Core SQL owner 覆盖活动／终态通知资格 |

周期 Flush 默认不发通知，因此不能只依赖既有终态通知。当前补丁在 Usage／Context 实际提交后，
检查 batch 中是否有终态 Run，仅这类周期提交补发 `monitoring.changed`；活动 Run 不增加通知。
如果分类查询失败，只保守失效，不恢复已经提交的 batch，不重复累计。

## 最小复现

```bash
pnpm exec vitest run apps/desktop/src/renderer/src/execution-metrics-reader.test.ts
cargo test -p rovai-core --lib execution_usage_batch_keeps_requested_scope
ROVAI_KEEP_EXECUTION_METRICS_FIXTURE=1 pnpm test:execution-metrics-ui
```

UI 入口挂载生产 CampWorkspace/CSS，复用已有封闭 Fixture API；Electron 使用本次临时 userData 和
Skill Library，不启动 Core／Runtime，也不访问日常数据库。保留目录含 `metrics-report.json` 和双主题截图。
测试脚本、固定请求与数值来源分别位于：

- [Renderer 生命周期 owner](../../../apps/desktop/src/renderer/src/execution-metrics-reader.test.ts)
- [Core 批量查询与迟到通知资格 owner](../../../crates/rovai-core/src/monitoring.rs)
- [Electron 入口](../../../scripts/lib/execution-metrics-ui.test.mjs)
- [生产 Renderer 动态步骤](../../../scripts/fixtures/camp-fast-layout/metrics-main.cjs)
- [封闭合成数据](../../../scripts/fixtures/camp-fast-layout/renderer.tsx)

60 秒固定稳定终态回放只产生首次读取（两个可见 Run）；主动展开另一个历史 Run 才新增一次。
原基线源码在该窗口为首次＋每 10 秒读取最多 500 条，即最多 7 次／3,500 个 Run 查询执行。
这不是两个真实 Provider 回答的性能比较，也不是新的 Runtime 支持证明。

## 范围与已知事项

- 全量 Vitest：224 文件／2,422 项通过；TypeScript 和 Desktop／Web 构建通过。
- 默认 Rust workspace：449 通过／1 项既有忽略；Monitoring 扩展 owner 12 项通过。
  通用 docs:test、docs:check 和对 main `4aa0e9ed` 的 diff-aware 文档门禁通过。
- UI 动态验收通过，1280×720 的双主题截图没有页面横向溢出。最低层控制器负责确定性请求次数、引用和竞争；
  Electron owner 只验证真实可见性、生产 UI 和传递 seam，不重复该输入矩阵。
- 尝试执行已有 `test:camp-fast-layout` 时，旧脚本仍读取已退出的 `.execution-disclosure.open`，在基线
  `e3bd6b62` 的 `hideSummary` Run Card 结构上也不适用；本轮没有改写该旧 Fast 验收场景，也不计为通过。
- 本轮未重新打包 App 或调用 Provider；前轮真实原生边界证据继续由[原生边界验收](native-boundaries-verification-2026-10-01.md)拥有。
  Windows／移动 Web 的实际可见性验收不从本次 macOS Electron 结果推定通过。
