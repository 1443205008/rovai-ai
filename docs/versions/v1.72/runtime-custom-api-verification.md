---
document_type: implementation-verification
version: v1.72
source_version: v1.72
status: implemented
last_updated: 2026-10-05
---

# Claude Code / Codex 原生连接编辑验收

当前实现以 [Runtime Launch v47](../../contracts/runtime-launch-and-verification-v47.md) 和
[V1.72-D13](decisions.md#v1-72-d13) 为准：Rovai 只在保存时编辑原生连接；正常执行由原生 CLI 读取配置、认证及发送请求。
先前为永久保留两套连接引入的启动覆盖、临时 provider 和额外身份核对已退出，历史验收不能代表当前行为。
范围仅包含 Claude Code、Codex；不增加表单字段、账户系统、探活、能力认证或版本白名单。

## 实施结果

- `native` 读取实际配置目录、环境与原生凭据引用，只回显地址、模型和凭据状态；打开页面不写文件，也不复制 Key。
  本地原生配置决定当前连接方式，旧 `_connectionMode` 不再控制运行或界面。官方方式与是否已登录分别展示。
- Renderer 保留完整编辑草稿，单选项往返没有请求或写入；新 Key 仅在当前编辑会话内存中保留。
  没有其他修改时返回原选项即恢复干净状态；放弃更改恢复基线并丢弃新 Key。
- 保存提交按最终选项投影。官方选项只提交普通启动字段和必要的模式／连接修订保护；隐藏的地址、模型和 Key
  不进入校验、目录生成或写入。失败和冲突保留完整草稿；成功后重新读取原生结果并清理未提交的敏感输入。
- API 保存合并用户实际编辑的字段。已有原生模型条目、provider 查询参数、传输／重试／超时配置及未知字段保留；
  Codex 默认项绑定稳定行 ID，默认项变化不重建目录，名称修改不覆盖模型能力。继承默认模型不产生允许名单。
- 官方保存停用当前 API 路径。Claude 移除相关原生 API 字段，并按原生 `settings.env` 语义停用继承的 API 环境覆盖；
  官方 OAuth 令牌和其他设置保留，请求头只过滤 Authorization／X-Api-Key，保留跟踪及独立代理认证。Codex 在当前 profile／根配置选择内置 OpenAI，移除当前 API 默认模型和目录引用；
  只清理文件认证中的 API Key 字段及 API 选择标记，保留 OAuth token、未知字段和无关 provider。不承诺 API 凭据可恢复。
- 普通 Rovai 数据库不保存 Key 或第二份连接选择。Key 保持原生来源；用户替换才更新当前连接对应的原生来源。
  Claude 静态 Key 保持 X-Api-Key／Bearer 方式。Codex `auto` 仍由原生选择钥匙串及文件回退，不提升文件 Key 的优先级。
- 正常启动、原有检查与恢复不再生成同一连接的配置覆盖，也不增加 `get_settings`、`config/read` 或身份读取门槛。
  队员显式模型、推理强度、工作目录、工具、审批和沙箱保持既有集成；“运行时默认”交给原生配置。
- 当前连接及凭据摘要继续参与进程复用／恢复兼容性，未使用 provider 和无关 UI/MCP 内容不构成连接冲突。
  辅助读取失败不单独禁止执行；正常原生错误仍反馈。保存后的共享文件可能影响外部会话，不承诺热切换。
- 保留共享原生配置的影响范围说明；没有新增保存提示、确认弹窗、常驻重新读取或后台轮询。

## 本轮验证（2026-10-04／05）

自动验收在 macOS arm64。实际原生版本：Claude Code **2.1.280**、Codex **0.159.2**。
初轮原生夹具根目录 `/private/tmp/rovai-native-save-acceptance-20261004`，最终确认目录 `/private/tmp/rovai-native-save-final-20261005`。
隔离 HOME、配置、工作目录，固定假 Key 与 loopback 服务。
Electron 界面 owner 使用独立 `user-data` 和 `managed-skill-library`，以内存 RPC 夹具挂载正式 React 组件，不启动 Core。
没有读取或修改日常 Rovai 数据，没有发送真实中转或官方模型请求。

| owner / 命令 | 本轮覆盖 |
| --- | --- |
| `cargo test -p rovai-core --features slow-tests --lib runtime_custom_api::` | 原生读取不写入、字段合并／冲突、隐藏 API 输入隔离、官方保存、原生／数据库失败回退、OAuth 保留、无 Key 副本、连接修订、元数据保留与包装入口；既有 5 项 |
| `cargo test -p rovai-core --features slow-tests --lib runtime_startup::` | 普通启动环境规则、并发 overlay 与父进程环境隔离；既有 2 项 |
| `cargo test -p rovai-core --features slow-tests --lib application::runtime_check_environment::tests` | 草稿和正式检查环境、保存与旧检查竞争、读取失败不发布旧环境；既有 3 项 |
| `cargo test -p rovai-core --features slow-tests --lib profile_and_installation_commands_are_idempotent_and_explicit` | 冻结连接摘要与绑定兼容性，旧 UI 模式不接管原生执行；既有 1 项 |
| `vitest run .../runtime-startup-draft.test.ts` | 最终选项投影、隐藏无效输入、模式往返、稳定默认行、API 编辑遇到外部模式变更的保护；既有 3 项 |
| `node --test scripts/lib/runtime-custom-api-ui.test.mjs` | 正式组件的草稿往返、Key 内存／隐藏状态、放弃更改、失败／冲突保留、成功重置、原生读取失败重试、模型行、日夜主题及窄屏 |
| `scripts/smoke-runtime-custom-api.py` + `custom_api_native_fixture` | 保存后实际 CLI 直接读取原生连接，原生目录／配置加载、路径／模型／认证头、新旧进程、恢复及官方保存 |

默认 Rust workspace、完整 `pnpm test`、类型检查、桌面构建、格式及三项通用文档门禁均按仓库路由执行。
默认 Rust：455 passed／1 既有 ignored；JavaScript：237 个 Vitest 文件／2587 项、Node 334 passed／2 平台 skipped。
完整 suite 后的收尾变更再次运行对应 owner；没有把“0 tests”当成通过。

### 原生调用结果

- Claude 实际收到 `/custom/prefix/v1/messages?beta=true` 和 `rovai-main`。Bearer、X-Api-Key 替换以及只在环境中的 Key 均成功，
  路径前缀、原生附加请求头与模型设置保留。官方保存后原生 API 字段已移除或停用，假 OAuth 输入仍在；不是实际订阅调用。
- Codex 模型 `gpt-6.1-sol` 与未知 ID `rovai-unknown` 均能由完整原生目录加载并调用。首次继承默认 A 不阻止显式 B。
  已有条目的 8192 上下文、纯文本输入、关闭 reasoning summary 与未知字段，在名称编辑后保留。
- Codex 原生 provider 的 `api-version=fixture-v1` 查询参数、传输／重试／超时字段保留。地址／Key 轮换后新进程使用
  `/custom/prefix/rotated/responses` 和新 Key，旧进程仍持原连接；新进程恢复线程时使用新原生连接。
  运行参数不再传入重建的 provider 或目录覆盖，也没有生成临时连接文件。
- Codex `auto` 文件回退实际完成 Responses 回复，未复制文件 Key。官方保存选择内置 provider，保留其余 provider／OAuth。
- 记录使用 `keyMatches`、`keyVersion` 而不打印 Key，固定提示词 `Reply OK.`，工作目录不含项目代码。

重跑须使用新的绝对隔离目录：

```bash
cargo build -p rovai-core --example custom_api_native_fixture
python3 scripts/smoke-runtime-custom-api.py --claude /absolute/claude --codex /absolute/codex --fixture-root /absolute/isolated-fixture
```

## 可用性与证据边界

- 以上证明配置交付正确，不证明真实中转或模型所有能力。没有保存前／执行前的额外 HTTP 检查。
- 真实官方往返、OAuth 刷新、Claude 官方订阅、真实钥匙串及其他平台未实测，不作为禁用入口或版本封禁的依据。
  前一轮本机 `codex login status` 只读确认 ChatGPT 已登录，未复制凭据或请求模型；无需用户重新登录或购买 Claude 订阅。
- Codex 常见包装入口通过实际启动入口的本地 `debug models --bundled` 读取；无资源 SHA 或版本号白名单。
  已有完整目录的小改不依赖重新扫描程序。未知模型的兼容默认值来自原生实现，不标为真实能力验证。
- `ANTHROPIC_REASONING_MODEL` 可保存、清空并由原生配置进入进程；不能据此宣称目标 Claude 实际识别了该兼容字段。
- 若 Codex 当前进程环境中仍有会抢占官方路径的 `OPENAI_API_KEY`、`CODEX_API_KEY` 或 `OPENAI_BASE_URL`，
  保存官方选择会指出来源及移除覆盖的处理办法，保留草稿；不在每次启动屏蔽变量，也不退出账号。
  已知强制 API 登录策略同样在保存时报告，未绕过组织策略；原生未知身份不新增运行门槛。
- 原生系统凭据继续由 CLI 消费。不可见来源的状态不等于已登录；普通环境引用可通过输入新 Key 替换连接，
  不强制迁移原来源。Codex 不明类型系统凭据切到新地址时需绑定该地址或输入新 Key，避免发送官方 token 到未知接口。
- Codex 的原生 HTTP 栈没有把 Proxy-Authorization 发到 origin；验证的是代理凭据配置保留与不误拦截，未声称真实代理认证成功。
- 保存成功只表示原生写回完成，相关 Rovai 实例按既有机制重连；对共享原生配置的外部会话不承诺无影响。

## 测试准入与退役

本轮没有新增独立 Rust test。`runtime_custom_api` 的 5 个 owner 保持不变：配置／身份 parser、SQLite 发布、
原生文件来源、目录转换、本地程序入口。SQLite owner 扩展保存时切换、隐藏草稿、真实字段冲突和跨文件回退；
纯 parser 不能证明原生文件与数据库发布之间的失败边界。Renderer 草稿和 Electron 测试分别拥有状态投影与真实组件事件。

同一改动中删除运行时连接覆盖、额外私有配置／身份验证生产路径及其断言，后继合同由原生写回 owner 和 CLI Smoke 负责；
未把这些退出的行为改成 ignored 测试。恢复兼容、未知模型、原生参数、权限、Key 脱敏、路径及不可变目录等仍有效的边界保留。
现有检查、协作、审批、取消和其他 Runtime 的 owner 没有退役。
