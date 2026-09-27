import type { CampCreationPreflight } from '@contracts'
import { uiAttribute } from './interface-language'

type ConversationCandidate = Pick<
  CampCreationPreflight['presentMembers'][number],
  'runtimeConfigured' | 'runtimeReadiness'
>

export function isNewConversationMemberAvailable(member: ConversationCandidate): boolean {
  return member.runtimeConfigured
    && (member.runtimeReadiness === 'ready' || member.runtimeReadiness === 'light_ready')
}

export function newConversationMemberStatus(member: ConversationCandidate): string {
  if (!member.runtimeConfigured || member.runtimeReadiness === 'runtime_not_configured') {
    return uiAttribute('未配置运行时')
  }
  return isNewConversationMemberAvailable(member) ? uiAttribute('可用') : uiAttribute('运行时不可用')
}
