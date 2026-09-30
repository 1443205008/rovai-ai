// Explicit, isolated real Runtime acceptance. Native content is relayed unchanged;
// only field names, identities, numeric counts and timings are retained by the probe.
import { mkdtemp, mkdir, writeFile, chmod, readFile, realpath, copyFile, cp } from 'node:fs/promises'
import { tmpdir, homedir } from 'node:os'
import { join, resolve, dirname } from 'node:path'
import { execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { performance } from 'node:perf_hooks'
import { startQualificationCore } from './lib/qualification-core.mjs'
import { configureProductRuntime } from './configure-product-runtime.mjs'
import { createConfiguredCampAndSend, composerDocumentForAddress } from './lib/create-configured-camp.mjs'
import { LiveTokenSpeedDisplay } from '../apps/desktop/src/renderer/src/execution-token-speed.ts'
import { querySqliteRows } from './lib/sqlite.mjs'

const repository = resolve(import.meta.dirname, '..')
const kind = process.argv[2]
const commands = {
  'codex-cli': ['codex', 'ROVAI_CODEX_BIN'],
  'claude-code-cli': ['claude', 'ROVAI_CLAUDE_CODE_BIN'],
  'opencode-cli': ['opencode', 'ROVAI_OPENCODE_BIN'],
  'codebuddy-cli': ['codebuddy', 'ROVAI_CODEBUDDY_BIN'],
  'qwen-code': ['qwen', 'ROVAI_QWEN_BIN'],
  pi: ['pi', 'ROVAI_PI_BIN'],
  'kimi-code-cli': ['kimi', 'ROVAI_KIMI_BIN'],
  'grok-build': ['grok', 'ROVAI_GROK_BIN'],
  'deepseek-harness': ['dsh', 'ROVAI_DEEPSEEK_HARNESS_BIN'],
  'qoder-cli': ['qoder', 'ROVAI_QODER_BIN'],
  'kiro-cli': ['kiro-cli', 'ROVAI_KIRO_BIN'],
  'trae-cn-cli': ['trae-cli', 'ROVAI_TRAE_CN_BIN'],
  'zcode-app': null,
  'antigravity-app': null
}
if (!Object.hasOwn(commands, kind)) throw new Error('Select an in-scope Runtime')
const fixture = await realpath(process.env.ROVAI_OBSERVABLE_FIXTURE_ROOT ?? await mkdtemp(join(tmpdir(), `rovai-observable-${kind}-`)))
const data = join(fixture, 'user-data'), workspacePath = join(fixture, 'workspace')
const rawPath = join(fixture, 'native-shapes.jsonl')
const coreSource = process.env.ROVAI_OBSERVABLE_CORE ?? join(repository, 'resources/bin/macos-arm64/rovai-core')
const fixtureCore = join(fixture, 'rovai-core')
await copyFile(coreSource, fixtureCore); await chmod(fixtureCore, 0o700)
await copyFile(join(dirname(coreSource), 'rovai'), join(fixture, 'rovai'))
await chmod(join(fixture, 'rovai'), 0o700)
const coreDigest = createHash('sha256').update(await readFile(fixtureCore)).digest('hex')
await mkdir(data); await mkdir(workspacePath)
await mkdir(join(data, 'managed-skill-library')); await writeFile(join(data, 'mcp.json'), '{}')
console.log(JSON.stringify({ kind, fixture, coreDigest, channel: 'automatic_acceptance', data, skillLibrary: join(data, 'managed-skill-library'), mcp: join(data, 'mcp.json') }))
if (kind === 'pi') {
  const piHome = join(fixture, 'pi-agent')
  await mkdir(piHome, { mode: 0o700 })
  for (const name of ['auth.json', 'settings.json', 'models.json']) {
    await copyFile(join(homedir(), '.pi', 'agent', name), join(piHome, name))
    await chmod(join(piHome, name), 0o600)
  }
  process.env.PI_CODING_AGENT_DIR = piHome
  if (process.env.ROVAI_OBSERVABLE_THINKING_LEVEL) {
    const settingsPath = join(piHome, 'settings.json')
    const settings = JSON.parse(await readFile(settingsPath, 'utf8'))
    settings.defaultThinkingLevel = process.env.ROVAI_OBSERVABLE_THINKING_LEVEL
    await writeFile(settingsPath, JSON.stringify(settings), { mode: 0o600 })
  }
}
if (process.env.ROVAI_OBSERVABLE_NATIVE_HOME) {
  const nativeHome = join(fixture, 'native-home')
  await mkdir(nativeHome, { mode: 0o700 })
  const names = kind === 'grok-build' ? ['config.toml'] : ['settings.yaml', 'cordis.patch.yml']
  for (const name of names) {
    try {
      await copyFile(join(process.env.ROVAI_OBSERVABLE_NATIVE_HOME, name), join(nativeHome, name))
      await chmod(join(nativeHome, name), 0o600)
    } catch (error) { if (error.code !== 'ENOENT') throw error }
  }
  if (kind === 'grok-build') process.env.GROK_HOME = nativeHome
  if (kind === 'deepseek-harness') {
    try { await cp(join(process.env.ROVAI_OBSERVABLE_NATIVE_HOME, 'profiles'), join(nativeHome, 'profiles'), { recursive: true }) } catch (error) { if (error.code !== 'ENOENT') throw error }
    process.env.DSH_HOME = nativeHome; process.env.DSH_AGENTS_HOME = join(fixture, 'agents-home')
  }
  if (['grok-build', 'deepseek-harness'].includes(kind)) {
    const claude = JSON.parse(await readFile(join(homedir(), '.claude/settings.json'), 'utf8')).env
    const token = claude.ANTHROPIC_AUTH_TOKEN ?? claude.ANTHROPIC_API_KEY
    const origin = new URL(claude.ANTHROPIC_BASE_URL).origin
    // These fixtures use the same authorised sub2api origin as Claude. A copied
    // env-key reference is resolved in memory; never print or inline the key.
    if (kind === 'grok-build') {
      const configPath = join(nativeHome, 'config.toml')
      let config = await readFile(configPath, 'utf8')
      const base = config.match(/^base_url\s*=\s*"([^"]+)"/m)?.[1]
      if (!base || new URL(base).origin !== origin) throw new Error('Probe provider origin does not match authorised sub2api')
      const keyName = config.match(/^env_key\s*=\s*"([A-Z_]+)"/m)?.[1]
      if (keyName) {
        if (!token) throw new Error('Probe credential reference is unavailable')
        process.env[keyName] = token
      }
      config = config.replace(/^model\s*=\s*"[^"]+"/m, `model = ${JSON.stringify(claude.ANTHROPIC_MODEL)}`)
      await writeFile(configPath, config, { mode: 0o600 })
    } else {
      const configPath = join(nativeHome, 'settings.yaml')
      let config = await readFile(configPath, 'utf8')
      const base = config.match(/baseURL:\s*(\S+)/)?.[1]
      if (!base || new URL(base).origin !== origin) throw new Error('Probe provider origin does not match authorised sub2api')
      const keyName = config.match(/apiKeyEnv:\s*(\S+)/)?.[1]
      if (!keyName || !token) throw new Error('Probe credential reference is unavailable')
      process.env[keyName] = token
      config = config.replaceAll('gpt-6-sol', claude.ANTHROPIC_MODEL)
      if (process.env.ROVAI_OBSERVABLE_THINKING_LEVEL) {
        const model = claude.ANTHROPIC_MODEL
        const effort = process.env.ROVAI_OBSERVABLE_THINKING_LEVEL
        // The copied custom route has no installed catalog metadata. Declare
        // only the effort being exercised, in the supported native settings.
        config = config.replace(new RegExp(`(id: ${model.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}\\s*\\n)([ ]+)`),
          `$1$2reasoningEfforts: { ${effort}: ${effort} }\n$2`)
      }
      await writeFile(configPath, config, { mode: 0o600 })
      const patchPath = join(nativeHome, 'cordis.patch.yml')
      try {
        const patch = (await readFile(patchPath, 'utf8')).replaceAll('gpt-6-sol', claude.ANTHROPIC_MODEL)
        await writeFile(patchPath, patch, { mode: 0o600 })
      } catch (error) { if (error.code !== 'ENOENT') throw error }
    }
  }
}
if (kind === 'kimi-code-cli' && process.env.ROVAI_OBSERVABLE_SUB2API === '1') {
  const claude = JSON.parse(await readFile(join(homedir(), '.claude/settings.json'), 'utf8')).env
  const origin = new URL(claude.ANTHROPIC_BASE_URL).origin
  const token = claude.ANTHROPIC_AUTH_TOKEN ?? claude.ANTHROPIC_API_KEY
  if (!token || !claude.ANTHROPIC_MODEL) throw new Error('Authorised Claude provider configuration is incomplete')
  // Core's explicit provider file overrides inherited env. Use its supported
  // path override instead of accidentally testing the daily MiniMax config.
  const path = join(fixture, 'kimi-code.env')
  await writeFile(path, [
    `KIMI_MODEL_NAME=${claude.ANTHROPIC_MODEL}`,
    'KIMI_MODEL_PROVIDER_TYPE=openai',
    `KIMI_MODEL_API_KEY=${token}`,
    `KIMI_MODEL_BASE_URL=${origin}/v1`
  ].join('\n') + '\n', { mode: 0o600 })
  process.env.ROVAI_KIMI_CONFIG = path
}
let nativeExecutable = null
if (commands[kind]) {
  const [command, override] = commands[kind]
  nativeExecutable = await realpath(execFileSync('/usr/bin/which', [command], { encoding: 'utf8' }).trim())
  const wrapper = join(fixture, 'native-observer')
  await writeFile(wrapper, `#!/usr/bin/env python3
import sys,subprocess,threading,json,time
child=subprocess.Popen([${JSON.stringify(nativeExecutable)}]+sys.argv[1:],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
def forward_input():
    try:
        for line in sys.stdin.buffer:
            child.stdin.write(line);child.stdin.flush()
        child.stdin.close()
    except (BrokenPipeError,ValueError): pass
threading.Thread(target=forward_input,daemon=True).start()
def discard_diagnostics():
    for line in child.stderr: pass
threading.Thread(target=discard_diagnostics,daemon=True).start()
numeric_keys={'input_tokens','output_tokens','prompt_tokens','completion_tokens','total_tokens','cached_tokens','cache_read_input_tokens','cache_creation_input_tokens','cache_write_input_tokens','reasoning_tokens','thinking_tokens','used','size','contextWindow','context_window','modelContextWindow','inputTokens','outputTokens','totalTokens','cachedInputTokens','cacheWriteInputTokens','input','output','cacheRead','cacheWrite'}
def numeric(v,path='',depth=0):
    if depth>6 or not isinstance(v,dict): return {}
    result={}
    for key,value in v.items():
        if key in {'content','delta','text','thinking','summary','arguments','output','input','systemPrompt'} and isinstance(value,(dict,list,str)): continue
        p=path+'/'+key
        if key in numeric_keys and isinstance(value,(int,float)) and not isinstance(value,bool): result[p]=value
        elif isinstance(value,dict): result.update(numeric(value,p,depth+1))
    return result
identities={}
def identity(v):
    if not isinstance(v,(str,int)): return None
    k=str(v)
    if k not in identities:
        if len(identities)>=512: return 'capacity-exceeded'
        identities[k]='identity-'+str(len(identities)+1)
    return identities[k]
def managed_context(v):
    if v.get('type')!='extension_ui_request' or v.get('method')!='setStatus' or v.get('statusKey')!='rovai-managed-context-usage': return None
    try:
        status=json.loads(v.get('statusText',''))
        return {k:status[k] for k in ['usedTokens','windowTokens','provider','modelId'] if k in status}
    except (ValueError,TypeError): return None
with open(${JSON.stringify(rawPath)},'a',buffering=1) as out:
    for line in child.stdout:
        try:
            v=json.loads(line); p=v.get('params') or {}; u=p.get('update') or {}; a=v.get('assistantMessageEvent') or {}; e=v.get('event') or {}; d=v.get('delta') or {}
            if not isinstance(p,dict): p={}
            text=p.get('delta') or u.get('content',{}).get('text') or a.get('delta') or d.get('thinking') or d.get('text') or ''
            if isinstance(e,dict) and isinstance(e.get('delta'),dict): text=e['delta'].get('thinking') or e['delta'].get('text') or text
            if not isinstance(text,str): text=''
            item=p.get('item') or {}
            out.write(json.dumps({'atMs':round(time.monotonic()*1000),'method':v.get('method'),'type':v.get('type'),'sessionUpdate':u.get('sessionUpdate'),'deltaType':a.get('type') or d.get('type') or (e.get('delta') or {}).get('type'),'keys':sorted(v.keys()),'paramsKeys':sorted(p.keys()),'updateKeys':sorted(u.keys()),'contentKeys':sorted((u.get('content') or {}).keys()),'eventKeys':sorted(e.keys()),'itemId':identity(p.get('itemId') or item.get('id') or u.get('messageId') or (e.get('message') or {}).get('id')),'turnId':identity(p.get('turnId')),'summaryIndex':p.get('summaryIndex'),'contentIndex':p.get('contentIndex',a.get('contentIndex')),'textOffset':p.get('textOffset',u.get('textOffset')),'parentPresent':any(u.get(k)!=None for k in ['agentId','sourceAgentId','subagentId','parentAgentId','parentSessionId']) or v.get('parent_tool_use_id')!=None,'usageFields':numeric(v),'managedContext':managed_context(v),'bytes':len(text.encode('utf-8')),'scalars':len(text)})+'\\n')
        except Exception: pass
        sys.stdout.buffer.write(line);sys.stdout.buffer.flush()
sys.exit(child.wait())
`, { mode: 0o700 })
  process.env[override] = wrapper
}
const events = [], samples = [], displays = [], metrics = [], runs = []
const started = performance.now()
const core = startQualificationCore({
  coreExecutable: fixtureCore,
  dataDirectory: data, workingDirectory: repository, runtimeCacheDirectory: join(fixture, 'cache'),
  mcpConfigPath: join(data, 'mcp.json'), onNotification(event) {
    if (event.method.startsWith('agent_run.') && !['agent_run.log', 'agent_run.execution_changed'].includes(event.method)) {
      events.push({ atMs: Math.round(performance.now() - started), method: event.method })
    }
  }
})
let run = null, campId = null, installation = null, failure = null
let failureDetail = null
try {
  await core.request('health.check')
  let configurationDeadline
  try {
    installation = await Promise.race([
      configureProductRuntime(core.request, kind, ['agent_1']),
      new Promise((_, reject) => { configurationDeadline = setTimeout(() => reject(new Error('timed out configuring Runtime')), 90000) })
    ])
  } finally { clearTimeout(configurationDeadline) }
  if (process.env.ROVAI_OBSERVABLE_MODEL) {
    const profile = await core.request('members.get', { agentId: 'agent_1' })
    const options = process.env.ROVAI_OBSERVABLE_MODEL_OPTIONS ? JSON.parse(process.env.ROVAI_OBSERVABLE_MODEL_OPTIONS) : {}
    const changed = await core.request('members.runtime.set', { commandId: crypto.randomUUID(), command: {
      agentId: 'agent_1', expectedVersion: profile.version, adapterKind: kind,
      permissions: profile.runtimeConfiguration.permissions,
      model: { mode: 'explicit', modelId: process.env.ROVAI_OBSERVABLE_MODEL, options }
    } })
    if (changed.status !== 'applied') throw new Error('Explicit probe model was not applied')
  }
  const workspace = await core.request('workspaces.inspect', { path: workspacePath })
  const accepted = await createConfiguredCampAndSend(core.request, { commandId: crypto.randomUUID(), workspace,
    memberAgentIds: ['agent_1'], defaultLeadAgentId: 'agent_1',
    body: (process.env.ROVAI_OBSERVABLE_PROMPT_FILE ? await readFile(process.env.ROVAI_OBSERVABLE_PROMPT_FILE, 'utf8') : process.env.ROVAI_OBSERVABLE_PROMPT) ?? 'This is an isolated output metrics acceptance. Analyze a resilient library catalogue design: discuss consistency, retries, concurrency, Unicode, recovery, and observability. Emit three substantial public assistant commentary sections, about 800 English words each, outside tool arguments, while you work. Between sections run the shell command sleep 6 once so the acceptance observes a tool pause and resumed output. Do not delegate or modify files. Finally use rovai send --public-only for a brief completion, following Session Charter. Finish normally.',
    purpose: 'Long real output and native observable reasoning acceptance' })
  campId = accepted.payload.campId
  const deadline = performance.now() + Number(process.env.ROVAI_OBSERVABLE_TIMEOUT_MS ?? 480000)
  let meter = null, nextProgress = performance.now() + 30000
  while (performance.now() < deadline) {
    const snapshot = await core.request('camps.snapshot', { campId }, 15000)
    run = snapshot.agentRuns.find(candidate => !runs.some(old => old.id === candidate.id))
    if (run) {
      const now = performance.now()
      meter ??= new LiveTokenSpeedDisplay(now)
      const value = await core.request('monitoring.observableOutput', { campId, agentRunId: run.id, executionEpoch: run.executionEpoch }, 15000)
      meter.observe(value, now)
      const display = meter.sample(now)
      samples.push({ atMs: Math.round(now - started), status: run.status, value })
      if (display !== undefined) displays.push({ atMs: Math.round(now - started), value: display })
      const projection = await core.request('monitoring.execution', { campId, agentRunIds: [run.id] }, 15000)
      if (JSON.stringify(projection) !== JSON.stringify(metrics.at(-1)?.projection)) {
        metrics.push({ atMs: Math.round(now - started), status: run.status, projection })
      }
      if (['succeeded', 'failed', 'cancelled'].includes(run.status)) {
        runs.push({ id: run.id, executionEpoch: run.executionEpoch, status: run.status, projection })
        if (process.env.ROVAI_OBSERVABLE_RESUME === '1' && runs.length === 1 && run.status === 'succeeded') {
          await core.request('camp.messages.send', { commandId: crypto.randomUUID(), campId,
            content: composerDocumentForAddress({ mode: 'default' }, 'Continue in this same native session for a second isolated metrics acceptance. Explain retry ownership in about 250 words, run sleep 3 once, then briefly describe recovery. Do not delegate or change files. Send a short completion with rovai send --public-only and finish normally.'),
            sourceAttachments: [], quotes: [], replyToCampMessageId: null,
            execution: { taskId: null, purpose: 'Same Session successor Usage baseline', completionRole: 'required' } })
          meter = null
          continue
        }
        break
      }
      if (now >= nextProgress) {
        console.log(JSON.stringify({ kind, stage: 'live', status: run.status, publicUnits: value?.publicTextUnits, reasoningUnits: value?.reasoningUnits }))
        nextProgress = now + 30000
      }
    }
    await new Promise(done => setTimeout(done, 500))
  }
  if (!['succeeded', 'failed', 'cancelled'].includes(run?.status)) failure = 'acceptance_deadline'
} catch (error) {
  // Do not include arbitrary provider error payloads, paths or credentials.
  failure = error.message.startsWith('timed out') ? error.message : error.message.split(':')[0].slice(0, 160)
  failureDetail = error.message.replace(/https?:\/\/\S+/g, '<endpoint>')
    .replace(/(?:\/[\w.@ -]+){2,}/g, '<path>')
    .replace(/[A-Za-z0-9_+\/=.-]{16,}/g, '<identifier>')
    .replace(/(?:api[_ -]?key|token|bearer|secret|password)\s*[:=]?\s*\S+/gi, '<credential>')
    .slice(0, 1200)
  events.push({ method: 'probe.failure', categories: ['model', 'auth', 'quota', 'permission', 'protocol', 'version', 'executable', 'unsupported', 'config'].filter(word => error.message.toLowerCase().includes(word)) })
} finally {
  const stopped = await core.stop()
  const frozen = (await querySqliteRows(join(data, 'rovai.sqlite'),
    'SELECT runtime_observed_model_id, runtime_model_selection_json, public_runtime_failure_json FROM agent_run ORDER BY started_at DESC LIMIT 1'))[0]
  let raw = []
  try { raw = (await readFile(rawPath, 'utf8')).trim().split('\n').filter(Boolean).map(line => JSON.parse(line)) } catch {}
  const groups = {}
  for (const event of raw) {
    const key = [event.method, event.type, event.sessionUpdate, event.deltaType].filter(Boolean).join('/')
    groups[key] ??= { count: 0, textScalars: 0, firstAtMs: event.atMs, lastAtMs: event.atMs }
    groups[key].count++; groups[key].textScalars += event.scalars
    groups[key].firstAtMs = Math.min(groups[key].firstAtMs, event.atMs)
    groups[key].lastAtMs = Math.max(groups[key].lastAtMs, event.atMs)
  }
  const report = { kind, fixture, coreDigest, status: run?.status ?? null, failure, version: installation?.snapshot?.reportedVersion ?? null,
    model: frozen?.runtime_observed_model_id ?? (frozen?.runtime_model_selection_json ? JSON.parse(frozen.runtime_model_selection_json) : null),
    requestedModel: frozen?.runtime_model_selection_json ? JSON.parse(frozen.runtime_model_selection_json) : null,
    observedModel: frozen?.runtime_observed_model_id ?? null,
    publicFailure: frozen?.public_runtime_failure_json ? (() => {
      const failure = JSON.parse(frozen.public_runtime_failure_json)
      return { code: failure.code, origin: failure.origin, phase: failure.phase,
        categories: ['model', 'reasoning', 'auth', 'quota', 'permission', 'protocol', 'version', 'executable', 'unsupported', 'config']
          .filter(word => (failure.detail ?? '').toLowerCase().includes(word)) }
    })() : null,
    rawGroups: groups,
    publicUnits: Math.max(0, ...samples.map(s => s.value?.publicTextUnits ?? 0)),
    reasoningUnits: Math.max(0, ...samples.map(s => s.value?.reasoningUnits ?? 0)),
    reasoningSources: [...new Set(samples.map(s => s.value?.reasoningSource).filter(Boolean))],
    streamConfirmed: samples.some(s => s.value?.streamConfirmed),
    meterDisplayCount: displays.filter(s => s.value !== null).length,
    rendererVerified: false, stopped: stopped.code === 0 }
  report.runs = runs
  report.metrics = metrics
  report.failureDetail = failureDetail
  await writeFile(join(fixture, 'evidence.json'), JSON.stringify({ report, samples, displays, events }, null, 2))
  console.log(JSON.stringify({ ...report, metrics: undefined }))
}
