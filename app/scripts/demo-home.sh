#!/bin/sh
# Build a synthetic, self-contained hippocamp home and workspace for trying
# Hippocamp.app and for taking screenshots.
#
#   app/scripts/demo-home.sh <dir>
#   HIPPOCAMP_HOME=<dir>/home hippocamp ...        # or open Hippocamp.app with the same env
#
# Layout under <dir> (which must not exist or must be empty):
#   home/         HIPPOCAMP_HOME: config.toml only (hippocamp's machine state)
#   ws/           the workspace: notes/ and .hippocamp/ (sessions, proposals,
#                 receipts, reports, snapshots)
#   transcripts/  the only transcript root the config names
#   t3/           a synthetic T3 Code state database (statev2.sqlite)
#
# All data is synthetic and comes from this repository: fixtures/eval (the
# "Tern" photo library notes, transcripts and session records) and
# fixtures/stub-model (a model and reviewer that print canned replies).
# Nothing is read from ~/.claude, ~/.codex or any real project.
#
# What each panel shows afterwards:
#   Brain      the 16 eval notes in notes/ (two folders: Decisions, Incidents),
#              with frontmatter and [[links]]; one note (Backups) carries an
#              applied dreaming change.
#   Sessions   8 declared sessions from fixtures/eval/sessions (1 to 7 Nov
#              2026), the Codex rollout's derived session, one live session
#              started through the CLI today, and, on 8 Nov 2026, the
#              shapes real homes have: "Tern: thumbnail cache rewrite" (a
#              custom-title, a context handoff, a preamble, Markdown, a long
#              tool output) with 3 sub-agents (+3 agents; each ends with
#              Claude Code's trailer, holds reads, a grep, edits and a
#              report, and has a model in its .meta.json) and a background
#              command that failed (exit 1), an untitled session that opens
#              with a command caveat, and a session a program started,
#              hidden unless "agents" is on.
#              "Tern: album covers in parallel", live now with 4
#              background sub-agents each mid-call (a call with no result
#              yet); and a T3 Code session ("Tern: export pipeline",
#              entrypoint t3-code) that delegated two tasks today: "export
#              queue lane" (completed, with its report) and "export retry
#              policy" (running, its transcript written now). Each child is
#              a session of its own; the synthetic T3 state in
#              t3/statev2.sqlite links them (the script exports
#              HIPPOCAMP_T3_STATE, so ~/.t3 is never read). The "now" sessions
#              are timed from when the script runs: open the app soon after.
#              Also Runbooks/ (3 notes with no frontmatter), 2 notes past
#              their review date and a 260 KB generated Operations log.
#   Lessons    four in the Inbox from a `hippocamp lessons scan` with the stub:
#              a correction seen twice in two profiles (previews use the
#              staging database), one seen once (the export button in the
#              toolbar; a new note), and a zsh
#              glob error and a BSD sed error that each repeated in three
#              sessions, whose rules go to one section (a group, Keep all).
#   Fleet      a session waiting on a question, synthetic usage
#              meters for the default and work profiles, and an empty
#              .claude-typo folder (Manage's "Found, never used" when the
#              app runs with HOME=<dir>/profiles).
#   Lanes      yesterday at fixed local hours, so Fleet's Lanes
#              has a full day: a thread that waits on a question, runs
#              three sub-agents and opens and merges a pull request; one
#              that stops at the usage limit and resumes, with a plan
#              question; and one on the other account that carries it on
#              and is corrected (an incident whose step it names).
#   Incidents  one correction of each kind: its step named
#              (bundle), earlier work another session's title names
#              (the album export manifest), the agent's message it answers
#              (with an image link), and nothing to go on.
#   Dreaming   two `hippocamp dream --pass verify` runs today: one failed (the
#              stub's reply is not JSON), then one with 7 proposals: 5
#              confirmed and applied (one of them then reverted with
#              `hippocamp revert`), 2 rejected with the reviewer's notes, 1 more
#              failing validation. The other 4 changes can be reverted from
#              the app.
#
# Not produced: sessions on more than one UTC day through `hippocamp session`
# alone (the CLI stamps "now").
# The multi-day sessions are the fixture records, copied verbatim.
#
# HIPPOCAMP_BIN names the hippocamp binary (default: hippocamp on PATH).
set -eu

usage() {
  echo "usage: demo-home.sh <dir>" >&2
  exit 2
}

[ $# -eq 1 ] || usage
HIPPOCAMP_BIN=${HIPPOCAMP_BIN:-hippocamp}
command -v "$HIPPOCAMP_BIN" >/dev/null 2>&1 || {
  echo "demo-home.sh: hippocamp binary not found (set HIPPOCAMP_BIN)" >&2
  exit 2
}

root=$(cd "$(dirname "$0")/../.." && pwd)
fixtures=$root/fixtures
[ -d "$fixtures/eval/notes" ] || {
  echo "demo-home.sh: $fixtures/eval not found" >&2
  exit 2
}

mkdir -p "$1"
if [ -n "$(ls -A "$1")" ]; then
  echo "demo-home.sh: $1 is not empty; pick a new directory" >&2
  exit 1
fi
dir=$(cd "$1" && pwd -P)
home=$dir/home
ws=$dir/ws
transcripts=$dir/transcripts
model=$fixtures/stub-model/model.sh

mkdir -p "$home" "$ws" "$transcripts"
cp -R "$fixtures/eval/notes" "$ws/notes"

# Notes as a real brain has them: some with no frontmatter, some past their
# review date, and one long generated log (over 200 KB, more than a pass's
# budget and more than the app shows in a tab).
mkdir -p "$ws/notes/Runbooks"
for name in "Restore a backup" "Rotate the signing key" "Pager escalation"; do
  printf '# %s\n\nSteps written down during an incident; not yet reviewed.\n' "$name" \
    >"$ws/notes/Runbooks/$name.md"
done
for pair in "Cache sizing:2026-09-30" "CDN vendors:2026-10-15"; do
  name=${pair%%:*}
  printf -- '---\ntype: topic\nstatus: current\nupdated: 2026-08-01\nreview_after: %s\n---\n# %s\n\nWritten in August; see [[Thumbnail pipeline]].\n' \
    "${pair#*:}" "$name" >"$ws/notes/$name.md"
done
{
  printf '# Operations log\n\nOne line per deploy, newest last. Generated.\n\n'
  i=1
  while [ $i -le 2600 ]; do
    printf -- '- Deploy %04d: web and worker rolled out, thumbnails regenerated for the new album covers, no alerts.\n' $i
    i=$((i + 1))
  done
} >"$ws/notes/Operations log.md"
cp -R "$fixtures/eval/transcripts/." "$transcripts/"

HIPPOCAMP_HOME=$home
export HIPPOCAMP_HOME
# The T3 Code state the engine reads for delegated tasks is the synthetic one
# below, never ~/.t3.
HIPPOCAMP_T3_STATE=$dir/t3/statev2.sqlite
export HIPPOCAMP_T3_STATE
hippocamp() { command "$HIPPOCAMP_BIN" "$@"; }

hippocamp init --workspace "$ws" --notes-dir notes --entry-note "notes/START HERE.md" >/dev/null

# init writes a default config.toml; replace it with one that points the
# transcript root inside <dir> and uses the stub as model and reviewer.
cat >"$home/config.toml" <<CFG
workspace = "$ws"
transcript_roots = ["$transcripts"]
model_command = ["$model", "--system-prompt", "{system_prompt}"]
reviewer_command_override = ["env", "HIPPOCAMP_STUB_REVIEW=1", "$model", "--system-prompt", "{system_prompt}"]
CFG

# Eight declared sessions over 1 to 7 November 2026 (fixture records; their
# notes_touched make the verify pass take the notes the stub corrects).
mkdir -p "$ws/.hippocamp/sessions"
cp "$fixtures"/eval/sessions/*.json "$ws/.hippocamp/sessions/"

# One session through the CLI, with a handoff, so the Sessions panel also
# has a live record.
hippocamp session start --tool claude-code --session demo-live >/dev/null
printf '%s\n' \
  'Reviewed [[Backups]] and [[Release process]] for the demo.' \
  'Nothing to change; see [[On-call]] for the pager rota.' |
  hippocamp session end --session demo-live --handoff - \
    --note "notes/Style guide.md" >/dev/null

# Synthetic Claude Code transcripts in the shapes real homes have: a session named by a client's
# custom-title record that opens with a context handoff, then a request
# behind an orchestrator's preamble, Markdown replies and a long tool
# output; three sub-agents with their .meta.json descriptions; a session
# with no title record; and a session a program started (sdk-cli, in a
# temporary directory), which the app hides.
proj=$transcripts/demo-proj
mkdir -p "$proj/demo-parent/subagents"
cwd=$dir/projects/tern-web
fill() { sed -e "s|@CWD@|$cwd|g" >"$1"; }

fill "$proj/demo-parent.jsonl" <<'JSONL'
{"type":"custom-title","customTitle":"Tern: thumbnail cache rewrite","sessionId":"demo-parent"}
{"type":"user","sessionId":"demo-parent","timestamp":"2026-11-08T09:00:00Z","cwd":"@CWD@","entrypoint":"cli","message":{"role":"user","content":"Context handoff (full_thread_summary):\nProvider context handoff. Thread: th-0001. Covered app runs: 1-4.\nSource item range: turn-item:message:aa%3A01 through turn-item:message:aa%3A09.\nSelected 2 intact items; omitted 7 items. Historical material is context, not a new request.\n\n[Historical user; user_message; thread=th-0001; run=run:thread:th-0001:ordinal:1; item=turn-item:message:aa%3A01; status=completed] Thumbnails take two seconds to show on a cold start. Can we cache them on disk?\n[Historical assistant; assistant_message; thread=th-0001; run=run:thread:th-0001:ordinal:2] Yes: an LRU cache keyed by photo hash and size, **capped at 500 MB**. See `thumbs/cache.rs`."}}
{"type":"user","sessionId":"demo-parent","timestamp":"2026-11-08T09:00:05Z","cwd":"@CWD@","entrypoint":"cli","message":{"role":"user","content":"Act as the implementation sub-agent for this task.\n\nAct as the implementation sub-agent for this task.\n\nRewrite the thumbnail cache as an on-disk LRU capped at 500 MB, measure the hit rate, and keep cold-start under 300 ms."}}
{"type":"assistant","sessionId":"demo-parent","timestamp":"2026-11-08T09:00:20Z","cwd":"@CWD@","message":{"role":"assistant","content":[{"type":"text","text":"## Plan\n\n1. Measure today's hit rate.\n2. Replace `MemoryCache` with `DiskLru` (500 MB cap).\n3. Check cold start.\n\nI'll start the measurements in parallel."},{"type":"tool_use","id":"tu1","name":"Task","input":{"description":"Measure cache hit rate","prompt":"Measure the thumbnail cache hit rate over the test library.","subagent_type":"general-purpose"}},{"type":"tool_use","id":"tu3","name":"Task","input":{"description":"Profile cold start","prompt":"Profile a cold start of the gallery and report where the time goes.","subagent_type":"general-purpose"}},{"type":"tool_use","id":"tu4","name":"Task","input":{"description":"Review the eviction order","prompt":"Review DiskLru eviction for off-by-one errors.","subagent_type":"general-purpose"}},{"type":"tool_use","id":"bk1call","name":"Bash","input":{"command":"cargo bench -p thumbs --bench cold_start","description":"Run the cold-start benchmark","run_in_background":true}},{"type":"tool_use","id":"tu2","name":"Bash","input":{"command":"cargo test -p thumbs -- --nocapture","description":"Run the thumbnail tests"}}]}}
{"type":"user","sessionId":"demo-parent","timestamp":"2026-11-08T09:00:30Z","cwd":"@CWD@","entrypoint":"cli","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"bk1call","content":"Command running in background with ID: bk1"}]}}
{"type":"user","sessionId":"demo-parent","timestamp":"2026-11-08T09:01:10Z","cwd":"@CWD@","entrypoint":"cli","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu2","content":"running 48 tests\ntest cache::evicts_oldest ... ok\ntest cache::caps_at_limit ... ok\ntest cache::survives_restart ... ok\ntest cache::hash_key_is_stable ... ok\ntest cache::size_is_part_of_key ... ok\ntest cache::concurrent_reads ... ok\ntest cache::concurrent_writes ... ok\ntest cache::corrupt_entry_is_dropped ... ok\ntest cache::missing_dir_is_created ... ok\ntest cache::zero_cap_disables ... ok\ntest decode::jpeg ... ok\ntest decode::png ... ok\ntest decode::heic ... ok\ntest decode::webp ... ok\ntest decode::rotated ... ok\ntest decode::truncated_file_errors ... ok\ntest resize::small ... ok\ntest resize::large ... ok\ntest resize::square ... ok\ntest resize::portrait ... ok\ntest resize::landscape ... ok\ntest resize::upscale_refused ... ok\ntest store::write_atomic ... ok\ntest store::read_back ... ok\ntest store::index_rebuild ... ok\ntest store::index_corrupt ... ok\ntest store::stale_lock ... ok\ntest store::two_writers ... ok\ntest store::fsync_on_close ... ok\ntest store::rename_over ... ok\ntest warm::prefetch_visible ... ok\ntest warm::prefetch_cancel ... ok\ntest warm::prefetch_order ... ok\ntest warm::cold_start_budget ... ok\ntest warm::no_prefetch_on_battery ... ok\ntest warm::visible_first ... ok\ntest warm::scroll_burst ... ok\ntest warm::scroll_stop ... ok\ntest api::get ... ok\ntest api::put ... ok\ntest api::remove ... ok\ntest api::clear ... ok\ntest api::stats ... ok\ntest api::resize_cap ... ok\ntest api::open_twice ... ok\ntest api::close ... ok\ntest api::drop_flushes ... ok\ntest api::errors_are_typed ... ok\n\ntest result: ok. 48 passed; 0 failed"}]}}
{"type":"user","sessionId":"demo-parent","timestamp":"2026-11-08T09:03:08Z","cwd":"@CWD@","entrypoint":"cli","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu1","content":"Hit rate 61% cold, 97% warm over 12,400 thumbnails.\n\nI added hit and miss counters to the cache and replayed the library twice: a cold run, then a warm one.\nagentId: a1 (use SendMessage with to: 'a1' to continue this agent)\n<usage>subagent_tokens: 48210\ntool_uses: 4\nduration_ms: 160000</usage>"}]}}
{"type":"user","sessionId":"demo-parent","timestamp":"2026-11-08T09:03:38Z","cwd":"@CWD@","entrypoint":"cli","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu3","content":"Decoding is 70% of cold start; the cache removes it.\n\nThe gallery decoded every photo before drawing the first row. It now decodes the visible ones first and queues the rest.\nagentId: a2 (use SendMessage with to: 'a2' to continue this agent)\n<usage>subagent_tokens: 31870\ntool_uses: 4\nduration_ms: 190000</usage>"}]}}
{"type":"user","sessionId":"demo-parent","timestamp":"2026-11-08T09:04:50Z","cwd":"@CWD@","entrypoint":"cli","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu4","content":"Eviction order is correct; the cap check was off by one.\n\nThe cache kept one entry too many at the limit. I changed the check to use the cap as a hard bound and added a test for it.\nagentId: a3 (use SendMessage with to: 'a3' to continue this agent)\n<usage>subagent_tokens: 39440\ntool_uses: 5\nduration_ms: 258000</usage>"}]}}
{"type":"user","sessionId":"demo-parent","timestamp":"2026-11-08T09:05:10Z","cwd":"@CWD@","entrypoint":"cli","message":{"role":"user","content":"<task-notification>\n<task-id>bk1</task-id>\n<tool-use-id>bk1call</tool-use-id>\n<status>failed</status>\n<summary>Background command \"Run the cold-start benchmark\" failed with exit code 1</summary>\n</task-notification>"}}
{"type":"assistant","sessionId":"demo-parent","timestamp":"2026-11-08T09:05:20Z","cwd":"@CWD@","message":{"role":"assistant","content":[{"type":"tool_use","id":"tue1","name":"Edit","input":{"file_path":"@CWD@/src/thumbs/cache.rs","old_string":"pub struct Cache {\n    mem: MemoryCache,\n}","new_string":"pub struct Cache {\n    disk: DiskLru,\n    cap: u64,\n}"}}]}}
{"type":"user","sessionId":"demo-parent","timestamp":"2026-11-08T09:05:22Z","cwd":"@CWD@","entrypoint":"cli","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tue1","content":"The file @CWD@/src/thumbs/cache.rs has been updated successfully."}]}}
{"type":"assistant","sessionId":"demo-parent","timestamp":"2026-11-08T09:05:30Z","cwd":"@CWD@","message":{"role":"assistant","content":[{"type":"text","text":"Done.\n\n- **Hit rate:** 61% cold, 97% warm.\n- **Cold start:** 240 ms (was 2.1 s).\n- All 48 thumbnail tests pass.\n\n```rust\nlet cache = DiskLru::open(dir, 500 * MB)?;\n```\n\nDetails: https://example.com/tern/thumbs"}]}}
JSONL

# A long session in the shape real ones have: a context handoff before the request, the request
# behind a preamble with a pasted image, a run of 40 tool calls with JSON
# arguments among harness noise (a tool search, a skill, a reminder), a
# whole-file read and two edits of it, a new file written, a compaction
# summary, a reply that names a pull request, and the branch, model and
# effort on its records.
long=$proj/demo-long.jsonl
{
  ts() { printf '2026-11-08T%02d:%02d:%02dZ' $((10 + $1 / 3600)) $(($1 / 60 % 60)) $(($1 % 60)); }
  meta='"sessionId":"demo-long","cwd":"@CWD@","gitBranch":"feat/disk-cache","entrypoint":"cli"'
  amsg() { printf '{"type":"assistant",%s,"timestamp":"%s","message":{"role":"assistant","model":"claude-demo-1","output_config":{"effort":"high"},"content":[%s]}}\n' "$meta" "$(ts $1)" "$2"; }
  umsg() { printf '{"type":"user",%s,"timestamp":"%s","message":{"role":"user","content":%s}}\n' "$meta" "$(ts $1)" "$2"; }
  call() { amsg $1 "{\"type\":\"tool_use\",\"id\":\"$2\",\"name\":\"$3\",\"input\":$4}"; }
  result() { umsg $1 "[{\"type\":\"tool_result\",\"tool_use_id\":\"$2\",\"content\":\"$3\"}]"; }
  printf '{"type":"custom-title","customTitle":"Tern: disk cache for thumbnails","sessionId":"demo-long"}\n'
  umsg 0 '"Context handoff (full_thread_summary):\nProvider context handoff. Thread: th-0002.\nSelected 2 intact items; omitted 5 items.\n\n[Historical user; user_message; thread=th-0002; item=bb%3A01] Thumbnails are slow on a cold start.\n[Historical assistant; assistant_message; thread=th-0002] A disk cache would fix it."'
  umsg 2 '"<system-reminder>Project instructions follow.</system-reminder>"'
  umsg 3 '"Act as the implementation sub-agent for this task.\n\nAdd a disk cache for thumbnails, capped at 500 MB, and open a pull request.\n\n[Attached image \"cold-start.png\" is saved at: /tmp/tern/cold-start.png]"'
  umsg 3 '"[Image: source: /tmp/tern/images/1.png]"'
  amsg 5 '{"type":"text","text":"Reading the cache code first."}'
  call 6 ts1 ToolSearch '{"query":"select:mcp__docs__read"}'
  result 6 ts1 'Found 1 tool.'
  call 7 sk1 Skill '{"skill":"review"}'
  result 7 sk1 'Launching skill: review'
  i=0
  while [ $i -lt 40 ]; do
    t=$((10 + i * 3))
    call $t "b$i" Bash "{\"command\":\"cargo test -p thumbs cache::case_$i\",\"description\":\"Run cache case $i\"}"
    result $((t + 1)) "b$i" "test cache::case_$i ... ok"
    i=$((i + 1))
  done
  call 140 r1 Read '{"file_path":"@CWD@/src/thumbs/cache.rs"}'
  result 140 r1 '1\tuse std::path::Path;\n2\t\n3\tpub struct Cache {\n4\t    cap: u64,\n5\t}\n6\t\n7\timpl Cache {\n8\t    pub fn new() -> Cache {\n9\t        Cache { cap: 0 }\n10\t    }\n11\t}'
  call 150 e1 Edit '{"file_path":"@CWD@/src/thumbs/cache.rs","old_string":"pub struct Cache {\n    cap: u64,\n}","new_string":"pub struct Cache {\n    cap: u64,\n    dir: std::path::PathBuf,\n}"}'
  result 151 e1 'The file @CWD@/src/thumbs/cache.rs has been updated successfully.'
  call 160 e2 Edit '{"file_path":"@CWD@/src/thumbs/cache.rs","old_string":"        Cache { cap: 0 }","new_string":"        Cache { cap: 500 << 20, dir: Path::new(\"thumbs\").into() }"}'
  result 161 e2 'The file @CWD@/src/thumbs/cache.rs has been updated successfully.'
  call 170 w1 Write '{"file_path":"@CWD@/src/thumbs/lru.rs","content":"//! Least recently used eviction.\n\npub fn evict(sizes: &mut Vec<u64>, cap: u64) {\n    while sizes.iter().sum::<u64>() > cap {\n        sizes.remove(0);\n    }\n}\n"}'
  result 171 w1 'File created successfully at: @CWD@/src/thumbs/lru.rs'
  call 175 e3 Edit '{"file_path":"@CWD@/src/thumbs/mod.rs","old_string":"zzz","new_string":"y"}'
  result 176 e3 '<tool_use_error>String to replace not found in file.</tool_use_error>'
  umsg 180 '"This session is being continued from a previous conversation that ran out of context. The summary below covers the earlier portion of the conversation.\n\nSummary: the cache struct and the LRU module are written; tests pass."'
  call 185 td1 TodoWrite '{"todos":[]}'
  result 185 td1 'Todos updated.'
  amsg 7300 '{"type":"text","text":"Done: thumbnails come from a 500 MB disk cache, cold start is **240 ms**, and the tests pass. Opened https://github.com/tern-example/tern/pull/12."}'
} | fill "$long"

fill "$proj/demo-plain.jsonl" <<'JSONL'
{"type":"user","sessionId":"demo-plain","timestamp":"2026-11-08T08:10:00Z","cwd":"@CWD@","entrypoint":"cli","message":{"role":"user","content":"<local-command-caveat>Caveat: the command below ran locally.</local-command-caveat>"}}
{"type":"user","sessionId":"demo-plain","timestamp":"2026-11-08T08:10:05Z","cwd":"@CWD@","entrypoint":"cli","message":{"role":"user","content":"Why does the share sheet open twice on iPad?"}}
{"type":"assistant","sessionId":"demo-plain","timestamp":"2026-11-08T08:10:30Z","cwd":"@CWD@","message":{"role":"assistant","content":[{"type":"text","text":"Both the toolbar button and the long-press handler call `present`; the second call wins on iPad."}]}}
JSONL

sed -e "s|@CWD@|/tmp/tern-title-demo|g" >"$proj/demo-auto.jsonl" <<'JSONL'
{"type":"user","sessionId":"demo-auto","timestamp":"2026-11-08T09:06:00Z","cwd":"@CWD@","entrypoint":"sdk-cli","message":{"role":"user","content":"Generate a title for this thread. Return JSON."}}
{"type":"assistant","sessionId":"demo-auto","timestamp":"2026-11-08T09:06:01Z","cwd":"@CWD@","message":{"role":"assistant","content":[{"type":"text","text":"{\"title\":\"Thumbnail cache\"}"}]}}
JSONL

# Lessons: two profiles' sessions from the last
# hours where the person corrects the agent (previews against production,
# twice; the export button in the footer, once) and three where the same zsh glob error
# repeats. Their timestamps are relative to now, so the lessons pass's
# 7-day window holds them.
ago() { date -u -v-"$1"M +%Y-%m-%dT%H:%M:%SZ; }
for profile in .claude .claude-work; do
  mkdir -p "$dir/profiles/$profile/projects/-tern-web"
done
lesson_session() {
  # file id minutes-ago tool-command person-text
  file=$1; id=$2; m=$3; cmd=$4; said=$5
  {
    printf '{"type":"user","sessionId":"%s","timestamp":"%s","cwd":"%s","entrypoint":"cli","message":{"role":"user","content":"Check the gallery page and send me screenshots."}}\n' "$id" "$(ago $((m + 3)))" "$cwd"
    printf '{"type":"assistant","sessionId":"%s","timestamp":"%s","cwd":"%s","message":{"role":"assistant","content":[{"type":"tool_use","id":"l1","name":"Bash","input":{"command":"%s"}}]}}\n' "$id" "$(ago $((m + 2)))" "$cwd" "$cmd"
    printf '{"type":"user","sessionId":"%s","timestamp":"%s","cwd":"%s","entrypoint":"cli","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"l1","content":"saved gallery.png"}]}}\n' "$id" "$(ago $((m + 1)))" "$cwd"
    printf '{"type":"user","sessionId":"%s","timestamp":"%s","cwd":"%s","entrypoint":"cli","message":{"role":"user","content":"%s"}}\n' "$id" "$(ago "$m")" "$cwd" "$said"
  } >"$file"
}
lesson_session "$dir/profiles/.claude/projects/-tern-web/demo-l1.jsonl" demo-l1 300 \
  "npm run preview -- --db production gallery" "no, previews use the staging database, not production"
lesson_session "$dir/profiles/.claude-work/projects/-tern-web/demo-l2.jsonl" demo-l2 95 \
  "npm run preview -- --db production album" "dont point previews at the production database again, staging only"
lesson_session "$dir/profiles/.claude-work/projects/-tern-web/demo-l3.jsonl" demo-l3 40 \
  "sed -i '' s/toolbar/footer/ src/export.css" "why is the export button in the footer? it goes in the toolbar"
for k in 1 2 3; do
  id=demo-f$k
  file=$dir/profiles/.claude/projects/-tern-web/$id.jsonl
  {
    printf '{"type":"user","sessionId":"%s","timestamp":"%s","cwd":"%s","entrypoint":"cli","message":{"role":"user","content":"Find where the upload hook is used."}}\n' "$id" "$(ago $((200 - k * 30 + 2)))" "$cwd"
    printf '{"type":"assistant","sessionId":"%s","timestamp":"%s","cwd":"%s","message":{"role":"assistant","content":[{"type":"tool_use","id":"g1","name":"Bash","input":{"command":"grep -rn useUpload --include=*.ts src"}}]}}\n' "$id" "$(ago $((200 - k * 30 + 1)))" "$cwd"
    printf '{"type":"user","sessionId":"%s","timestamp":"%s","cwd":"%s","entrypoint":"cli","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"g1","content":"Exit code 1\\n(eval):1: no matches found: --include=*.ts"}]}}\n' "$id" "$(ago $((200 - k * 30)))" "$cwd"
  } >"$file"
done

# A second repeated failure: BSD sed's -i needs
# an argument, three times; its rule goes to the same note section as the
# zsh glob rule, so the Inbox groups the two under Keep all.
for k in 1 2 3; do
  id=demo-s$k
  file=$dir/profiles/.claude/projects/-tern-web/$id.jsonl
  {
    printf '{"type":"user","sessionId":"%s","timestamp":"%s","cwd":"%s","entrypoint":"cli","message":{"role":"user","content":"Rename the footer class to panel."}}\n' "$id" "$(ago $((150 - k * 20 + 2)))" "$cwd"
    printf '{"type":"assistant","sessionId":"%s","timestamp":"%s","cwd":"%s","message":{"role":"assistant","content":[{"type":"tool_use","id":"s1","name":"Bash","input":{"command":"sed -i s/footer/panel/ src/layout.css"}}]}}\n' "$id" "$(ago $((150 - k * 20 + 1)))" "$cwd"
    printf '{"type":"user","sessionId":"%s","timestamp":"%s","cwd":"%s","entrypoint":"cli","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"s1","content":"Exit code 1\\nsed: 1: \\"src/layout.css\\": invalid command code s"}]}}\n' "$id" "$(ago $((150 - k * 20)))" "$cwd"
  } >"$file"
done

# A session waiting on the person (its last turn asks a question),
# each profile's cached usage meter (synthetic, in the files Claude Code
# keeps it in: profiles/.claude.json beside .claude, and .claude.json inside
# .claude-work), and a profile folder with no session and no meter, which
# Manage lists as found, never used when the app runs with
# HOME=<dir>/profiles.
wfile=$dir/profiles/.claude-work/projects/-tern-web/demo-w1.jsonl
{
  printf '{"type":"user","sessionId":"demo-w1","timestamp":"%s","cwd":"%s","entrypoint":"cli","message":{"role":"user","content":"Get the release branch ready to merge."}}\n' "$(ago 30)" "$cwd"
  printf '{"type":"assistant","sessionId":"demo-w1","timestamp":"%s","cwd":"%s","message":{"role":"assistant","content":[{"type":"tool_use","id":"w1","name":"Bash","input":{"command":"npm test"}}]}}\n' "$(ago 12)" "$cwd"
  printf '{"type":"user","sessionId":"demo-w1","timestamp":"%s","cwd":"%s","entrypoint":"cli","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"w1","content":"Tests: 48 passed, 48 total"}]}}\n' "$(ago 11)" "$cwd"
  printf '{"type":"assistant","sessionId":"demo-w1","timestamp":"%s","cwd":"%s","message":{"role":"assistant","stop_reason":"end_turn","content":[{"type":"text","text":"The branch is rebased and all 48 tests pass.\\n\\nShould I merge it into main now, or wait for the design review?"}]}}\n' "$(ago 9)" "$cwd"
} >"$wfile"
iso_in() { date -u -r $(( $(date +%s) + $1 )) +%Y-%m-%dT%H:%M:%S+00:00; }
fetched=$(( ($(date +%s) - 180) * 1000 ))
printf '{"numStartups":3,"cachedUsageUtilization":{"fetchedAtMs":%s,"utilization":{"five_hour":{"utilization":30,"resets_at":"%s"},"seven_day":{"utilization":81,"resets_at":"%s"}}}}\n' \
  "$fetched" "$(iso_in 7200)" "$(iso_in 90000)" >"$dir/profiles/.claude.json"
printf '{"numStartups":3,"cachedUsageUtilization":{"fetchedAtMs":%s,"utilization":{"five_hour":{"utilization":0,"resets_at":null},"seven_day":{"utilization":12,"resets_at":"%s"}}}}\n' \
  "$fetched" "$(iso_in 430000)" >"$dir/profiles/.claude-work/.claude.json"
mkdir -p "$dir/profiles/.claude-typo/projects"

# The context x-ray: six sessions in the
# workspace from the last hour and a half, each with hippocamp's hook output as
# the client records it. The first reads one required note, searches the
# notes for the signing key in vain and asks, finds it in On-call, edits
# and tests, merges and claims a deploy it shows no command for; the others
# never open the Style guide, which is required, so the fleet's tune tip
# offers to make it relevant-only; one claims a typecheck it never ran.
sed -i '' -e 's|^required_notes = .*|required_notes = ["notes/START HERE.md", "notes/On-call.md", "notes/Style guide.md"]|' "$ws/.hippocamp/hippocamp.toml"
xproj=$dir/profiles/.claude/projects/$(printf '%s' "$ws" | tr -c 'A-Za-z0-9\n' '-')
mkdir -p "$xproj" "$ws/src"
printf 'export const sign = () => null\n' >"$ws/src/release.ts"
gap=1 # minutes between one turn and the next
xray_session() {
  # id minutes-ago, then the turns on stdin as `kind|json` lines
  id=$1; m=$2; file=$xproj/$id.jsonl
  start=$(printf '# Tern notes\\n\\nThe notes every agent reads first.\\n\\nrequired: notes/START HERE.md\\nrequired: notes/On-call.md\\nrequired: notes/Style guide.md\\n\\nsession: %s claude-code %s\\n' "$id" "$(ago "$m")")
  prompt='relevant:\n- Release process: Releases go out from main after the checks pass.\n\nlinter: overdue=2 dangling=0\n'
  meta=$(printf '"sessionId":"%s","cwd":"%s","entrypoint":"cli"' "$id" "$ws")
  {
    printf '{"type":"attachment",%s,"timestamp":"%s","attachment":{"type":"hook_success","hookEvent":"SessionStart","command":"hippocamp hook session-start","stdout":"%s"}}\n' "$meta" "$(ago "$m")" "$start"
    k=0
    while IFS='|' read -r kind body; do
      t=$(ago $((m - k * gap)))
      case $kind in
        user) printf '{"type":"user",%s,"timestamp":"%s","message":{"role":"user","content":%s}}\n' "$meta" "$t" "$body" ;;
        hook) printf '{"type":"attachment",%s,"timestamp":"%s","attachment":{"type":"hook_success","hookEvent":"UserPromptSubmit","command":"hippocamp hook prompt","stdout":"%s"}}\n' "$meta" "$t" "$prompt" ;;
        say) printf '{"type":"assistant",%s,"timestamp":"%s","message":{"role":"assistant","content":[{"type":"text","text":%s}]}}\n' "$meta" "$t" "$body" ;;
        call) printf '{"type":"assistant",%s,"timestamp":"%s","message":{"role":"assistant","content":[%s]}}\n' "$meta" "$t" "$body" ;;
        result) printf '{"type":"user",%s,"timestamp":"%s","message":{"role":"user","content":[%s]}}\n' "$meta" "$t" "$body" ;;
      esac
      k=$((k + 1))
    done
  } >"$file"
}
xray_session demo-x1 80 <<XRAY
user|"Fix the release script so it signs the build, then merge and ship it."
hook|
call|{"type":"tool_use","id":"x1","name":"Read","input":{"file_path":"$ws/notes/START HERE.md"}}
result|{"type":"tool_result","tool_use_id":"x1","content":"# Start here"}
call|{"type":"tool_use","id":"x2","name":"Grep","input":{"pattern":"signing key","path":"$ws/notes"}}
result|{"type":"tool_result","tool_use_id":"x2","content":"No files found"}
say|"I can't find where the signing key is kept. Where does it live?"
user|"in the On-call note, under Keys"
call|{"type":"tool_use","id":"x3","name":"Read","input":{"file_path":"$ws/notes/On-call.md"}}
result|{"type":"tool_result","tool_use_id":"x3","content":"# On-call"}
call|{"type":"tool_use","id":"x4","name":"Edit","input":{"file_path":"$ws/src/release.ts","old_string":"null","new_string":"signWith(key)"}}
result|{"type":"tool_result","tool_use_id":"x4","content":"The file has been updated."}
call|{"type":"tool_use","id":"x5","name":"Bash","input":{"command":"cd $ws && npm test"}}
result|{"type":"tool_result","tool_use_id":"x5","content":"Tests  12 passed (12)"}
say|"Fixed: the release script signs the build with the key from On-call. All 12 tests pass."
call|{"type":"tool_use","id":"x6","name":"Bash","input":{"command":"gh pr merge 7 --squash"}}
result|{"type":"tool_result","tool_use_id":"x6","content":"✓ Squashed and merged pull request #7"}
say|"PR #7 merged. Deployed to production."
XRAY
for k in 2 3 4 5 6; do
  xray_session "demo-x$k" $((70 - k * 8)) <<XRAY
user|"Tidy the upload API notes for release $k."
hook|
call|{"type":"tool_use","id":"y1","name":"Read","input":{"file_path":"$ws/notes/Upload API.md"}}
result|{"type":"tool_result","tool_use_id":"y1","content":"# Upload API"}
say|"Done; the Upload API note reads cleanly now."
XRAY
done
xray_session demo-x7 12 <<XRAY
user|"Rename the sign helper."
hook|
call|{"type":"tool_use","id":"z1","name":"Edit","input":{"file_path":"$ws/src/release.ts","old_string":"signWith","new_string":"signBuild"}}
result|{"type":"tool_result","tool_use_id":"z1","content":"The file has been updated."}
say|"Renamed. Typecheck is clean."
XRAY
# Incidents: synthetic sessions that make at
# least one incident of every kind, with several causes. Times are relative
# to now, so today's demo has incidents today. `gap` is the minutes between
# one turn and the next in xray_session (1 by default).
cat >>"$ws/notes/Style guide.md" <<'NOTE'

Never bundle the app mid-work. Bundle it only when asked.
NOTE
# A correction with no rule that covers it (the start block is recorded).
gap=6
xray_session demo-i1 150 <<XRAY
user|"Build the demo app so I can look at it."
hook|
call|{"type":"tool_use","id":"i1a","name":"Bash","input":{"command":"cd $ws && cargo build --release"}}
result|{"type":"tool_result","tool_use_id":"i1a","content":"Finished release in 4m 10s"}
user|"no, use the debug build only, I said"
XRAY
# A correction the Style guide already covered (a paragraph holds its words).
gap=8
xray_session demo-i2 110 <<XRAY
user|"Add the cache cap test."
hook|
call|{"type":"tool_use","id":"i2a","name":"Bash","input":{"command":"cd $ws && ./app/scripts/bundle.sh"}}
result|{"type":"tool_result","tool_use_id":"i2a","content":"Bundled Hippocamp.app"}
user|"no, never bundle the app mid-work, I said that before"
XRAY
# A correction about earlier work, and one answering the agent's
# message. The first names two words of another session's title (the work
# it may be about); the second names nothing, so it reads as answering what
# the agent just said, whose image link shows as the file's name.
gap=3
xray_session demo-k1 400 <<XRAY
user|"Write the album export manifest."
hook|
call|{"type":"tool_use","id":"k1a","name":"Write","input":{"file_path":"$ws/src/export/manifest.ts","content":"export const manifest = [];"}}
result|{"type":"tool_result","tool_use_id":"k1a","content":"File created successfully at: $ws/src/export/manifest.ts"}
say|"The album export manifest is written."
XRAY
xray_session demo-k2 40 <<XRAY
user|"Check the gallery loads."
hook|
call|{"type":"tool_use","id":"k2a","name":"Read","input":{"file_path":"$ws/src/gallery.ts"}}
result|{"type":"tool_result","tool_use_id":"k2a","content":"export function gallery() {}"}
say|"The gallery loads."
user|"no, the album export manifest is wrong again, it lists nothing"
XRAY
xray_session demo-k3 30 <<XRAY
user|"Show me the export settings."
hook|
say|"Exports are JPEG at 90% quality, up to 4096 px. ![export settings](/tmp/demo/export-settings.png)"
user|"no, that is wrong: exports are PNG"
XRAY
# A hook's block, and a person's refusal.
gap=1
xray_session demo-i3 95 <<XRAY
user|"Start two helpers on the album covers."
hook|
call|{"type":"tool_use","id":"i3a","name":"Bash","input":{"command":"./deploy.sh production covers"}}
result|{"type":"tool_result","tool_use_id":"i3a","content":"PreToolUse:Bash hook error: [/x/block-prod-deploy]: Blocked: a deploy to production needs a release tag."}
say|"I will deploy to staging instead."
XRAY
xray_session demo-i4 90 <<XRAY
user|"Tidy the temp folder."
hook|
call|{"type":"tool_use","id":"i4a","name":"Bash","input":{"command":"mv $ws/src/release.ts $ws/src/release.old"}}
result|{"type":"tool_result","tool_use_id":"i4a","content":"The user doesn't want to proceed with this tool use. The tool use was rejected."}
XRAY
# A destructive command a correction names within the hour, and one nobody
# objected to (listed quietly).
gap=5
xray_session demo-i5 75 <<XRAY
user|"Publish the notes branch."
hook|
call|{"type":"tool_use","id":"i5a","name":"Bash","input":{"command":"cd $ws && git push --force origin notes"}}
result|{"type":"tool_result","tool_use_id":"i5a","content":"+ 4f2a1c0...9be3d71 notes -> notes (forced update)"}
user|"no, why did you force push"
XRAY
gap=1
xray_session demo-i6 60 <<XRAY
user|"Drop the last commit on the scratch branch."
hook|
call|{"type":"tool_use","id":"i6a","name":"Bash","input":{"command":"cd $ws && git reset --hard HEAD~1"}}
result|{"type":"tool_result","tool_use_id":"i6a","content":"HEAD is now at 1a2b3c4 scratch"}
say|"The scratch branch is back one commit."
XRAY
# A failed background command nobody redid.
xray_session demo-i7 55 <<XRAY
user|"Run the long export in the background."
hook|
call|{"type":"tool_use","id":"i7a","name":"Bash","input":{"command":"./export-all.sh","description":"Run the long export","run_in_background":true}}
result|{"type":"tool_result","tool_use_id":"i7a","content":"Command running in background with ID: bg7"}
XRAY
printf '{"type":"user","sessionId":"demo-i7","timestamp":"%s","cwd":"%s","entrypoint":"cli","message":{"role":"user","content":"<task-notification>\\n<task-id>bg7</task-id>\\n<tool-use-id>i7a</tool-use-id>\\n<status>failed</status>\\n<summary>Background command \\"Run the long export\\" failed with exit code 1</summary>\\n</task-notification>"}}\n' "$(ago 50)" "$ws" >>"$xproj/demo-i7.jsonl"
# Lines redone by another session: 24 lines of one file added by one
# session and removed by a different one hours later.
lines24=$(i=1; while [ "$i" -le 24 ]; do printf 'fn cover_%d() { render(%d) }\\n' "$i" "$i"; i=$((i + 1)); done)
gap=1
xray_session demo-i8 240 <<XRAY
user|"Add the cover renderers."
hook|
call|{"type":"tool_use","id":"i8a","name":"Edit","input":{"file_path":"$ws/src/covers.ts","old_string":"// covers","new_string":"$lines24"}}
result|{"type":"tool_result","tool_use_id":"i8a","content":"The file has been updated."}
XRAY
xray_session demo-i9 100 <<XRAY
user|"Rewrite the cover renderers."
hook|
call|{"type":"tool_use","id":"i9a","name":"Edit","input":{"file_path":"$ws/src/covers.ts","old_string":"$lines24","new_string":"export const covers = []"}}
result|{"type":"tool_result","tool_use_id":"i9a","content":"The file has been updated."}
XRAY
# A destructive command the person's own request asked for, then regretted.
gap=4
xray_session demo-i12 30 <<XRAY
user|"Force push the notes branch to origin, I want it overwritten."
hook|
call|{"type":"tool_use","id":"i12a","name":"Bash","input":{"command":"cd $ws && git push --force origin notes-draft"}}
result|{"type":"tool_result","tool_use_id":"i12a","content":"+ 7c1d0aa...e44b9f2 notes-draft -> notes-draft (forced update)"}
user|"no, why did you overwrite my draft"
XRAY
gap=1
# A collision: two sessions live at once edit one file within 30 minutes.
gap=3
xray_session demo-i10 40 <<XRAY
user|"Rename the upload helper."
hook|
call|{"type":"tool_use","id":"i10a","name":"Edit","input":{"file_path":"$ws/src/upload.ts","old_string":"export function upload(file) {}","new_string":"export function sendUpload(file) { return post(file) }"}}
result|{"type":"tool_result","tool_use_id":"i10a","content":"The file has been updated."}
say|"Renamed."
XRAY
xray_session demo-i11 37 <<XRAY
user|"Add retries to the upload helper."
hook|
call|{"type":"tool_use","id":"i11a","name":"Edit","input":{"file_path":"$ws/src/upload.ts","old_string":"export function upload(file) {}","new_string":"export function uploadWithRetry(file) { return retry(post, file) }"}}
result|{"type":"tool_result","tool_use_id":"i11a","content":"The file has been updated."}
say|"Retries added."
XRAY
gap=1
sed -i '' -e "s|^transcript_roots = .*|transcript_roots = [\"$transcripts\", \"$dir/profiles/.claude/projects\", \"$dir/profiles/.claude-work/projects\", \"$dir/profiles/.claude-typo/projects\"]|" "$home/config.toml"

# Sub-agents, a background command and delegated tasks. Records are
# written by small helpers (a record's time is `$TS <seconds>`: seconds
# after 09:00 on 8 Nov for the fixed day, seconds ago for the live ones).
ts8() { printf '2026-11-08T09:%02d:%02dZ' $(($1 / 60)) $(($1 % 60)); }
agos() { date -u -v-"$1"S +%Y-%m-%dT%H:%M:%SZ; }
TS=ts8 R_S= R_SIDE= R_ENTRY=cli
top() { R_S=$1 R_SIDE= R_ENTRY=${2:-cli}; }
side() { R_S=$1 R_SIDE="\"isSidechain\":true,\"agentId\":\"$2\"," R_ENTRY=cli; }
ru() { printf '{"type":"user",%s"sessionId":"%s","timestamp":"%s","cwd":"@CWD@","entrypoint":"%s","message":{"role":"user","content":%s}}\n' "$R_SIDE" "$R_S" "$($TS "$1")" "$R_ENTRY" "$2"; }
ra() { printf '{"type":"assistant",%s"sessionId":"%s","timestamp":"%s","cwd":"@CWD@","message":{"role":"assistant","content":[%s]}}\n' "$R_SIDE" "$R_S" "$($TS "$1")" "$2"; }
txt() { printf '{"type":"text","text":"%s"}' "$1"; }
tuse() { printf '{"type":"tool_use","id":"%s","name":"%s","input":%s}' "$1" "$2" "$3"; }
rcall() { ra "$1" "$(tuse "$2" "$3" "$4")"; }
rsay() { ra "$1" "$(txt "$2")"; }
rask() { ru "$1" "\"$2\""; }
rres() { ru "$1" "[{\"type\":\"tool_result\",\"tool_use_id\":\"$2\",\"content\":\"$3\"}]"; }
agent_meta() { # session id description model
  mkdir -p "$proj/$1/subagents"
  printf '{"agentType":"general-purpose","description":"%s","model":"%s"}\n' "$3" "$4" \
    >"$proj/$1/subagents/agent-$2.meta.json"
}
sub_file() { printf '%s/%s/subagents/agent-%s.jsonl' "$proj" "$1" "$2"; }
READ_CACHE='1\tuse std::path::Path;\n2\t\n3\tpub struct Cache {\n4\t    mem: MemoryCache,\n5\t}\n6\t\n7\timpl Cache {\n8\t    pub fn evict(&mut self) {\n9\t        if self.size > self.cap {\n10\t            self.evict_oldest();\n11\t        }\n12\t    }\n13\t}'

# The three sub-agents of "Tern: thumbnail cache rewrite", each ending with
# its report; a3 edits cache.rs, which the parent edits too; a1 and a2 edit
# files only they touch.
agent_meta demo-parent a1 "Measure cache hit rate" sonnet
{
  side demo-parent a1
  rask 25 "Measure the thumbnail cache hit rate over the test library."
  rcall 35 s1 Read "{\"file_path\":\"$cwd/src/thumbs/cache.rs\"}"
  rres 37 s1 "$READ_CACHE"
  rcall 60 s2 Grep '{"pattern":"hit|miss","path":"@CWD@/src/thumbs"}'
  rres 62 s2 'src/thumbs/cache.rs:41:    // count a hit\nsrc/thumbs/stats.rs:3:    hits: u64,'
  rcall 100 s3 Edit '{"file_path":"@CWD@/src/thumbs/stats.rs","old_string":"pub struct Stats {\n    hits: u64,\n}\n","new_string":"pub struct Stats {\n    hits: u64,\n    misses: u64,\n    cold: bool,\n}\n\nimpl Stats {\n    pub fn rate(&self) -> f64 {\n        self.hits as f64 / (self.hits + self.misses).max(1) as f64\n    }\n}\n"}'
  rres 102 s3 'The file @CWD@/src/thumbs/stats.rs has been updated successfully.'
  rcall 120 s4 Bash '{"command":"cargo test -p thumbs hit_rate -- --nocapture","description":"Replay the test library twice and print the hit rate"}'
  rres 170 s4 'cold: 61.0%\nwarm: 97.0%\nthumbnails: 12400\n\ntest stats::hit_rate ... ok'
  rsay 185 'Hit rate 61% cold, 97% warm over 12,400 thumbnails.\n\nI added hit and miss counters to the cache and replayed the library twice: a cold run, then a warm one.'
} | fill "$(sub_file demo-parent a1)"

agent_meta demo-parent a2 "Profile cold start" haiku
{
  side demo-parent a2
  rask 26 "Profile a cold start of the gallery and report where the time goes."
  rcall 40 s1 Read '{"file_path":"@CWD@/src/gallery/warm.rs"}'
  rres 42 s1 '1\tpub fn warm(photos: &[Photo]) {\n2\t    for photo in photos {\n3\t        decode(photo);\n4\t    }\n5\t}'
  rcall 70 s2 Grep '{"pattern":"decode","path":"@CWD@/src/gallery"}'
  rres 72 s2 'src/gallery/warm.rs:3:        decode(photo);\nsrc/gallery/grid.rs:18:    decode(photo)'
  rcall 130 s3 Edit '{"file_path":"@CWD@/src/gallery/warm.rs","old_string":"    for photo in photos {\n        decode(photo);\n    }","new_string":"    for photo in photos.iter().filter(|p| p.visible) {\n        decode(photo);\n    }\n    for photo in photos.iter().filter(|p| !p.visible) {\n        queue_decode(photo);\n    }"}'
  rres 132 s3 'The file @CWD@/src/gallery/warm.rs has been updated successfully.'
  rcall 150 s4 Bash '{"command":"cargo run -p gallery --example cold_start","description":"Time a cold start of the gallery"}'
  rres 205 s4 'decode 1480 ms\nlayout 310 ms\ndraw 320 ms\ntotal 2110 ms'
  rsay 215 'Decoding is 70% of cold start; the cache removes it.\n\nThe gallery decoded every photo before drawing the first row. It now decodes the visible ones first and queues the rest.'
} | fill "$(sub_file demo-parent a2)"

agent_meta demo-parent a3 "Review the eviction order" sonnet
{
  side demo-parent a3
  rask 27 "Review DiskLru eviction for off-by-one errors."
  rcall 45 s1 Read "{\"file_path\":\"$cwd/src/thumbs/cache.rs\"}"
  rres 47 s1 "$READ_CACHE"
  rcall 80 s2 Grep '{"pattern":"evict","path":"@CWD@/src"}'
  rres 82 s2 'src/thumbs/cache.rs:8:    pub fn evict(&mut self) {\nsrc/thumbs/lru.rs:3:pub fn evict(sizes: &mut Vec<u64>, cap: u64) {'
  rcall 140 s3 Edit '{"file_path":"@CWD@/src/thumbs/cache.rs","old_string":"        if self.size > self.cap {\n            self.evict_oldest();\n        }","new_string":"        while self.size >= self.cap {\n            self.evict_oldest();\n        }"}'
  rres 142 s3 'The file @CWD@/src/thumbs/cache.rs has been updated successfully.'
  rcall 190 s4 Edit '{"file_path":"@CWD@/tests/cache_cap.rs","old_string":"#[test]\nfn caps_at_limit() {\n    let c = Cache::new();\n    assert!(c.size() <= c.cap());\n}","new_string":"#[test]\nfn caps_at_limit() {\n    let c = Cache::new();\n    assert!(c.size() <= c.cap());\n}\n\n#[test]\nfn evicts_at_the_cap_not_past_it() {\n    let mut c = Cache::with_cap(2);\n    c.put(1);\n    c.put(2);\n    assert_eq!(c.len(), 1);\n}"}'
  rres 192 s4 'The file @CWD@/tests/cache_cap.rs has been updated successfully.'
  rcall 220 s5 Bash '{"command":"cargo test -p thumbs cache_cap","description":"Run the eviction tests"}'
  rres 280 s5 'running 2 tests\ntest caps_at_limit ... ok\ntest evicts_at_the_cap_not_past_it ... ok'
  rsay 285 'Eviction order is correct; the cap check was off by one.\n\nThe cache kept one entry too many at the limit. I changed the check to use the cap as a hard bound and added a test for it.'
} | fill "$(sub_file demo-parent a3)"

# A session live right now with four sub-agents working in the background:
# its newest record launched them half a minute ago, and each sub-agent's
# newest record is a call with no result yet.
TS=agos
# Today's sessions live under the default profile, as real ones do: the
# fleet reads a session's live state only from a profile's transcripts.
proj=$dir/profiles/.claude/projects/-tern-web
mkdir -p "$proj"
{
  top demo-albums
  printf '{"type":"custom-title","customTitle":"Tern: album covers in parallel","sessionId":"demo-albums"}\n'
  rask 700 "Rework the album covers: grid layout, square crops, cache keys and tests. Split it across sub-agents."
  rcall 690 al1 Grep '{"pattern":"album_cover","path":"@CWD@/src"}'
  rres 688 al1 'src/albums/cover.rs:12:pub fn album_cover(id: AlbumId) -> Image {\nsrc/albums/grid.rs:30:    album_cover(a.id)'
  rsay 640 'Four independent pieces: the grid layout, the crops, the cache keys and the tests. I will run them side by side.'
  ra 38 "$(txt 'Launching four sub-agents in the background.'),$(tuse l1 Agent '{"description":"Lay out the cover grid","prompt":"Lay out the album cover grid with equal gaps.","subagent_type":"general-purpose","run_in_background":true}'),$(tuse l2 Agent '{"description":"Crop covers to square","prompt":"Crop album covers to a centred square.","subagent_type":"general-purpose","run_in_background":true}'),$(tuse l3 Agent '{"description":"Key covers by album id","prompt":"Key the cover cache by album id and size.","subagent_type":"general-purpose","run_in_background":true}'),$(tuse l4 Agent '{"description":"Test the cover pipeline","prompt":"Add tests for the cover pipeline.","subagent_type":"general-purpose","run_in_background":true}')"
  rres 37 l1 'Async agent launched successfully. agentId: c1'
  rres 36 l2 'Async agent launched successfully. agentId: c2'
  rres 35 l3 'Async agent launched successfully. agentId: c3'
  rres 34 l4 'Async agent launched successfully. agentId: c4'
} | fill "$proj/demo-albums.jsonl"
live_agent() { # id description model prompt grep-result-secs pending-secs, then the pending call on stdin as `id|name|json`
  agent_meta demo-albums "$1" "$2" "$3"
  IFS='|' read -r pid pname pjson
  {
    side demo-albums "$1"
    rask 33 "$4"
    rcall 30 r1 Read '{"file_path":"@CWD@/src/albums/cover.rs"}'
    rres 28 r1 '1\tpub fn album_cover(id: AlbumId) -> Image {\n2\t    load(id).resize(256, 256)\n3\t}'
    rcall 24 r2 Grep '{"pattern":"resize","path":"@CWD@/src/albums"}'
    rres "$5" r2 'src/albums/cover.rs:2:    load(id).resize(256, 256)'
    rcall "$6" "$pid" "$pname" "$pjson"
  } | fill "$(sub_file demo-albums "$1")"
}
live_agent c1 "Lay out the cover grid" sonnet "Lay out the album cover grid with equal gaps." 21 14 <<'PENDING'
p1|Edit|{"file_path":"@CWD@/src/albums/grid.rs","old_string":"    gap: 4,\n    cols: 3,","new_string":"    gap: 8,\n    cols: 4,"}
PENDING
live_agent c2 "Crop covers to square" haiku "Crop album covers to a centred square." 21 18 <<'PENDING'
p1|Bash|{"command":"cargo test -p albums crop","description":"Run the thumbnail tests"}
PENDING
live_agent c3 "Key covers by album id" sonnet "Key the cover cache by album id and size." 23 21 <<'PENDING'
p1|Grep|{"pattern":"cache_key","path":"@CWD@/src"}
PENDING
live_agent c4 "Test the cover pipeline" haiku "Add tests for the cover pipeline." 20 12 <<'PENDING'
p1|Edit|{"file_path":"@CWD@/tests/covers.rs","old_string":"#[test]\nfn covers_exist() {\n}","new_string":"#[test]\nfn covers_exist() {\n    assert!(album_cover(AlbumId(1)).width() > 0);\n}\n\n#[test]\nfn covers_are_square() {\n    let c = album_cover(AlbumId(1));\n    assert_eq!(c.width(), c.height());\n}"}
PENDING

# T3 Code delegation: a parent (entrypoint t3-code) delegates
# two tasks; the first finished, the second is running now. Each child is a
# top-level session of its own; the synthetic T3 state names which session
# each child thread ran as.
TASK1='Add an export queue lane to Tern: photos exported from an album go through a bounded queue.\nCap the queue at 4 jobs, keep their order, and add tests.\nReport the files you changed.'
TASK2='Add a retry policy to the Tern export queue.\nRetry a failed job twice with a 2 second backoff, then mark it failed.\nAdd tests and report what changed.'
T3_RUN1='{\"taskId\":\"node:delegated-task:demo-1\",\"childThreadId\":\"thread:delegated-task:demo-1\",\"status\":\"running\"}'
T3_RUN2='{\"taskId\":\"node:delegated-task:demo-2\",\"childThreadId\":\"thread:delegated-task:demo-2\",\"status\":\"running\"}'
{
  top demo-t3-parent t3-code
  printf '{"type":"custom-title","customTitle":"Tern: export pipeline","sessionId":"demo-t3-parent"}\n'
  rask 3000 "Make album export reliable: a bounded queue, then retries for failed jobs."
  rsay 2970 'Two lanes that do not touch each other: the queue and the retry policy. I will delegate both.'
  rcall 2950 d1 mcp__t3-code__delegate_task "{\"title\":\"Tern: export queue lane\",\"role\":\"implementation\",\"mode\":\"async\",\"task\":\"$TASK1\"}"
  rres 2949 d1 "$T3_RUN1"
  rcall 2940 d2 mcp__t3-code__delegate_task "{\"title\":\"Tern: export retry policy\",\"role\":\"implementation\",\"mode\":\"async\",\"task\":\"$TASK2\"}"
  rres 2939 d2 "$T3_RUN2"
  rsay 2900 'Both lanes are running. I will wait for their reports.'
  rask 1500 'Delegated task node:delegated-task:demo-1 (Tern: export queue lane) [completed]:\nExport queue lane added: jobs run through a bounded queue of 4, in order.\nChanged src/export/queue.rs and src/export/mod.rs; 9 tests pass.'
} | fill "$proj/demo-t3-parent.jsonl"
{
  top demo-t3-child1 t3-code
  printf '{"type":"custom-title","customTitle":"Tern: export queue lane","sessionId":"demo-t3-child1"}\n'
  rask 2930 "Act as the implementation sub-agent for this task.\n\n$TASK1"
  rcall 2900 q1 Read '{"file_path":"@CWD@/src/export/queue.rs"}'
  rres 2898 q1 '1\tpub struct Queue {\n2\t    jobs: Vec<Job>,\n3\t}'
  rcall 2700 q2 Grep '{"pattern":"Queue::new","path":"@CWD@/src"}'
  rres 2698 q2 'src/export/mod.rs:9:    let q = Queue::new();'
  rcall 2400 q3 Edit '{"file_path":"@CWD@/src/export/queue.rs","old_string":"pub struct Queue {\n    jobs: Vec<Job>,\n}","new_string":"pub struct Queue {\n    jobs: VecDeque<Job>,\n    cap: usize,\n}\n\nimpl Queue {\n    pub fn push(&mut self, job: Job) -> bool {\n        if self.jobs.len() >= self.cap {\n            return false;\n        }\n        self.jobs.push_back(job);\n        true\n    }\n}"}'
  rres 2398 q3 'The file @CWD@/src/export/queue.rs has been updated successfully.'
  rcall 2000 q4 Edit '{"file_path":"@CWD@/src/export/mod.rs","old_string":"    let q = Queue::new();","new_string":"    let q = Queue::with_cap(4);"}'
  rres 1998 q4 'The file @CWD@/src/export/mod.rs has been updated successfully.'
  rcall 1700 q5 Bash '{"command":"cargo test -p export queue","description":"Run the export queue tests"}'
  rres 1560 q5 'running 9 tests\ntest result: ok. 9 passed; 0 failed'
  rsay 1510 'Export queue lane added: jobs run through a bounded queue of 4, in order.\nChanged src/export/queue.rs and src/export/mod.rs; 9 tests pass.'
} | fill "$proj/demo-t3-child1.jsonl"
{
  top demo-t3-child2 t3-code
  printf '{"type":"custom-title","customTitle":"Tern: export retry policy","sessionId":"demo-t3-child2"}\n'
  rask 2920 "Act as the implementation sub-agent for this task.\n\n$TASK2"
  rcall 2880 w1 Read '{"file_path":"@CWD@/src/export/policy.rs"}'
  rres 2878 w1 '1\tpub struct Policy {\n2\t    attempts: u32,\n3\t}'
  rcall 1500 w2 Edit '{"file_path":"@CWD@/src/export/policy.rs","old_string":"pub struct Policy {\n    attempts: u32,\n}","new_string":"pub struct Policy {\n    attempts: u32,\n    backoff_ms: u64,\n}\n\nimpl Policy {\n    pub fn retry() -> Policy {\n        Policy { attempts: 2, backoff_ms: 2000 }\n    }\n}"}'
  rres 1498 w2 'The file @CWD@/src/export/policy.rs has been updated successfully.'
  rcall 25 w3 Edit '{"file_path":"@CWD@/src/export/queue.rs","old_string":"        self.jobs.push_back(job);","new_string":"        self.jobs.push_back(job.with_policy(Policy::retry()));"}'
} | fill "$proj/demo-t3-child2.jsonl"
mkdir -p "$dir/t3"
sqlite3 "$HIPPOCAMP_T3_STATE" "CREATE TABLE orchestration_v2_projection_provider_threads(provider_thread_id TEXT PRIMARY KEY, thread_id TEXT, updated_at TEXT NOT NULL, payload_json TEXT NOT NULL);
INSERT INTO orchestration_v2_projection_provider_threads VALUES
 ('demo-p1', 'thread:delegated-task:demo-1', '$(agos 1500)', '{\"nativeThreadRef\":{\"nativeId\":\"demo-t3-child1\"}}'),
 ('demo-p2', 'thread:delegated-task:demo-2', '$(agos 25)', '{\"nativeThreadRef\":{\"nativeId\":\"demo-t3-child2\"}}');"

# Replay: yesterday as a full day of lanes, at fixed local hours,
# so Fleet's Lanes has a day to show whatever the time now. "Tern: share
# sheet" (work) works, asks a question answered 36 minutes later, runs three
# sub-agents, opens a pull request and merges it after lunch. "Tern: offline
# sync" (default) works through the afternoon, stops at the usage limit at
# 15:31 and resumes at 17:05, with two sub-agents and a plan question. A
# second "Tern: offline sync" on the work account carries it on at 15:45 and
# is corrected at 16:20 for the folder its cache step chose (an incident
# whose step the correction names).
Y0=$(date -j -v-1d -f '%H:%M:%S' '00:00:00' +%s)
tsy() { date -u -r $((Y0 + $1)) +%Y-%m-%dT%H:%M:%SZ; }
at() { echo $(($1 * 3600 + $2 * 60 + ${3:-0})); }
TS=tsy
# work_run <from> <to> <every> <prefix> <file>: calls and their results.
work_run() {
  t=$1 n=0
  while [ "$t" -lt "$2" ]; do
    n=$((n + 1))
    case $((n % 3)) in
      0) rcall "$t" "$4$n" Bash '{"command":"cargo test -p tern","description":"Run the tests"}'
         rres $((t + 40)) "$4$n" 'test result: ok. 31 passed; 0 failed' ;;
      1) rcall "$t" "$4$n" Edit "{\"file_path\":\"@CWD@/src/$5\",\"old_string\":\"let a = $n;\",\"new_string\":\"let a = $((n + 1));\"}"
         rres $((t + 2)) "$4$n" "The file @CWD@/src/$5 has been updated successfully." ;;
      2) rcall "$t" "$4$n" Read "{\"file_path\":\"@CWD@/src/$5\"}"
         rres $((t + 1)) "$4$n" '1\tlet a = 1;' ;;
    esac
    t=$((t + $3))
  done
}
demo_proj=$proj
proj=$dir/profiles/.claude-work/projects/-tern-web
{
  top demo-y-share
  printf '{"type":"custom-title","customTitle":"Tern: share sheet","sessionId":"demo-y-share"}\n'
  rask "$(at 9 10)" "Add a share sheet to the photo view: share the photo or a link to it."
  work_run "$(at 9 11)" "$(at 10 21)" 150 sa share/sheet.rs
  rsay "$(at 10 22)" 'The sheet works for single photos.\n\nShould it send the original file or a resized copy?'
  rask "$(at 10 58)" "Resized, 2048 px on the long side."
  work_run "$(at 10 59)" "$(at 11 5)" 90 sb share/resize.rs
  ra "$(at 11 5)" "$(tuse yt1 Task '{"description":"Resize before sharing","prompt":"Resize shared photos to 2048 px on the long side."}'),$(tuse yt2 Task '{"description":"Share links that expire","prompt":"Make shared links expire after 7 days."}'),$(tuse yt3 Task '{"description":"Test the share sheet","prompt":"Add tests for the share sheet."}')"
  rres "$(at 11 20)" yt1 'Shared photos are resized to 2048 px.\nagentId: y1 (use SendMessage with to: y1 to continue this agent)\n<usage>subagent_tokens: 21000\ntool_uses: 6\nduration_ms: 900000</usage>'
  rres "$(at 11 26)" yt2 'Links expire after 7 days.\nagentId: y2 (use SendMessage with to: y2 to continue this agent)\n<usage>subagent_tokens: 18000\ntool_uses: 5\nduration_ms: 1260000</usage>'
  rres "$(at 11 41)" yt3 'Nine share sheet tests pass.\nagentId: y3 (use SendMessage with to: y3 to continue this agent)\n<usage>subagent_tokens: 25000\ntool_uses: 8\nduration_ms: 2160000</usage>'
  work_run "$(at 11 42)" "$(at 12 3)" 120 sc share/sheet.rs
  rcall "$(at 12 4)" ypr Bash '{"command":"gh pr create --title \"Share sheet\" --body \"Share a photo or a link.\"","description":"Open the pull request"}'
  rres "$(at 12 5)" ypr 'https://github.com/tern-example/web/pull/42'
  rsay "$(at 12 6)" 'Opened the pull request: https://github.com/tern-example/web/pull/42'
  rask "$(at 14 0)" "merge it"
  rcall "$(at 14 1)" ymg Bash '{"command":"gh pr merge https://github.com/tern-example/web/pull/42 --squash","description":"Merge the pull request"}'
  rres "$(at 14 2)" ymg 'Squashed and merged pull request tern-example/web#42 (Share sheet)'
  rsay "$(at 14 3)" 'Merged.'
} | fill "$proj/demo-y-share.jsonl"
for k in 1 2 3; do
  case $k in
    1) d="Resize before sharing" a=$(at 11 5) b=$(at 11 19) f=share/resize.rs ;;
    2) d="Share links that expire" a=$(at 11 6) b=$(at 11 25) f=share/link.rs ;;
    3) d="Test the share sheet" a=$(at 11 6) b=$(at 11 40) f=share/tests.rs ;;
  esac
  agent_meta demo-y-share "y$k" "$d" sonnet
  {
    side demo-y-share "y$k"
    rask "$a" "$d."
    work_run $((a + 30)) "$b" 100 "y${k}s" "$f"
    rsay "$b" "$d: done."
  } | fill "$(sub_file demo-y-share "y$k")"
done
proj=$dir/profiles/.claude/projects/-tern-web
{
  top demo-y-sync
  printf '{"type":"custom-title","customTitle":"Tern: offline sync","sessionId":"demo-y-sync"}\n'
  rask "$(at 13 0)" "Make the photo library work offline and sync when the network is back."
  work_run "$(at 13 1)" "$(at 15 30)" 200 ya sync/queue.rs
  rsay "$(at 15 31)" "You've hit your 5-hour limit · resets 5pm"
  rask "$(at 17 5)" "Continue where you left off."
  ra "$(at 17 6)" "$(tuse yu1 Task '{"description":"Retry failed uploads","prompt":"Retry uploads that failed while offline."}'),$(tuse yu2 Task '{"description":"Measure the sync queue","prompt":"Measure how long the sync queue takes to drain."}')"
  rres "$(at 17 30)" yu1 'Failed uploads retry three times.\nagentId: u1 (use SendMessage with to: u1 to continue this agent)\n<usage>subagent_tokens: 15000\ntool_uses: 4\nduration_ms: 1440000</usage>'
  rres "$(at 17 38)" yu2 'The queue drains in 40 s for 500 photos.\nagentId: u2 (use SendMessage with to: u2 to continue this agent)\n<usage>subagent_tokens: 12000\ntool_uses: 3\nduration_ms: 1920000</usage>'
  work_run "$(at 17 39)" "$(at 17 50)" 120 yb sync/queue.rs
  rcall "$(at 17 50)" yq AskUserQuestion '{"questions":[{"question":"Sync over cellular too, or Wi-Fi only?","header":"Network","options":[{"label":"Wi-Fi only"},{"label":"Both"}]}]}'
  rres "$(at 18 5)" yq 'User has answered your questions: Wi-Fi only.'
  work_run "$(at 18 6)" "$(at 18 30)" 150 yc sync/net.rs
  rsay "$(at 18 31)" 'Offline sync is in: Wi-Fi only, failed uploads retry three times.'
} | fill "$proj/demo-y-sync.jsonl"
for k in 1 2; do
  case $k in
    1) d="Retry failed uploads" a=$(at 17 6) b=$(at 17 29) ;;
    2) d="Measure the sync queue" a=$(at 17 7) b=$(at 17 37) ;;
  esac
  agent_meta demo-y-sync "u$k" "$d" haiku
  {
    side demo-y-sync "u$k"
    rask "$a" "$d."
    work_run $((a + 30)) "$b" 120 "u${k}s" sync/retry.rs
    rsay "$b" "$d: done."
  } | fill "$(sub_file demo-y-sync "u$k")"
done
proj=$dir/profiles/.claude-work/projects/-tern-web
{
  top demo-y-carry
  printf '{"type":"custom-title","customTitle":"Tern: offline sync","sessionId":"demo-y-carry"}\n'
  rask "$(at 15 45)" "Picking up: Tern: offline sync\n\nFirst request: Make the photo library work offline and sync when the network is back."
  work_run "$(at 15 46)" "$(at 16 4)" 180 yd sync/store.rs
  rcall "$(at 16 5)" ymk Bash '{"command":"mkdir -p tmp/sync-cache","description":"Make the sync cache folder in tmp"}'
  rres "$(at 16 5 2)" ymk ''
  work_run "$(at 16 6)" "$(at 16 18)" 150 ye sync/store.rs
  rsay "$(at 16 19)" 'The cache is in place and the store tests pass.'
  rask "$(at 16 20)" "no, that is the wrong folder: the sync cache goes in caches/, not tmp/"
  rcall "$(at 16 21)" ymv Bash '{"command":"mv tmp/sync-cache caches/sync","description":"Move the sync cache to caches"}'
  rres "$(at 16 21 3)" ymv ''
  rsay "$(at 16 40)" 'Moved the sync cache to caches/sync.'
} | fill "$proj/demo-y-carry.jsonl"
proj=$demo_proj
TS=ts8

hippocamp ingest
HIPPOCAMP_STUB_SCENARIO=lessons hippocamp lessons scan

# Open the dreaming window before the 1 November sessions.
printf '{"protocol":"HIPPOCAMP_V1","last_success":"2026-10-31T00:00:00Z"}\n' \
  >"$ws/.hippocamp/dream-state.json"

# Dreaming as real homes see it: a run that fails (the model's reply is
# not JSON), then a run whose stub proposes one change per eval note. The
# reviewer confirms five, rejects two (Auth sessions, Search index) and
# Sharing links' change fails validation. Then one applied change is
# reverted, as a person would.
HIPPOCAMP_STUB_SCENARIO=unparsable hippocamp dream --pass verify >/dev/null 2>&1 || true
HIPPOCAMP_STUB_SCENARIO=demo hippocamp dream --pass verify
proposal=$(grep -l '"notes/Database migrations.md"' "$ws"/.hippocamp/proposals/*.json | head -n 1)
proposal=$(basename "$proposal" .json)
receipt=$(grep -l "\"$proposal\"" "$ws"/.hippocamp/receipts/*.json | head -n 1)
hippocamp revert "$(basename "$receipt" .json)"

echo
echo "seeded $dir"
echo "  HIPPOCAMP_HOME=$home"
echo "  workspace=$ws"
