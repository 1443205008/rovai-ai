import { useEffect, useId, useState } from 'react'
import type { RuntimeApiKeyChange } from '@contracts'
import { reusableCredential, usesCustomApi, type ConnectionObservation, type NativeCredential, type RuntimeCustomApiConfiguration } from './runtime-connection-editor'
import { AppDialogGlyph, DialogControlIcon } from './AppDialog'
import { CopyIcon } from './CopyIcon'
import { newCommandId } from '../../shared/command-id'
import { UiText, uiAttribute } from './interface-language'

export function RuntimeCustomApiFields({ value, apiKey, credential, disabled, observation, onChange, onKeyChange }: {
  value: RuntimeCustomApiConfiguration
  apiKey: RuntimeApiKeyChange
  credential?: NativeCredential
  disabled: boolean
  observation?: ConnectionObservation
  onChange(value: RuntimeCustomApiConfiguration): void
  onKeyChange(value: RuntimeApiKeyChange): void
}): React.JSX.Element {
  const id = useId()
  const [revealed, setRevealed] = useState(false)
  const activeApi = usesCustomApi(value)
  const keyAvailable = reusableCredential(credential)
  const nativeKey = keyAvailable ? credential?.value ?? '' : ''
  const keyInput = apiKey.action === 'replace' ? apiKey.value : apiKey.action === 'clear' ? '' : nativeKey
  const clearing = apiKey.action === 'clear'
  const credentialNote = apiKey.action !== 'keep' ? null
    : credential?.source === 'native_cloud' ? `${credential.sourceLabel} 原生路由`
      : keyAvailable ? credential?.source === 'native_managed' ? credential.sourceLabel : null
      : credential?.status === 'unknown' ? '由原生 CLI 管理'
      : credential?.status === 'invalid_reference' ? uiAttribute('未能读取 {0}。可填写新 Key，或修复该来源。', credential.sourceLabel)
        : '未找到可复用的凭据，请填写 API Key。'
  useEffect(() => setRevealed(false), [credential?.version])
  const loginStatus = observation?.loginStatus ?? 'unknown'
  const loginCommand = observation?.loginCommand || (value.kind === 'claude-code-cli' ? 'claude' : 'codex login')
  return <section className="runtime-startup-section runtime-custom-api" aria-labelledby={`${id}-title`}>
    <div className="runtime-startup-section-heading"><h2 id={`${id}-title`}><UiText zh={"连接设置"} /></h2></div>
      <div className="runtime-connection-choice">
        <span id={`${id}-mode`}><UiText zh={"连接方式"} /></span>
        <fieldset className="segments runtime-connection-segments" aria-labelledby={`${id}-mode`} disabled={disabled}>
          <legend><UiText zh={"连接方式"} /></legend>
          <label><input type="radio" name={`${id}-connection`} value="official_login" checked={value.mode === 'official_login'} onChange={() => { setRevealed(false); onChange({ ...value, mode: 'official_login' }) }} /><UiText zh={"官方登录"} /></label>
          <label><input type="radio" name={`${id}-connection`} value="custom_api" checked={value.mode === 'custom_api'} onChange={() => { setRevealed(false); onChange({ ...value, mode: 'custom_api' }) }} /><UiText zh={"自定义 API"} /></label>
        </fieldset>
      </div>
      {value.mode === null && <p className="runtime-custom-api-note runtime-connection-unselected"><UiText zh={"请选择连接方式。"} /></p>}
      {!activeApi && <div className="runtime-connection-login">
        <div className="runtime-connection-login-row" role="status" aria-atomic="true"><span><UiText zh={"登录状态"} /></span><span className={`runtime-login-status is-${loginStatus}`}>
          {uiAttribute(loginStatus === 'signed_in' ? '已登录' : loginStatus === 'signed_out' ? '未登录' : '未确认')}
        </span></div>
        <details className="runtime-connection-help runtime-login-help" open={loginStatus === 'signed_out' || undefined}>
          <summary><UiText zh={"登录与账号操作"} /></summary>
          <RuntimeLoginCommand key={loginCommand} command={loginCommand} />
          <p><UiText zh="账号操作在 CLI 中完成，操作后重新进入此页。" /></p>
        </details>
      </div>}
    {observation?.conflict && <p role="alert" className="runtime-startup-result is-warning runtime-connection-conflict">{observation.conflict}</p>}
    {activeApi && <div id={`${id}-api-fields`} className="runtime-custom-api-fields">
      <label><span><UiText zh={"接口地址（Base URL）"} /></span><input type="url" value={value.baseUrl} placeholder="https://api.example.com" autoComplete="off" spellCheck={false} disabled={disabled}
        onChange={(event) => onChange({ ...value, baseUrl: event.target.value })} /></label>
      <div className="runtime-custom-api-key-row">
        <label htmlFor={`${id}-key`}>API Key</label>
        <div className="runtime-custom-api-key-input">
          <input id={`${id}-key`} type={revealed && keyInput ? 'text' : 'password'} autoComplete="new-password" spellCheck={false} disabled={disabled || credential?.canReplace === false}
            value={keyInput} aria-describedby={[credentialNote && `${id}-credential-note`, credential?.canReplace === false && `${id}-credential-restriction`].filter(Boolean).join(' ') || undefined}
            placeholder={clearing ? '' : keyAvailable && !nativeKey ? '••••••••••••••••' : uiAttribute('输入 API Key')}
            onChange={(event) => {
              const input = event.target.value
              if (!input) setRevealed(false)
              onKeyChange(input === nativeKey ? { action: 'keep' } : input ? { action: 'replace', value: input } : nativeKey ? { action: 'clear' } : { action: 'keep' })
            }} />
          <button type="button" className="quiet-button runtime-startup-icon runtime-key-visibility" disabled={disabled || !keyInput}
            aria-label={uiAttribute(revealed ? '隐藏 API Key' : '显示 API Key')} aria-pressed={Boolean(revealed && keyInput)}
            title={uiAttribute(revealed ? '隐藏 API Key' : '显示 API Key')} onClick={() => setRevealed(!revealed)}>
            <DialogControlIcon name={revealed && keyInput ? 'eye-off' : 'eye'} />
          </button>
        </div>
      </div>
      {credential?.canReplace === false && <p id={`${id}-credential-restriction`} className="runtime-credential-restriction" role="status"><UiText zh={"来源："} />{credential.sourceLabel}。{credential.restriction}。{credential.remedy}</p>}
      {credentialNote && <div className="runtime-native-credential-row">
        <p id={`${id}-credential-note`} className={`runtime-native-credential-note${!keyAvailable && apiKey.action === 'keep' ? ' is-warning' : ''}`} role="status">{uiAttribute(credentialNote)}</p>
      </div>}
      {value.kind === 'claude-code-cli' && <>
        {([['model', '主模型'], ['reasoningModel', '推理模型（Thinking）'], ['haikuModel', 'Haiku 默认模型'], ['sonnetModel', 'Sonnet 默认模型'], ['opusModel', 'Opus 默认模型']] as const).map(([field, label]) =>
          <label key={field}><span>{uiAttribute(label)}</span><input value={value.models[field]} placeholder={uiAttribute("模型 ID（选填）")} autoComplete="off" spellCheck={false} disabled={disabled}
            onChange={(event) => onChange({ ...value, models: { ...value.models, [field]: event.target.value } })} /></label>)}
      </>}
      {value.kind === 'codex-cli' && <CodexApiModels value={value} disabled={disabled} onChange={onChange} />}
    </div>}
  </section>
}

function RuntimeLoginCommand({ command }: { command: string }): React.JSX.Element {
  const [copiedAt, setCopiedAt] = useState<number | null>(null)
  const [copyFailed, setCopyFailed] = useState(false)
  const copied = copiedAt !== null
  useEffect(() => {
    if (copiedAt === null) return
    const timer = window.setTimeout(() => setCopiedAt(null), 1600)
    return () => window.clearTimeout(timer)
  }, [copiedAt])
  return <>
    <p><UiText zh="在本机终端运行：" /></p>
    <div className="runtime-login-command"><code>{command}</code><button type="button" className="quiet-button message-copy-button"
      aria-label={uiAttribute(copied ? '已复制' : '复制命令')} title={uiAttribute('复制命令')} onClick={async () => {
        try { await navigator.clipboard.writeText(command); setCopiedAt(Date.now()); setCopyFailed(false) }
        catch { setCopiedAt(null); setCopyFailed(true) }
      }}><CopyIcon copied={copied} /></button></div>
    {copyFailed && <p role="alert"><UiText zh="复制失败，请选中命令复制。" /></p>}
  </>
}

function CodexApiModels({ value, disabled, onChange }: {
  value: Extract<RuntimeCustomApiConfiguration, { kind: 'codex-cli' }>
  disabled: boolean
  onChange(value: RuntimeCustomApiConfiguration): void
}): React.JSX.Element {
  const id = useId()
  const [error, setError] = useState<string | null>(null)
  return <fieldset className="runtime-custom-api-models"><legend><UiText zh="模型列表" /></legend>
    <div className="runtime-api-model-labels" aria-hidden="true"><span><UiText zh="模型 ID" /></span><span><UiText zh="显示名称（可选）" /></span><span><UiText zh="默认" /></span><span /></div>
    {value.models.map((model, index) => <div className="runtime-api-model-row" key={model.rowId} data-model-row={model.rowId}>
      <label className="runtime-api-model-field"><span><UiText zh="模型 ID" /></span><input aria-label={uiAttribute('模型 ID {0}', String(index + 1))} value={model.id} placeholder={uiAttribute("模型 ID")} disabled={disabled} autoComplete="off" spellCheck={false}
        onChange={(event) => { setError(null); onChange({ ...value, defaultModel: value.defaultRowId === model.rowId ? event.target.value : value.defaultModel,
          models: value.models.map((row, position) => position === index ? { ...row, id: event.target.value } : row) }) }} />
      </label><label className="runtime-api-model-field"><span><UiText zh="显示名称（可选）" /></span><input aria-label={uiAttribute('显示名称 {0}', String(index + 1))} value={model.displayName} placeholder={uiAttribute("显示名称")} disabled={disabled}
        onChange={(event) => onChange({ ...value, models: value.models.map((row, position) => position === index ? { ...row, displayName: event.target.value } : row) })} />
      </label><label className="runtime-api-model-field runtime-api-model-default"><span><UiText zh="默认" /></span><input type="radio" name={`${id}-default`} aria-label={uiAttribute('设为默认模型 {0}', model.id || String(index + 1))} checked={value.defaultRowId === model.rowId} disabled={disabled}
        onChange={() => { setError(null); onChange({ ...value, defaultRowId: model.rowId, defaultModel: model.id }) }} />
      </label><button type="button" className="quiet-button runtime-startup-icon runtime-api-model-delete" aria-label={uiAttribute('删除模型 {0}', model.id || String(index + 1))} disabled={disabled} onClick={() => {
        if (model.rowId === value.defaultRowId) { setError(uiAttribute('请先指定新的默认模型，再删除当前默认项。')); return }
        setError(null); onChange({ ...value, models: value.models.filter((_, position) => position !== index) })
      }}><AppDialogGlyph name="trash" /></button>
    </div>)}
    {error && <p role="alert" className="runtime-environment-error">{error}</p>}
    <button type="button" className="quiet-button" disabled={disabled || value.models.length >= 128} onClick={() => {
      onChange({ ...value, models: [...value.models, { rowId: newCommandId(), id: '', displayName: '' }] })
    }}><DialogControlIcon name="plus" /><UiText zh="添加模型" /></button>
  </fieldset>
}
