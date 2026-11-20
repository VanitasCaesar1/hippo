#!/usr/bin/env node
// Checks what one agent's guard run left in its scratch folder: the
// requests the stub model received, the guard's log, the agent's output
// and its network sockets. Prints one report per agent.
//
// Usage: node check.mjs <agent> <dir> <exit status>

import { existsSync, readFileSync } from 'node:fs'
import { join } from 'node:path'

const [agent, dir, status] = process.argv.slice(2)
const problems = []
const facts = []
const read = path => (existsSync(path) ? readFileSync(path, 'utf8') : '')
const lines = path => read(path).split('\n').filter(Boolean).map(line => JSON.parse(line))

// Whether the person (in the agent's headless output) and the model see a
// warning, per agent: what docs/designs/guard.md says each hook can do. A
// run that shows less fails. Gemini CLI's BeforeTool cannot give the model
// context; dsh's headless profile prints only the final answer (the warning
// is in the session, which dsh's web UI shows).
const EXPECT = {
  claude: { user: true, model: true },
  codex: { user: true, model: true },
  gemini: { user: true, model: false },
  opencode: { user: true, model: true },
  dsh: { user: false, model: true },
}[agent] ?? { user: true, model: true }

const requests = lines(join(dir, 'requests.jsonl'))
const proxied = requests.filter(r => r.proxy).map(r => r.proxy)
const model = requests.filter(r => r.api)
const steps = model.filter(r => r.move?.call).map(r => r.move.step)
const done = model.find(r => r.move?.text?.startsWith('E2E-DONE'))

if (model.length === 0) problems.push('the stub model received no request')
for (const step of [0, 1, 2, 3]) {
  if (!steps.includes(step)) problems.push(`the stub never reached step ${step}`)
}
if (!done) problems.push('the conversation did not reach the final answer')
const shellTool = model.find(r => r.move?.call)?.move.call.name
facts.push(`${model.length} model request(s) (${model[0]?.api ?? '-'} API), shell tool ${shellTool ?? '-'}`)

// What the model saw by the end: the last request's text.
const seen = done ? JSON.stringify(done.body) : ''
if (done) {
  if (!seen.includes('SYNTHETIC-E2E-KEY')) problems.push('the flagged key read did not run (its output never reached the model)')
  if (!/example\.invalid|Could not resolve|curl/.test(seen)) problems.push('the flagged download did not run')
  if (!seen.includes('hippo guard blocked this')) problems.push('the model never saw the self-protection refusal')
  const warned = seen.includes('hippo guard: ')
  facts.push(`the model ${warned ? 'saw' : 'did not see'} the warning`)
  if (EXPECT.model && !warned) problems.push('the model never saw the guard\'s warning')
}

// The guard's log: the two flagged calls and the refusal, not `ls`.
const log = lines(join(dir, 'home', '.hippo', 'guard', 'log.jsonl'))
const ruled = rule => log.filter(e => e.rules?.some(r => r.includes(rule)))
if (ruled('remote-script').length === 0) problems.push('the guard did not log remote-script')
if (ruled('credential-read').length === 0) problems.push('the guard did not log credential-read')
const blocked = ruled('self-protection').filter(e => e.decision === 'block')
if (blocked.length === 0) problems.push('the guard did not block hippo guard allow')
if (log.some(e => /^\s*ls\s*$/.test(e.command ?? ''))) problems.push('the guard logged the harmless ls')
for (const e of log) {
  if (!['remote-script', 'credential-read', 'self-protection'].some(r => e.rules.join(' ').includes(r))) {
    problems.push(`the guard logged an unexpected rule: ${e.rules.join(', ')} on ${e.command}`)
  }
  if (e.decision === 'block' && !e.rules.includes('self-protection')) problems.push(`a non-self-protection rule blocked: ${e.command}`)
}
facts.push(`guard log: ${log.map(e => `${e.rules.join('+')}=${e.decision}`).join(', ') || 'empty'}`)

// The refused call did not run: no allow entry was written.
if (read(join(dir, 'home', '.hippo', 'guard', 'machine.toml')).includes('paste.example')) {
  problems.push('hippo guard allow ran: machine.toml has the entry')
}

// What the user sees: the agent's own output.
const out = read(join(dir, 'out.txt')) + read(join(dir, 'err.txt'))
const shown = agent === 'codex' ? /^hook warning: hippo guard: /m.test(out) : out.includes('hippo guard: ')
facts.push(`the user ${shown ? 'saw' : 'did not see'} the warning in the agent's output`)
if (EXPECT.user && !shown) problems.push('the warning is not in the agent\'s output')

// The network: only 127.0.0.1.
const sockets = read(join(dir, 'net.txt')).split('\n').filter(Boolean)
const remotes = new Set()
for (const s of sockets) {
  const name = s.split(' ')[1] ?? ''
  const remote = name.includes('->') ? name.split('->')[1] : ''
  if (remote) remotes.add(remote.replace(/:\d+$/, ''))
}
const outside = [...remotes].filter(r => !['127.0.0.1', '[::1]', 'localhost'].includes(r))
if (outside.length > 0) problems.push(`connections to ${outside.join(', ')}`)
if (sockets.length === 0) facts.push('note: the socket sampler caught no open socket (the run was quicker than lsof); the proxy log is the check')
facts.push(`network: ${sockets.length} socket sample(s), remote addresses ${[...remotes].join(', ') || 'none'}; proxy refused ${proxied.length ? [...new Set(proxied)].join(', ') : 'nothing'}`)

facts.push(`exit status ${status}`)
for (const f of facts) console.log(`agent-e2e: ${agent}: ${f}`)
if (problems.length > 0) {
  for (const p of problems) console.error(`agent-e2e: ${agent}: FAIL: ${p}`)
  console.error(`agent-e2e: ${agent}: output in ${dir}`)
  process.exit(1)
}
console.log(`agent-e2e: ${agent}: PASS`)
