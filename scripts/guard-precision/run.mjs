#!/usr/bin/env node
// Measures the guard's precision and recall per rule on the labelled
// synthetic history history.mjs writes: every call through
// `hippo guard check`, in a scratch home, then the same history through
// `hippo ingest`, `hippo guard test` and `hippo incidents`.
//
// Usage: node run.mjs <hippo binary> [calls] [seed]
// Prints a table, the misses and false alarms, and samples of each rule's
// alarms to check by hand. Writes only under a new temporary folder.

import { execFileSync, spawnSync } from 'node:child_process'
import { existsSync, mkdtempSync, readFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const [hippo, calls = '6000', seed = '7'] = process.argv.slice(2)
if (!hippo) { console.error('usage: run.mjs <hippo binary> [calls] [seed]'); process.exit(2) }
const here = dirname(fileURLToPath(import.meta.url))
const out = mkdtempSync(join(process.env.TMPDIR ?? tmpdir(), 'hippo-guard-precision-'))
const env = {
  HOME: out, HIPPO_HOME: join(out, '.hippo'), TZ: 'UTC', PATH: '/usr/bin:/bin',
  XDG_CONFIG_HOME: join(out, '.config'), XDG_DATA_HOME: join(out, '.local/share'),
  XDG_CACHE_HOME: join(out, '.cache'), XDG_STATE_HOME: join(out, '.local/state'),
  CODEX_HOME: join(out, '.codex'),
}
console.log(execFileSync(process.execPath, [join(here, 'history.mjs'), out, calls, seed], { encoding: 'utf8' }).trim())

const KINDS = ['remote-script', 'fresh-clone-exec', 'credential-read', 'guard-bypass', 'self-protection']
const stats = Object.fromEntries(KINDS.map(k => [k, { tp: 0, fp: 0, fn: 0, alarms: [], wrong: [], missed: [] }]))
const log = join(env.HIPPO_HOME, 'guard', 'log.jsonl')
const lines = readFileSync(join(out, 'calls.jsonl'), 'utf8').trim().split('\n').map(l => JSON.parse(l))
let seen = 0
const times = []
for (const c of lines) {
  const started = process.hrtime.bigint()
  const r = spawnSync(hippo, ['guard', 'check', '--agent', c.agent], { input: JSON.stringify(c.payload), env, encoding: 'utf8' })
  times.push(Number(process.hrtime.bigint() - started) / 1e6)
  if (r.status !== 0) { console.error(`check failed on call ${c.n}: ${r.stderr}`); process.exit(1) }
  const logged = existsSync(log) ? readFileSync(log, 'utf8').trim().split('\n').filter(Boolean) : []
  const fresh = logged.slice(seen)
  seen = logged.length
  const rules = new Set(fresh.flatMap(l => JSON.parse(l).rules))
  const what = c.payload.tool_input.command ?? c.payload.tool_input.file_path ?? c.payload.tool_input.absolute_path
  for (const k of KINDS) {
    const want = c.expect.includes(k)
    const got = rules.has(k)
    const s = stats[k]
    if (want && got) { s.tp++; s.alarms.push(`${c.agent} ${c.payload.tool_name}: ${what}`) }
    if (!want && got) { s.fp++; s.wrong.push(`${c.agent} ${c.payload.tool_name} in ${c.payload.cwd}: ${what}`) }
    if (want && !got) { s.fn++; s.missed.push(`${c.agent} ${c.payload.tool_name} in ${c.payload.cwd}: ${what}`) }
  }
}
times.sort((a, b) => a - b)
const q = p => times[Math.min(times.length - 1, Math.floor(p * times.length))].toFixed(1)
console.log(`\n${lines.length} calls through hippo guard check (all agents' payload shapes); per call p50 ${q(0.5)} ms, p95 ${q(0.95)} ms (process start included)\n`)
console.log('rule               alarms  true  false  missed  precision  recall')
for (const k of KINDS) {
  const s = stats[k]
  const p = s.tp + s.fp ? (s.tp / (s.tp + s.fp)).toFixed(3) : '-'
  const r = s.tp + s.fn ? (s.tp / (s.tp + s.fn)).toFixed(3) : '-'
  console.log(`${k.padEnd(18)} ${String(s.tp + s.fp).padStart(6)} ${String(s.tp).padStart(5)} ${String(s.fp).padStart(6)} ${String(s.fn).padStart(7)}  ${p.padStart(9)}  ${r.padStart(6)}`)
}
for (const k of KINDS) {
  const s = stats[k]
  for (const w of [...new Set(s.wrong)].slice(0, 10)) console.log(`FALSE ${k}: ${w}`)
  for (const m of [...new Set(s.missed)].slice(0, 10)) console.log(`MISSED ${k}: ${m}`)
}
// Samples of each rule's alarms, for a person to check.
console.log('\nsamples to check by hand (distinct alarms, up to 6 per rule):')
for (const k of KINDS) {
  for (const a of [...new Set(stats[k].alarms)].slice(0, 6)) console.log(`  ${k}: ${a}`)
}

// The same history as transcripts: guard test (every layer, no folders)
// and the incidents Part A finds.
const run = args => execFileSync(hippo, args, { env, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] })
run(['start', '--yes']).toString()
run(['ingest'])
const test = JSON.parse(run(['guard', 'test', '--days', '30', '--json']))
console.log(`\nhippo guard test over the ingested history (${test.calls} calls):`)
for (const r of test.rules) {
  console.log(`  ${r.rule}: ${r.warn + r.ask + r.block}`)
  for (const rep of (r.repeats ?? []).filter(x => x.calls > 1).slice(0, 2)) console.log(`      ${rep.calls}× ${rep.reason.slice(0, 110)}`)
}
const incidents = JSON.parse(run(['incidents', '--days', '14', '--json']))
const list = Array.isArray(incidents) ? incidents : incidents.incidents ?? []
const byKind = {}
for (const i of list) byKind[i.kind] = (byKind[i.kind] ?? 0) + 1
console.log(`hippo incidents (Part A, from the transcripts): ${Object.entries(byKind).map(([k, n]) => `${k} ${n}`).join(', ') || 'none'}`)
console.log(`\nscratch: ${out}`)
const wrong = KINDS.reduce((n, k) => n + stats[k].fp + stats[k].fn, 0)
if (wrong > 0) { console.error(`guard-precision: ${wrong} false alarm(s) or miss(es)`); process.exit(1) }
