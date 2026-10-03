import React, { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { ThreadClientProvider } from '@renderer/camp-client'
import { RuntimeStartupSettings } from '@renderer/RuntimeStartupSettings'
import '@renderer/styles.css'
import '@renderer/member-editor.css'

const state = { startup: {}, failure: null }
const requests = []
const labels = {'claude-code-cli':'Claude Code','codex-cli':'Codex CLI','kimi-code-cli':'Kimi Code','grok-build':'Grok Build'}
const client = {
  platform:'darwin', selectRuntimeExecutable:async()=>null,
  request:async(method,params={})=>{
    requests.push({method,params:structuredClone(params)})
    if(state.failure===method) {state.failure=null; throw Error('隔离测试：保存失败，草稿保留。')}
    if(method==='runtime.startup.get') return structuredClone(state.startup[params.runtimeKind]??{runtimeKind:params.runtimeKind,revision:0,apiKeyConfigured:false,configuration:{programPath:null,environment:[]}})
    if(method==='runtime.startup.save') {
      const previous=state.startup[params.runtimeKind]
      if(previous && previous.revision!==params.expectedRevision) throw Error('启动设置已被更新，请重新读取后再保存。')
      const saved={runtimeKind:params.runtimeKind,revision:params.expectedRevision+1,configuration:structuredClone(params.configuration),apiKeyConfigured:params.apiKey.action==='clear'?false:params.apiKey.action==='replace'?true:previous?.apiKeyConfigured??false}
      state.startup[params.runtimeKind]=saved
      return structuredClone(saved)
    }
    throw Error('Unexpected request: '+method)
  }
}
let navigate
function App(){
  const [kind,setKind]=useState(null)
  navigate=()=>setKind(null)
  return <ThreadClientProvider client={client}><main className="content settings-content" style={{height:'100vh'}}><section className="settings-workbench"><div className="settings-panel">
    {kind?<RuntimeStartupSettings key={kind} runtimeKind={kind} health={null} onBack={()=>setKind(null)} onReload={async()=>{}}/>:<div>{Object.entries(labels).map(([kind,label])=><button key={kind} className="runtime-product-settings" aria-label={`${label} 启动设置`} onClick={()=>setKind(kind)}>{label}</button>)}</div>}
  </div></section></main></ThreadClientProvider>
}
window.settingsTest={state,requests,navigate:(...args)=>navigate(...args),settle:()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(()=>setTimeout(resolve,25))))}
document.documentElement.dataset.theme='day'
createRoot(document.getElementById('root')).render(<App/> )
