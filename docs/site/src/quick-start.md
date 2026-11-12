# Quick start

This page takes you from an installed `hippo` to a working setup. The first steps need only your agents' history. You choose a notes folder in step 4.

## 1. Start with your history

```sh
hippo
```

With no setup yet, `hippo` finds the history your agents left on this machine (Claude Code, Codex, DeepSeek Harness, OpenCode and Muse Code), shows what it found, and asks `Index it now? [Y/n]`. Answer yes. Nothing is sent anywhere, and no notes folder is needed. In a script, or with no terminal, use:

```sh
hippo start --yes
```

hippo writes `~/.hippo/config.toml` (`$HIPPO_HOME/config.toml` if you set `HIPPO_HOME`), indexes the transcripts, and shows the last active day: its sessions, merged pull requests and corrections, then `Try next:` with three commands that fit your data. Add another folder of transcripts with `hippo start --add-root ~/.claude-work`. To see what is on the machine before indexing, run `hippo detect`.

hippo reads Claude Code and Codex transcripts, DeepSeek Harness (dsh) session logs, OpenCode's database and Muse Code session logs. `hippo detect` also lists Cursor and Gemini folders, but hippo has no adapter for them and does not read them.

Once hippo is set up, `hippo` on its own updates the index when it is stale and shows the last active day again.

## 2. Look around

```sh
hippo incidents
hippo ask --sources-only "how did we fix the webhook retry"
hippo xray <id>
hippo fleet
hippo resume <id>
```

- `hippo incidents` lists what went wrong in the last 7 days, which agent did it and at which step.
- `hippo ask --sources-only "<question>"` ranks the sessions that match your question. It stays on your machine. Without `--sources-only`, `hippo ask` also asks a model for an answer that cites them.
- `hippo xray <id>` shows what context a session was given and used, and what it searched for. `<id>` is a full session id or a unique prefix, as the other commands print it.
- `hippo fleet` lists today's sessions: live, stopped at a usage limit, or finished.
- `hippo resume <id>` prints the command that resumes a session, with a short handoff.

## 3. Search

```sh
hippo recall "sub-agent"
```

```text
stub-1 claude-code 2026-11-01T10:00:04.000000Z assistant I'll ask a sub-agent where the sender sleeps between retries.
```

Each line is `<session> <tool> <time> <role> [<tool name>] <snippet>`. Add `--json` for one JSON object per line, `--limit N` for more or fewer than 20 results.

By default hippo looks for Claude Code transcripts under `~/.claude*/projects` Codex transcripts under `~/.codex/sessions`, dsh logs under `~/.dsh/sessions`, OpenCode's database under `~/.local/share/opencode` (or `$XDG_DATA_HOME/opencode`), and Muse Code logs under `~/.local/share/muse/sessions` (or `$XDG_DATA_HOME/muse/sessions`). A configuration written before one of them was read keeps its own list: add the folder with `hippo start --add-root ~/.dsh/sessions` (or `$DSH_HOME/sessions` if you set `DSH_HOME`), `hippo start --add-root ~/.local/share/opencode` or `hippo start --add-root ~/.local/share/muse/sessions`. Change `transcript_roots` in `config.toml` to look elsewhere, or run `hippo config add transcript_roots <glob> --yes`. `hippo ingest` brings the index up to date; a run that finds nothing new prints `indexed 0 files, skipped 3, derived 0 sessions`.

## 4. Add a notes folder

Notes, context and dreaming need a workspace: a directory with a notes folder and an entry note. Run this in it:

```sh
cd /home/you/work
hippo init --workspace /home/you/work --notes-dir notes --entry-note "notes/START HERE.md"
```

```text
wrote /home/you/work/.hippo/hippo.toml
wrote /home/you/work/.hippo/.gitignore
wrote /home/you/work/.hippo/packs/core/passes/consolidate.toml
wrote /home/you/work/.hippo/packs/core/passes/verify.toml
wrote /home/you/work/.hippo/packs/core/prompts/consolidate.md
wrote /home/you/work/.hippo/packs/core/prompts/verify.md
wrote /home/you/work/.hippo/packs/core/personas/consolidate-reviewer.md
wrote /home/you/work/.hippo/packs/core/personas/verify-reviewer.md
wrote /home/you/.hippo/config.toml
```

`init` writes two things:

- `~/.hippo/config.toml`, the machine's configuration. It now names the one workspace every command uses. Without `--force`, `init` refuses to overwrite a `config.toml` that already names a workspace; adding a workspace to a history-only setup is allowed.
- `.hippo/` in the workspace: the workspace settings, a `.gitignore` that excludes `snapshots/`, and the [core pack](concepts.md#the-core-pack).

If the notes folder does not exist, `init` creates it. If the entry note does not exist, it writes a starter entry note for you to edit. It never overwrites a file. If you already keep Markdown notes, point `--notes-dir` and `--entry-note` at them.

If the workspace already has a `.hippo/`, for example because you cloned it on a new machine, run `hippo init` with no flags. It writes only `config.toml` and prints `kept <path>` for the existing `.hippo/`.

Sessions you indexed in step 1 have no session records yet. Give them records:

```sh
hippo ingest --full
```

```text
indexed 3 files, skipped 0, derived 3 sessions
```

## 5. See what an agent would see

```sh
hippo context --task "fix the topic links"
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

To give every Claude Code session this context, run `hippo hooks install`. It adds three hooks to the workspace's `.claude/settings.json`, once each, and keeps everything else in the file. `hippo hooks status` checks them. Or add them by hand:

```json
{
  "hooks": {
    "SessionStart": [{ "hooks": [{ "type": "command", "command": "hippo hook session-start" }] }],
    "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "hippo hook prompt" }] }],
    "SessionEnd": [{ "hooks": [{ "type": "command", "command": "hippo hook session-end" }] }]
  }
}
```

- `session-start` records a declared session and prints the required part of the context.
- `prompt` runs on the first prompt of a session only. It records the prompt as the session's task and prints the relevant notes and the linter counts.
- `session-end` ends the session with no further information.

A hook never fails your session. On any error it prints one line to stderr (`hippo hook: <error>`) and exits 0.

For Codex, run `hippo hooks install --agent codex`. It adds the same three hooks, with `--agent codex`, to `$CODEX_HOME/hooks.json` (`~/.codex/hooks.json`), which Codex runs for every session; hippo acts only for sessions in the workspace. The next time Codex starts it lists the new hooks for review. Trust them there (or later with `/hooks`); until then Codex skips them. A Codex session then starts with the same block, gets the relevant notes on its first prompt, and ends with its last answer as the handoff.

## 7. End sessions with a handoff

`session-end` alone records that a session ended, not what it did. Doctor treats a session that ended with no notes and no handoff as `unspecified`, and it reports a problem when more than half of recent sessions are. Ask your agents, in your own instruction files, to finish with:

```sh
hippo session end --session <id> --handoff handoff.md
```

The handoff is Markdown that names the notes the session changed or relied on with `[[Note name]]` links. The `consolidate` dreaming pass reads handoffs. If a session changed no note, say so:

```sh
hippo session end --session <id> --no-change "read-only investigation"
```

hippo cannot make an agent do this. Only your instructions can.

## 8. Check the setup

```sh
hippo doctor
```

`doctor` prints one row per check and exits 1 if any row is a `problem`. A healthy new setup passes. A schedule that is not installed and a dreaming run that has not happened yet are `ok` rows whose detail starts `not set up yet:`. The model check is opt-in: `hippo doctor --model` sends one word through your `model_command`. See [Scheduling](scheduling.md), then [Troubleshooting](troubleshooting.md) for every row.

## 9. Try dreaming

Dreaming runs your `model_command`, so read [Security and privacy](security.md) first. Then:

```sh
hippo dream --dry-run
```

A dry run does everything except write: it ingests, runs the model and the reviewer, and writes only the report, `.hippo/reports/<date>-dream.md`. Read the report. When you are satisfied, run `hippo dream`, or install the schedule so it runs every night.
