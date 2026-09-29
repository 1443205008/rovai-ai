import { expect, it } from 'vitest'
import { LiveTokenSpeed, LiveTokenSpeedDisplay, estimatedVisibleTokens } from './execution-token-speed'
import type { LiveRuntimeEvent } from './ui-model'

function delta(runId: string, epoch: number, blockId: string, text: string, eventType = 'agent.text.delta', offset = 0): LiveRuntimeEvent {
  return {
    id: `${blockId}:${text.length}`,
    agentRunId: runId,
    executionEpoch: epoch,
    eventType,
    payload: { blockId, textOffset: offset, delta: text },
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
  expect(speed.sample(1500)).toBeNull()
  const measured = speed.sample(2100)
  expect(measured).not.toBeNull()
  expect(measured!).toBeGreaterThan(0)
  speed.observe([grown], 'run', 2, 2200)
  expect(speed.sample(2600)).toBeLessThan(measured!)
  expect(speed.sample(6200)).toBeNull()
})

it('does not invent a speed for a single final block or a replayed snapshot', () => {
  const speed = new LiveTokenSpeed(0)
  speed.observe([delta('run', 1, 'a', 'entire final answer')], 'run', 1, 100)
  expect(speed.sample(500)).toBeNull()
  speed.observe([delta('run', 1, 'b', 'another complete answer')], 'run', 1, 600)
  expect(speed.sample(1000)).toBeNull()
})

it('uses a versioned character heuristic for visible text only', () => {
  expect(estimatedVisibleTokens('中文')).toBeCloseTo(1.2)
  expect(estimatedVisibleTokens('abc')).toBeCloseTo(0.75)
  expect(estimatedVisibleTokens('🙂')).toBe(1)
  expect(estimatedVisibleTokens('𠀀')).toBeCloseTo(0.6)
  expect(estimatedVisibleTokens('abc 123!')).toBe(2)
  expect(estimatedVisibleTokens('かな 한글')).toBe(4.25)
})

it.each([
  ['English', 'The quick brown fox jumps.'],
  ['Chinese', '今天天气真好，我们继续。'],
  ['mixed', 'Run 42 已完成 🙂'],
  ['code', 'const count = (items: string[]) => items.length;'],
  ['JSON', '{"ok":true,"count":42}'],
  ['Markdown', '## Heading\n- **bold** link'],
  ['whitespace', ' \t\n   '],
  ['emoji', '🙂🧑‍💻𠀀']
])('%s estimate is invariant to event partitions, overlap and repeated snapshots', (_kind, text) => {
  const unsplit = new LiveTokenSpeed(0)
  const partitioned = new LiveTokenSpeed(0)
  const baseline = delta('run', 1, 'body', 'B')
  unsplit.observe([baseline], 'run', 1, 0)
  partitioned.observe([baseline], 'run', 1, 0)
  unsplit.observe([delta('run', 1, 'body', text, 'agent.text.delta', 1)], 'run', 1, 100)
  const spans: LiveRuntimeEvent[] = []
  for (let index = 0; index < text.length; index++) {
    spans.push(delta('run', 1, 'body', text.slice(index, index + 1), 'agent.text.delta', index + 1))
  }
  // Duplicate and overlapping offsets are common after a reconnect.
  spans.push(...spans)
  if (text.length > 1) spans.push(delta('run', 1, 'body', text.slice(0, 2), 'agent.text.delta', 1))
  partitioned.observe(spans, 'run', 1, 100)
  expect(unsplit.sample(600)).toBeNull()
  expect(partitioned.sample(600)).toBeNull()
  expect(partitioned.sample(1100)).toBeCloseTo(unsplit.sample(1100)!, 8)
})

it('joins an emoji split between consecutive UTF-16 delta notifications', () => {
  const complete = new LiveTokenSpeed(0)
  const split = new LiveTokenSpeed(0)
  const baseline = delta('run', 1, 'body', 'B')
  complete.observe([baseline], 'run', 1, 0)
  split.observe([baseline], 'run', 1, 0)
  complete.observe([delta('run', 1, 'body', '🙂', 'agent.text.delta', 1)], 'run', 1, 100)
  const high = '🙂'.slice(0, 1)
  const low = '🙂'.slice(1)
  split.observe([delta('run', 1, 'body', high, 'agent.text.delta', 1)], 'run', 1, 100)
  split.observe([delta('run', 1, 'body', low, 'agent.text.delta', 2)], 'run', 1, 100)
  expect(complete.sample(600)).toBeNull()
  expect(split.sample(600)).toBeNull()
  expect(split.sample(1100)).toBeCloseTo(complete.sample(1100)!, 8)
})

it('starts timing from fresh text after a long baseline wait and resets after idle', () => {
  const speed = new LiveTokenSpeed(0)
  speed.observe([delta('run', 1, 'body', 'historical answer')], 'run', 1, 0)
  expect(speed.sample(10_000)).toBeNull()
  speed.observe([delta('run', 1, 'body', 'fresh words', 'agent.text.delta', 17)], 'run', 1, 10_100)
  expect(speed.sample(10_600)).toBeNull()
  expect(speed.sample(11_100)).toBeGreaterThan(0)
  expect(speed.sample(15_100)).toBeNull()
  speed.observe([delta('run', 1, 'body', 'again', 'agent.text.delta', 28)], 'run', 1, 15_200)
  expect(speed.sample(15_700)).toBeNull()
  expect(speed.sample(16_200)).toBeGreaterThan(0)
})

it('publishes at most 1 Hz through stable output, a tool pause and resumed output', () => {
  const display = new LiveTokenSpeedDisplay(0)
  display.observe([delta('run', 1, 'body', 'B')], 'run', 1, 0)
  let offset = 1
  const publications: { at: number; speed: number | null }[] = []
  for (let at = 500; at <= 30_000; at += 500) {
    const text = 'abcd'.repeat(20)
    display.observe([delta('run', 1, 'body', text, 'agent.text.delta', offset)], 'run', 1, at)
    offset += text.length
    const speed = display.sample(at)
    if (speed !== undefined) publications.push({ at, speed })
  }
  expect(publications).toHaveLength(30)
  expect(publications.every(({ at }) => at % 1000 === 0)).toBe(true)
  expect(publications[0].speed).toBeNull()
  expect(publications.at(-1)!.speed).toBeGreaterThan(0)

  // A tool phase has no public text and must expire the old rate.
  for (let at = 30_500; at <= 35_000; at += 500) {
    const speed = display.sample(at)
    if (speed !== undefined) publications.push({ at, speed })
  }
  expect(publications.at(-1)).toEqual({ at: 35_000, speed: null })

  display.observe([delta('run', 1, 'body', 'abcd'.repeat(20), 'agent.text.delta', offset)], 'run', 1, 35_500)
  expect(display.sample(35_500)).toBeUndefined()
  expect(display.sample(36_000)).toBeNull()
  expect(display.sample(36_500)).toBeUndefined()
  expect(display.sample(37_000)).toBeGreaterThan(0)
})
