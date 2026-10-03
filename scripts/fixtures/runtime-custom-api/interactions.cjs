const assert = require('node:assert/strict')
module.exports = async ({window,run,click,settle,waitFor,navigate,capture,noOverflow}) => {
  const save = '.runtime-startup-actions button[type=submit]'
  const discard = '.runtime-startup-actions button[type=button]'
  const toggle = '.runtime-custom-api input[role=switch]'
  const key = '.runtime-custom-api input[type=password]'
  const address = '.runtime-custom-api input[type=url]'
  const fill = async (selector, value) => {
    await click(selector)
    await run(`(() => { const e=document.querySelector(${JSON.stringify(selector)});e.select();document.execCommand('insertText',false,${JSON.stringify(value)}) })()`)
    await settle()
  }
  const open = async label => {
    await navigate('general'); await navigate('runtime')
    await click(`.runtime-product-settings[aria-label="${label} 启动设置"]`)
    await waitFor("document.querySelector('.runtime-startup-form') && document.querySelector('.runtime-startup-page').getAttribute('aria-busy')==='false'")
  }
  await open('Codex CLI')
  assert.equal(await run(`document.querySelector('${toggle}').checked`), false)
  const checksBefore = await run("window.settingsTest.requests.filter(r=>['runtime.startup.check','runtime.startup.inspect'].includes(r.method)).length")
  await click(toggle); await fill(address, 'https://offline.invalid/custom-prefix'); await fill(key, 'isolated-ui-key')
  for (const [index,id] of [[1,'private-a'],[2,'private-b']]) {
    await click('.runtime-custom-api-models > button')
    await fill(`input[aria-label="模型 ID ${index}"]`, id)
  }
  await click('input[aria-label="设为默认模型 private-a"]')
  await click('button[aria-label="删除模型 private-a"]')
  assert.equal(await run("document.querySelectorAll('.runtime-api-model-row').length"), 2)
  assert.ok(await run("document.querySelector('.runtime-custom-api-models [role=alert]').textContent.includes('先指定')"))
  await click('input[aria-label="设为默认模型 private-b"]')
  await run("window.settingsTest.state.failure='runtime.startup.save'")
  await click(save); await waitFor("document.querySelector('.runtime-startup-form .inline-error')")
  assert.equal(await run(`document.querySelector('${key}').value`), 'isolated-ui-key', 'failed save keeps the private draft')
  await click(save); await waitFor(`document.querySelector('${save}').disabled`)
  assert.equal(await run(`document.querySelector('${key}').value`), '')
  assert.equal(await run("window.settingsTest.state.startup['codex-cli'].configuration.customApi.defaultModel"), 'private-b')
  assert.equal(await run("window.settingsTest.state.startup['codex-cli'].apiKeyConfigured"), true)
  await fill('input[aria-label="显示名称 1"]', '开发模型')
  await click(save); await waitFor(`document.querySelector('${save}').disabled`)
  assert.equal(await run("window.settingsTest.requests.filter(r=>r.method==='runtime.startup.save').at(-1).params.apiKey.action"), 'keep')
  assert.equal(await run("window.settingsTest.requests.filter(r=>['runtime.startup.check','runtime.startup.inspect'].includes(r.method)).length"), checksBefore, 'save must not trigger an API/check probe')
  await fill(key, 'discard-this-key'); await click('.runtime-startup-back')
  await waitFor("document.querySelector('[role=dialog]')")
  assert.ok(await run("document.querySelector('[role=dialog]').textContent.includes('尚未保存')"))
  await click('[data-dialog-autofocus]'); await click(discard)
  assert.equal(await run(`document.querySelector('${key}').value`), '')
  await click(toggle); await click(save); await waitFor(`document.querySelector('${save}').disabled`)
  assert.equal(await run("window.settingsTest.state.startup['codex-cli'].apiKeyConfigured"), true, 'disable keeps key')
  await click('.runtime-custom-api-key-actions .danger-text'); await click(save); await waitFor(`document.querySelector('${save}').disabled`)
  assert.equal(await run("window.settingsTest.requests.filter(r=>r.method==='runtime.startup.save').at(-1).params.apiKey.action"), 'clear')
  assert.equal(await run("window.settingsTest.state.startup['codex-cli'].apiKeyConfigured"), false)
  // Reopen each production form with saved fake data for desktop/narrow theme coverage.
  const configs = {
    'claude-code-cli': { models: {model:'relay-main',reasoningModel:'relay-thinking',haikuModel:'relay-haiku',sonnetModel:'relay-sonnet',opusModel:'relay-opus'} },
    'codex-cli': { models: [{id:'private-a',displayName:'开发模型'},{id:'private-b',displayName:'轻量模型'}], defaultModel:'private-b' },
    'kimi-code-cli': { apiType:'openai', model:'private-model' },
    'grok-build': { model:'grok-4.6' }
  }
  for (const [kind,label] of [['claude-code-cli','Claude Code'],['codex-cli','Codex CLI'],['kimi-code-cli','Kimi Code'],['grok-build','Grok Build']]) {
    await run(`window.settingsTest.state.startup[${JSON.stringify(kind)}]={runtimeKind:${JSON.stringify(kind)},revision:4,apiKeyConfigured:true,configuration:{programPath:null,environment:[],customApi:${JSON.stringify({kind,enabled:true,baseUrl:'https://relay.example/custom-prefix',...configs[kind]})}}}`)
    await open(label)
    assert.equal(await run("document.querySelectorAll('.runtime-custom-api input[type=password]').length"), 1)
    assert.equal(await run("document.querySelectorAll('.runtime-custom-api select').length"), kind==='kimi-code-cli'?1:0)
    assert.equal(await run(`document.querySelector('${key}').value`), '')
    if(kind==='claude-code-cli') assert.equal(await run("document.querySelectorAll('.runtime-custom-api input:not([type])').length"), 5)
    for (const theme of ['day','night']) {
      await run(`document.documentElement.dataset.theme=${JSON.stringify(theme)}`)
      for (const [size,width,height] of [['desktop',1440,1100],['narrow',390,1100]]) {
        window.setContentSize(width,height); await settle()
        await run("document.querySelector('.runtime-startup-page').scrollIntoView({block:'start'})"); await settle()
        await noOverflow(`${kind}/${theme}/${size}`); await capture(`custom-api-${kind}-${theme}-${size}`)
      }
    }
    if(kind==='kimi-code-cli') {
      await run("document.querySelector('.runtime-custom-api select').focus()")
      window.webContents.sendInputEvent({type:'keyDown',keyCode:'A'}); window.webContents.sendInputEvent({type:'keyUp',keyCode:'A'})
      await settle(); await click(discard)
    }
  }
  window.setContentSize(1440,920)
  console.error('settings fixture: custom API interactions passed')
}
