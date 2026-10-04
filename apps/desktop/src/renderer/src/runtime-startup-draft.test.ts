import type { RuntimeNativeCredential, RuntimeCustomApiConfiguration, RuntimeStartupSettings } from '@contracts'
import { describe, expect, it } from 'vitest'
import { customApiError, emptyCustomApi, normalizedStartupConfiguration, runtimeEnvironmentErrors, runtimeStartupKey, startupEdits, startupSubmission } from './runtime-startup-draft'

describe('startup editor draft contract', () => {
  it('treats returning to the saved values as clean and preserves empty or spaced values', () => {
    const saved = { programPath: null, environment: [{ name: 'TOKEN', value: '  value  ' }, { name: 'EMPTY', value: '' }] }
    const reordered = { ...saved, environment: [...saved.environment].reverse() }
    expect(runtimeStartupKey(saved)).toBe(runtimeStartupKey(reordered))
    const changed = { ...saved, environment: [...saved.environment, { name: 'HTTP_PROXY', value: 'http://localhost:8080' }] }
    expect(runtimeStartupKey(changed)).not.toBe(runtimeStartupKey(saved))
    expect(normalizedStartupConfiguration(saved)).toEqual(saved)
  })
  it('reports invalid and duplicate variable names while allowing valid drafts to save without probing', () => {
    const draft = { programPath: null, environment: [{ name: 'TOKEN', value: '' }, { name: 'token', value: 'value' }] }
    expect(runtimeEnvironmentErrors(draft, false, 'pi')).toEqual({})
    expect(Object.keys(runtimeEnvironmentErrors(draft, true, 'pi'))).toEqual(['0', '1'])
    for (const name of ['ANTHROPIC_API_KEY', 'OPENAI_API_KEY']) {
      const keyDraft = { ...draft, environment: [{ name, value: 'fixture-key' }] }
      for (const kind of ['pi', 'kimi-code-cli', 'grok-build'] as const) expect(runtimeEnvironmentErrors(keyDraft, false, kind)).toEqual({})
      expect(Boolean(runtimeEnvironmentErrors(keyDraft, false, 'claude-code-cli')[0])).toBe(name === 'ANTHROPIC_API_KEY')
      expect(Boolean(runtimeEnvironmentErrors(keyDraft, false, 'codex-cli')[0])).toBe(name === 'OPENAI_API_KEY')
    }
    for (const name of ['', '1KEY', 'BAD-KEY', 'ROVAI_CONTEXT']) {
      expect(runtimeEnvironmentErrors({ ...draft, environment: [{ name, value: '' }] }, false, 'pi')[0]).toBeTruthy()
    }
  })
  it('validates a single connection locally without requiring online model verification', () => {
    const key = { action: 'keep' } as const
    const credential: RuntimeNativeCredential = { status: 'available', source: 'environment_reference', sourceLabel: 'RELAY_KEY', version: 'v1', sourceWritable: false, canReplace: true, canClear: true, restriction: null, remedy: null }
    const codex: Extract<RuntimeCustomApiConfiguration, { kind: 'codex-cli' }> = { kind: 'codex-cli', mode: 'custom_api', baseUrl: 'https://offline.invalid/prefix', models: [{ rowId: 'one', id: 'private-id', displayName: '' }, { rowId: 'two', id: 'private-id-2', displayName: '' }], defaultModel: 'private-id-2', defaultRowId: 'two' }
    const draft = { programPath: null, environment: [], customApi: { ...codex, models: [...codex.models] } }
    expect(customApiError(draft, key, credential)).toBeNull()
    const inherited = { ...draft, customApi: { ...codex, models: [], defaultRowId: null, defaultModel: '' } }
    expect(customApiError(inherited, key, credential, false)).toBeNull()
    expect(customApiError(inherited, key, credential, true)).toContain('至少添加一个模型')
    expect(customApiError(draft, key, undefined)).toContain('API Key')
    for (const status of ['missing', 'invalid_reference'] as const) {
      expect(customApiError(draft, key, { ...credential, status }, true, false)).toBeNull()
    }
    expect(customApiError(draft, { action: 'clear' }, credential)).toBeNull()
    expect(customApiError({ ...draft, customApi: { ...draft.customApi, models: [draft.customApi.models[0]] } }, key, credential)).toContain('默认模型')
    expect(customApiError({ ...draft, customApi: { ...draft.customApi, models: [draft.customApi.models[0], draft.customApi.models[0]] } }, key, credential)).toContain('重复')
    expect(customApiError({ ...draft, customApi: { ...draft.customApi, baseUrl: 'https://user:pass@offline.invalid' } }, key, credential)).toContain('地址')
    expect(customApiError({ ...draft, customApi: { ...draft.customApi, mode: 'official_login', models: [] } }, { action: 'clear' }, credential)).toBeNull()
    const claude: RuntimeCustomApiConfiguration = { kind: 'claude-code-cli', mode: 'custom_api', models: { model: '', reasoningModel: '', haikuModel: '', sonnetModel: '', opusModel: '' }, baseUrl: 'http://localhost:1234/prefix' }
    expect(customApiError({ ...draft, customApi: claude }, { action: 'replace', value: 'fake-local-key' }, undefined)).toBeNull()
    expect(emptyCustomApi('pi')).toBeNull()
    expect(emptyCustomApi('kimi-code-cli')).toBeNull()
    expect(emptyCustomApi('grok-build')).toBeNull()
    const renamed = { ...draft, customApi: { ...draft.customApi, models: draft.customApi.models.map(row => row.rowId === 'two' ? { ...row, id: 'renamed' } : row) } }
    expect(normalizedStartupConfiguration(renamed).customApi).toMatchObject({ defaultRowId: 'two', defaultModel: 'renamed' })
    for (const baseUrl of ['https://offline.invalid/prefix/', 'https://another.invalid/prefix']) expect(customApiError({ ...draft, customApi: { ...draft.customApi, baseUrl } }, key, credential)).toBeNull()
    expect(customApiError(draft, { action: 'replace', value: 'new-key' }, credential)).toBeNull()
    const official = { ...draft, customApi: { ...codex, mode: 'official_login' as const, baseUrl: 'invalid', models: [] } }
    expect(customApiError(official, { action: 'replace', value: 'bad key\n' }, { ...credential, canReplace: false })).toBeNull()
    const saved: RuntimeStartupSettings = { runtimeKind: 'codex-cli', revision: 1, configuration: draft, credential, nativeRevision: 'native-1', connectionObservation: null, connectionReadError: null, reconnectRequired: false, nativeWritten: false }
    const keyDraft = { action: 'replace', value: 'memory-only-key' } as const
    expect(startupSubmission(saved, draft, { action: 'replace', value: ' \t padded-key \r\n' }).apiKey).toEqual({ action: 'replace', value: 'padded-key' })
    const submission = startupSubmission(saved, official, keyDraft)
    expect(submission.apiKey).toEqual({ action: 'keep' })
    expect(submission.edits.map(edit => edit.path[0])).toEqual(['mode', 'nativeRevision'])
    expect(JSON.stringify(submission)).not.toContain('memory-only-key')
    // The full editing session still owns hidden changes for retries and conflict rebasing.
    expect(startupEdits(saved, official, keyDraft).map(edit => edit.path[0])).toContain('baseUrl')
    expect(startupEdits(saved, official, keyDraft).map(edit => edit.path[0])).toContain('credentialVersion')
    expect(startupSubmission(saved, { ...draft, customApi: { ...codex, mode: 'official_login' } }, key).edits).toHaveLength(2)
    expect(startupSubmission(saved, { ...draft, customApi: { ...codex, mode: 'custom_api' } }, key).edits).toEqual([])
    expect(startupSubmission(saved, renamed, key).edits).toContainEqual({ path: ['mode'], before: 'custom_api', after: 'custom_api', label: '连接方式' })
  })

})
