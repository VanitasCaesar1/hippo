#!/usr/bin/env node
// A tiny OpenAI-compatible chat completions server with canned replies, for
// the plugin's end-to-end check. It calls no model and needs no key. Every
// request body is appended to $FAKE_LLM_LOG (one JSON object per line).
//
// The script, decided from the request alone:
//   - a request without tools (a title, a summary): a short plain answer;
//   - no tool result yet: call hippo_ask;
//   - one tool result: call hippo_day for the day the synthetic history is on;
//   - two or more: a final answer that quotes both results' first lines.
//
// Usage: FAKE_LLM_LOG=requests.jsonl node fake-llm.mjs [port]
// It prints the port it listens on, on 127.0.0.1 only.

import { appendFileSync } from 'node:fs'
import { createServer } from 'node:http'

const log = process.env.FAKE_LLM_LOG
const day = process.env.FAKE_LLM_DAY ?? '2026-11-02'
const question = process.env.FAKE_LLM_QUESTION ?? 'what thumbnail sizes do we make'

function reply(body) {
  const messages = body.messages ?? []
  const results = messages.filter(m => m.role === 'tool')
  if (!Array.isArray(body.tools) || body.tools.length === 0) return { text: 'hippo plugin check' }
  if (results.length === 0) return { call: { name: 'hippo_ask', arguments: { question } } }
  if (results.length === 1) return { call: { name: 'hippo_day', arguments: { date: day } } }
  const first = m => String(typeof m.content === 'string' ? m.content : m.content?.map(c => c.text ?? '').join('')).split('\n')[0]
  return { text: `Done. hippo_ask said: ${first(results[0])} | hippo_day said: ${first(results[1])}` }
}

function chunk(id, model, delta, finish = null, usage) {
  return { id, object: 'chat.completion.chunk', created: Math.floor(Date.now() / 1000), model, choices: [{ index: 0, delta, finish_reason: finish }], ...(usage ? { usage } : {}) }
}

const server = createServer((req, res) => {
  let raw = ''
  req.on('data', data => { raw += data })
  req.on('end', () => {
    if (req.method === 'GET' && req.url?.endsWith('/models')) {
      res.writeHead(200, { 'content-type': 'application/json' })
      res.end(JSON.stringify({ object: 'list', data: [{ id: 'fake-model', object: 'model' }] }))
      return
    }
    if (req.method !== 'POST' || !req.url?.endsWith('/chat/completions')) {
      res.writeHead(404).end()
      return
    }
    const body = JSON.parse(raw || '{}')
    if (log) appendFileSync(log, JSON.stringify(body) + '\n')
    const out = reply(body)
    const id = `fake-${Date.now()}`
    const model = body.model ?? 'fake-model'
    const usage = { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 }
    const toolCall = out.call && { index: 0, id: `call_${out.call.name}_${Date.now()}`, type: 'function', function: { name: out.call.name, arguments: JSON.stringify(out.call.arguments) } }
    if (!body.stream) {
      const message = out.call ? { role: 'assistant', content: null, tool_calls: [{ ...toolCall, index: undefined }] } : { role: 'assistant', content: out.text }
      res.writeHead(200, { 'content-type': 'application/json' })
      res.end(JSON.stringify({ id, object: 'chat.completion', model, choices: [{ index: 0, message, finish_reason: out.call ? 'tool_calls' : 'stop' }], usage }))
      return
    }
    res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' })
    const send = payload => res.write(`data: ${JSON.stringify(payload)}\n\n`)
    send(chunk(id, model, { role: 'assistant', content: '' }))
    if (out.call) send(chunk(id, model, { tool_calls: [toolCall] }))
    else send(chunk(id, model, { content: out.text }))
    send(chunk(id, model, {}, out.call ? 'tool_calls' : 'stop'))
    send({ id, object: 'chat.completion.chunk', model, choices: [], usage })
    res.end('data: [DONE]\n\n')
  })
})

server.listen(Number(process.argv[2] ?? 0), '127.0.0.1', () => {
  console.log(server.address().port)
})
