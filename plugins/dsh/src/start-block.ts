/**
 * The start block: hippo's required context, given to a new session whose
 * working directory is inside hippo's workspace, and again after a
 * compaction hides it, as Claude Code's SessionStart hook fires again
 * after /compact.
 */

import { randomUUID } from 'node:crypto'
import { realpath } from 'node:fs/promises'
import { homedir } from 'node:os'
import { sep } from 'node:path'
import type { Config } from './config.ts'
import type { Context, PreStepDecision, ProjectionHandle, Session, SessionEvent, UserMessage } from './dsh.ts'
import { HippoError, runHippo } from './hippo.ts'

/** The `source.kind` of the start block's message. */
export const SOURCE_KIND = 'hippo-start'

/**
 * What a session's log says about the start block: the log position of the
 * one the model still sees (`null` when there is none), and how many times
 * a compaction has hidden one.
 */
export interface StartState {
  seq: number | null
  hidden: number
}

function isState(value: unknown): value is StartState {
  if (value === null || typeof value !== 'object') return false
  const { seq, hidden } = value as Record<string, unknown>
  return (seq === null || Number.isSafeInteger(seq)) && Number.isSafeInteger(hidden)
}

export const projection = {
  key: 'hippoStart',
  stateVersion: 1,
  stateSchema: {
    parse(value: unknown): StartState {
      if (!isState(value)) throw new TypeError('hippoStart state must be { seq, hidden }')
      return value
    },
  },
  init: (): StartState => ({ seq: null, hidden: 0 }),
  apply(state: StartState, event: SessionEvent): StartState {
    if (event.type === 'user/message') {
      const kind = (event.data as { source?: { kind?: unknown } } | undefined)?.source?.kind
      return kind === SOURCE_KIND && typeof event.seq === 'number' ? { ...state, seq: event.seq } : state
    }
    if ((event.type === 'compaction/summary' || event.type === 'compaction/prune') && state.seq !== null) {
      const shadowed = (event.data as { shadowedSeqs?: unknown } | undefined)?.shadowedSeqs
      if (Array.isArray(shadowed) && shadowed.includes(state.seq)) return { seq: null, hidden: state.hidden + 1 }
    }
    return state
  },
}

/**
 * The block hippo gives a session in `cwd`, or `undefined` when there is
 * none: no workspace, `cwd` not a local folder inside it, or nothing to give.
 */
export async function startBlock(cwd: string, config: Config, signal?: AbortSignal): Promise<string | undefined> {
  const run = (args: string[], dir: string) => runHippo(args, { command: config.command, cwd: dir, timeoutMs: config.timeoutMs, signal })
  const shown = JSON.parse(await run(['config', 'show', '--json'], homedir())) as { workspace?: unknown }
  if (typeof shown.workspace !== 'string' || shown.workspace === '') return undefined
  // A directory that is not on this machine (an SSH workspace) is outside.
  const [dir, workspace] = await Promise.all([realpath(cwd).catch(() => undefined), realpath(shown.workspace).catch(() => undefined)])
  if (dir === undefined || workspace === undefined || !inside(dir, workspace)) return undefined
  const block = await run(['context', '--parts', 'required'], workspace)
  return block.trim() === '' ? undefined : block
}

/** Whether `dir` is `root` or a folder under it; both already resolved. */
export function inside(dir: string, root: string): boolean {
  return dir === root || dir.startsWith(root.endsWith(sep) ? root : root + sep)
}

/** The user-role message that carries `block`, as dsh's `createUserMessage` makes one. */
export function startMessage(block: string): UserMessage {
  const safe = block.replaceAll('</system-reminder>', '<\\/system-reminder>')
  const text = '<system-reminder>\n'
    + "hippo's start block for this workspace (`hippo context --parts required`):\n\n"
    + `${safe.trimEnd()}\n`
    + '</system-reminder>'
  return deepFreeze({
    role: 'user' as const,
    id: randomUUID(),
    content: [{ type: 'text' as const, text }],
    source: { kind: SOURCE_KIND, form: 'snapshot', sections: [{ name: SOURCE_KIND, text: block }] },
  })
}

function deepFreeze<T>(value: T): T {
  if (value !== null && typeof value === 'object') {
    for (const field of Object.values(value)) deepFreeze(field)
    Object.freeze(value)
  }
  return value
}

/**
 * Join `agent/pre-step`: at step 1 of a turn, put the start block ahead of
 * the turn's messages when the session should have it and does not.
 */
export function addStartBlock(ctx: Context, config: Config, handle: Promise<ProjectionHandle<StartState>>): void {
  // For each session, the `hidden` count this process last decided at: hippo
  // is asked once per session, and again only after a compaction hides the block.
  const decided = new WeakMap<Session, number>()
  let warnedMissing = false
  ctx.on('agent/pre-step', async ({ agent, step, signal }, next): Promise<PreStepDecision> => {
    const states = await handle
    const decision = await next()
    if (decision.kind !== 'enter' || step !== 1 || decision.messages.length === 0 || signal.aborted) return decision
    const { session } = agent
    // A sub-agent gets none, as a Claude Code sub-agent gets no SessionStart hook.
    if (session.header.origin === 'subagent') return decision
    const state = states.stateOf(session)
    if (state.seq !== null || decided.get(session) === state.hidden) return decision
    decided.set(session, state.hidden)
    let block: string | undefined
    try {
      block = await startBlock(ctx.workingDirectory.get(session), config, signal)
    } catch (error) {
      if (signal.aborted) {
        decided.delete(session)
        return decision
      }
      const missing = error instanceof HippoError && error.reason === 'missing'
      if (!missing || !warnedMissing) {
        const why = error instanceof HippoError ? error.message : `unexpected: ${String(error)}`
        ctx.logger.warn(`hippo: no start block for this session: ${why}`)
      }
      warnedMissing ||= missing
      return decision
    }
    if (block === undefined) return decision
    return { ...decision, messages: [startMessage(block), ...decision.messages] }
  }, { prepend: true })
}
