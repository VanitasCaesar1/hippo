import assert from 'node:assert/strict'
import { realpathSync } from 'node:fs'
import { homedir } from 'node:os'
import { afterEach, test } from 'node:test'
import { Config, validate } from '../src/config.ts'
import { apply } from '../src/index.ts'
import { dayArgs, renderAsk, utcOffset } from '../src/tools.ts'
import { cleanup, fakeContext, fakeSession, useFakeHippo } from './helpers.ts'

let scratch: string[] = []
afterEach(() => {
  for (const dir of scratch) cleanup(dir)
  scratch = []
})

async function tools(config: Record<string, unknown> = {}, env: Record<string, string> = {}) {
  const fake = useFakeHippo(env)
  scratch.push(fake.dir)
  const ctx = fakeContext()
  await apply(ctx, config)
  const cwd = realpathSync(homedir())
  const call = (name: string, args: unknown) => {
    const tool = ctx.tools_.find(t => t.name === name)
    if (tool === undefined) throw new Error(`no tool ${name}`)
    return tool.execute(args, { signal: new AbortController().signal, agent: { session: fakeSession(fake.dir) } })
  }
  return { ctx, cwd, call, calls: fake.calls }
}

test('two tools, each with a JSON Schema and a text renderer', async () => {
  const { ctx } = await tools()
  assert.deepEqual(ctx.tools_.map(t => t.name), ['hippo_ask', 'hippo_day'])
  for (const tool of ctx.tools_) {
    assert.equal(tool.parameters.type, 'object')
    assert.deepEqual(tool.output.schema, { type: 'string' })
    assert.deepEqual(tool.output.render({}, 'out'), [{ type: 'text', text: 'out' }])
    assert.match(tool.description, /[Rr]ead-only/)
  }
})

const ASKED = JSON.stringify({
  question: 'thumbnail sizes',
  terms: ['thumbnail', 'sizes'],
  searched: { sessions: 41, notes: 12, ms: 20 },
  sources: [
    { n: 1, kind: 'session', id: 'eval-02', title: 'the grid thumbnails\nlook soft', profile: '.claude', tool: 'claude-code', project: 'tern', time: '2026-11-02T10:31:05Z', why: 'agent: The pipeline writes 320 and 1280.', matched: [], hits: 3 },
    { n: 2, kind: 'note', path: 'notes/Thumbnails.md', title: 'Thumbnails', profile: null, project: null, time: '2026-10-30T09:12:00Z', why: 'note: sizes', matched: [], hits: 1 },
  ],
  answer: null,
})

test('hippo_ask runs ask --sources-only --json in the home folder and writes the sources with their ids', async () => {
  const { call, calls, cwd } = await tools({}, { FAKE_HIPPO_OUTPUT: ASKED })
  assert.equal(await call('hippo_ask', { question: '  what thumbnail sizes?  ' }), [
    'searched 41 sessions and 12 notes for: thumbnail sizes',
    '[1] the grid thumbnails look soft  (session · claude-code · tern · 2026-11-02 10:31)',
    '    agent: The pipeline writes 320 and 1280.',
    '    session eval-02',
    '[2] Thumbnails  (note · 2026-10-30 09:12)',
    '    note: sizes',
    '    notes/Thumbnails.md',
    '',
  ].join('\n'))
  assert.deepEqual(calls(), [[cwd, 'ask', '--sources-only', '--json', '--', 'what thumbnail sizes?']])
})

test('renderAsk: no terms, no match, and output that is not hippo\'s JSON', () => {
  assert.equal(renderAsk('{"terms":[],"sources":[]}'), 'nothing to search for in that question\n')
  assert.equal(renderAsk('{"terms":["x"],"searched":{"sessions":1,"notes":0},"sources":[]}'), 'searched 1 sessions and 0 notes for: x\nno session or note matched\n')
  assert.equal(renderAsk('no index; run hippo ingest\n'), 'no index; run hippo ingest\n')
})

test('a question that starts with a dash stays a question', async () => {
  const { call, calls } = await tools()
  await call('hippo_ask', { question: '--json' })
  assert.deepEqual(calls()[0]?.slice(1), ['ask', '--sources-only', '--json', '--', '--json'])
})

test('hippo_ask refuses bad arguments before running hippo', async () => {
  const { call, calls } = await tools()
  await assert.rejects(call('hippo_ask', {}), /question must be a non-empty string/)
  await assert.rejects(call('hippo_ask', { question: ' ' }), /question must be a non-empty string/)
  await assert.rejects(call('hippo_ask', { question: 'x'.repeat(2001) }), /at most 2000 characters/)
  await assert.rejects(call('hippo_ask', { question: 'x', limit: 3 }), /unknown argument limit/)
  await assert.rejects(call('hippo_ask', { question: 'x', extra: 1 }), /unknown argument extra/)
  await assert.rejects(call('hippo_ask', 'x'), /arguments must be an object/)
  assert.deepEqual(calls(), [])
})

test('hippo_day shows today, or a given day, with the machine\'s UTC offset', async () => {
  const { call, calls } = await tools({}, { FAKE_HIPPO_OUTPUT: 'the day' })
  assert.equal(await call('hippo_day', {}), 'the day')
  await call('hippo_day', { date: '2026-11-03' })
  const [today, day] = calls().map(c => c.slice(1))
  assert.deepEqual(today?.slice(0, 3), ['show', 'day', '--offset'])
  assert.match(today?.[3] ?? '', /^[+-]\d\d:\d\d$/)
  assert.deepEqual(day?.slice(0, 4), ['show', 'day', '2026-11-03', '--offset'])
})

test('hippo_day refuses a malformed date', async () => {
  const { call, calls } = await tools()
  await assert.rejects(call('hippo_day', { date: 'yesterday' }), /date must be a day as YYYY-MM-DD/)
  await assert.rejects(call('hippo_day', { date: '2026-13-45' }), /YYYY-MM-DD/)
  assert.deepEqual(calls(), [])
})

test('the offset for a date is taken on that date, and written as +HH:MM', () => {
  assert.equal(utcOffset(-570), '+09:30')
  assert.equal(utcOffset(0), '+00:00')
  assert.equal(utcOffset(420), '-07:00')
  assert.deepEqual(dayArgs({ date: '2026-07-01' }, date => `at ${date ?? 'now'}`), ['show', 'day', '2026-07-01', '--offset', 'at 2026-07-01'])
  assert.deepEqual(dayArgs(undefined, date => `at ${date ?? 'now'}`), ['show', 'day', '--offset', 'at now'])
})

test('hippo not set up, missing or failing: the tool fails with what to do', async () => {
  const notSetUp = await tools({}, { FAKE_HIPPO_MODE: 'notsetup' })
  await assert.rejects(notSetUp.call('hippo_ask', { question: 'x' }), /hippo is not set up yet .*run hippo/)
  const missing = await tools({ command: 'hippo-is-not-installed-here' })
  await assert.rejects(missing.call('hippo_day', {}), /hippo is not installed.*install\.sh.*hippo start/)
  const failing = await tools({}, { FAKE_HIPPO_MODE: 'fail' })
  await assert.rejects(failing.call('hippo_day', {}), /hippo show day --offset \S+ failed \(exit 2\): hippo: the index is damaged/)
})

test('tools off: none registered, the start block still on', async () => {
  const { ctx } = await tools({ tools: false })
  assert.deepEqual(ctx.tools_, [])
  assert.notEqual(ctx.preStep, undefined)
})

test('the config fills defaults and refuses what it does not know', () => {
  assert.deepEqual(validate(undefined), { value: { command: 'hippo', timeoutMs: 10000, tools: true } })
  assert.equal(Config['~standard'].version, 1)
  assert.deepEqual(Config['~standard'].validate({ timeoutMs: 500 }), { value: { command: 'hippo', timeoutMs: 500, tools: true } })
  const bad = validate({ timeoutMs: -1, command: '', subagents: true })
  assert.deepEqual(bad.issues?.map(issue => issue.path?.[0]), ['timeoutMs', 'command', 'subagents'])
  assert.ok(validate([]).issues)
})

test('apply refuses an invalid config', async () => {
  await assert.rejects(apply(fakeContext(), { timeoutMs: 0 }), /timeoutMs must be a positive whole number/)
})
