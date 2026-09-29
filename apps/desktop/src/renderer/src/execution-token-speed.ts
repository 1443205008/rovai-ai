import type { LiveRuntimeEvent } from './ui-model'

export const VISIBLE_TEXT_HEURISTIC_VERSION = 'visible-text-heuristic-v2'
export const SAMPLE_INTERVAL_MS = 500
export const DISPLAY_INTERVAL_MS = 1000
export const SMOOTHING_TAU_MS = 2500
export const WARMUP_MS = 1000
export const IDLE_EXPIRY_MS = 5000
export const DISPLAY_DECIMALS = 1
const HAN_CHARACTER = /\p{Script=Han}/u

/** Display-only approximation. Native Usage and Context never consume this value. */
export function estimatedVisibleTokens(text: string): number {
  let units = 0
  for (const character of text) {
    const code = character.codePointAt(0) ?? 0
    if (code <= 0x7f) units += 0.25
    else if (HAN_CHARACTER.test(character)) units += 0.60
    else units += 1
  }
  return units
}

type TextSpan = { start: number; end: number; text: string }
type BlockCursor = { end: number; lastCodeUnit: number | null; pendingHighSurrogate: boolean }

function trailingCodeUnit(text: string): number | null {
  return text.length > 0 ? text.charCodeAt(text.length - 1) : null
}

function isHighSurrogate(code: number | null): boolean {
  return code !== null && code >= 0xd800 && code <= 0xdbff
}

export class LiveTokenSpeed {
  private readonly seen = new Map<string, BlockCursor>()
  private pending = 0
  private ema = 0
  private emaWeight = 0
  private lastSampleAt: number
  private firstTextAt: number | null = null
  private lastTextAt: number | null = null

  constructor(now: number) {
    this.lastSampleAt = now
  }

  observe(events: readonly LiveRuntimeEvent[], runId: string, executionEpoch: number, now: number): void {
    const spans = new Map<string, TextSpan[]>()
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
      const blockSpans = spans.get(payload.blockId) ?? []
      blockSpans.push({ start, end, text: payload.delta })
      spans.set(payload.blockId, blockSpans)
    }
    for (const [blockId, blockSpans] of spans) {
      blockSpans.sort((a, b) => a.start - b.start || a.end - b.end)
      let cursor = this.seen.get(blockId)
      if (!cursor) {
        // A new block may be replayed history or a complete final answer.
        const last = blockSpans.reduce((best, span) => span.end >= best.end ? span : best)
        cursor = { end: last.end, lastCodeUnit: trailingCodeUnit(last.text), pendingHighSurrogate: false }
        this.seen.set(blockId, cursor)
        continue
      }
      for (const span of blockSpans) {
        if (span.end <= cursor.end) continue
        if (span.start > cursor.end) {
          // An offset gap cannot establish a trustworthy generation rate.
          cursor.end = span.end
          cursor.lastCodeUnit = trailingCodeUnit(span.text)
          cursor.pendingHighSurrogate = false
          continue
        }
        const suffix = span.text.slice(cursor.end - span.start)
        let units = estimatedVisibleTokens(suffix)
        if (isHighSurrogate(cursor.lastCodeUnit) && suffix.length > 0) {
          const first = suffix.charCodeAt(0)
          if (first >= 0xdc00 && first <= 0xdfff) {
            const pair = String.fromCharCode(cursor.lastCodeUnit!, first)
            units += cursor.pendingHighSurrogate ? estimatedVisibleTokens(pair) - 1 : -1
          }
        }
        const lastCodeUnit = trailingCodeUnit(suffix)
        const pendingHighSurrogate = isHighSurrogate(lastCodeUnit)
        if (pendingHighSurrogate) units -= 1
        if (units > 0) {
          if (this.lastTextAt === null || now - this.lastTextAt >= IDLE_EXPIRY_MS) {
            this.firstTextAt = now
            this.lastSampleAt = now
            this.ema = 0
            this.emaWeight = 0
          }
          this.pending += units
          this.lastTextAt = now
        }
        cursor.end = span.end
        cursor.lastCodeUnit = lastCodeUnit
        cursor.pendingHighSurrogate = pendingHighSurrogate
      }
    }
    while (this.seen.size > 512) this.seen.delete(this.seen.keys().next().value!)
  }

  sample(now: number): number | null {
    const elapsed = Math.max(1, now - this.lastSampleAt)
    this.lastSampleAt = now
    const rate = this.pending * 1000 / elapsed
    this.pending = 0
    if (this.lastTextAt === null || now - this.lastTextAt >= IDLE_EXPIRY_MS) {
      this.firstTextAt = null
      this.ema = 0
      this.emaWeight = 0
      return null
    }
    const alpha = 1 - Math.exp(-elapsed / SMOOTHING_TAU_MS)
    this.ema = this.ema * (1 - alpha) + alpha * rate
    this.emaWeight = this.emaWeight * (1 - alpha) + alpha
    if (this.firstTextAt === null || now - this.firstTextAt < WARMUP_MS) return null
    const corrected = this.emaWeight > 0 ? this.ema / this.emaWeight : 0
    return corrected >= 0.1 ? corrected : null
  }
}

/** Keeps the 500 ms measurement clock separate from the 1 Hz Renderer publication clock. */
export class LiveTokenSpeedDisplay {
  private readonly meter: LiveTokenSpeed
  private lastPublishedAt: number

  constructor(now: number) {
    this.meter = new LiveTokenSpeed(now)
    this.lastPublishedAt = now
  }

  observe(events: readonly LiveRuntimeEvent[], runId: string, executionEpoch: number, now: number): void {
    this.meter.observe(events, runId, executionEpoch, now)
  }

  sample(now: number): number | null | undefined {
    const sampled = this.meter.sample(now)
    if (now - this.lastPublishedAt < DISPLAY_INTERVAL_MS) return undefined
    this.lastPublishedAt = now
    return sampled === null ? null : Number(sampled.toFixed(DISPLAY_DECIMALS))
  }
}
