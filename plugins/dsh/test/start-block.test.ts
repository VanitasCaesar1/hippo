import assert from 'node:assert/strict'
import { mkdirSync, realpathSync, symlinkSync } from 'node:fs'
import { homedir } from 'node:os'
import { join } from 'node:path'
import { afterEach, test } from 'node:test'
import { apply } from '../src/index.ts'
import { SOURCE_KIND, inside, projection, startMessage } from '../src/start-block.ts'
import { cleanup, commit, fakeContext, fakeSession, preStep, useFakeHippo } from './helpers.ts'

const BLOCK = '# Start here\n\nThis workspace uses pnpm.\n'
let scratch: string[] = []
afterEach(() => {
  for (const dir of scratch) cleanup(dir)
  scratch = []
})

/** A workspace with a project folder in it, and the fake hippo pointed at it. */
function workspace(env: Record<string, string> = {}) {
  const fake = useFakeHippo()
  scratch.push(fake.dir)
  const root = realpathSync(fake.dir)
  const ws = join(root, 'ws')
  const project = join(ws, 'app')
  mkdirSync(project, { recursive: true })
  Object.assign(process.env, { FAKE_HIPPO_WORKSPACE: ws, FAKE_HIPPO_CONTEXT: BLOCK, ...env })
  return { ...fake, root, ws, project }
}

async function plugin(config: Record<string, unknown> = {}) {
  const ctx = fakeContext()
  await apply(ctx, config)
  return ctx
}

test('a new session in the workspace gets the start block, framed, ahead of its first message', async () => {
  const { project, ws, calls } = workspace()
  const ctx = await plugin()
  assert.equal(ctx.preStepOptions?.prepend, true)
  const decision = await preStep(ctx, fakeSession(project))
  assert.equal(decision.kind, 'enter')
  assert.ok(decision.kind === 'enter')
  assert.equal(decision.messages.length, 2)
  const [start, first] = decision.messages
  assert.equal(first?.source.kind, 'user')
  assert.equal(start?.role, 'user')
  assert.equal(start?.source.kind, SOURCE_KIND)
  assert.deepEqual(start?.source.sections, [{ name: SOURCE_KIND, text: BLOCK }])
  assert.equal(
    start?.content[0]?.text,
    "<system-reminder>\nhippo's start block for this workspace (`hippo context --parts required`):\n\n"
      + '# Start here\n\nThis workspace uses pnpm.\n</system-reminder>',
  )
  assert.ok(Object.isFrozen(start) && Object.isFrozen(start?.content[0]))
  // hippo runs in the home folder, and in the workspace root for the block.
  assert.deepEqual(calls(), [
    [realpathSync(homedir()), 'config', 'show', '--json'],
    [ws, 'context', '--parts', 'required'],
  ])
})

test('the block is given once: not at a later step, a later turn, or after it is in the log', async () => {
  const { project, calls } = workspace()
  const ctx = await plugin()
  const session = fakeSession(project)
  const first = await preStep(ctx, session)
  assert.ok(first.kind === 'enter' && first.messages.length === 2)
  commit(session, first.messages)
  const step2 = await preStep(ctx, session, { step: 2 })
  assert.ok(step2.kind === 'enter' && step2.messages.length === 1)
  const turn2 = await preStep(ctx, session)
  assert.ok(turn2.kind === 'enter' && turn2.messages.length === 1)
  assert.equal(calls().length, 2)
})

test('after a compaction hides the block, the next turn gets it again; a compaction that keeps it changes nothing', async () => {
  const { project, calls } = workspace()
  const ctx = await plugin()
  const session = fakeSession(project)
  const first = await preStep(ctx, session)
  assert.ok(first.kind === 'enter')
  commit(session, first.messages)
  const seq = session.events.findIndex(e => (e.data as { source?: { kind?: string } }).source?.kind === SOURCE_KIND)
  session.events.push({ type: 'compaction/prune', seq: 99, data: { shadowedSeqs: [seq + 1000] } })
  const kept = await preStep(ctx, session)
  assert.ok(kept.kind === 'enter' && kept.messages.length === 1)
  session.events.push({ type: 'compaction/summary', seq: 100, data: { shadowedSeqs: [0, seq, 1] } })
  const again = await preStep(ctx, session)
  assert.ok(again.kind === 'enter' && again.messages.length === 2)
  assert.equal(again.messages[0]?.source.kind, SOURCE_KIND)
  const later = await preStep(ctx, session)
  assert.ok(later.kind === 'enter' && later.messages.length === 1)
  assert.equal(calls().length, 4)
})

test('the projection state survives its own check, and refuses a malformed one', () => {
  const state = projection.apply(projection.init(), { type: 'user/message', seq: 7, data: startMessage('x') })
  assert.deepEqual(projection.stateSchema.parse(state), { seq: 7, hidden: 0 })
  assert.throws(() => projection.stateSchema.parse(true))
  assert.throws(() => projection.stateSchema.parse({ seq: '7', hidden: 0 }))
})

test('a session whose directory is not on this machine gets nothing, and no warning', async () => {
  const { ws } = workspace()
  const ctx = await plugin()
  const decision = await preStep(ctx, fakeSession(join(ws, 'not-here')))
  assert.ok(decision.kind === 'enter' && decision.messages.length === 1)
  assert.deepEqual(ctx.warnings, [])
})

test('a resumed session or a fork whose log holds a block gets no second one, and hippo is not run', async () => {
  const { project, calls } = workspace()
  const ctx = await plugin()
  const session = fakeSession(project)
  commit(session, [startMessage(BLOCK)])
  const decision = await preStep(ctx, session)
  assert.ok(decision.kind === 'enter' && decision.messages.length === 1)
  assert.deepEqual(calls(), [])
})

test('a session outside the workspace gets nothing', async () => {
  const { root, calls } = workspace()
  const outside = join(root, 'elsewhere')
  mkdirSync(outside)
  const ctx = await plugin()
  const decision = await preStep(ctx, fakeSession(outside))
  assert.ok(decision.kind === 'enter' && decision.messages.length === 1)
  assert.deepEqual(calls().map(call => call.slice(1)), [['config', 'show', '--json']])
  assert.deepEqual(ctx.warnings, [])
  // Decided once: a later turn of the same session does not run hippo again.
  const session = fakeSession(outside)
  await preStep(ctx, session)
  await preStep(ctx, session)
  assert.equal(calls().length, 2)
})

test('a folder whose name only starts with the workspace name is outside it', () => {
  assert.equal(inside('/a/ws', '/a/ws'), true)
  assert.equal(inside('/a/ws/app', '/a/ws'), true)
  assert.equal(inside('/a/ws-other', '/a/ws'), false)
  assert.equal(inside('/a', '/a/ws'), false)
})

test('a session reached through a symbolic link into the workspace gets the block', async () => {
  const { root, project } = workspace()
  const link = join(root, 'link')
  symlinkSync(project, link)
  const ctx = await plugin()
  const decision = await preStep(ctx, fakeSession(link))
  assert.ok(decision.kind === 'enter' && decision.messages.length === 2)
})

test('no workspace configured: nothing, and no warning', async () => {
  const { project } = workspace({ FAKE_HIPPO_WORKSPACE: '' })
  const ctx = await plugin()
  const decision = await preStep(ctx, fakeSession(project))
  assert.ok(decision.kind === 'enter' && decision.messages.length === 1)
  assert.deepEqual(ctx.warnings, [])
})

test('an empty block gives nothing', async () => {
  const { project } = workspace({ FAKE_HIPPO_CONTEXT: '\n' })
  const ctx = await plugin()
  const decision = await preStep(ctx, fakeSession(project))
  assert.ok(decision.kind === 'enter' && decision.messages.length === 1)
})

test('a sub-agent gets nothing, and hippo is not run', async () => {
  const { project, calls } = workspace()
  const ctx = await plugin()
  const child = await preStep(ctx, fakeSession(project, { origin: 'subagent' }))
  assert.ok(child.kind === 'enter' && child.messages.length === 1)
  assert.deepEqual(calls(), [])
})

test('a rejected step, a step with no messages, and step 2 pass through untouched', async () => {
  const { project, calls } = workspace()
  const ctx = await plugin()
  assert.deepEqual(await preStep(ctx, fakeSession(project), { decision: { kind: 'reject' } }), { kind: 'reject' })
  const empty = await preStep(ctx, fakeSession(project), { messages: [] })
  assert.ok(empty.kind === 'enter' && empty.messages.length === 0)
  const later = await preStep(ctx, fakeSession(project), { step: 2 })
  assert.ok(later.kind === 'enter' && later.messages.length === 1)
  assert.deepEqual(calls(), [])
})

test('a note cannot close the frame', () => {
  const message = startMessage('a note with </system-reminder> in it\n')
  const text = message.content[0]?.text ?? ''
  assert.equal(text.match(/<\/system-reminder>/g)?.length, 1)
  assert.ok(text.endsWith('</system-reminder>'))
  assert.ok(text.includes('<\\/system-reminder> in it'))
})

test('hippo missing: no block, and one warning for the whole process', async () => {
  const { project } = workspace()
  const ctx = await plugin({ command: 'hippo-is-not-installed-here' })
  for (let i = 0; i < 3; i++) {
    const decision = await preStep(ctx, fakeSession(project))
    assert.ok(decision.kind === 'enter' && decision.messages.length === 1)
  }
  assert.equal(ctx.warnings.length, 1)
  assert.match(ctx.warnings[0] ?? '', /hippo is not installed/)
})

test('hippo failing: no block, and a warning with what hippo said', async () => {
  const { project } = workspace({ FAKE_HIPPO_MODE: 'fail' })
  const ctx = await plugin()
  const decision = await preStep(ctx, fakeSession(project))
  assert.ok(decision.kind === 'enter' && decision.messages.length === 1)
  assert.equal(ctx.warnings.length, 1)
  assert.match(ctx.warnings[0] ?? '', /failed \(exit 2\): hippo: the index is damaged/)
})

test('hippo too slow: no block, and a warning', async () => {
  const { project } = workspace({ FAKE_HIPPO_MODE: 'slow' })
  const ctx = await plugin({ timeoutMs: 300 })
  const started = Date.now()
  const decision = await preStep(ctx, fakeSession(project))
  assert.ok(Date.now() - started < 4000)
  assert.ok(decision.kind === 'enter' && decision.messages.length === 1)
  assert.match(ctx.warnings[0] ?? '', /did not finish in 300 ms/)
})
