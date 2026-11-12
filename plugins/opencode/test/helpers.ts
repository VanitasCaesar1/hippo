/**
 * A fake OpenCode plugin `ctx`, and a fake `hippo` on PATH (test/bin/hippo)
 * driven by FAKE_HIPPO_* variables.
 */

import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { delimiter, dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import type { Context, SessionContext, SessionInfo, ToolInfo } from '../src/opencode.ts'

export const binDir = join(dirname(fileURLToPath(import.meta.url)), 'bin')

export interface FakeContext extends Context {
  sessions: Map<string, SessionInfo>
  contextHook?: (event: SessionContext) => void | Promise<void>
  tools_: ToolInfo[]
  transforms: number
  gets: number
}

export function fakeContext(options: Record<string, unknown> = {}, directory = '/nowhere'): FakeContext {
  const ctx: FakeContext = {
    location: { directory },
    options,
    sessions: new Map(),
    tools_: [],
    transforms: 0,
    gets: 0,
    session: {
      async hook(name, callback) {
        if (name !== 'context') throw new Error(`unexpected hook ${name}`)
        ctx.contextHook = callback
        return { dispose() {} }
      },
      async get({ sessionID }) {
        ctx.gets += 1
        const info = ctx.sessions.get(sessionID)
        if (info === undefined) throw new Error(`session not found: ${sessionID}`)
        return info
      },
    },
    tool: {
      async transform(callback) {
        ctx.transforms += 1
        callback({ add: tool => ctx.tools_.push(tool) })
      },
    },
  }
  return ctx
}

/** Add a session in `directory`; a sub-agent when `parentID` is given. */
export function addSession(ctx: FakeContext, id: string, directory: string, parentID?: string): void {
  ctx.sessions.set(id, { id, location: { directory }, ...(parentID === undefined ? {} : { parentID }) })
}

/** One model request of `sessionID`: the system parts after the plugin's hook. */
export async function request(ctx: FakeContext, sessionID: string): Promise<string[]> {
  if (ctx.contextHook === undefined) throw new Error('the plugin added no context hook')
  const event: SessionContext = { sessionID, agent: 'build', system: [{ type: 'text', text: 'You are OpenCode.' }] }
  await ctx.contextHook(event)
  return event.system.map(part => part.text)
}

/** Point the fake hippo at a fresh log and the given answers; returns a reader for its calls. */
export function useFakeHippo(env: Record<string, string | undefined> = {}): { calls(): string[][]; dir: string } {
  const dir = mkdtempSync(join(tmpdir(), 'opencode-plugin-hippo-'))
  const log = join(dir, 'calls.log')
  for (const key of Object.keys(process.env)) if (key.startsWith('FAKE_HIPPO_')) delete process.env[key]
  process.env.FAKE_HIPPO_LOG = log
  for (const [key, value] of Object.entries(env)) {
    if (value === undefined) delete process.env[key]
    else process.env[key] = value
  }
  if (!(process.env.PATH ?? '').split(delimiter).includes(binDir)) {
    process.env.PATH = [binDir, process.env.PATH].join(delimiter)
  }
  return {
    dir,
    calls: () => (existsSync(log) ? readFileSync(log, 'utf8').trimEnd().split('\n').filter(Boolean).map(line => line.split('\t')) : []),
  }
}

export function cleanup(dir: string): void {
  rmSync(dir, { recursive: true, force: true })
}
