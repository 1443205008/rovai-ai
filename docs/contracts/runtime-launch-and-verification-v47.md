---
document_type: contract
name: Runtime Launch and Verification
version: v47
status: accepted
source_version: v1.72
last_updated: 2026-10-05
---

# Runtime Launch and Verification v47

继承 [v46](runtime-launch-and-verification-v46.md) 的启动、审批、平台准入、资格和恢复边界。
本版扩展 [v40](runtime-launch-and-verification-v40.md) 的启动设置，仅为 Claude Code 与 Codex 提供
原生连接配置的读取、编辑和执行接入，不认证中转或模型能力。证据见[本期验收](../versions/v1.72/runtime-custom-api-verification.md)。

## 配置权威与输入

原生设置及其实际凭据来源是连接权威。打开页面、重新进入或失败重试只读取，不导入、迁移或复制 Key。
已存在的原生连接不以点击保存为使用前提；原生可使用的凭据引用不以 Host 能取得明文为前提。
普通 SQLite 启动记录只拥有程序路径与非凭据环境，不保存 API 地址、模型、Key 或第二份连接选择。
旧 `_connectionMode` 不再覆盖原生读取结果，保存普通设置时移除该旧元数据；读取本身不迁移或写文件。
`customApi` 是读取投影；内部冻结连接只保存来源、配置身份和摘要，不含密钥。
新增的密钥环境变量限制按 Runtime kind 应用，仅约束 Claude/Codex 各自的原生认证变量；其他智能体继续沿用普通环境编辑语义。

| kind | 投影字段 |
| --- | --- |
| `claude-code-cli` | `mode`、`baseUrl`、`models: {model, reasoningModel, haikuModel, sonnetModel, opusModel}` |
| `codex-cli` | `mode`、`baseUrl`、`models: Array<{rowId,id,displayName}>`、`defaultRowId`、`defaultModel` |

`mode` 为 `official_login | custom_api | null`，是原生当前连接的回显与编辑草稿。原生默认官方路径
与“已登录”是不同事实；外部 CLI 修改原生配置后，下次读取以实际来源为准。
没有发现 Key 不代表官方已登录。`rowId` 只作编辑身份，不写入原生模型目录；编辑 ID 时不改变当前行或默认选择。
模型 ID 非空、唯一，启用 API 的 Codex 至少一项且恰有一个默认项。删除默认模型先选另一项；不可用的成员显式模型
保留原选择，提示“当前接口未配置此模型”，执行不悄悄换模型。

`runtime.startup.get/save` 返回 `credential`（状态、来源标签、版本摘要、可替换／可清除能力和具体限制）、
`connectionObservation`（首次方式、独立登录状态、冲突）、`nativeRevision`、`connectionReadError`，以及本次保存的
`nativeWritten/reconnectRequired`。不回传原 Key 或可编辑的私有凭据对象；密码框用状态生成掩码，眼睛只显示本次输入。

`runtime.startup.save` 接受 `runtimeKind`、`edits: Array<{path,before,after,label}>` 与写入专用 `apiKey`：

- 缺省或 `{action:"keep"}` 保留当前来源；清空尚未保存的输入恢复 keep。
- `{action:"replace",value:"…"}` 替换当前连接的原生 Key；拒绝空白、控制字符和掩码。
- `{action:"clear"}` 为明确清除，不等于退出官方登录；只操作该 API 凭据。不可写的来源说明具体处理办法。
- 替换／清除须带 `credentialVersion` 字段补丁，以摘要处理并发，不把 Key 放入补丁的 before/after。

保留旧的程序路径／普通环境保存形状作为有修订校验的兼容入口；它不能写原生连接或 Key。
单选项仅修改内存草稿；保存前往返切换保留地址、模型、原生 Key 可复用状态及本次新 Key，不写浏览器存储、
数据库或文件。纯往返恢复无修改状态；放弃更改恢复已保存基线并丢弃新 Key。官方保存的后端在校验、冲突判断与
目录生成前排除隐藏 API 修改和 Key 操作，不因不完整的隐藏草稿失败，也不先写新 Key 再清除。
失败及冲突重合并保留完整草稿，成功后以原生结果重建基线并清理未提交敏感输入。

新表单只提交最终选项对应的修改字段。保存重读当前来源，比较原值、草稿和最新值：不相关修改合并；相同结果幂等；真正冲突返回
`{status:"conflict",latest,conflicts}`。界面保留全部草稿和本次 Key 输入，按字段选择我的／外部值，然后再次校验。
API 字段保存携带所选模式的比较补丁，外部模式变化时保留选择与草稿并报告冲突。
`nativeRevision` 为当前有效连接摘要；官方切换携带同名只读比较补丁，避免删除打开页面后被外部更新的连接。
它不包含无关 provider 或 UI/MCP/Skills 内容，写入阶段另用完整原生修订和字节检查防止覆盖并发修改。
原生读取失败不阻塞独立的程序路径或普通环境保存。原子替换失败不破坏旧文件，多文件关联更新失败回退；
回退只还原仍是本次写入结果的文件，不覆盖之后的外部修改。
TOML 保留未知字段和注释，JSON 保留无关字段；不替换整套 Home、不清除 OAuth 或登录状态。

地址为有主机的 HTTP/HTTPS URL，拒绝内嵌账号密码和 fragment，保留路径前缀，不自动补 `/v1`。
HTTP 有传输风险提示。保存只做本地校验、目录构造和解析，不触发探活、模型列表请求或测试提示词。
既有 Runtime 检查／认证／协议初始化继续沿用，没有新增测试 API、后台轮询或同步面板。

## 原生读写与运行接入

Claude 使用实际 `CLAUDE_CONFIG_DIR`／原生 Home 下的设置、相关环境和凭据引用。替换已有静态 Key 时保留
`ANTHROPIC_API_KEY`（X-Api-Key）或 `ANTHROPIC_AUTH_TOKEN`（Bearer）的原认证方式，包括来自环境的静态 Key；
新配置无静态来源时默认写入原生 `env.ANTHROPIC_AUTH_TOKEN`。未替换的环境引用或 `apiKeyHelper` 继续按原生方式复用。
五项模型分别映射 `ANTHROPIC_MODEL`、`ANTHROPIC_REASONING_MODEL`、三个
`ANTHROPIC_DEFAULT_{HAIKU,SONNET,OPUS}_MODEL`；清空字段消除对应值，不把所有家族填成主模型。
主模型按有效 `ANTHROPIC_MODEL`、顶层 `model` 的顺序回显，编辑原有来源；清空时不复活被遮蔽的旧主模型。
Thinking 只作兼容字段透传，不改推理强度，不把变量进入进程当成运行时实际识别。
普通启动直接读取已保存原生配置，不生成连接用 `--settings`、认证环境覆盖或额外 `get_settings`／身份验证门槛。
既有 `auth status` 的明确原生身份仅用于状态回显，诊断展示文案不作身份依据。

Codex 使用实际 `CODEX_HOME`、活动 profile、provider、模型目录及引用凭据；来源不局限于 auth.json。
普通 env 引用不可写时仍可换 Key：adapter 在当前原生 provider 中使用该版本支持的 inline bearer，并解除该连接的旧 env 引用，
不改外部环境，不复制旧 Key，不改官方登录文件。原生管理凭据保持由原生运行时消费；受限组合明确报告来源与限制。
正常启动与 thread start/resume 不再重建 provider，不传重复连接或默认模型覆盖，也不为模式选择增加
`config/read`、`account/read`。查询参数、传输、超时、重试及未知字段全部留给原生读取。
`auto` 保留 Codex 的钥匙串优先、缺失或不可用时回退 `auth.json` 语义；不把回退文件中的 Key 提升为覆盖钥匙串的
环境 Key，也不把回退文件等同于最终身份。设置读取失败、身份缺失、未知或未登录不构成本功能的执行门槛。
原生检查与正常调用继续负责认证结果；成员显式模型、推理参数、cwd、权限、沙箱与协作工具沿用原有集成。
Claude/Codex 的 `Proxy-Authorization` 与模型认证独立，不要求值与模型 Key 相同；代理头的传输范围交给原生 HTTP 实现。

原生默认模型不是允许模型集合。首次继承原生 API（有无目录均可）继续沿用既有模型发现与成员选择，
无需首次保存；只改地址、Key、显示名称或默认项不建立限制名单。用户主动添加、删除或更改模型 ID 后，用目录中的
`rovai_managed_model_list` 标记该列表的所有权，此时删除模型才使对应成员选择显示不可用。
该标记及快照中的 `configuredModelIds` 为派生语义，不是用户输入字段。

已有完整目录的模型条目按稳定行标识保留，名称或 ID 编辑只更新对应字段，保留能力声明、未知字段与内部条目；
只改默认项只写原生 `model`，不重建目录。需要新增元数据时优先通过实际启动入口运行本地
`debug models --bundled`；该命令不刷新远端模型，不是 API 探活。原生二进制无此入口时可读取其内嵌完整目录，
不以版本号或固定资源 SHA 限制使用，不读取另一份安装或旧目录冒充新元数据。
精确已知 ID 复用所选运行时元数据；未知 ID 使用 adapter 的原生兼容默认值，不按名称猜能力，也不宣称实际接口支持。
目录解析和正常执行报告具体不兼容；未实测版本不自动被禁止。无法获取完整目录的错误只针对需要新建／恢复目录的操作，
已有目录的简单编辑可独立完成。生成文件位于原生配置目录内、按内容修订、不含 Key；失败不修改 config.toml 指针，
不覆盖旧进程可能读取的文件。`model/list` 不作为完整目录输入。

## 官方登录与兼容性

官方登录与登录状态独立。Claude 复用原生 Claude 账号，Codex 复用 ChatGPT 登录；切换不删除账号，不接管 OAuth、额度或刷新。
选择官方只改草稿，保存时才停用当前原生 API 路径。Claude 移除当前 API 的地址、静态 Key、helper 与模型映射；
存在相关继承环境时，在原生 settings.env 保存空值以阻止旧值重新生效，保留官方 OAuth 输入。
自定义请求头仅移除会抢占模型认证的 Authorization／X-Api-Key，保留跟踪及代理认证等其他请求头。
Codex 在活动 profile／根配置选择原生 OpenAI，清理参与该连接回退的 API 地址、默认模型和目录引用；
其他 provider、profile、完整目录文件及未知字段保留。需要停用 file／auto 的认证文件 API 回退时只移除
`OPENAI_API_KEY` 及对应 API 模式标志，保留 tokens、刷新令牌与未知字段。不改钥匙串、不执行 logout，
不以 `forced_login_method` 强制切换。确认无法通过可写原生配置停用的环境／策略来源，在保存时报具体原因
并保留草稿，不假报成功，也不恢复启动覆盖。未知认证状态交给原生处理。
保存官方后不承诺原 API 地址、Key 和模型可无损恢复；再次选择 API 按实际剩余内容填写，保存前完整草稿必须保留。
纯官方配置的原生主模型继续保留；没有订阅或真实官方验收不禁用官方入口。
登录提示用“本机”，默认安装保留短命令，自定义程序／配置目录提供匹配来源的可复制命令（不含凭据）。
登录状态可复用既有原生 `auth status` 的明确官方身份；该内存提示只短时回显、来源变化即失效，不参与资格或准入。
没有可靠身份时显示“由原生 CLI 管理，尚未确认”，不阻止原有连接，不新增后台检查。
状态回显不要求用户先重新登录、具有订阅或完成开发验收；登录状态不证明订阅额度或真实模型调用成功。

编辑的是共享原生配置，其他 CLI／应用也可能受影响，页面一次说明作用范围。保存成功不等于热切换成功。
连接、凭据摘要与模型目录进入既有 Host 和 binding compatibility digest；新执行重新读取，不误用旧认证进程。
保存写入的全文件并发检查与有效连接摘要分离：恢复只比较当前使用的连接、凭据、模型目录及相关策略；未选 provider、
无关 UI/MCP/Skills 内容、未使用的认证文件和原生记账字段不通过本功能阻断恢复。官方模式也不绑定停用 API 的 Key。
运行中的进程可以继续使用已捕获值；重建／恢复旧快照前核对实际来源，变化时明确要求重新连接，保留 Rovai 历史。
不通过保留旧 Key 副本重放旧快照，不无条件承诺外部运行会话不受共享文件变化影响。

## 凭据与资格边界

Key 只存在原生凭据位置和必要的运行内存／子进程环境中；原生文件写入沿用受限文件权限和原子替换。
既有程序路径／启动检查使用已保存的原生连接，不提交或覆盖未保存的 API 草稿。
写入输入不实现 Debug/Serialize，不进入命令回执；解析错误不嵌入原文。输出边界清除已知当前 Key；未知来源不冒充已验证。
模型能力声明、目录解析成功、本地假服务实测和真实中转能力必须分别记录。没有新增真实服务验收不阻止保存。
Kimi/Grok 的既有路径、平台准入、Skills、MCP、协作工具、审批与取消保持原合同。
