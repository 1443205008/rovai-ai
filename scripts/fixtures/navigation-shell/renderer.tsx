import { useMemo, useState } from 'react'
import { createRoot } from 'react-dom/client'
import { NavigationShell } from '../../../apps/desktop/src/renderer/src/NavigationShell'
import { WindowsApplicationMenu } from '../../../apps/desktop/src/renderer/src/WindowsApplicationMenu'
import { ThreadNavigation } from '../../../apps/desktop/src/renderer/src/ThreadNavigation'
import { createDesktopNavigation, type NavigationTarget } from '../../../apps/desktop/src/renderer/src/desktop-navigation'
import { WindowDragStrip } from '../../../apps/desktop/src/renderer/src/App'
import type { NavigationThreadItem, NavigationPin, NavigationSnapshot } from '@contracts'
import '../../../apps/desktop/src/renderer/src/styles.css'
import '../../../apps/web/src/styles.css'

const noop = (): void => undefined
const navigationListeners = new Set<(direction: 'back' | 'forward') => void>()
Object.assign(window, { rovai: { platform: 'win32', windowControls: {
  popupApplicationMenu: async () => undefined,
  onNavigationRequested: (listener: (direction: 'back' | 'forward') => void) => {
    navigationListeners.add(listener)
    return () => navigationListeners.delete(listener)
  }
} } })
let renders = 0
const pinnedCamp = {
  id: 'pinned-camp', title: '置顶对话', activationState: 'active', projectPath: '/fixture/quick-chat',
  projectBindingKind: 'quick_chat', defaultLead: null, marker: 'unread_completed',
  lastActivityAt: '2026-09-17T00:00:02Z', lastActivityGlobalSequence: 2,
  latestCompletionGlobalSequence: 2, version: 1
} satisfies NavigationThreadItem
const ordinaryCamp = {
  ...pinnedCamp, id: 'ordinary-camp', title: '普通对话', marker: 'none',
  lastActivityAt: '2026-09-17T00:00:01Z', lastActivityGlobalSequence: 1,
  latestCompletionGlobalSequence: 0
} satisfies NavigationThreadItem
const navigationSnapshot = {
  schemaVersion: 3,
  throughGlobalSequence: 2,
  quickChat: { totalCount: 2, recentThreads: [pinnedCamp, ordinaryCamp] },
  projects: []
} satisfies NavigationSnapshot
const navigationPins = [{
  kind: 'camp', targetKey: pinnedCamp.id, pinnedAt: '2026-09-17T00:00:03Z'
}] satisfies NavigationPin[]
function Content(): React.JSX.Element { renders++; return <textarea aria-label="保留的草稿" defaultValue="未发送内容" /> }
function Fixture(): React.JSX.Element {
  const [settings, setSettings] = useState(false)
  const [browser, setBrowser] = useState(false)
  const [target, setTarget] = useState<NavigationTarget>({ kind: 'quick_chat' })
  const navigation = useMemo(() => {
    const value = createDesktopNavigation(async (next, transaction) => {
      if (transaction.commit()) { setTarget(next); setSettings(next.kind === 'settings') }
    })
    value.reset({ kind: 'quick_chat' })
    return value
  }, [])
  const [platform, setPlatform] = useState<'win32' | 'darwin'>('win32')
  const [disabled, setDisabled] = useState(false)
  document.documentElement.dataset.rovaiPlatform = browser ? 'browser' : platform
  document.documentElement.dataset.rovaiSurface = browser ? 'web' : 'desktop'
  Object.assign(window, { navigationTest: {
    renders: () => renders, setSettings, setPlatform, setDisabled, setBrowser, navigation, target,
    hostNavigation: (direction: 'back' | 'forward') => navigationListeners.forEach(listener => listener(direction)),
    hostListenerCount: () => navigationListeners.size,
    settle: () => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(() => setTimeout(resolve, 90))))
  } })
  return <><WindowsApplicationMenu /><NavigationShell platform={platform} browser={browser} disabled={disabled} settings={settings} navigation={navigation} nativeWindowControls={browser ? undefined : window.rovai.windowControls}>
    <ThreadNavigation platform={platform} view={settings ? 'settings' : 'compose'} state="ready" navigation={navigationSnapshot} pins={navigationPins} activeThreadId={pinnedCamp.id} pendingMemoryCount={0}
      onNewConversation={noop} onMembers={noop} onMemory={noop} onSettings={() => setSettings(true)} onSettingsBack={() => setSettings(false)}
      onOpenProject={noop} onCamp={noop} onRemoveProject={async () => undefined} onRename={async () => undefined} onDelete={async () => undefined} onError={noop} />
    <WindowDragStrip page={settings ? 'settings' : 'compose'} /><main className={`content task-content ${settings ? 'settings-content' : 'compose-content'}`}><Content /></main>
  </NavigationShell></>
}
createRoot(document.getElementById('root')!).render(<Fixture />)
