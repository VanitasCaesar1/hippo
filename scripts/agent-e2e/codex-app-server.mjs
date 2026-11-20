#!/usr/bin/env node
// Runs one Codex task through `codex app-server`, the JSON-RPC interface
// Codex's TUI and IDE extensions use, and prints what such a client is
// sent. `codex exec` prints a hook's status but not its message; the TUI
// shows a `PreToolUse` hook's `systemMessage` from the `hook/completed`
// notification's warning entries (codex-rs/tui/src/history_cell/
// hook_cell.rs, 0.170.0). This client prints each such entry as
// `hook warning: <text>`, the line the person would see.
//
// Usage: node codex-app-server.mjs <codex binary> <cwd> <prompt>
// The thread trusts hooks for this run only (`bypass_hook_trust`), as
// `codex exec --dangerously-bypass-hook-trust` does: it stands in for the
// person trusting the hook in Codex's review.

import { spawn } from 'node:child_process'
import { createInterface } from 'node:readline'

const [codex, cwd, prompt] = process.argv.slice(2)
const server = spawn(codex, ['app-server'], { cwd, stdio: ['pipe', 'pipe', 'inherit'] })
const lines = createInterface({ input: server.stdout })
const waiting = new Map()
let next = 1

function send(message) {
  server.stdin.write(JSON.stringify(message) + '\n')
}

function call(method, params) {
  const id = next++
  send({ method, id, params })
  return new Promise((resolve, reject) => waiting.set(id, { resolve, reject }))
}

const done = new Promise(resolve => {
  lines.on('line', line => {
    let message
    try { message = JSON.parse(line) } catch { return }
    if (message.id !== undefined && waiting.has(message.id) && (message.result !== undefined || message.error !== undefined)) {
      const { resolve: ok, reject } = waiting.get(message.id)
      waiting.delete(message.id)
      if (message.error) reject(new Error(JSON.stringify(message.error)))
      else ok(message.result)
      return
    }
    if (message.id !== undefined && message.method) {
      // A request from the server (an approval): decline it.
      send({ id: message.id, result: { decision: 'decline' } })
      console.log(JSON.stringify({ declined: message.method }))
      return
    }
    console.log(JSON.stringify(message))
    if (message.method === 'hook/completed') {
      for (const entry of message.params?.run?.entries ?? []) {
        if (entry.kind === 'warning') console.log(`hook warning: ${entry.text}`)
        if (entry.kind === 'feedback') console.log(`hook feedback: ${entry.text}`)
      }
    }
    if (message.method === 'turn/completed') resolve()
  })
})

const timer = setTimeout(() => { console.error('codex-app-server: no turn/completed in 150 s'); server.kill(); process.exit(1) }, 150_000)
try {
  await call('initialize', { clientInfo: { name: 'hippo-agent-e2e', title: 'hippo agent e2e', version: '0' } })
  send({ method: 'initialized' })
  const started = await call('thread/start', { cwd, approvalPolicy: 'never', config: { bypass_hook_trust: true } })
  await call('turn/start', { threadId: started.thread.id, input: [{ type: 'text', text: prompt, text_elements: [] }] })
  await done
} catch (error) {
  console.error(`codex-app-server: ${error.message}`)
  process.exitCode = 1
} finally {
  clearTimeout(timer)
  server.stdin.end()
  server.kill()
}
