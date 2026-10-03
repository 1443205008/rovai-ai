import { useId, useState } from 'react'
import type { RuntimeApiKeyChange, RuntimeCustomApiConfiguration } from '@contracts'
import { DialogControlIcon } from './AppDialog'
import { newCommandId } from '../../shared/command-id'
import { UiText, uiAttribute } from './interface-language'

export function RuntimeCustomApiFields({ value, apiKey, keyConfigured, disabled, onChange, onKeyChange }: {
  value: RuntimeCustomApiConfiguration
  apiKey: RuntimeApiKeyChange
  keyConfigured: boolean
  disabled: boolean
  onChange(value: RuntimeCustomApiConfiguration): void
  onKeyChange(value: RuntimeApiKeyChange): void
}): React.JSX.Element {
  const id = useId()
  return <section className="runtime-startup-section runtime-custom-api" aria-labelledby={`${id}-title`}>
    <div className="runtime-startup-section-heading"><h2 id={`${id}-title`}><UiText zh="自定义 API" /></h2></div>
    <p className="runtime-custom-api-scope"><UiText zh="对当前 Host 中所有使用该智能体的队员生效。" /></p>
    <label className="runtime-custom-api-toggle"><span><UiText zh="启用自定义 API" /></span>
      <input type="checkbox" role="switch" checked={value.enabled} disabled={disabled} onChange={(event) => onChange({ ...value, enabled: event.target.checked })} />
    </label>
    <p className="runtime-custom-api-note">{value.enabled
      ? <UiText zh="模型请求会发往填写的服务，可能包含提示词、代码和工具结果。" />
      : <UiText zh="关闭时继承原有配置，已填写的内容和密钥会保留。" />}</p>
    <div className="runtime-custom-api-fields">
      {value.kind === 'kimi-code-cli' && <label><span><UiText zh="接口类型" /></span><select value={value.apiType} disabled={disabled}
        onChange={(event) => onChange({ ...value, apiType: event.target.value as 'kimi' | 'anthropic' | 'openai' })}>
        <option value="kimi">Kimi</option><option value="anthropic">Anthropic</option><option value="openai">OpenAI</option>
      </select></label>}
      <label><span><UiText zh="接口地址" /></span><input type="url" value={value.baseUrl} placeholder="https://api.example.com" autoComplete="off" spellCheck={false} disabled={disabled}
        onChange={(event) => onChange({ ...value, baseUrl: event.target.value })} /></label>
      {value.baseUrl.trim().toLowerCase().startsWith('http:') && <p className="runtime-startup-result is-warning" role="status"><UiText zh="HTTP 不加密，凭据与请求内容可能在传输中泄露。建议使用 HTTPS。" /></p>}
      <label htmlFor={`${id}-key`}><span>API Key</span><input id={`${id}-key`} type="password" autoComplete="new-password" spellCheck={false} disabled={disabled}
        value={apiKey.action === 'replace' ? apiKey.value : ''}
        placeholder={apiKey.action === 'clear' ? uiAttribute('保存后清除') : keyConfigured ? uiAttribute('已保存；留空保持不变') : uiAttribute('输入 API Key')}
        onChange={(event) => onKeyChange(event.target.value ? { action: 'replace', value: event.target.value } : { action: 'keep' })} /></label>
      <div className="runtime-custom-api-key-actions">
        {apiKey.action !== 'keep' && <button type="button" className="quiet-button" disabled={disabled} onClick={() => onKeyChange({ action: 'keep' })}><UiText zh="保持原密钥" /></button>}
        {keyConfigured && apiKey.action !== 'clear' && <button type="button" className="quiet-button danger-text" disabled={disabled || value.enabled}
          title={value.enabled ? uiAttribute('先关闭自定义 API，再清除密钥') : undefined} onClick={() => onKeyChange({ action: 'clear' })}><UiText zh="清除已保存密钥" /></button>}
        {apiKey.action === 'clear' && <span role="status"><UiText zh="保存后清除密钥。" /></span>}
      </div>
      {value.kind === 'claude-code-cli' && <>
        {([['model', '主模型'], ['reasoningModel', '推理模型（Thinking）'], ['haikuModel', 'Haiku 默认模型'], ['sonnetModel', 'Sonnet 默认模型'], ['opusModel', 'Opus 默认模型']] as const).map(([field, label]) =>
          <label key={field}><span>{uiAttribute(label)}</span><input value={value.models[field]} placeholder={uiAttribute('留空不覆盖')} autoComplete="off" spellCheck={false} disabled={disabled}
            onChange={(event) => onChange({ ...value, models: { ...value.models, [field]: event.target.value } })} /></label>)}
        <p className="runtime-custom-api-note"><UiText zh="接口须支持 Anthropic Messages 与 Bearer 认证。推理模型按兼容字段透传，是否生效取决于运行时版本。" /></p>
      </>}
      {value.kind === 'codex-cli' && <CodexApiModels value={value} disabled={disabled} onChange={onChange} />}
      {(value.kind === 'kimi-code-cli' || value.kind === 'grok-build') && <label><span><UiText zh="默认模型" /></span><input value={value.model} autoComplete="off" spellCheck={false} disabled={disabled}
        onChange={(event) => onChange({ ...value, model: event.target.value })} /></label>}
      {value.kind === 'grok-build' && <p className="runtime-custom-api-note"><UiText zh="覆盖原生 API-key 路径的端点，使用当前运行时已识别的模型。辅助功能须能通过同一连接完成。" /></p>}
    </div>
  </section>
}

function CodexApiModels({ value, disabled, onChange }: {
  value: Extract<RuntimeCustomApiConfiguration, { kind: 'codex-cli' }>
  disabled: boolean
  onChange(value: RuntimeCustomApiConfiguration): void
}): React.JSX.Element {
  const id = useId()
  const [rowIds, setRowIds] = useState(() => value.models.map(() => newCommandId()))
  const [error, setError] = useState<string | null>(null)
  return <fieldset className="runtime-custom-api-models"><legend><UiText zh="模型列表" /></legend>
    <div className="runtime-api-model-labels" aria-hidden="true"><span><UiText zh="模型 ID" /></span><span><UiText zh="显示名称（可选）" /></span><span><UiText zh="默认" /></span><span /></div>
    {value.models.map((model, index) => <div className="runtime-api-model-row" key={rowIds[index] ?? index}>
      <label className="runtime-api-model-field"><span><UiText zh="模型 ID" /></span><input aria-label={uiAttribute('模型 ID {0}', String(index + 1))} value={model.id} disabled={disabled} autoComplete="off" spellCheck={false}
        onChange={(event) => { setError(null); onChange({ ...value, defaultModel: value.defaultModel === model.id && model.id !== '' ? event.target.value : value.defaultModel,
          models: value.models.map((row, position) => position === index ? { ...row, id: event.target.value } : row) }) }} />
      </label><label className="runtime-api-model-field"><span><UiText zh="显示名称（可选）" /></span><input aria-label={uiAttribute('显示名称 {0}', String(index + 1))} value={model.displayName} disabled={disabled}
        onChange={(event) => onChange({ ...value, models: value.models.map((row, position) => position === index ? { ...row, displayName: event.target.value } : row) })} />
      </label><label className="runtime-api-model-field runtime-api-model-default"><span><UiText zh="默认" /></span><input type="radio" name={`${id}-default`} aria-label={uiAttribute('设为默认模型 {0}', model.id || String(index + 1))} checked={Boolean(model.id) && value.defaultModel === model.id} disabled={disabled || !model.id.trim()}
        onChange={() => { setError(null); onChange({ ...value, defaultModel: model.id }) }} />
      </label><button type="button" className="quiet-button runtime-startup-icon" aria-label={uiAttribute('删除模型 {0}', model.id || String(index + 1))} disabled={disabled} onClick={() => {
        if (model.id && model.id === value.defaultModel) { setError(uiAttribute('请先指定新的默认模型，再删除当前默认项。')); return }
        setError(null); setRowIds(rowIds.filter((_, position) => position !== index)); onChange({ ...value, models: value.models.filter((_, position) => position !== index) })
      }}><DialogControlIcon name="trash" /></button>
    </div>)}
    {error && <p role="alert" className="runtime-environment-error">{error}</p>}
    <button type="button" className="quiet-button" disabled={disabled || value.models.length >= 128} onClick={() => {
      setRowIds([...rowIds, newCommandId()]); onChange({ ...value, models: [...value.models, { id: '', displayName: '' }] })
    }}><DialogControlIcon name="plus" /><UiText zh="添加模型" /></button>
    <p className="runtime-custom-api-note"><UiText zh="使用 OpenAI Responses；所有模型共用上面的地址和密钥。" /></p>
  </fieldset>
}
