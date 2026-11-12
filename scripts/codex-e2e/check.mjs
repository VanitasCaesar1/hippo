#!/usr/bin/env node
// Checks what the Codex hooks end-to-end run left in its scratch folder:
// the requests the canned server received and hippo's session record.
//
// Usage: node check.mjs <scratch>

import { readFileSync, readdirSync } from 'node:fs'
import { join } from 'node:path'

const scratch = process.argv[2]
const fail = message => { console.error(`e2e: FAIL: ${message}`); process.exit(1) }

const requests = readFileSync(join(scratch, 'requests.jsonl'), 'utf8').trim().split('\n').filter(Boolean).map(line => JSON.parse(line))
if (requests.length === 0) fail('the canned server received no request')
const first = JSON.stringify(requests[0].input ?? [])
for (const marker of ['E2E-START', 'E2E-RELEVANT']) {
  if (!first.includes(marker)) fail(`the first request does not carry ${marker}`)
}
// The start block is in a developer message, not in the person's.
const developer = (requests[0].input ?? []).filter(item => item.role === 'developer').map(item => JSON.stringify(item.content)).join('\n')
if (!developer.includes('E2E-START')) fail('the start block is not in a developer message')
const user = (requests[0].input ?? []).filter(item => item.role === 'user').map(item => JSON.stringify(item.content)).join('\n')
if (user.includes('E2E-START')) fail('the start block is in a user message')

const sessions = join(scratch, 'ws', '.hippo', 'sessions')
const files = readdirSync(sessions).filter(name => name.startsWith('codex-'))
if (files.length !== 1) fail(`expected one codex- session record, found ${files.length}`)
const record = JSON.parse(readFileSync(join(sessions, files[0]), 'utf8'))
if (record.tool !== 'codex' || record.derived) fail(`the record is not a declared codex session: ${files[0]}`)
if (record.task !== 'What sizes do we make thumbnails at?') fail(`the task is ${JSON.stringify(record.task)}`)
if (!record.ended) fail('the session did not end')
if (!record.handoff?.includes('E2E-HANDOFF') || !record.handoff_derived) fail(`the handoff is ${JSON.stringify(record.handoff)}`)
if (!record.context_start?.includes('E2E-START') || !record.context_prompt?.includes('E2E-RELEVANT')) fail('the record lacks the context it gave')

console.log(`e2e: ${requests.length} request(s); the first carried the start block and the relevant note; ${files[0]} recorded with task, end and handoff`)
