import { describe, expect, it } from 'vitest'
import { customApiError, emptyCustomApi, normalizedStartupConfiguration, runtimeEnvironmentErrors, runtimeStartupKey } from './runtime-startup-draft'

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
    expect(runtimeEnvironmentErrors(draft, false)).toEqual({})
    expect(Object.keys(runtimeEnvironmentErrors(draft, true))).toEqual(['0', '1'])
    for (const name of ['', '1KEY', 'BAD-KEY', 'ROVAI_CONTEXT']) {
      expect(runtimeEnvironmentErrors({ ...draft, environment: [{ name, value: '' }] }, false)[0]).toBeTruthy()
    }
  })
  it('validates a single connection locally without requiring online model verification', () => {
    const key = { action: 'keep' } as const
    const codex = { ...emptyCustomApi('codex-cli')!, kind: 'codex-cli', enabled: true, baseUrl: 'https://offline.invalid/prefix', models: [{ id: 'private-id', displayName: '' }, { id: 'private-id-2', displayName: '' }], defaultModel: 'private-id-2' } as const
    const draft = { programPath: null, environment: [], customApi: { ...codex, models: [...codex.models] } }
    expect(customApiError(draft, key, true)).toBeNull()
    expect(customApiError(draft, key, false)).toContain('API Key')
    expect(customApiError(draft, { action: 'clear' }, true)).toContain('API Key')
    expect(customApiError({ ...draft, customApi: { ...draft.customApi, models: [draft.customApi.models[0]] } }, key, true)).toContain('默认模型')
    expect(customApiError({ ...draft, customApi: { ...draft.customApi, models: [draft.customApi.models[0], draft.customApi.models[0]] } }, key, true)).toContain('重复')
    expect(customApiError({ ...draft, customApi: { ...draft.customApi, baseUrl: 'https://user:pass@offline.invalid' } }, key, true)).toContain('地址')
    expect(customApiError({ ...draft, customApi: { ...draft.customApi, enabled: false, models: [] } }, { action: 'clear' }, true)).toBeNull()
    const claude = { ...emptyCustomApi('claude-code-cli')!, enabled: true, baseUrl: 'http://localhost:1234/prefix' }
    expect(customApiError({ ...draft, customApi: claude }, { action: 'replace', value: 'fake-local-key' }, false)).toBeNull()
    expect(emptyCustomApi('pi')).toBeNull()
  })

})
