---
document_type: verification-report
version: v1.72
last_updated: 2026-10-08
---

# 全会话用户消息锚点验收

范围依据 User 确认：完整用户目录、Core 权威首条关联回复、按需预览、单个独立定位窗口及必要失效处理。
保留正文分页、单行标题、三行回复、原有宽度门槛、横线样式、内部滚动与键盘操作；不增加通用历史窗口框架。
合同为 [Camp Open Projection v26](../../contracts/camp-open-projection-v26.md)。

## 原问题与实现

原锚点由 `conversationTimeline` 中的已加载消息投影，数量和范围受正文首屏限制。用户提供的会话当时有 22 条用户
消息，20 条首屏正文中仅有 9 条用户消息。取消分页会同时放大正文传输、hydration、布局测量和消息 DOM。

新增 Core 轻量读模型从当前 Thread 用户业务字段生成完整目录；批量读取名称及附件 metadata，不读取全部 Agent
正文或执行历史。首条回复先定位 ID，再生成一个摘要。两个入口沿 Desktop allowlist 与已授权 Web operation 进入
同一 Core 实现。前端局部状态隔离索引／预览／点击代次与正文分页，消息合并按 ID 和版本处理。

已有 Gateway 在 commit 后沿 Host 输出通知消息变化，Web SSE 只保留公开失效元数据。目录不随普通 Run 状态刷新；
已访问预览按会话失效，不后台预取。实际下一条消息的 sequence 作为 around 窗口边界补充，避免把编号跳号当作缺口。

## 可重复验证

| Owner / 命令 | 验证边界 |
| --- | --- |
| `cargo test --workspace` | 默认工作区 455 项通过，包含 Core、Host、Web 与 CLI 边界 |
| `pnpm docs:test` / `pnpm docs:check` / `DOCS_BASE_REF=a77b537d pnpm docs:check:ci` | 通用文档和基于变更的治理门禁通过 |
| `pnpm typecheck` | Desktop/Web/共享合同类型一致 |
| `pnpm exec vitest run` | 239 文件、2,603 项通过；包含异步代次、预览请求合并、失败重试、撤回缓存、只更新可撤回状态不重读目录、SSE 元数据与界面英文词条 |
| `pnpm test:message-anchors` | 隔离 Electron 使用生产 ThreadWorkspace；240 条锚点保持既有样式，1,000 条完整目录仅挂载首屏及单个定位窗口，连续跳转不累积，分页和自动加载仍从正常区间起点读取；预览失败保留标题，键盘重试后焦点留在锚点 |
| `cargo test -p rovai-core --features slow-tests --lib user_anchor` | 2 个 SQL owner：完整目录不受正文窗口影响，禁止读取 Run/Turn/Evidence/Managed 文件/事件历史仍成功；只读计数不变，Unicode/附件/提及/引用/空文本回退、外部用户与撤回删除过滤；Core 完整 Run 输入、显式回复优先、Turn 回退与顺序边界 |
| `cargo test -p rovai-core --features slow-tests --lib command::tests::committed_results_replay_after_reopen_and_handler_errors_roll_back_atomically` | 复用事务 owner 验证提示只在提交后产生，回滚、重启后幂等回放不重发 |
| `cargo test -p rovai-core --features slow-tests --lib read_model::slow_tests::message_around_reads_a_bounded_old_window_without_leaking_unavailable_sources` | 复用 around owner，验证 41 条窗口及真实下一行边界，保留不存在／跨会话／删除语义 |

两个新增 Rust owner 使用既有 `camp_open_slow_tests::business_fixture`。现有 Open owner 只拥有有界正文读取，不能证明
全用户目录或完整回复身份，因此新增 SQL 读取边界 owner；普通前端回复推断及其测试随生产路径退出，关系矩阵由 Core
唯一拥有。数据库 authorizer 与业务关系约束需要 SQLite，纯函数测试不足。其余事务与 around 性质扩展既有测试，未增加
第二套进程或数据库夹具。所有数据来自临时隔离目录，不使用日常 App userData、真实模型或用户会话数据库写入。

## 性能与验证范围

目录传输随用户问题数增长，每项只含 ID、顺序、短标题和版本；传输摘要沿用 240 scalar 预算，不限制 UI 行数。
正文窗口仍有界，预览只按访问目标读取，普通执行事件不发起全量目录请求。索引的读取成本仍与该会话用户文本总量
相关；极大数量问题的负载和网络时延没有通过这次功能夹具推断成性能承诺。此记录证明读取边界和有界正文挂载，
不把模拟数据的响应时间作为真实会话或跨设备压测结果。
