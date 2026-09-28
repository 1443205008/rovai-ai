import { expect, it } from 'vitest'
import { LiveTokenSpeed, estimatedVisibleTokens } from './execution-token-speed'
import type { LiveRuntimeEvent } from './ui-model'

function delta(runId: string, epoch: number, blockId: string, text: string, eventType = 'agent.text.delta'): LiveRuntimeEvent {
  return {
    id: `${blockId}:${text.length}`,
    agentRunId: runId,
    executionEpoch: epoch,
    eventType,
    payload: { blockId, textOffset: 0, delta: text },
    createdAt: '2026-09-28T00:00:00Z'
  }
}

it('measures only growth after the current run and block baseline', () => {
  const speed = new LiveTokenSpeed(0)
  const first = delta('run', 2, 'body', 'existing text')
  speed.observe([first], 'run', 2, 0)
  expect(speed.sample(500)).toBeNull()
  speed.observe([first, delta('other', 2, 'other', 'ignore'), delta('run', 1, 'old', 'ignore'),
    delta('run', 2, 'tool', 'ignore', 'runtime.action')], 'run', 2, 600)
  expect(speed.sample(1000)).toBeNull()

  const grown = delta('run', 2, 'body', 'existing text and fresh text')
  speed.observe([grown], 'run', 2, 1100)
  const measured = speed.sample(1500)
  expect(measured).not.toBeNull()
  expect(measured!).toBeGreaterThan(0)
  speed.observe([grown], 'run', 2, 1600)
  expect(speed.sample(2000)).toBeLessThan(measured!)
  expect(speed.sample(5000)).toBeNull()
})

it('does not invent a speed for a single final block or a replayed snapshot', () => {
  const speed = new LiveTokenSpeed(0)
  speed.observe([delta('run', 1, 'a', 'entire final answer')], 'run', 1, 100)
  expect(speed.sample(500)).toBeNull()
  speed.observe([delta('run', 1, 'b', 'another complete answer')], 'run', 1, 600)
  expect(speed.sample(1000)).toBeNull()
})

it('uses a versioned character heuristic for visible text only', () => {
  expect(estimatedVisibleTokens('中文')).toBe(2)
  expect(estimatedVisibleTokens('abc')).toBeCloseTo(0.78)
  expect(estimatedVisibleTokens('🙂')).toBe(1)
})
