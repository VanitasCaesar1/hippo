/**
 * A fake dsh `ctx` for the plugin, and a fake `hippo` on PATH
 * (test/bin/hippo) driven by FAKE_HIPPO_* variables.
 */

import { mkdtempSync, readFileSync, existsSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { delimiter, dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import type {
  Agent,
  Context,
  PreStepDecision,
  PreStepPayload,
  ProjectionDefinition,
  Session,
  SessionEvent,
  ToolDefinition,
  UserMessage,
} from '../src/dsh.ts'

export const binDir = join(dirname(fileURLToPath(import.meta.url)), 'bin')

/** A session with a log the fake projection folds. */
export interface FakeSession extends Session {
  cwd: string
  events: SessionEvent[]
}

export function fakeSession(cwd: string, header: Partial<Session['header']> = {}): FakeSession {
  return { header: { id: `s-${Math.random().toString(36).slice(2)}`, cwd, ...header }, cwd, events: [] }
}

/** Commit messages to a session's log, as dsh does once a step is admitted. */
export function commit(session: FakeSession, messages: readonly UserMessage[]): void {
  for (const data of messages) session.events.push({ type: 'user/message', seq: session.events.length, data })
}

export interface FakeContext extends Context {
  warnings: string[]
  tools_: ToolDefinition[]
  preStep?: (payload: PreStepPayload, next: () => Promise<PreStepDecision>) => Promise<PreStepDecision>
  preStepOptions?: { prepend?: boolean }
}

export function fakeContext(): FakeContext {
  const ctx: FakeContext = {
    warnings: [],
    tools_: [],
    logger: { warn: message => ctx.warnings.push(message) },
    on(_event, listener, options) {
      ctx.preStep = listener
      ctx.preStepOptions = options
      return () => {}
    },
    tools: { register: definition => ctx.tools_.push(definition) },
    sessionProjections: {
      async register<S>(definition: ProjectionDefinition<S>) {
        return {
          stateOf(session: Session): S {
            let state = definition.init()
            for (const event of (session as FakeSession).events) state = definition.apply(state, event)
            return definition.stateSchema.parse(state)
          },
        }
      },
    },
    workingDirectory: { get: session => (session as FakeSession).cwd },
  }
  return ctx
}

const userText = (text: string): UserMessage => ({ id: 'm-user', role: 'user', content: [{ type: 'text', text }], source: { kind: 'user' } })

/** Run the plugin's pre-step listener for one step of `session`. */
export async function preStep(
  ctx: FakeContext,
  session: FakeSession,
  options: { step?: number; messages?: UserMessage[]; decision?: PreStepDecision } = {},
): Promise<PreStepDecision> {
  if (ctx.preStep === undefined) throw new Error('the plugin added no pre-step listener')
  const messages = options.messages ?? [userText('fix the build')]
  const decision: PreStepDecision = options.decision ?? { kind: 'enter', messages }
  const agent: Agent = { session }
  return ctx.preStep({ agent, messages, turn: 1, step: options.step ?? 1, signal: new AbortController().signal }, async () => decision)
}

/** Point the fake hippo at a fresh log and the given answers; returns a reader for its calls. */
export function useFakeHippo(env: Record<string, string | undefined> = {}): { calls(): string[][]; dir: string } {
  const dir = mkdtempSync(join(tmpdir(), 'dsh-plugin-hippo-'))
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
