import { expect, it } from 'vitest'
import type { ObservableOutputSample } from '@contracts'
import { LiveTokenSpeedDisplay } from './execution-token-speed'

function sample(sequence: number, sampledAtMs: number, publicTextUnits: number,
  reasoningUnits = 0, options: Partial<ObservableOutputSample> = {}): ObservableOutputSample {
  return {
    agentRunId: 'run', executionEpoch: 2, counterGeneration: 'core-1', sequence,
    algorithmVersion: 'observable-output-heuristic-v3', unicodeDataVersion: 'icu4x-2.2.0',
    sampledAtMs, lastOutputAtMs: publicTextUnits + reasoningUnits ? sampledAtMs : null,
    publicTextUnits, reasoningUnits, reasoningSource: reasoningUnits ? 'stream_text' : 'none',
    streamConfirmed: true, ...options
  }
}

it('combines public and reasoning growth in the same Core interval and publishes at 1 Hz', () => {
  const display = new LiveTokenSpeedDisplay(0)
  display.observe(sample(1, 0, 0), 0)
  const published: Array<{ at: number; speed: number | null; scope?: string }> = []
  for (let at = 500; at <= 30_000; at += 500) {
    display.observe(sample(1 + at / 500, at, at * 2, at,
      { reasoningSource: 'stream_text' }), at)
    const value = display.sample(at)
    if (value !== undefined) published.push({ at, speed: value?.speed ?? null, scope: value?.scope })
  }
  expect(published).toHaveLength(30)
  expect(published.every(({ at }) => at % 1000 === 0)).toBe(true)
  expect(published[0].speed).toBeNull()
  expect(published[1]).toMatchObject({ at: 2000, scope: 'mixed_text' })
  expect(published.at(-1)!.speed).toBeCloseTo(30, 1)
})

it('keeps the mixed source description while the smoothing window includes thought', () => {
  const display = new LiveTokenSpeedDisplay(0)
  display.observe(sample(1, 0, 0), 0)
  display.observe(sample(2, 500, 100), 500)
  display.observe(sample(3, 1000, 100, 100), 1000)
  display.observe(sample(4, 1500, 200, 100), 1500)
  expect(display.sample(1500)?.scope).toBe('mixed_text')
})

it('does not turn a single final block or a mid-run baseline into speed', () => {
  const display = new LiveTokenSpeedDisplay(0)
  display.observe(sample(9, 4000, 5000, 0, { streamConfirmed: false }), 0)
  expect(display.sample(1000)).toBeNull()
  display.observe(sample(10, 4500, 5000, 0, { streamConfirmed: false }), 1500)
  expect(display.sample(2000)).toBeNull()
  display.observe(sample(11, 5000, 5100, 0, { streamConfirmed: true }), 2500)
  expect(display.sample(3000)).toBeNull()
  display.observe(sample(12, 5500, 5200, 0, { streamConfirmed: true }), 3500)
  expect(display.sample(4000)?.speed).toBeGreaterThan(0)
})

it('handles reasoning-only output, idle expiry, and a newly warmed output window', () => {
  const display = new LiveTokenSpeedDisplay(0)
  display.observe(sample(1, 0, 0), 0)
  display.observe(sample(2, 500, 0, 100, { reasoningSource: 'stream_summary' }), 500)
  display.observe(sample(3, 1000, 0, 200, { reasoningSource: 'stream_summary' }), 1000)
  expect(display.sample(1500)?.scope).toBe('reasoning_summary')
  for (let at = 2000; at <= 5500; at += 500) {
    display.observe(sample(3 + at / 500, at, 0, 200, { reasoningSource: 'stream_summary' }), at)
    display.sample(at)
  }
  expect(display.sample(6500)).toBeNull()
  display.observe(sample(15, 7000, 0, 300, { reasoningSource: 'stream_summary' }), 7000)
  expect(display.sample(7500)).toBeNull()
  display.observe(sample(16, 7500, 0, 400, { reasoningSource: 'stream_summary' }), 7500)
  expect(display.sample(8500)?.scope).toBe('reasoning_summary')
})

it('drops stale, reset, and reconnect batches before measuring again', () => {
  const display = new LiveTokenSpeedDisplay(0)
  display.observe(sample(1, 0, 0), 0)
  display.observe(sample(2, 500, 100), 500)
  display.observe(sample(2, 500, 100), 600) // duplicate
  display.observe(sample(3, 1000, 200), 1000)
  expect(display.sample(1500)?.speed).toBeGreaterThan(0)
  display.observe(sample(4, 1500, 200, 0, { counterGeneration: 'core-2' }), 2000)
  expect(display.sample(2500)).toBeNull()
  display.observe(sample(5, 2000, 300, 0, { counterGeneration: 'core-2' }), 2500)
  display.observe(sample(6, 2500, 400, 0, { counterGeneration: 'core-2' }), 3000)
  expect(display.sample(3500)?.speed).toBeGreaterThan(0)
  display.observe(sample(7, 8000, 9999, 0, { counterGeneration: 'core-2' }), 8000)
  expect(display.sample(8500)).toBeNull()
})
