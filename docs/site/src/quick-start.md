# Quick start

This page takes you from an installed `hippocamp` to a working setup. The first steps need only your agents' history. You choose a notes folder in step 4.

## 1. Start with your history

```sh
hippocamp
```

With no setup yet, `hippocamp` finds the history your agents left on this machine (Claude Code and Codex), shows what it found, and asks `Index it now? [Y/n]`. Answer yes. Nothing is sent anywhere, and no notes folder is needed. In a script, or with no terminal, use:

```sh
hippocamp start --yes
```

hippocamp writes `~/.hippocamp/config.toml` (`$HIPPOCAMP_HOME/config.toml` if you set `HIPPOCAMP_HOME`), indexes the transcripts, and shows the last active day: its sessions, merged pull requests and corrections, then `Try next:` with three commands that fit your data. Add another folder of transcripts with `hippocamp start --add-root ~/.claude-work`. To see what is on the machine before indexing, run `hippocamp detect`.

hippocamp reads Claude Code and Codex transcripts. `hippocamp detect` also lists OpenCode and Cursor folders, but hippocamp has no adapter for them and does not read them.

Once hippocamp is set up, `hippocamp` on its own updates the index when it is stale and shows the last active day again.

## 2. Look around

```sh
hippocamp incidents
hippocamp ask --sources-only "how did we fix the webhook retry"
hippocamp xray <id>
hippocamp fleet
hippocamp resume <id>
```

- `hippocamp incidents` lists what went wrong in the last 7 days, which agent did it and at which step.
- `hippocamp ask --sources-only "<question>"` ranks the sessions that match your question. It stays on your machine. Without `--sources-only`, `hippocamp ask` also asks a model for an answer that cites them.
- `hippocamp xray <id>` shows what context a session was given and used, and what it searched for. `<id>` is a full session id or a unique prefix, as the other commands print it.
- `hippocamp fleet` lists today's sessions: live, stopped at a usage limit, or finished.
- `hippocamp resume <id>` prints the command that resumes a session, with a short handoff.

## 3. Search

```sh
hippocamp recall "sub-agent"
```

```text
stub-1 claude-code 2026-11-01T10:00:04.000000Z assistant I'll ask a sub-agent where the sender sleeps between retries.
```

Each line is `<session> <tool> <time> <role> [<tool name>] <snippet>`. Add `--json` for one JSON object per line, `--limit N` for more or fewer than 20 results.

By default hippocamp looks for Claude Code transcripts under `~/.claude*/projects` and Codex transcripts under `~/.codex/sessions`. Change `transcript_roots` in `config.toml` to look elsewhere, or run `hippocamp config add transcript_roots <glob> --yes`. `hippocamp ingest` brings the index up to date; a run that finds nothing new prints `indexed 0 files, skipped 3, derived 0 sessions`.

## 4. Add a notes folder

Notes, context and dreaming need a workspace: a directory with a notes folder and an entry note. Run this in it:

```sh
cd /home/you/work
hippocamp init --workspace /home/you/work --notes-dir notes --entry-note "notes/START HERE.md"
```

```text
wrote /home/you/work/.hippocamp/hippocamp.toml
wrote /home/you/work/.hippocamp/.gitignore
wrote /home/you/work/.hippocamp/packs/core/passes/consolidate.toml
wrote /home/you/work/.hippocamp/packs/core/passes/verify.toml
wrote /home/you/work/.hippocamp/packs/core/prompts/consolidate.md
wrote /home/you/work/.hippocamp/packs/core/prompts/verify.md
wrote /home/you/work/.hippocamp/packs/core/personas/consolidate-reviewer.md
wrote /home/you/work/.hippocamp/packs/core/personas/verify-reviewer.md
wrote /home/you/.hippocamp/config.toml
```

`init` writes two things:

- `~/.hippocamp/config.toml`, the machine's configuration. It now names the one workspace every command uses. Without `--force`, `init` refuses to overwrite a `config.toml` that already names a workspace; adding a workspace to a history-only setup is allowed.
- `.hippocamp/` in the workspace: the workspace settings, a `.gitignore` that excludes `snapshots/`, and the [core pack](concepts.md#the-core-pack).

If the notes folder does not exist, `init` creates it. If the entry note does not exist, it writes a starter entry note for you to edit. It never overwrites a file. If you already keep Markdown notes, point `--notes-dir` and `--entry-note` at them.

If the workspace already has a `.hippocamp/`, for example because you cloned it on a new machine, run `hippocamp init` with no flags. It writes only `config.toml` and prints `kept <path>` for the existing `.hippocamp/`.

Sessions you indexed in step 1 have no session records yet. Give them records:

```sh
hippocamp ingest --full
```

```text
indexed 3 files, skipped 0, derived 3 sessions
```

## 5. See what an agent would see

```sh
hippocamp context --task "fix the topic links"
```

```text
# Start here

A synthetic entry note for the parser and linter tests.

relevant:
- Start here: A synthetic entry note for the parser and linter tests.
- Topic A: A synthetic topic that other notes link to.
- Topic B: A synthetic note in a subdirectory. The bare link [[Topic B]] resolves here because only one note has that file name.

linter: overdue=1 dangling=2
```

The block starts with the part of your entry note before its first `## ` heading, then lists up to five notes whose title or headings match the task, then counts overdue notes and dangling links. [The context budget](concepts.md#the-context-budget) explains the limit.

## 6. Add the Claude Code hooks

To give every Claude Code session this context, run `hippocamp hooks install`. It adds three hooks to the workspace's `.claude/settings.json`, once each, and keeps everything else in the file. `hippocamp hooks status` checks them. Or add them by hand:

```json
{
  "hooks": {
    "SessionStart": [{ "hooks": [{ "type": "command", "command": "hippocamp hook session-start" }] }],
    "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "hippocamp hook prompt" }] }],
    "SessionEnd": [{ "hooks": [{ "type": "command", "command": "hippocamp hook session-end" }] }]
  }
}
```

- `session-start` records a declared session and prints the required part of the context.
- `prompt` runs on the first prompt of a session only. It records the prompt as the session's task and prints the relevant notes and the linter counts.
- `session-end` ends the session with no further information.

A hook never fails your session. On any error it prints one line to stderr (`hippocamp hook: <error>`) and exits 0.

## 7. End sessions with a handoff

`session-end` alone records that a session ended, not what it did. Doctor treats a session that ended with no notes and no handoff as `unspecified`, and it reports a problem when more than half of recent sessions are. Ask your agents, in your own instruction files, to finish with:

```sh
hippocamp session end --session <id> --handoff handoff.md
```

The handoff is Markdown that names the notes the session changed or relied on with `[[Note name]]` links. The `consolidate` dreaming pass reads handoffs. If a session changed no note, say so:

```sh
hippocamp session end --session <id> --no-change "read-only investigation"
```

hippocamp cannot make an agent do this. Only your instructions can.

## 8. Check the setup

```sh
hippocamp doctor
```

`doctor` prints one row per check and exits 1 if any row is a `problem`. A healthy new setup passes. A schedule that is not installed and a dreaming run that has not happened yet are `ok` rows whose detail starts `not set up yet:`. The model check is opt-in: `hippocamp doctor --model` sends one word through your `model_command`. See [Scheduling](scheduling.md), then [Troubleshooting](troubleshooting.md) for every row.

## 9. Try dreaming

Dreaming runs your `model_command`, so read [Security and privacy](security.md) first. Then:

```sh
hippocamp dream --dry-run
```

A dry run does everything except write: it ingests, runs the model and the reviewer, and writes only the report, `.hippocamp/reports/<date>-dream.md`. Read the report. When you are satisfied, run `hippocamp dream`, or install the schedule so it runs every night.
