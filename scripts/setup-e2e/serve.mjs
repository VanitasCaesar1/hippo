#!/usr/bin/env node
// Serves one folder over HTTP on 127.0.0.1, read-only, as a stand-in for a
// GitHub release's download URL. Prints the port. Every request is
// appended to $SERVE_LOG.
//
// Usage: SERVE_LOG=log node serve.mjs <folder>

import { appendFileSync, createReadStream, statSync } from 'node:fs'
import { createServer } from 'node:http'
import { join, normalize } from 'node:path'

const root = process.argv[2]
const server = createServer((req, res) => {
  const path = normalize(decodeURIComponent(new URL(req.url, 'http://x').pathname)).replace(/^(\.\.[/\\])+/, '')
  const file = join(root, path)
  if (process.env.SERVE_LOG) appendFileSync(process.env.SERVE_LOG, `${req.method} ${req.url}\n`)
  let ok = false
  try { ok = file.startsWith(root) && statSync(file).isFile() } catch {}
  if (req.method !== 'GET' || !ok) { res.writeHead(404); res.end(); return }
  res.writeHead(200, { 'content-type': 'application/octet-stream' })
  createReadStream(file).pipe(res)
})
server.listen(0, '127.0.0.1', () => console.log(server.address().port))
