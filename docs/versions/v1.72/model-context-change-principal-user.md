---
document_type: model-context-change
version: v1.72
revision: 1
confirmation_status: pending
last_updated: 2026-10-01
---

# Principal 改为 User 的上下文变更说明

本稿 r1 待审阅。[完整前后对照](principal-user-context-comparison.md)包含 20 组替换文本，覆盖三种
Bootstrap、A2A 返回指导、结构化用户提及、CLI 教学与四项 Skill 的八份 Markdown；另列八段保持原文的相邻指令。
当前没有修改产品实现、Schema 或当前合同。

本次将人类用户的主要称呼统一为 **User**，命令主用法改为 `--to-user`，Agent 提及主投影改为 `@User`。
**旧 Native Session 保持原绑定和冻结 Bootstrap，新绑定才生成新 Bootstrap；Skill 随新版程序同步原路径。**
旧命令和提及写法继续进入同一处理逻辑，不给每轮上下文增加迁移说明。

## 基线与工作目录

| 项目 | 记录 |
| --- | --- |
| 源码与对照基线 | `4aa0e9ede69b035952afa1e032ee82dbc4666ca7`，已 fetch 的 `origin/main` |
| 当前版本 | 由[版本索引](../README.md)解析为 v1.72 |
| 分支 | `rovai/principal-user-context` |
| Worktree | `/Users/murray.xue/VSCodeProjects/opensource/rovai-ai-principal-user-context` |
| Governance | 当前仅准备独立提案，没有本次已确认治理提交 |
| 状态与下一步 | `active`；审阅 r1，后续继续复用此 worktree |

本稿依据当前源码和有效合同；已合入的 Thread 改名与 Skill 英文化只提供已有机制的事实，不把其确认继承为本次确认。

## 变更前

当前 Charter 同时使用 Principal 和 User 指人类用户；Send 主帮助教 `--to-principal`，CLI 已接受
`--to-user` 别名。Core 根据结构化 `CurrentUserMention(local_user)` 将 Agent 可见正文呈现为 `@Principal`；
符合行首 mention cluster 规则的显式 Send 正文也可用 `@Principal` 请求用户注意。

当前已经具备两项需要保留的机制：

- [上下文兼容身份](../../../crates/rovai-core/src/context_contract.rs)将 Charter revision 17 与绑定兼容 revision 16 分开；
  [Bootstrap 准备](../../../crates/rovai-core/src/context.rs)优先读取既有绑定保存的 Charter、平台 Skill 索引和 Memory Entrypoint。
- [受管 Skill 同步](../../../crates/rovai-core/src/managed_skills.rs)在 Core 启动和新 Run 准备时更新安装包拥有的文件；
  原路径不变，模型过去已读的文字不受文件更新影响。

完整原文见附录；其中 Rust 字符串已经还原为模型实际看到的文本，不用源码转义或摘要代替原文。

## 变更后

### 生效范围

| 对象 | 拟采用行为 |
| --- | --- |
| 新 Native Binding | 首次生成 User 文案的 Bootstrap |
| 已有 Native Binding | 原 Session ID、generation 与兼容身份保持；恢复继续读取旧 Bootstrap |
| 压缩补发 | 使用该绑定冻结的 Charter、平台索引和 Memory Entrypoint；既有 MEMBER_IDENTITY 刷新规则照常 |
| 旧对话中的新队员或首次单聊 | 以是否新建 Native Binding 为准，新绑定使用新版；不按 Thread 创建日期分流 |
| 新准备的上下文 | 新 A2A 返回句子和 `@User` 投影；section、字段、选择与预算保持 |
| 已冻结输入与成功回执 | 按自己的版本验证并复用原字节；不重渲染成新称呼，不重复发布或通知 |
| 安装并启动新版 Core | 既有同步流程将随包 Skill 正文与 references 更新到原路径，无需重新导入 |
| 旧 Session 使用 Skill | 再次实际读文件时得到新版；已进入历史的旧正文仍保留，旧写法继续可执行 |

这里保证本次命名改动不造成额外 Session 失效；Runtime 原有安装、协议、权限与供应商 Session 失效检查继续有效。
“最新 Skill”指当前安装版本随包发布的文件，不增加远程追踪或后台更新服务。

### 上下文文字

公开批次和非批次的新 Charter 统一用一行定义：

```text
The User is the human who owns the Thread objective. --to-user requests their attention.
```

共享 CLI Charter 的通知句收短为：

```text
Ordinary Thread messages are visible to the User. Use --to-user only for a new decision, answer or action needed from them, or an explicitly requested important-result notification.
```

Single Chat 只同步两处人类称呼，A2A return 只把 `Principal-facing` 改为 `User-facing`。
CLI 的具体帮助继续保留“仅本条消息”“不代表批准”“不创建 Agent Delivery”等现有约束。
完整替换文本以[附录](principal-user-context-comparison.md)为准，不扩展提示的权限或任务含义。

### 命令与提及兼容

`--to-user` 是新的主要拼写，`--to-principal` 是隐式兼容入口；两者归一到既有 `mentionUser` 布尔字段。
同一调用同时提供两种拼写时，按重复参数拒绝，不静默覆盖。`--to` 继续只选择 Agent。

`@User` 与 `@Principal` 使用同一条现有保留人类目标语法：大小写精确，位于逻辑行首或有效 mention cluster，
后接空白或正文结束。新增加的 `User` 保留名与既有 `Principal` 保留名都优先于同名队员；同名队员仍可用
`@agent_N` 或显式 `--to` 寻址。代码、转义、URL、普通行中正文、未知名称中断等排除规则保持。

两种拼写都生成同一个 `CurrentUserMention(local_user)`。重复 authored occurrences 保留可见位置，
每条消息的通知仍只产生一次；flag 与正文共同出现也不重复插入前缀。`--public-only` 继续抑制 Agent 寻址，
用户提及仍有效。用户 Composer、引用、Runtime narration、自动最终发布和历史 Text 不获得新的解析入口。

新 Agent read/search 和新准备输入统一把结构化提及呈现为 `@User`。检索在结构化用户提及支路将两种拼写视为同一身份，
其余搜索文字保持原规则；匹配位置、摘要和截断以实际返回的新投影计算。普通 Text 内的 `@Principal` 仍是原文，
不能通过整段 query/body 替换制造或丢失命中。旧成功工具回执按原记录返回，可以继续含 `@Principal`。
Desktop/Web 的“你”或昵称以及渠道原生 Owner mention 不改变。

### Skill 文件

本次八份文件归属于 `cli-operations`、`campfire`、`grill-duo` 和 `grill-duo-with-docs`。
只替换 Principal 称呼和通知参数；名称、description、来源集合、队员选择、路径及其他指令逐字保持。
因此平台索引与工具箱索引无需因本次 Skill 正文改词新增字段、刷新消息或改变发现机制。
同一 Run 恢复仍使用冻结的索引和链接；文件能被读到不等于模型已重新读取。

## 版本与恢复

本次用已有版本轴区分新旧文字，不新增 Session 配置或兼容管理层。

| 版本轴 | 当前值 | 拟采用值 |
| --- | --- | --- |
| Session Charter revision | 17 | 18 |
| Native Binding Charter compatibility | 16 | 16 |
| Native Bootstrap contract / formatter | v5 / 5 | 保持 |
| Codex session guidance revision | 1 | 1 |
| Antigravity 绑定工具兼容身份 | 32 及当前固定 digest | 保持；实时工具目录变化不进入绑定身份 |
| Agent message projection audience | `agent_v1` | 新投影为 `agent_v2`，旧证据继续接受 `agent_v1` |
| A2A Guidance Evidence | 2，兼容历史 1 | 新证据为 3，保留历史 1 / 2 的原文校验 |
| 普通 Formatter / Manifest | 28 / 28 | 保持，投影差异由既有 audience 字段表达 |
| 公开批次 Formatter / Manifest | 32 / 32 | 保持，同上 |
| Run Facts / Delivery Profile | 普通 6 / 7；公开 9 / 10 | 保持 |
| Single Chat 动态 guidance | v3 | 保持 |
| Built-in tool contract / CLI command | 33 / 33 | 34 / 34 |
| Agent output contract | 6 | 7，标识新的正文主投影 |
| IPC / Envelope / receipt | 2 / 1 / 1 | 保持 |
| Data projection schema / Migration | 128 / 178 | 129 / 179，仅扩展投影版本约束 |

源码的 `context_manifest.message_projection_audience` 当前 CHECK 只接受 `agent_v1`，多处恢复校验也只接受该常量。
因此 `@User` 不能直接覆盖旧投影常量后忽略旧摘要：新准备的输入记录 `agent_v2`，旧 Manifest / Delivery 继续按其
已冻结的 `agent_v1` 读取及必要的来源校验。A2A 指令同样由既有 `schemaVersion` 区分。

Migration 179 只把已有 CHECK 扩展为 `agent_v1 | agent_v2`，不增加业务字段或表；保留所有旧行、Blob、digest、
Bootstrap、Session ID 与绑定 generation。未冻结上下文的新 preparation 使用当前投影；已有冻结证据优先使用原值，
不能按 App 版本或新模板猜测。未知投影和未知 A2A 证据版本仍拒绝，不删除旧校验来换取恢复成功。

新旧消息投影的差异已由 audience、A2A evidence 和实际 payload digest 留证，既有 JSON 结构和总体格式版本无需同时扩号。
各 Runtime 的 binding digest 逐一对照基线，必须保持；进程按原运行机制恢复不等于创建新的 Native Session。
若实施前主线已占用上述版本，先更新本稿记录再进入实现。

## 明确不变

- Bootstrap 与 Dynamic Context 的 section、顺序、纳入条件、历史选择、预算、FIFO、水位、引用和附件规则保持。
- `MEMBER_IDENTITY`、队员资料、用户内容、Memory、Task、Mission 和 Skill selection 不因改称呼重写。
- `local_user`、`mentionUser`、`mentionsCurrentUser`、`current_user_mention` 以及渠道 `external_principal` 保留现有身份含义；
  本次不合并本地与渠道身份，不更改 Owner 验证、用户权限或 Agent 调度。
- 私有／公开隔离、普通消息用户可见性、message-local attention、通知次数、幂等与成功回执语义保持。
- 旧原文、普通 Text、历史引用、Bootstrap Evidence、冻结投递与成功工具回执不批量改名；历史版本文档只作为背景。
- Skill 的随包同步和缺项处理沿用现有实现，不增加原生 Harness 热重载承诺，不修改已安装的日常 Skill 文件。

## 合同影响

确认并实施时同步更新当前 ContextManifest、Built-in Transport、Camp Message Send、Agent 历史投影与用户注意力合同，
以及拥有它们的架构说明和领域词汇。必要的已冻结版本读取规则明确保留；当前 UI 名称和个人资料行为不变。
本提案阶段只在当前版本增加主文、完整对照与导航，未将待确认内容写成当前有效规范。

## 二次确认

当前为 `pending`。用户已要求完整前后对照并开启 worktree，但尚未审阅本稿的完整 r1；该指令不记作 r1 的实施确认。
依据[核心模型上下文变更治理](../../development/model-context-change-governance.md#二次确认门槛)：
“未取得确认时可以继续调查和编辑提案文档，但不得修改实现、Schema、当前合同或执行 clean break。”
后续明确确认本稿后，再记录确认人、实际时间和相等的 `confirmed_revision`；不继承其他命名或英文化任务的确认。

## 验证

### 本轮文档核对

本轮只验证提案原文、覆盖范围、链接和治理记录，不启动 App 或 Runtime，不改日常数据。

| 检查 | 2026-10-01 结果 |
| --- | --- |
| 原文核对 | 20 组前后文本、8 段不变文字与固定基线来源匹配；Rust 换行与 include_str 已展开；八份 Skill 正文及其 frontmatter 已核对 |
| 替换范围 | 新文本中无 Principal 主称呼和旧通知参数；保留原命令约束，Skill description / 路径 / 相对链接不变；附录 48 个代码块与对照清单一致 |
| `pnpm docs:test` | 10/10 通过 |
| 通用决策与链接检查 | 固定 base 的 `check-doc-decisions.mjs --require-base` 通过；未添加例外 |
| `pnpm docs:check` / `pnpm docs:check:ci` | 未通过，仅缺 r1 的确认状态、confirmed revision、确认人和确认时间四项；保留 pending |
| 工作区 | 仅新增主文和完整对照，并增加当前版本导航；`git diff --check` 通过 |

20 组对照文本合计由 29,668 变为 29,233 UTF-8 字节，包含两种公开 Bootstrap 中重复展开的 CLI。
该数字只说明对照文本没有膨胀，不是实际每轮 token、性能或模型行为评测。

### 实施验收

按[Rust 测试政策](../../development/testing.md#rust-测试准入与退役门槛)扩展现有 owner，验证业务边界而非新增大量匹配措辞的测试。

| 范围 | 必须证明的行为 | 现有 owner |
| --- | --- | --- |
| Session | 所有 Adapter 绑定摘要不变；旧绑定 resume、原 Charter 补发；新绑定用 User；身份原刷新规则保持 | `context_contract`、`agent_runtime_adapter`、`context::slow_tests` |
| 冻结恢复 | 旧 audience / A2A 1、2 / 成功 receipt 原字节及摘要可验证；新旧混标、缺证据和未知版本拒绝 | context recovery、Builtin receipt、数据库迁移 owner |
| CLI 与寻址 | 新旧 flag 同语义，重复参数拒绝；两种保留提及、同名队员、代码排除、PublicOnly、一次通知和幂等重放 | `bin/rovai`、`message_delivery`、Send fixtures |
| 历史投影 | 新旧 query 找到结构化提及；字面旧文本仍可查；返回正文、摘要位置与新投影一致；历史正文未重写 | `camp_history`、`camp_content`、引用 fixtures |
| Skill 升级 | 新包在同一路径更新正文与 references；描述和配置不变；旧 Bootstrap 与新版命令共同可用 | `managed_skills::tests`、Skill 既有检查 |

真实升级验收使用隔离实例：先以基线创建 Session 并保存 ID／Bootstrap digest，再在同一隔离实例换候选 Core，
继续原 Session、触发一次原机制 Bootstrap 补发，并在新 Session 读取新 Charter；旧会话读取同路径新版 Skill 后执行新旧通知命令。
不能用新建 Session 代替旧 Session resume 的验证，也不使用日常 Electron userData。

按[上下文 Gate](../../development/evaluation.md#上下文改动-gate)，本次共享 CLI／核心上下文影响使用基线仓库
`qualification/context-regression/suite.json` 的 12 个通用 Case DEMO-101–112，Suite 2.12.0，评分
`generic-task-quality@2.10.0`。基线固定为本稿 SHA，候选固定为实施 commit；同 Runtime、模型、权限和预算。
每例一次，每 campaign `wallSeconds=14400`、`maxParallelCases=2`、`judgeSeconds=2400`，最多两次。
通过标准为现有完整 Gate 通过且上表兼容项无失败；未知与未运行不计通过，不继承其他任务的豁免。
三位评测队员、Runtime／模型、固定 Judge 与独立产物路径在执行前通过既有 freeze plan 固定；本轮未选择或运行。

文档门禁使用：

```bash
pnpm docs:test
pnpm docs:check
DOCS_BASE_REF=4aa0e9ede69b035952afa1e032ee82dbc4666ca7 pnpm docs:check:ci
DOCS_BASE_REF=4aa0e9ede69b035952afa1e032ee82dbc4666ca7 node scripts/check-doc-decisions.mjs --require-base
```

待审阅提案预期会被版本门禁要求补充二次确认；保留该结果，不改门禁、不伪填确认，也不把文档核对当作产品兼容验收。
