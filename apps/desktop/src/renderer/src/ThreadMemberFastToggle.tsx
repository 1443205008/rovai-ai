import * as DropdownMenu from '@radix-ui/react-dropdown-menu'
import { uiAttribute } from './interface-language'
import type { ThreadMemberFastView } from '@contracts'

export function effectiveThreadMemberFast(value: ThreadMemberFastView): boolean {
  return value.fastOverride ?? value.runtimeDefaultFast ?? false
}

export function ThreadMemberFastToggle({
  value, displayName, pending, onToggle
}: {
  value: ThreadMemberFastView
  displayName: string
  pending: boolean
  onToggle(next: boolean | null): void
}): React.JSX.Element {
  const enabled = effectiveThreadMemberFast(value)
  const unknown = value.fastOverride === null
  const stateLabel = unknown ? uiAttribute('跟随 Runtime 默认') : enabled ? uiAttribute('后续执行请求 Fast') : uiAttribute('后续执行请求标准速度')
  return <span className="camp-fast-control">
    <button
      type="button"
      className={`camp-fast-toggle ${enabled ? 'is-on' : ''}`}
      aria-label={uiAttribute("{0}的 Fast，{1}", String(displayName), String(stateLabel))}
      aria-pressed={unknown ? 'mixed' : enabled}
      aria-disabled={pending}
      aria-busy={pending}
      onClick={() => { if (!pending) onToggle(!enabled) }}
    >
      <span className="camp-fast-pill">
        <svg viewBox="0 0 16 16" aria-hidden="true" fill={enabled ? 'currentColor' : 'none'} stroke="currentColor" strokeWidth="1.25" strokeLinejoin="round"><path d="m9 1-6 8h4l-1 6 7-9H9z" /></svg>
        {unknown ? uiAttribute('Fast · 默认') : 'Fast'}
      </span>
    </button>
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <button type="button" className="camp-fast-options" aria-label={uiAttribute('{0}的 Fast 偏好', displayName)} disabled={pending}>
          <svg viewBox="0 0 16 16" aria-hidden="true" fill="none" stroke="currentColor" strokeWidth="1.4"><path d="m4 6 4 4 4-4" /></svg>
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content className="camp-member-menu camp-fast-menu" align="end" sideOffset={5} collisionPadding={10}>
          <DropdownMenu.RadioGroup value={value.fastOverride === null ? 'default' : value.fastOverride ? 'on' : 'off'}
            onValueChange={next => { if (!pending) onToggle(next === 'default' ? null : next === 'on') }}>
            {([['default', '跟随 Runtime 默认'], ['on', '开启 Fast'], ['off', '关闭 Fast']] as const).map(([key, label]) =>
              <DropdownMenu.RadioItem className="camp-member-menu-item camp-fast-option" key={key} value={key} data-fast-choice={key}>
                <span>{uiAttribute(label)}</span>
                <DropdownMenu.ItemIndicator><svg viewBox="0 0 16 16" aria-hidden="true" fill="none" stroke="currentColor" strokeWidth="1.4"><path d="m3 8 3 3 7-7" /></svg></DropdownMenu.ItemIndicator>
              </DropdownMenu.RadioItem>
            )}
          </DropdownMenu.RadioGroup>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  </span>
}
