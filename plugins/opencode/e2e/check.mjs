#!/usr/bin/env node
// Checks what the canned model received in an e2e run (see run.sh) and
// prints it. Exits 1 on the first expectation that fails.
//
//   node check.mjs <scratch folder>

import { readFileSync } from 'node:fs'
import { join } from 'node:path'

const scratch = process.argv[2]
const requests = readFileSync(join(scratch, 'requests.jsonl'), 'utf8').trim().split('\n').map(line => JSON.parse(line))
const agent = requests.filter(r => Array.isArray(r.tools) && r.tools.length > 0)

const text = m => (typeof m.content === 'string' ? m.content : (m.content ?? []).map(c => c.text ?? '').join(''))
const fail = message => {
  console.error(`e2e: FAIL: ${message}`)
  process.exit(1)
}
const expect = (ok, message) => ok || fail(message)

expect(agent.length >= 3, `expected at least 3 model requests with tools, got ${agent.length}`)
const names = agent[0].tools.map(t => t.function?.name ?? t.name)
expect(names.includes('hippo_ask') && names.includes('hippo_day'), `tools offered: ${names.join(', ')}`)

const marker = "hippo's start block"
for (const [i, r] of agent.entries()) {
  const system = r.messages.filter(m => m.role === 'system').map(text).join('\n')
  const count = system.split(marker).length - 1
  expect(count === 1, `request ${i + 1} holds ${count} start blocks in its system prompt, not 1`)
  expect(system.includes('E2E-MARKER'), `request ${i + 1}: the start block lacks the entry note`)
  const elsewhere = r.messages.filter(m => m.role !== 'system' && text(m).includes(marker))
  expect(elsewhere.length === 0, `request ${i + 1}: the start block is in a ${elsewhere[0]?.role} message`)
}

const results = agent.at(-1).messages.filter(m => m.role === 'tool').map(text)
expect(results.length >= 2, `expected 2 tool results, got ${results.length}`)
expect(/searched \d+ sessions/.test(results[0]) && /session eval-\d+/.test(results[0]), `hippo_ask result: ${results[0].slice(0, 300)}`)
expect(results[1].trim() !== '' && !/fail|not installed|error/i.test(results[1]), `hippo_day result: ${results[1].slice(0, 300)}`)

const run = readFileSync(join(scratch, 'run.jsonl'), 'utf8')
expect(run.includes('Done. hippo_ask said'), 'the final answer is not in the run output')

const system = agent[0].messages.filter(m => m.role === 'system').map(text).join('\n')
const at = system.indexOf('<system-reminder>')
console.log(`e2e: ${requests.length} requests to the canned model, ${agent.length} with tools (${names.filter(n => n.startsWith('hippo')).join(', ')} among ${names.length})`)
console.log('e2e: the start block in the system prompt, once per request:')
console.log(system.slice(at, system.indexOf('</system-reminder>', at) + 18).replace(/^/gm, '    '))
console.log('e2e: hippo_ask result:')
console.log(results[0].trimEnd().split('\n').slice(0, 8).join('\n').replace(/^/gm, '    '))
console.log('e2e: hippo_day result:')
console.log(results[1].trimEnd().split('\n').slice(0, 8).join('\n').replace(/^/gm, '    '))
console.log('e2e: ok')
