import type { CampChannelSource } from '@contracts'
import { getInterfaceLanguage, uiAttribute } from './interface-language'

export const FIRST_RUN_CAMP_TITLE = '初次集结'

const CHANNEL_LABELS = {
  feishu: { p2p: '飞书私聊', group: '飞书群聊', topic: '飞书话题' },
  lark: { p2p: 'Lark私聊', group: 'Lark群聊', topic: 'Lark话题' },
  dingtalk: { p2p: '钉钉私聊', group: '钉钉群聊' }
} as const

/** Display only: edits and commands must continue to use the undecorated camp.title. */
export function formatCampTitle(camp: {
  id?: string
  title: string
  channelSource?: CampChannelSource | null
}, firstRunCampId: string | null = null): string {
  const source = camp.channelSource
  if (!source) {
    // The Desktop onboarding checkpoint identifies the App-owned default.
    // A matching title alone must never translate a user's conversation name.
    return firstRunCampId !== null && camp.id === firstRunCampId && camp.title === FIRST_RUN_CAMP_TITLE
      ? uiAttribute(FIRST_RUN_CAMP_TITLE)
      : camp.title
  }
  const labels: Partial<Record<string, string>> | undefined = CHANNEL_LABELS[source.provider]
  const label = labels?.[source.conversationKind]
  if (!label) return camp.title
  return getInterfaceLanguage() === 'en'
    ? `[${uiAttribute(label)}] ${camp.title}`
    : `【${label}】${camp.title}`
}
