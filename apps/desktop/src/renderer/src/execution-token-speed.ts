import type { LiveRuntimeEvent } from './ui-model'

/** Display-only heuristic v1. It never enters Runtime Usage or persisted totals. */
export function estimatedVisibleTokens(text: string): number {
  let units = 0
  for (const character of text) {
    const code = character.codePointAt(0) ?? 0
    if ((code >= 0x3400 && code <= 0x9fff)
      || (code >= 0x3040 && code <= 0x30ff)
      || (code >= 0xac00 && code <= 0xd7af)
      || code > 0xffff) units += 1
    else if (/\s/u.test(character)) units += 0.1
    else if (/[a-z0-9]/iu.test(character)) units += 0.26
    else units += 0.5
  }
  return units
}

export class LiveTokenSpeed {
  private readonly seen = new Map<string, number>()
  private pending = 0
  private smoothed: number | null = null
  private lastSampleAt: number
  private lastTextAt: number | null = null

  constructor(now: number) {
    this.lastSampleAt = now
  }

  observe(events: readonly LiveRuntimeEvent[], runId: string, executionEpoch: number, now: number): void {
    const latest = new Map<string, { start: number; end: number; text: string }>()
    for (const event of events) {
      if (event.agentRunId !== runId || event.executionEpoch !== executionEpoch
        || event.eventType !== 'agent.text.delta') continue
      const payload = event.payload !== null && typeof event.payload === 'object'
        ? event.payload as Record<string, unknown> : null
      if (!payload || typeof payload.blockId !== 'string'
        || typeof payload.textOffset !== 'number'
        || !Number.isSafeInteger(payload.textOffset)
        || payload.textOffset < 0
        || typeof payload.delta !== 'string') continue
      const start = payload.textOffset
      const end = start + payload.delta.length
      if (!Number.isSafeInteger(end)) continue
      const previous = latest.get(payload.blockId)
      if (!previous || end > previous.end) {
        latest.set(payload.blockId, { start, end, text: payload.delta })
      }
    }
    for (const [blockId, current] of latest) {
      const previousEnd = this.seen.get(blockId)
      if (previousEnd === undefined) {
        this.seen.set(blockId, current.end)
      } else if (current.end > previousEnd) {
        this.seen.set(blockId, current.end)
        if (current.start <= previousEnd) {
          const newText = current.text.slice(previousEnd - current.start)
          this.pending += estimatedVisibleTokens(newText)
          this.lastTextAt = now
        }
      }
    }
    while (this.seen.size > 512) this.seen.delete(this.seen.keys().next().value!)
  }

  sample(now: number): number | null {
    const elapsed = Math.max(1, now - this.lastSampleAt)
    this.lastSampleAt = now
    const rate = this.pending * 1000 / elapsed
    this.pending = 0
    if (this.lastTextAt === null || now - this.lastTextAt > 3000) {
      this.smoothed = null
      return null
    }
    const alpha = 1 - Math.exp(-elapsed / 1500)
    this.smoothed = this.smoothed === null ? rate : this.smoothed + alpha * (rate - this.smoothed)
    return this.smoothed >= 0.1 ? Math.round(this.smoothed * 10) / 10 : null
  }
}
