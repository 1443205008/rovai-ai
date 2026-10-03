import type { AdapterKind, RuntimeApiKeyChange, RuntimeCustomApiConfiguration, RuntimeStartupConfiguration } from '@contracts'

export function normalizedStartupConfiguration(draft: RuntimeStartupConfiguration): RuntimeStartupConfiguration {
  return {
    programPath: draft.programPath?.trim() || null,
    environment: draft.environment.map(({ name, value }) => ({ name: name.trim(), value })),
    ...(draft.customApi ? { customApi: normalizedCustomApi(draft.customApi) } : {})
  }
}

export function emptyCustomApi(kind: AdapterKind): RuntimeCustomApiConfiguration | null {
  const connection = { enabled: false, baseUrl: '' }
  switch (kind) {
    case 'claude-code-cli': return { ...connection, kind, models: { model: '', reasoningModel: '', haikuModel: '', sonnetModel: '', opusModel: '' } }
    case 'codex-cli': return { ...connection, kind, models: [], defaultModel: '' }
    case 'kimi-code-cli': return { ...connection, kind, apiType: 'kimi', model: '' }
    case 'grok-build': return { ...connection, kind, model: '' }
    default: return null
  }
}

function normalizedCustomApi(api: RuntimeCustomApiConfiguration): RuntimeCustomApiConfiguration {
  const connection = { ...api, baseUrl: api.baseUrl.trim() }
  switch (connection.kind) {
    case 'claude-code-cli': return { ...connection, models: Object.fromEntries(Object.entries(connection.models).map(([key, value]) => [key, value.trim()])) as typeof connection.models }
    case 'codex-cli': return { ...connection, defaultModel: connection.defaultModel.trim(), models: connection.models.map(({ id, displayName }) => ({ id: id.trim(), displayName: displayName.trim() })) }
    default: return { ...connection, model: connection.model.trim() }
  }
}

export function customApiError(draft: RuntimeStartupConfiguration, key: RuntimeApiKeyChange, keyConfigured: boolean): string | null {
  const api = draft.customApi ? normalizedCustomApi(draft.customApi) : null
  if (!api?.enabled) return null
  try {
    const url = new URL(api.baseUrl)
    if (!['https:', 'http:'].includes(url.protocol) || !url.hostname || url.username || url.password || url.hash) throw new Error()
  } catch { return '请输入有效的 HTTP 或 HTTPS 地址，且不要在地址中包含账号或密码。' }
  if (key.action === 'clear' || (key.action === 'keep' && !keyConfigured) || (key.action === 'replace' && !key.value.trim())) return '启用自定义 API 需要已保存或新输入的 API Key。'
  if (api.kind === 'codex-cli') {
    if (!api.models.length || api.models.some((model) => !model.id)) return '请至少添加一个模型，并填写每个模型 ID。'
    if (new Set(api.models.map((model) => model.id)).size !== api.models.length) return '模型 ID 不能重复。'
    if (!api.defaultModel || !api.models.some((model) => model.id === api.defaultModel)) return '请选择一个默认模型。'
  } else if (api.kind !== 'claude-code-cli' && !api.model) return '请填写默认模型。'
  return null
}

export function runtimeStartupKey(draft: RuntimeStartupConfiguration): string {
  const normalized = normalizedStartupConfiguration(draft)
  return JSON.stringify({ ...normalized, environment: [...normalized.environment].sort((a, b) => a.name.localeCompare(b.name)) })
}

export function runtimeEnvironmentErrors(draft: RuntimeStartupConfiguration, windows: boolean): Record<number, string> {
  const errors: Record<number, string> = {}
  const names = new Map<string, number>()
  draft.environment.forEach(({ name: raw, value }, index) => {
    const name = raw.trim()
    if (!/^[A-Za-z_][A-Za-z0-9_]{0,255}$/.test(name)) {
      errors[index] = '变量名需以字母或下划线开头，只含字母、数字、下划线。'
    } else if (name.toUpperCase().startsWith('ROVAI_')) {
      errors[index] = 'ROVAI_ 开头的变量由应用管理。'
    } else if (value.includes('\0') || value.length > 65536) {
      errors[index] = '变量值包含空字符或超过长度限制。'
    }
    const key = windows ? name.toUpperCase() : name
    const previous = names.get(key)
    if (previous !== undefined) errors[index] = errors[previous] = '变量名重复。'
    names.set(key, index)
  })
  return errors
}
