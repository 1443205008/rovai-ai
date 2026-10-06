import type { AdapterKind, RuntimeApiKeyChange } from '@contracts'
import { configurationFromSnapshot, editableSnapshot, editedFields, initialConfiguration, reusableCredential, supportsOfficialLogin, usesCustomApi, withSnapshotValue, type FieldEdit, type NativeCredential, type RuntimeCustomApiConfiguration, type RuntimeStartupConfiguration, type RuntimeStartupSettings } from './runtime-connection-editor'

// Only a saved launch/directory selection schedules another source observation.
// Input edits and ordinary environment saves do not query native processes.
export function startupSourceKey(configuration: RuntimeStartupConfiguration): string {
  const selectors = new Set(['CODEX_HOME', 'HOME', 'USERPROFILE', ...(configuration.programPath ? [] : ['PATH'])])
  return JSON.stringify([configuration.programPath, configuration.environment.filter(entry => selectors.has(entry.name.toUpperCase())).sort((a, b) => a.name.localeCompare(b.name))])
}

// A newly selected source may finish resolving while the user is already typing.
// Rebase only their edits; never treat all values of the provisional file as intent.
export function draftAfterSourceObservation(saved: RuntimeStartupSettings, draft: RuntimeStartupConfiguration, observed: RuntimeStartupSettings, key: RuntimeApiKeyChange): RuntimeStartupConfiguration {
  const current = editableSnapshot(draft)
  let merged = editableSnapshot(initialConfiguration(observed))
  const edits = editedFields(editableSnapshot(initialConfiguration(saved)), current)
  for (const edit of edits) {
    if (edit.path[0] === 'codexModels' && edit.path.length > 2 && !merged.codexModels[edit.path[1]]) {
      merged = withSnapshotValue(merged, ['codexModels', edit.path[1]], current.codexModels[edit.path[1]])
    }
    if (edit.path[0] === 'defaultRowId' && edit.after && !merged.codexModels[edit.after as string]) {
      merged = withSnapshotValue(merged, ['codexModels', edit.after as string], current.codexModels[edit.after as string])
    }
    merged = withSnapshotValue(merged, edit.path, edit.after)
  }
  // The current editing session owns its selected mode and incomplete env rows.
  if (key.action !== 'keep' || edits.some(edit => !['programPath', 'environment'].includes(edit.path[0]))) merged.mode = current.mode
  return { ...configurationFromSnapshot(initialConfiguration(observed), merged), programPath: draft.programPath, environment: draft.environment }
}

export function normalizedStartupConfiguration(draft: RuntimeStartupConfiguration): RuntimeStartupConfiguration {
  return {
    programPath: draft.programPath?.trim() || null,
    environment: draft.environment.map(({ name, value }) => ({ name: name.trim(), value })),
    ...(draft.customApi ? { customApi: normalizedCustomApi(draft.customApi) } : {})
  }
}

export function emptyCustomApi(kind: AdapterKind): RuntimeCustomApiConfiguration | null {
  switch (kind) {
    case 'claude-code-cli': return { baseUrl: '', mode: null, kind, models: { model: '', reasoningModel: '', haikuModel: '', sonnetModel: '', opusModel: '' } }
    case 'codex-cli': return { baseUrl: '', mode: null, kind, models: [], defaultModel: '', defaultRowId: null }
    default: return null
  }
}

function normalizedCustomApi(api: RuntimeCustomApiConfiguration): RuntimeCustomApiConfiguration {
  const connection = { ...api, baseUrl: api.baseUrl.trim() }
  switch (connection.kind) {
    case 'claude-code-cli': return { ...connection, models: Object.fromEntries(Object.entries(connection.models).map(([key, value]) => [key, value.trim()])) as typeof connection.models }
    case 'codex-cli': return { ...connection, defaultModel: connection.models.find(row => row.rowId === connection.defaultRowId)?.id.trim() ?? '', models: connection.models.map(({ rowId, id, displayName }) => ({ rowId, id: id.trim(), displayName: displayName.trim() })) }
  }
}

// Produce only the fields edited in this form, never an old native-file copy.
export function startupEdits(saved: RuntimeStartupSettings, draft: RuntimeStartupConfiguration, key: RuntimeApiKeyChange): FieldEdit[] {
  const edits = editedFields(editableSnapshot(normalizedStartupConfiguration(initialConfiguration(saved))), editableSnapshot(normalizedStartupConfiguration(draft)))
  // Observation may initialize the radio; its value is never a file CAS baseline.
  for (const edit of edits) if (edit.path[0] === 'mode') edit.before = saved.configuration.customApi?.mode ?? null
  if (key.action !== 'keep') edits.push({ path: ['credentialVersion'], before: saved.credential?.version ?? null, after: key.action, label: 'API Key' })
  return edits
}

// The editing session retains the whole draft; only the final selected mode is submitted.
export function startupSubmission(saved: RuntimeStartupSettings, draft: RuntimeStartupConfiguration, key: RuntimeApiKeyChange): { edits: FieldEdit[]; apiKey: RuntimeApiKeyChange } {
  if (key.action === 'replace') key = { action: 'replace', value: key.value.trim() }
  const all = startupEdits(saved, draft, key)
  if (draft.customApi?.mode !== 'official_login') {
    if (all.some(edit => !['programPath', 'environment'].includes(edit.path[0]))) {
      all.push({ path: ['nativeRevision'], before: saved.nativeRevision ?? null, after: saved.nativeRevision ?? null, label: '当前连接' })
    }
    return { edits: all, apiKey: key }
  }
  const edits = all.filter(edit => ['programPath', 'environment', 'mode'].includes(edit.path[0]))
  if (edits.some(edit => edit.path[0] === 'mode')) {
    edits.push({ path: ['nativeRevision'], before: saved.nativeRevision ?? null, after: saved.nativeRevision ?? null, label: '当前连接' })
  }
  return { edits, apiKey: { action: 'keep' } }
}

export function nativeConnectionChange(saved: RuntimeStartupSettings, draft: RuntimeStartupConfiguration, key: RuntimeApiKeyChange): FieldEdit[] | null {
  const edits = startupSubmission(saved, draft, key).edits.filter(edit => !['programPath', 'environment'].includes(edit.path[0]))
  return edits.length ? edits : null
}

export function customApiError(draft: RuntimeStartupConfiguration, key: RuntimeApiKeyChange, credential: NativeCredential | undefined, requireModelList = true, requireCredential = true, requireAddress = true): string | null {
  const api = draft.customApi ? normalizedCustomApi(draft.customApi) : null
  if (api?.mode === 'official_login') return null
  if (key.action === 'replace' && credential?.canReplace === false) return [credential.sourceLabel, credential.restriction, credential.remedy].filter(Boolean).join('。')
  if (key.action === 'clear' && credential?.canClear === false) return `无法在此清除 ${credential.sourceLabel}。${credential.remedy ?? '请在该原生来源处理。'}`
  if (api && supportsOfficialLogin(api) && api.mode === null && (requireCredential || requireAddress)) return '请选择官方登录或自定义 API。'
  if (!api) return null
  if (requireAddress) try {
    const url = new URL(api.baseUrl)
    if (!['https:', 'http:'].includes(url.protocol) || !url.hostname || url.username || url.password || url.hash) throw new Error()
  } catch { return '请输入有效的 HTTP 或 HTTPS 地址，且不要在地址中包含账号或密码。' }
  if (credential?.source === 'native_cloud' && requireAddress && key.action !== 'replace') return '改用 Messages 接口时请填写该接口的 API Key；原有云厂商认证不会迁移。'
  // Explicit removal is savable; the resulting missing-credential state remains visible.
  if (requireCredential && key.action === 'keep' && !reusableCredential(credential)) return '当前连接没有可复用的凭据，请填写 API Key 或修复原生凭据来源。'
  if (key.action === 'replace' && !key.value.trim()) return '请输入 API Key。'
  if (api.kind === 'codex-cli' && requireModelList) {
    if (!api.models.length || api.models.some((model) => !model.id)) return '请至少添加一个模型，并填写每个模型 ID。'
    if (new Set(api.models.map((model) => model.id)).size !== api.models.length) return '模型 ID 不能重复。'
    if (!api.defaultRowId || !api.models.some((model) => model.rowId === api.defaultRowId)) return '请选择一个默认模型。'
  }
  return null
}

export function runtimeStartupKey(draft: RuntimeStartupConfiguration): string {
  const normalized = normalizedStartupConfiguration(draft)
  return JSON.stringify({ ...normalized, environment: [...normalized.environment].sort((a, b) => a.name.localeCompare(b.name)) })
}

export function runtimeEnvironmentErrors(draft: RuntimeStartupConfiguration, windows: boolean, kind: AdapterKind): Record<number, string> {
  const errors: Record<number, string> = {}
  const names = new Map<string, number>()
  draft.environment.forEach(({ name: raw, value }, index) => {
    const name = raw.trim()
    if (!/^[A-Za-z_][A-Za-z0-9_]{0,255}$/.test(name)) {
      errors[index] = '变量名需以字母或下划线开头，只含字母、数字、下划线。'
    } else if (name.toUpperCase().startsWith('ROVAI_')) {
      errors[index] = 'ROVAI_ 开头的变量由应用管理。'
    } else if ((kind === 'claude-code-cli' ? ['ANTHROPIC_API_KEY', 'ANTHROPIC_AUTH_TOKEN', 'CLAUDE_CODE_OAUTH_TOKEN'] : kind === 'codex-cli' ? ['OPENAI_API_KEY', 'CODEX_API_KEY', 'CODEX_ACCESS_TOKEN'] : []).includes(name.toUpperCase())) {
      errors[index] = '请在原生凭据来源中配置此密钥。'
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
