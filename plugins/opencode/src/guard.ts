/**
 * hippo's guard in OpenCode (`guard: true`): before every tool call, the
 * plugin runs `hippo guard check --agent opencode` with the call on
 * standard input (docs/designs/guard.md). A refusal is thrown from
 * `execute.before`, which refuses the call, and the model reads the
 * reason as the call's error. A warning lets the call run and is put at
 * the top of its result in `execute.after`, so the model reads it and the
 * person sees it in the tool's output; it also goes where the plugin
 * reports problems. Checked against a running OpenCode 2.3.2 by
 * scripts/agent-e2e. In hippo 0.6 only hippo's self-protection rules
 * refuse; everything else warns.
 *
 * When hippo cannot answer, the call goes on, unless
 * `$HIPPO_HOME/guard/compiled.json` says `fail = "closed"`.
 */

import { execFile } from 'node:child_process'
import { readFileSync } from 'node:fs'
import { homedir } from 'node:os'
import { join } from 'node:path'

import type { Config } from './config.ts'
import type { Context, ToolExecuteAfter } from './opencode.ts'
import { resolveCommand, searchPath } from './hippo.ts'
import type { Warn } from './start-block.ts'

/** What `hippo guard check --agent opencode` answers. */
export interface GuardAnswer {
  decision: 'none' | 'warn' | 'ask' | 'block'
  reason?: string
  failed?: string
}

const BACKSTOP_MS = 5_000

/** `fail` in the compiled rules. */
export function failMode(env: NodeJS.ProcessEnv = process.env, home = homedir()): 'open' | 'closed' {
  const hippoHome = env.HIPPO_HOME ? env.HIPPO_HOME : join(home, '.hippo')
  try {
    const compiled = JSON.parse(readFileSync(join(hippoHome, 'guard', 'compiled.json'), 'utf8')) as { fail?: unknown }
    return compiled.fail === 'closed' ? 'closed' : 'open'
  } catch {
    return 'open'
  }
}

function failed(why: string): GuardAnswer {
  if (failMode() === 'closed') {
    return {
      decision: 'block',
      reason: `hippo guard could not check this call (${why}) and fail = "closed", so it is refused. Tell the person.`,
      failed: why,
    }
  }
  return { decision: 'none', failed: why }
}

/** Run `hippo guard check --agent opencode` on one call. */
export function checkCall(config: Config, call: Record<string, unknown>, cwd: string): Promise<GuardAnswer> {
  return new Promise(resolve => {
    const child = execFile(
      resolveCommand(config.command),
      ['guard', 'check', '--agent', 'opencode'],
      {
        cwd,
        env: { ...process.env, PATH: searchPath() },
        timeout: Math.min(config.timeoutMs, BACKSTOP_MS),
        killSignal: 'SIGKILL',
        maxBuffer: 1024 * 1024,
        encoding: 'utf8',
        windowsHide: true,
      },
      (error, stdout) => {
        if (error !== null) {
          resolve(failed((error as { killed?: boolean }).killed === true ? 'no answer in time' : error.message.split('\n')[0] ?? 'failed'))
          return
        }
        try {
          const answer = JSON.parse(stdout.trim()) as GuardAnswer
          if (!['none', 'warn', 'ask', 'block'].includes(answer.decision)) throw new Error('unknown decision')
          resolve(answer)
        } catch {
          resolve(failed('an answer that does not parse'))
        }
      },
    )
    child.stdin?.on('error', () => {})
    child.stdin?.end(JSON.stringify(call))
  })
}

/** The warning at the top of a call's result. */
export function withWarning(event: ToolExecuteAfter, warning: string): void {
  if (event.status !== 'completed') return
  const content = event.result.content
  const note = { type: 'text' as const, text: warning }
  event.result = {
    ...event.result,
    content: content === undefined ? [note] : typeof content === 'string' ? `${warning}\n\n${content}` : [note, ...content],
  }
}

/** Add the guard's `execute.before` and `execute.after` hooks. */
export async function addGuard(ctx: Context, config: Config, warn: Warn): Promise<void> {
  if (ctx.tool.hook === undefined) {
    warn('hippo: guard is on, but this OpenCode has no tool hooks; the guard is not running')
    return
  }
  // Warnings waiting for their call's result, by session and call id.
  const pending = new Map<string, string>()
  const key = (event: { sessionID: string; id?: string }) => `${event.sessionID}\u0000${event.id ?? ''}`
  await ctx.tool.hook('execute.after', event => {
    const warning = pending.get(key(event))
    if (warning === undefined) return
    pending.delete(key(event))
    withWarning(event, warning)
  })
  await ctx.tool.hook('execute.before', async event => {
    let cwd = ctx.location.directory
    try {
      cwd = (await ctx.session.get({ sessionID: event.sessionID })).location?.directory ?? cwd
    } catch {
      // The plugin's folder, then.
    }
    const answer = await checkCall(
      config,
      { tool_name: event.tool, tool_input: event.input ?? null, cwd, session_id: event.sessionID, headless: false },
      cwd,
    )
    if (answer.failed !== undefined) warn(`hippo guard: ${answer.failed}`)
    if (answer.decision === 'block' || answer.decision === 'ask') {
      throw new Error(answer.reason ?? 'hippo guard refused this call')
    }
    if (answer.decision === 'warn' && answer.reason) {
      warn(answer.reason)
      if (event.id !== undefined) pending.set(key(event), answer.reason)
    }
  })
}
