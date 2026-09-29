import type { ObservableOutputSample } from '@contracts'

export const OBSERVABLE_OUTPUT_HEURISTIC_VERSION = 'observable-output-heuristic-v3'
export const SAMPLE_INTERVAL_MS = 500
export const DISPLAY_INTERVAL_MS = 1000
export const SMOOTHING_TAU_MS = 2500
export const WARMUP_MS = 1000
export const IDLE_EXPIRY_MS = 5000
export const DISPLAY_DECIMALS = 1
const RECONNECT_GAP_MS = 2000

export type OutputScope = 'public' | 'reasoning_text' | 'reasoning_summary' | 'mixed_text' | 'mixed_summary'
export type SpeedValue = { speed: number; scope: OutputScope }

/** Uses only numeric Core snapshots. Core's monotonic clock supplies rate intervals;
 * the local clock supplies connection freshness and the screen publication cadence. */
export class LiveTokenSpeedDisplay {
  private previous: ObservableOutputSample | null = null
  private previousReceivedAt: number | null = null
  private lastOutputAt: number | null = null
  private firstOutputAt: number | null = null
  private ema = 0
  private emaWeight = 0
  private scope: OutputScope = 'public'
  private windowHasPublic = false
  private windowReasoningSource: 'none' | 'text' | 'summary' = 'none'
  private streamConfirmed = false
  private activeWindowIncrements = 0
  private lastPublishedAt: number

  constructor(now: number) {
    this.lastPublishedAt = now
  }

  observe(snapshot: ObservableOutputSample | null, now: number): void {
    if (snapshot === null || snapshot.algorithmVersion !== OBSERVABLE_OUTPUT_HEURISTIC_VERSION
      || !Number.isSafeInteger(snapshot.sequence) || !Number.isSafeInteger(snapshot.sampledAtMs)
      || !Number.isSafeInteger(snapshot.publicTextUnits) || !Number.isSafeInteger(snapshot.reasoningUnits)
      || snapshot.publicTextUnits < 0 || snapshot.reasoningUnits < 0) {
      this.reset(snapshot, now)
      return
    }
    const previous = this.previous
    const receivedAt = this.previousReceivedAt
    if (previous === null || receivedAt === null
      || previous.agentRunId !== snapshot.agentRunId
      || previous.executionEpoch !== snapshot.executionEpoch
      || previous.counterGeneration !== snapshot.counterGeneration) {
      this.reset(snapshot, now)
      return
    }
    if (snapshot.sequence <= previous.sequence) return
    const elapsed = snapshot.sampledAtMs - previous.sampledAtMs
    const localElapsed = now - receivedAt
    const bodyUnits = snapshot.publicTextUnits - previous.publicTextUnits
    const reasoningUnits = snapshot.reasoningUnits - previous.reasoningUnits
    if (elapsed <= 0 || elapsed > RECONNECT_GAP_MS || localElapsed <= 0
      || localElapsed > RECONNECT_GAP_MS || bodyUnits < 0 || reasoningUnits < 0) {
      this.reset(snapshot, now)
      return
    }
    this.previous = snapshot
    this.previousReceivedAt = now
    this.streamConfirmed = snapshot.streamConfirmed
    const addedUnits = bodyUnits + reasoningUnits
    if (addedUnits > 0) {
      if (this.lastOutputAt === null || now - this.lastOutputAt >= IDLE_EXPIRY_MS) {
        this.ema = 0
        this.emaWeight = 0
        this.firstOutputAt = now
        this.activeWindowIncrements = 0
        this.windowHasPublic = false
        this.windowReasoningSource = 'none'
      }
      this.activeWindowIncrements += 1
      this.lastOutputAt = now
      if (bodyUnits > 0) this.windowHasPublic = true
      if (reasoningUnits > 0) {
        this.windowReasoningSource = snapshot.reasoningSource === 'stream_summary' ? 'summary' : 'text'
      }
      this.scope = this.windowReasoningSource === 'none' ? 'public'
        : this.windowHasPublic ? `mixed_${this.windowReasoningSource}`
          : `reasoning_${this.windowReasoningSource}`
    }
    const instant = addedUnits / 100 / (elapsed / 1000)
    const alpha = 1 - Math.exp(-elapsed / SMOOTHING_TAU_MS)
    this.ema = this.ema * (1 - alpha) + alpha * instant
    this.emaWeight = this.emaWeight * (1 - alpha) + alpha
  }

  /** `undefined` means the 1 Hz screen clock has not elapsed. */
  sample(now: number): SpeedValue | null | undefined {
    if (now - this.lastPublishedAt < DISPLAY_INTERVAL_MS) return undefined
    this.lastPublishedAt = now
    if (this.lastOutputAt === null || this.firstOutputAt === null
      || now - this.lastOutputAt >= IDLE_EXPIRY_MS
      || now - this.firstOutputAt < WARMUP_MS || !this.streamConfirmed
      || this.activeWindowIncrements < 2) return null
    const corrected = this.emaWeight > 0 ? this.ema / this.emaWeight : 0
    return corrected >= 0.1 ? { speed: Number(corrected.toFixed(DISPLAY_DECIMALS)), scope: this.scope } : null
  }

  private reset(snapshot: ObservableOutputSample | null, now: number): void {
    this.previous = snapshot
    this.previousReceivedAt = snapshot === null ? null : now
    this.lastOutputAt = null
    this.firstOutputAt = null
    this.ema = 0
    this.emaWeight = 0
    this.scope = 'public'
    this.windowHasPublic = false
    this.windowReasoningSource = 'none'
    this.streamConfirmed = snapshot?.streamConfirmed ?? false
    this.activeWindowIncrements = 0
  }
}
