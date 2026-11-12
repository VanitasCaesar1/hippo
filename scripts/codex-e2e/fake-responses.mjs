#!/usr/bin/env node
// A tiny server that answers like the OpenAI Responses API with one canned
// reply, for the Codex hooks end-to-end check. It calls no model and needs
// no key. Every request body is appended to $FAKE_LLM_LOG (one JSON object
// per line); other requests get an empty list.
//
// Usage: FAKE_LLM_LOG=requests.jsonl node fake-responses.mjs [port]
// It prints the port it listens on, on 127.0.0.1 only.

import { appendFileSync } from 'node:fs'
import { createServer } from 'node:http'

const log = process.env.FAKE_LLM_LOG
const answer = process.env.FAKE_LLM_ANSWER ?? 'E2E-HANDOFF: the canned model read the notes.'

const server = createServer((req, res) => {
  let raw = ''
  req.on('data', data => { raw += data })
  req.on('end', () => {
    if (req.method !== 'POST' || !req.url?.endsWith('/responses')) {
      res.writeHead(200, { 'content-type': 'application/json' })
      res.end(JSON.stringify({ object: 'list', data: [], models: [] }))
      return
    }
    const body = JSON.parse(raw || '{}')
    if (log) appendFileSync(log, JSON.stringify(body) + '\n')
    const id = `resp_fake_${Date.now()}`
    const events = [
      { type: 'response.created', response: { id } },
      { type: 'response.output_item.done', item: { type: 'message', role: 'assistant', id: `msg_${Date.now()}`, content: [{ type: 'output_text', text: answer }] } },
      { type: 'response.completed', response: { id, usage: { input_tokens: 10, input_tokens_details: null, output_tokens: 5, output_tokens_details: null, total_tokens: 15 } } },
    ]
    res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' })
    for (const event of events) res.write(`event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`)
    res.end()
  })
})

server.listen(Number(process.argv[2] ?? 0), '127.0.0.1', () => {
  console.log(server.address().port)
})
