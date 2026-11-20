#!/usr/bin/env node
// Prints the skills Codex loads for a folder, through `codex app-server`'s
// skills/list (the request Codex's TUI makes). No model is called.
//
// Usage: node skills-list.mjs <codex binary> <cwd>
// Prints one line per skill, `<name> <path>`, then `errors <json>`.

import { spawn } from 'node:child_process'
import { createInterface } from 'node:readline'

const [codex, cwd] = process.argv.slice(2)
const server = spawn(codex, ['app-server'], { cwd, stdio: ['pipe', 'pipe', 'inherit'] })
const lines = createInterface({ input: server.stdout })
const waiting = new Map()
let next = 1
lines.on('line', line => {
  let message
  try { message = JSON.parse(line) } catch { return }
  if (message.id === undefined || !waiting.has(message.id)) return
  const { resolve, reject } = waiting.get(message.id)
  waiting.delete(message.id)
  if (message.error) reject(new Error(JSON.stringify(message.error)))
  else resolve(message.result)
})
const call = (method, params) => {
  const id = next++
  server.stdin.write(JSON.stringify({ method, id, params }) + '\n')
  return new Promise((resolve, reject) => waiting.set(id, { resolve, reject }))
}
const timer = setTimeout(() => { console.error('skills-list: no answer in 30 s'); server.kill(); process.exit(1) }, 30_000)
try {
  await call('initialize', { clientInfo: { name: 'hippo-plugin-check', title: 'hippo plugin check', version: '0' } })
  server.stdin.write(JSON.stringify({ method: 'initialized' }) + '\n')
  const listed = await call('skills/list', { cwds: [cwd], forceReload: true })
  for (const entry of listed.data ?? []) {
    for (const skill of entry.skills ?? []) console.log(`${skill.name} ${skill.path}`)
    console.log(`errors ${JSON.stringify(entry.errors ?? [])}`)
  }
} catch (error) {
  console.error(`skills-list: ${error.message}`)
  process.exitCode = 1
} finally {
  clearTimeout(timer)
  server.stdin.end()
  server.kill()
}
