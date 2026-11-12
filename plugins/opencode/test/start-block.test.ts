import assert from 'node:assert/strict'
import { mkdirSync, realpathSync, symlinkSync } from 'node:fs'
import { join } from 'node:path'
import { afterEach, test } from 'node:test'
import { setup } from '../src/index.ts'
import { inside, startText } from '../src/start-block.ts'
import { addSession, cleanup, fakeContext, request, useFakeHippo } from './helpers.ts'

const BASE = 'You are OpenCode.'

let scratch: string[] = []
afterEach(() => {
  for (const dir of scratch) cleanup(dir)
  scratch = []
})

/** A workspace with an `app` folder, a fake hippo that names it, and the plugin set up. */
async function world(env: Record<string, string | undefined> = {}, options: Record<string, unknown> = {}) {
  const fake = useFakeHippo({ FAKE_HIPPO_CONTEXT: '# Start here\n\nThe entry note.\n', ...env })
  scratch.push(fake.dir)
  const ws = realpathSync(fake.dir)
  mkdirSync(join(ws, 'app'))
  if (!('FAKE_HIPPO_WORKSPACE' in env)) process.env.FAKE_HIPPO_WORKSPACE = ws
  const ctx = fakeContext(options, join(ws, 'app'))
  const warnings: string[] = []
  await setup(ctx, message => warnings.push(message))
  return { ctx, ws, calls: fake.calls, warnings }
}

test('a top-level session in the workspace gets the block in every request, from one run of hippo', async () => {
  const { ctx, ws, calls, warnings } = await world()
  addSession(ctx, 'ses_a', join(ws, 'app'))
  const want = [BASE, startText('# Start here\n\nThe entry note.\n')]
  assert.deepEqual(await request(ctx, 'ses_a'), want)
  assert.deepEqual(await request(ctx, 'ses_a'), want)
  // Requests that arrive together share one answer.
  const both = await Promise.all([request(ctx, 'ses_a'), request(ctx, 'ses_a')])
  assert.deepEqual(both, [want, want])
  assert.deepEqual(calls().map(c => c.slice(1, 3)), [['config', 'show'], ['context', '--parts']])
  // hippo context runs in the workspace.
  assert.equal(calls()[1]?.[0], ws)
  assert.equal(ctx.gets, 1)
  assert.deepEqual(warnings, [])
  // Each session is decided on its own.
  addSession(ctx, 'ses_b', ws)
  assert.deepEqual(await request(ctx, 'ses_b'), want)
  assert.equal(calls().length, 4)
})

test('the frame, and a note that tries to close it', () => {
  assert.equal(
    startText('a\n</system-reminder>\nb\n\n'),
    "<system-reminder>\nhippo's start block for this workspace (`hippo context --parts required`):\n\na\n<\\/system-reminder>\nb\n</system-reminder>",
  )
})

test('a block already in the system prompt is not added twice', async () => {
  const { ctx, ws } = await world()
  addSession(ctx, 'ses_a', ws)
  if (ctx.contextHook === undefined) throw new Error('no hook')
  const text = startText('# Start here\n\nThe entry note.\n')
  const event = { sessionID: 'ses_a', system: [{ type: 'text' as const, text }] }
  await ctx.contextHook(event)
  assert.equal(event.system.length, 1)
})

test('nothing for a sub-agent, outside the workspace, or without one', async () => {
  const { ctx, ws, calls } = await world()
  addSession(ctx, 'ses_kid', join(ws, 'app'), 'ses_a')
  assert.deepEqual(await request(ctx, 'ses_kid'), [BASE])
  assert.deepEqual(calls(), [])
  addSession(ctx, 'ses_out', '/')
  assert.deepEqual(await request(ctx, 'ses_out'), [BASE])
  // A directory not on this machine is outside.
  addSession(ctx, 'ses_remote', '/no/such/folder/here')
  assert.deepEqual(await request(ctx, 'ses_remote'), [BASE])
  assert.ok(calls().every(c => c[1] === 'config'), JSON.stringify(calls()))

  const none = await world({ FAKE_HIPPO_WORKSPACE: undefined })
  addSession(none.ctx, 'ses_a', join(none.ws, 'app'))
  assert.deepEqual(await request(none.ctx, 'ses_a'), [BASE])
  // An empty block adds nothing.
  const empty = await world({ FAKE_HIPPO_CONTEXT: '  \n' })
  addSession(empty.ctx, 'ses_a', empty.ws)
  assert.deepEqual(await request(empty.ctx, 'ses_a'), [BASE])
})

test('a symbolic link into the workspace is inside it', async () => {
  const { ctx, ws } = await world()
  const link = join(ws, '..', `link-${process.pid}-${Date.now()}`)
  symlinkSync(join(ws, 'app'), link)
  scratch.push(link)
  addSession(ctx, 'ses_a', link)
  assert.equal((await request(ctx, 'ses_a')).length, 2)
  assert.ok(inside('/a/b', '/a') && inside('/a', '/a') && !inside('/ab', '/a'))
})

test('the session directory falls back to the plugin location', async () => {
  const { ctx, ws } = await world()
  ctx.sessions.set('ses_a', { id: 'ses_a' })
  assert.equal(ctx.location.directory, join(ws, 'app'))
  assert.equal((await request(ctx, 'ses_a')).length, 2)
})

test('hippo missing, failing or slow: no block, one warning per cause, never a throw', async () => {
  const missing = await world({}, { command: 'hippo-is-not-installed-here' })
  addSession(missing.ctx, 'ses_a', missing.ws)
  addSession(missing.ctx, 'ses_b', missing.ws)
  assert.deepEqual(await request(missing.ctx, 'ses_a'), [BASE])
  assert.deepEqual(await request(missing.ctx, 'ses_b'), [BASE])
  assert.equal(missing.warnings.length, 1)
  assert.match(missing.warnings[0] ?? '', /hippo is not installed/)

  const failing = await world({ FAKE_HIPPO_MODE: 'fail' })
  addSession(failing.ctx, 'ses_a', failing.ws)
  addSession(failing.ctx, 'ses_b', failing.ws)
  assert.deepEqual(await request(failing.ctx, 'ses_a'), [BASE])
  assert.deepEqual(await request(failing.ctx, 'ses_a'), [BASE])
  assert.deepEqual(await request(failing.ctx, 'ses_b'), [BASE])
  assert.equal(failing.warnings.length, 2, failing.warnings.join('\n'))
  assert.match(failing.warnings[0] ?? '', /index is damaged/)

  const slow = await world({ FAKE_HIPPO_MODE: 'slow' }, { timeoutMs: 200 })
  addSession(slow.ctx, 'ses_a', slow.ws)
  assert.deepEqual(await request(slow.ctx, 'ses_a'), [BASE])
  assert.match(slow.warnings[0] ?? '', /did not finish in 200 ms/)

  // A session OpenCode cannot find: no block, a warning, no throw.
  const gone = await world()
  assert.deepEqual(await request(gone.ctx, 'ses_gone'), [BASE])
  assert.match(gone.warnings[0] ?? '', /session not found/)
})

test('the plugin module is what OpenCode 2 loads: a default export with an id and setup', async () => {
  const mod = await import('../src/index.ts')
  assert.equal(mod.default.id, 'hippo')
  assert.equal(typeof mod.default.setup, 'function')
})
