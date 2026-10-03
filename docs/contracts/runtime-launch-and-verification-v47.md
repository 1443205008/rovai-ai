---
document_type: contract
name: Runtime Launch and Verification
version: v47
status: accepted
source_version: v1.72
last_updated: 2026-10-03
---

# Runtime Launch and Verification v47

继承 [v46](runtime-launch-and-verification-v46.md) 的启动、审批、资格和恢复边界；本版扩展
[v40](runtime-launch-and-verification-v40.md) 的 Runtime Startup Settings，增加当前 Host 按 Runtime Kind 保存的自定义 API。
本合同只拥有配置交付与切换正确性，不认证中转服务或模型能力。实现与实际版本证据见
[本期验收](../versions/v1.72/runtime-custom-api-verification.md)。

## 配置与密钥输入

`RuntimeStartupConfiguration` 增加可选 `customApi`，缺省表示不覆盖。四种 shape 使用现有 Runtime Kind 作为 `kind`：

| kind | 共同字段之外的字段 |
| --- | --- |
| `claude-code-cli` | `models: {model, reasoningModel, haikuModel, sonnetModel, opusModel}`，五项字符串默认空 |
| `codex-cli` | `models: Array<{id, displayName}>`、`defaultModel` |
| `kimi-code-cli` | `apiType: kimi \| anthropic \| openai`、`model` |
| `grok-build` | `model` |

共同字段为 `enabled: boolean`、`baseUrl: string`。不新增供应商、账户、配置档、成员级连接或模型能力表单。
普通字段不得含 Key。`runtime.startup.save/inspect/check` 增加独立写入参数 `apiKey`：

- 缺省或 `{action: "keep"}`：保持已有密钥；空密码框代表 keep。
- `{action: "replace", value: "…"}`：替换为新凭据版本，拒绝空值、控制字符、空白和掩码。
- `{action: "clear"}`：明确清除；启用时拒绝 clear，先关闭或同次关闭后清除。

`runtime.startup.get/save` 的响应增加 `apiKeyConfigured: boolean`，不回传原值、掩码或可编辑凭据引用。
SQLite 普通配置只保存内部修订与凭据引用；实际 Key 写入 Host 数据根下受限私有存储，Unix 目录 0700、文件 0600，
Windows 沿用 private_storage 的 ACL。Key 不进入命令行、普通回执、事件、日志、诊断或导出。
含密钥的输入反序列化错误只返回通用格式错误，原生 stdout/stderr 与公开检查结果精确清除本次 Key。
Claude 私有最终配置控制响应独立消费，不进入普通输出流或 Evidence。

默认关闭；关闭只停止覆盖，字段和密钥保留。清除、替换只操作本功能保存的 Key，不导入或清除原生登录。
地址必须是带 host 的 HTTP/HTTPS URL，拒绝账号密码和 fragment，保留合法路径前缀，不自动补 `/v1`。
HTTP 在界面明确提示风险。启用需要地址和已有／新 Key；Codex 至少一个非空、唯一 ID，默认引用必须命中列表。
Claude 五个模型字段可空；Kimi/Grok 默认模型必填。Codex 删除默认项前先指定新默认项，删除成员已选模型不改成员原值。
成员界面显示“当前接口未配置此模型”，执行拒绝该选择。

保存只验证本地输入，不请求接口、不刷新远端模型、不发送提示词。没有“测试 API”按钮或额外执行前探活。
原有显式检查、协议初始化、认证和平台准入继续使用；生成合法配置不证明后端支持请求中的能力。

## 原生适配

| Runtime | 连接与模型映射 |
| --- | --- |
| Claude | `ANTHROPIC_BASE_URL`、`ANTHROPIC_AUTH_TOKEN`；主模型 `ANTHROPIC_MODEL`、推理兼容字段 `ANTHROPIC_REASONING_MODEL`、三个 `ANTHROPIC_DEFAULT_{HAIKU,SONNET,OPUS}_MODEL` |
| Codex | 内部 `rovai_custom` provider；`base_url`、私有环境 `ROVAI_CUSTOM_API_KEY` 的 `env_key`、`wire_api=responses`、`requires_openai_auth=false`；`model` 和受管 `model_catalog_json` |
| Kimi | `KIMI_MODEL_PROVIDER_TYPE`、`KIMI_MODEL_BASE_URL`、`KIMI_MODEL_API_KEY`、`KIMI_MODEL_NAME` |
| Grok | `GROK_XAI_API_BASE_URL`、`XAI_API_KEY`、`GROK_DEFAULT_MODEL`；原生认证明确选择 `xai.api_key` |

Claude 固定 Bearer／Anthropic Messages。私有 `--settings` 环境覆盖用户 settings 的重复值；初始化后的
`get_settings/get_status` 核对最终地址、认证来源、已覆盖字段及模型。组织策略或版本不支持最终值检查时明确失败。
空模型项不注入；家族映射不自动复制主模型。推理模型只透传，不改变 Thinking 或现有强度；进入进程与实际识别分别验收。

Codex adapter 从实际选中的可执行文件读取经版本核对的完整原生资源，不把 `model/list` 写回目录。
精确 ID 使用原生条目；未知 ID 使用该版本原生 fallback 的默认声明，不按名字套用其他模型能力。
内部条目保留且不作为本连接的用户可选项展示；目录替换内置目录，生成失败不得回用旧文件冒充成功。
资源 digest 未适配、包装程序没有可读资源或目录无法构造时明确报告 adapter 不兼容。
`config/read` 核对最终 provider、地址、凭据引用和协议；新建与恢复显式绑定同一 provider。
模型列表／能力的来源是原生元数据或原生兼容默认值，不是中转实测。

Kimi 复用既有临时模型环境注入函数。启用时整套新连接取代旧私有文件来源，不读取该文件拼接 Key，也不修改它。
启动后核对 `__kimi_env_model__`，显式模型选择只能指向本连接的模型；不能切回另一个 provider。
Rovai 列表将私有临时别名映射为用户填写的模型 ID。上下文和能力继承目标版本原生默认与合并规则，不另加识别器或白名单。

Grok 保留原生 Home、Skills、MCP 和会话。当前执行的主模型及搜索、摘要、图片描述、提示建议模型均纳入路由检查；
原生模型级地址、字面 Key、env_key、认证 helper、请求头等若不能绑定本连接则报错，绝不切换备用账号。
可安全处理的当前模型 env_key 仅在目标进程设置为本次 Key；不调用旧 `.env` 注入函数。
三字段覆盖不注册任意模型／协议，不认识的默认模型报错，防止原生静默回退。
组织 requirements/MDM 或条件覆盖的路由无法确定时，只阻断这条新入口，不更改策略或原生登录状态。

## 一致性、运行快照与恢复

结构化配置与既有启动环境含同义字段时明确报告冲突，不删除用户原值。关闭后回到原有来源；“继承原生”不表示官方账号。
成员明确选模优先，运行时默认使用本功能默认项；不可用的显式选择报错，不悄悄选择第一行或另一个 provider。

CAS、并发保存、安装 generation 与资格失效沿用原实现。冻结 Runtime 增加可选内部 `customApi` 快照，含配置、修订、
凭据版本与私有存储位置，不含 Key。连接身份进入 host/binding compatibility digest 和恢复判断。
保存后的新执行使用新配置；已冻结执行（包括冻结的“未启用”）在重新绑定时保持原快照。
换地址、目录、凭据版本不能复用旧认证进程；不兼容恢复沿用既有明确处理并保留 Rovai 历史。

受管目录／settings 按修订身份生成，以原子写入安装。同一身份内容不一致时拒绝覆盖。
旧 Key 与派生文件在当前设置或非终态冻结 Run 仍引用时保留；终态历史引用不保留可重放凭据。
草稿检查使用独立临时私存副本，寿命绑定既有检查任务；正式保存／旧 Key 清理不破坏正在检查的草稿。
不修改全局环境、原生认证文件、权限、沙箱或组织准入，不为新配置建立第二套调度或资格系统。
