---
document_type: implementation-record
version: v1.72
authority: implementation-evidence
last_updated: 2026-10-07
---

# 用户主动继续执行实施记录

## 工作区

- worktree：`/Users/murray.xue/VSCodeProjects/opensource/rovai-ai-run-continuation`
- branch：`rovai/run-continuation`；base：`origin/main` / `b20b1f69`；状态：ready。
- Governance：无独立主线先行提交要求；已确认 r2 的合同和实施记录随本分支交付。
- 授权：User 在审阅 r2 后明确要求独立 worktree 实现并推送；[输入对照](model-context-change-run-continuation.md) 已确认。
- 原工作区既有图片菜单变更及其他 worktree 均保留。

## 实施大纲

1. 新 User 命令原子提交系统操作、授权记录与 waiting Delivery，复用命令回执和唯一调度器。
2. 队首按批次边界领取，完整承接原业务输入；Core 重验当前范围，清理完成才创建新 Run。
3. 新 Run 复用当前 Context builder；会话兼容时恢复，显式新会话保留工作区，恢复失败不隐式回退。
4. Desktop / Web 共享 24×24 继续图标与单次会话确认；原 Run 状态不传播、同一来源可多次请求。
5. Migration 184 / schema 134 增量保存请求；旧输入／Manifest／接受事实不修改。

## 影响范围

| 范围 | 结论 |
| --- | --- |
| 产品与 UI | 执行卡片纯图标；等待请求与新 Run 独立显示 |
| 架构 | 继续是 User 新授权，沿用现有 lane 与 Conversation |
| 合同与传输 | Continue v1、Recovery v7、Delivery v11、Host Web v5、Surface v44；Web protocolVersion 仍为 4 |
| 数据与迁移 | v1.72/schema 133 → 134，保留全部旧证据，不做 clean break |
| 模型上下文 | 输入选择入口新增；现有 builder、字段、Formatter 32、Run Facts 9、Profile 10 保持 |
| Runtime 与工作区 | 清理门禁保持，续做恢复失败不得空会话回退，工作区不重置 |
| 权限与业务 | 仅 User，选中 Run 范围；不重开 Task/Mission/Automation |
| 测试与观测 | 事务、幂等、隔离、顺序、迁移和生产组件交互；系统操作进入普通时间线 |
| 分发与运维 | 独立分支提交／推送；不替换日常 App，不修改日常数据 |

## 测试准入

- `delivery_queue::tests::user_continuation_preserves_source_and_claims_independent_fifo_batches` 拥有新授权跨模块事务：
  原实现无此入口，需证明回滚、同命令幂等、同来源多次请求、重开数据库、FIFO、清理及来源不变，纯函数不足。
- `delivery_queue::tests::continuation_rechecks_scope_and_requires_explicit_session_replacement` 拥有 User 准入与会话确认：
  覆盖越权、活动 Run、Thread 不匹配、绑定不兼容、确认后轮换、恢复失败和排队输入失效。
- `db::run_continuation::tests::continuation_migration_rolls_back_and_preserves_frozen_evidence` 拥有新约束迁移回滚与重开：
  需要 SQLite DDL、回执及证据一起提交；已有 pending-draft owner 不拥有输入约束变化。
- 三项新增 Rust owner 均进入 `extended-tests`。最小命令：
  `cargo test -p rovai-core --lib --features extended-tests continuation`。
- Renderer 批次边界扩展现有 `App.test.ts`；生产图标、键盘、连点、同请求核对与新会话确认由
  `pnpm test:run-continuation-ui` 的隔离 Chrome fixture 拥有，不调用真实模型。

## 验证记录

| 命令／范围 | 结果 |
| --- | --- |
| `pnpm typecheck` | 通过 |
| `pnpm test` | 238 个 Vitest 文件／2602 项通过；Node 334 项通过、2 项平台跳过 |
| `pnpm test:rust:pr` | Rust workspace 453 项通过、1 项既有忽略；Core、CLI、Host 与 Web 均通过 |
| `pnpm exec vitest run apps/desktop/src/renderer/src/App.test.ts` | 最终队列排序修订后 181 项通过；相同时间戳保留后端排队顺序 |
| `cargo test -p rovai-core --lib --features extended-tests delivery_queue:: -- --test-threads=2` | 24 项通过，包括两项新增续做 owner |
| `cargo test -p rovai-core --lib --features extended-tests db:: -- --test-threads=2` | 覆盖 99 项；首次 95 项通过，4 项旧版本夹具修正后分别定向复验通过 |
| `pnpm test:run-continuation-ui` | 生产组件在隔离 Chrome 中通过图标尺寸、键盘、连点、独立请求、原状态不变、响应丢失核对与新会话确认 |
| `pnpm build:desktop` | Web／Execution Web／Electron 构建通过 |
| `cargo fmt --all -- --check`、`git diff --check` | 通过 |
| `DOCS_BASE_REF=b20b1f69 pnpm docs:check:ci` | 版本及 diff-aware 文档治理通过 |

数据库回归修正的是测试夹具：当前 migration receipt 预期、导航读模型调用前补齐当前增量表、
历史 requeue 领取前升级至当前 schema，以及旧 Thread lineage 测试撤销后续迁移表。
未放宽生产准入、未退役测试、未添加治理检查例外。新增 Migration 184 的事务回滚与证据保留 owner 通过。

浏览器截图保存在此 worktree 的 `out/verification/run-continuation/`；测试使用 Mock 传输，
不代表真实 Runtime 恢复或模型质量对照。

真实模型 Gate **未执行**。本项尚无冻结的实际 Runtime/model、Judge 和预算配置；不沿用其他工作项的豁免。
按[上下文变更治理](../../development/model-context-change-governance.md#真实任务-gate)“PR 前保留新旧实际执行对照”的要求，
完成 User 授权的分支提交／推送后，保留 worktree，暂不创建 PR 或合入主线。
下一步是冻结本项真实任务评测配置、补齐 Gate 证据，再进入 PR review。
