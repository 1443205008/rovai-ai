import React, { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { ThreadClientProvider } from '@renderer/camp-client'
import { RuntimeStartupSettings } from '@renderer/RuntimeStartupSettings'
import { editableSnapshot, configurationFromSnapshot, snapshotValue, withSnapshotValue } from '@renderer/runtime-connection-editor'
import '@renderer/styles.css'
import '@renderer/member-editor.css'

const clone = value => structuredClone(value)
const receipt = value => { const result=clone(value); if(result.credential)delete result.credential.value; return result }
const settings = kind => ({runtimeKind:kind,revision:0,nativeRevision:'native-0',connectionReadError:null,reconnectRequired:false,nativeWritten:false,
  credential:{status:'available',source:'native_file',sourceLabel:'fixture native config',version:'key-1',sourceWritable:true,canReplace:true,canClear:true,restriction:null,remedy:null,value:'fixture-native-key'},
  connectionObservation:{initialMode:'custom_api',loginStatus:'signed_in',conflict:null},
  configuration:{programPath:null,environment:[],customApi:kind==='codex-cli'?{kind,mode:'custom_api',baseUrl:'https://relay.example/prefix',models:[{rowId:'one',id:'model-a',displayName:'开发模型'},{rowId:'two',id:'model-b',displayName:''}],defaultModel:'model-a',defaultRowId:'one'}:{kind,mode:'custom_api',baseUrl:'https://relay.example/prefix',models:{model:'claude-main',reasoningModel:'think',haikuModel:'small',sonnetModel:'medium',opusModel:'large'}}}})
const state={startup:Object.fromEntries(['claude-code-cli','codex-cli'].map(kind=>[kind,settings(kind)])),failure:null}
const requests=[]
const client={platform:'darwin',selectRuntimeExecutable:async()=>state.selectedExecutable??null,request:async(method,params={})=>{
  requests.push({method,params:clone(params)})
  if(state.failure===method){state.failure=null;throw Error('隔离测试：读取或保存失败，草稿保留。')}
  const saved=state.startup[params.runtimeKind]
  if(method==='runtime.startup.get')return clone(saved)
  if(method==='runtime.startup.observe'){
    if(state.holdObservation) await new Promise(resolve=>{state.releaseObservation=resolve})
    if(state.observationFailure) throw Error('optional native observation unavailable')
    return clone(state.observed??saved)
  }
  if(method==='runtime.startup.save'){
    if(state.holdSave) await new Promise(resolve=>{state.releaseSave=resolve})
    let current=editableSnapshot(saved.configuration);current.credentialVersion=saved.credential.version;current.nativeRevision=saved.nativeRevision
    const conflicts=params.edits.filter(edit=>JSON.stringify(snapshotValue(current,edit.path))!==JSON.stringify(edit.before)&&JSON.stringify(snapshotValue(current,edit.path))!==JSON.stringify(edit.after)).map(edit=>({...edit,current:snapshotValue(current,edit.path)}))
    if(conflicts.length)return {status:'conflict',latest:receipt(saved),conflicts}
    for(const edit of params.edits)current=withSnapshotValue(current,edit.path,edit.after)
    saved.configuration=configurationFromSnapshot(saved.configuration,current)
    if(current.mode==='official_login'){
      saved.configuration.customApi.baseUrl=''
      if(params.runtimeKind==='claude-code-cli')saved.configuration.customApi.models={model:'',reasoningModel:'',haikuModel:'',sonnetModel:'',opusModel:''}
      else Object.assign(saved.configuration.customApi,{models:[],defaultModel:'',defaultRowId:null})
      saved.credential.status='missing'
      saved.credential.value=null
    }
    if(params.apiKey.action!=='keep'){saved.credential.version+='-next';saved.credential.status=params.apiKey.action==='clear'?'missing':'available';saved.credential.value=params.apiKey.action==='clear'?null:params.apiKey.value}
    saved.nativeWritten=params.edits.some(edit=>!['environment','programPath'].includes(edit.path[0]))
    if(!state.keepNativeRevision&&(saved.nativeWritten||params.edits.some(edit=>edit.path[0]==='programPath'||edit.path[0]==='environment'&&['CODEX_HOME','HOME','USERPROFILE'].includes(edit.path[1])))) saved.nativeRevision+='-next'
    saved.revision++;saved.reconnectRequired=true
    return receipt(saved)
  }
  if(method==='runtime.startup.inspect')return{status:'recognized',executablePath:'/fixture/runtime',reportedVersion:'fixture'}
  throw Error('Unexpected request: '+method)
}}
let navigate
function App(){const[kind,setKind]=useState(null);navigate=(kind=null)=>setKind(kind);return <ThreadClientProvider client={client}><main className="content settings-content" style={{height:'100vh'}}><section className="settings-workbench"><div className="settings-panel">{kind?<RuntimeStartupSettings key={kind} runtimeKind={kind} health={null} onBack={()=>setKind(null)} onReload={async()=>{state.reloadCalls=(state.reloadCalls??0)+1;throw Error("unrelated reload unavailable")}}/>:<div>{Object.entries({'claude-code-cli':'Claude Code','codex-cli':'Codex'}).map(([kind,label])=><button key={kind} onClick={()=>setKind(kind)}>{label}</button>)}</div>}</div></section></main></ThreadClientProvider>}
window.settingsTest={state,requests,reset:kind=>{state.startup[kind]=settings(kind)},navigate:(...args)=>navigate(...args),settle:()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(()=>setTimeout(resolve,25))))}
document.documentElement.dataset.theme='day'
createRoot(document.getElementById('root')).render(<App/> )
