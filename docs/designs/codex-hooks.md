# Codex hooks

Status: accepted, to build in 0.4.0.

## Background

hippo indexes what coding agents did on a machine and keeps a folder of plain-Markdown notes that agents start from. The folder belongs to one *workspace*, set up with `hippo init`. The *start block* is the part of those notes every new session should see (the entry note and any required notes); the *relevant notes* are the notes whose title or headings match the session's first prompt. `hippo hook session-start` and `hippo hook prompt` print them under the workspace's byte budget (`budget_bytes`, 25 600 bytes by default), and record a *declared* session (`.hippo/sessions/<id>.json`).

A Claude Code session gets both through three hooks that `hippo hooks install` adds to the workspace's `.claude/settings.json`. hippo has read Codex's sessions since 0.1.0 ([the adapter](../../crates/hippo-core/src/adapters/codex.rs)), but gives a Codex session nothing back: the README says Codex gets the notes "through `hippo brief --task "<task>"` when your instructions ask for it, for now".

## The problem

A Codex session starts without the start block, and its agent sees hippo's notes only if the person's own instruction files tell it to run `hippo brief`. That depends on the model following an instruction, costs a tool call, and gives no per-task relevance unless the agent passes the task.

## What Codex offers

All of this was read from Codex's source at tag `rust-v0.167.2` (commit `ce21072`, November 2026), the version checked here, and confirmed by running it (see [Tests](#tests)).

- **Hooks are on by default.** The `hooks` feature (`codex_hooks` before) is stable and enabled by default (`codex-rs/features/src/lib.rs`).
- **Where hooks are declared.** Each configuration layer may declare hooks in a `hooks.json` next to its `config.toml`, or in a `[hooks]` table in that `config.toml`. For the user layer that is `$CODEX_HOME/hooks.json` (`~/.codex/hooks.json` unless `CODEX_HOME` is set). A project layer is a `.codex/` folder between the session's directory and its project root (the nearest `.git` by default), and is read only for a project the person trusts (`codex-rs/hooks/src/engine/discovery.rs`, `codex-rs/config/src/state.rs`).
- **The file** is `{ "description"?: string, "hooks": { "<Event>": [ { "matcher"?: string, "hooks": [ { "type": "command", "command": string, "timeout"?: seconds, "additionalContextLimit"?: tokens, … } ] } ] } }`, the same shape as Claude Code's `hooks` setting. Unknown top-level keys make Codex skip the whole file with a warning (`HooksFile` denies them).
- **Events.** `SessionStart` runs before the first model request of a thread, with `source` one of `startup`, `resume`, `clear`, `compact` or `fork`; it runs again after a compaction. A sub-agent gets `SubagentStart` instead, and `SessionEnd` is not run for one. `UserPromptSubmit` runs for every prompt and carries `prompt`. `SessionEnd` runs when the thread shuts down, after Codex has flushed the transcript. Every input carries `session_id` (the thread id), `cwd`, `transcript_path` and `hook_event_name`.
- **Output.** A command hook that exits 0 may print plain text, which becomes model context, or JSON. A start or prompt hook's JSON may carry `hookSpecificOutput.additionalContext`. Plain text that starts with `{` or `[` is taken for JSON and, when it does not parse, fails the hook. Codex records the context as a `developer` message in the thread, so it stays in the transcript and in the model's view until a compaction.
- **Large output.** Context over `additionalContextLimit` tokens (2 500 by default) is written to a file and the model gets a preview and the file's path instead; `0` turns this off.
- **Timeouts.** A hook may run 600 seconds by default. `SessionEnd` defaults to 1 second and is held to at most 3.
- **Trust.** A hook from a user or project file runs only after the person has trusted it. Codex keeps a hash of each trusted hook under `[hooks.state."<file>:<event>:<group>:<handler>"]` in the user `config.toml`; a hook that is new or whose declaration changed is listed at Codex's next start, where the person reviews it, trusts all, or goes on without it (`codex-rs/tui/src/startup_hooks_review.rs`). `/hooks` reviews them later. `codex exec` has no review screen; it runs only trusted hooks unless given `--dangerously-bypass-hook-trust`.

So Codex has a start-of-session mechanism that matches Claude Code's, including the prompt, which a static instruction file could not give.

## What changes for a user

```text
hippo hooks install --agent codex
hippo hooks uninstall --agent codex
hippo hooks status --agent codex
```

`install --agent codex` adds three hooks to `$CODEX_HOME/hooks.json`, once each, keeping everything else in the file. The next time Codex starts it lists them for review; the person trusts them there (or with `/hooks`). From then on:

- A top-level Codex session whose directory is inside hippo's workspace starts with the start block, and its first prompt brings the relevant notes, as in Claude Code. hippo records it as a declared session with the id the adapter gives the same session, `codex-<session_id>`, so the session's record, x-ray and the indexed transcript line up.
- A session outside the workspace, a sub-agent, or a machine where hippo has no notes folder gets nothing, and hippo writes nothing for it.
- When the session ends, hippo records the end, with the last assistant message as a derived handoff, as ingest would have.

The `--agent` flag takes `claude-code` (the default, what the commands do today) or `codex`. `--workspace` stays a Claude Code option; Codex's hooks are the person's, for every directory, and hippo decides per session whether it is in the workspace.

## The hooks

`install --agent codex` adds, unless the event already has a hook whose command runs the same `hippo hook` subcommand with `--agent codex`:

```json
{
  "hooks": {
    "SessionStart": [{ "hooks": [{ "type": "command", "command": "hippo hook session-start --agent codex", "additionalContextLimit": 0 }] }],
    "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "hippo hook prompt --agent codex", "additionalContextLimit": 0 }] }],
    "SessionEnd": [{ "hooks": [{ "type": "command", "command": "hippo hook session-end --agent codex", "timeout": 3 }] }]
  }
}
```

`additionalContextLimit: 0` keeps the block in the model's context instead of a file it would have to open: hippo already holds the block under `budget_bytes`. The `SessionEnd` timeout is the most Codex allows.

The file is read into an order-keeping tree, the entries are appended, and it is written back with two-space indentation, atomically, as for Claude Code's settings. A file that is not a JSON object, or whose `hooks` or an event in it has the wrong type, is refused and left alone. `uninstall` removes only hippo's three hooks, then any group left with no hooks and any event left with no groups; it writes nothing when none was there, and leaves an emptied file as `{ "hooks": {} }`. `status` says which of the three are in the file. Neither command reads or writes Codex's `config.toml`, so neither touches trust: Codex's own review is the person's consent, and a hook hippo changes later is listed for review again.

If the person's `config.toml` already declares hooks in a `[hooks]` table, Codex loads both and warns. `install` says so and still writes `hooks.json`; it does not move the person's hooks.

## `hippo hook … --agent codex`

Each of the three reads Codex's hook JSON on stdin. Like the Claude Code hooks, each exits 0 whatever happens, and prints an error on one line to stderr.

1. **The session.** The hook does nothing, prints nothing and writes nothing unless `hippo config` names a workspace with a notes folder and the input's `cwd`, with symbolic links resolved, is that workspace or inside it. `UserPromptSubmit` with an `agent_id` (a sub-agent's prompt) is passed over too. The session id is `codex-<session_id>`.
2. **`session-start`** starts the declared session with `tool: codex` (or reopens it, for `resume` and `compact`), compiles the required parts, records their size and the text, and prints `{"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": "<the block>"}}`. JSON rather than plain text, because a block that began with `[` would otherwise fail the hook.
3. **`prompt`** does what the Claude Code hook does on a session's first prompt (relevant notes and linter counts within what is left of the budget, the prompt recorded as the task), and prints them in the same JSON with `"hookEventName": "UserPromptSubmit"`. A session hippo did not start (outside the workspace) and later prompts get nothing.
4. **`session-end`** ends the declared session. When `transcript_path` is readable, the Codex adapter parses it and the last assistant message becomes the record's handoff with `handoff_derived: true`, so the record is no poorer than the one ingest would have derived; otherwise it ends like the Claude Code hook (`unspecified`). A missing record is not an error here.

The record format does not change: `tool: codex` and `handoff_derived` already exist, and a declared record already stops ingest from deriving one for the same id.

The adapter already skips `developer` messages, so the block Codex records in the transcript is never read back as something the person typed.

## What it cannot do

- **Without the person's trust, nothing runs.** Until they trust the hooks in Codex's review, Codex skips them; `codex exec` alone never shows that review.
- **Codex sessions that do not go through the hooks**, such as one started before the hooks were trusted, are still read by ingest, as now.
- **Windows.** Codex runs hooks there through `commandWindows` when set; hippo does not set it and has not tried Windows.
- **A project-level install** (`<repo>/.codex/hooks.json`) is not offered: Codex reads it only up to the repository root and only for trusted projects, and a hippo workspace usually holds several repositories.

## Tests

- **Unit tests** in `hippo_core::hooks`: install into a missing, empty and populated `hooks.json`, once each, keeping other keys and hooks in order; refusal of a non-object file and a wrong-typed `hooks`; uninstall removing only hippo's three and the emptied groups and events; the Claude Code paths unchanged.
- **CLI tests** run `hippo hook … --agent codex` with synthetic Codex inputs in a scratch `HOME` and `HIPPO_HOME`: the JSON output and the record for a session in the workspace; nothing outside it, for a sub-agent's prompt, or with no notes folder; a later prompt; the derived handoff from a synthetic transcript, and the plain end without one.
- **End to end**, by hand before a release that changes this, in `scripts/codex-e2e/`: the real `codex` binary with scratch `HOME`, `CODEX_HOME`, `HIPPO_HOME` and `XDG_*` folders, `hippo hooks install --agent codex` run there, and a small local server that answers like the Responses API with canned replies, so no model and no key are used. `codex exec --dangerously-bypass-hook-trust` stands in for the person's trust. The check reads the requests the server received and confirms that the start block and the relevant notes were in the first request, that the declared record has the session's id, task and handoff, and that `hippo ingest` then indexes the same session from the scratch `CODEX_HOME`.

## Alternatives

- **A hippo-managed block in an instruction file** (`AGENTS.md`, or a file named in `project_doc_fallback_filenames` or `model_instructions_file`). Static: one text for every task, refreshed only when hippo rewrites it, and written into files the person or their repositories own. Rejected now that Codex has hooks.
- **Write the trust hash for the person.** It would make the hooks run at once, but it bypasses the review Codex shows for exactly this, with a hash format hippo would have to copy. Rejected.
- **An MCP server.** It gives the model tools, not context at the start; the model would still have to decide to call it. Possible later for `hippo ask`, separately.
- **Hooks in `config.toml`.** Same effect, but hippo would have to edit TOML the person writes by hand and keep its comments. `hooks.json` is a file hippo can merge with the same code as Claude Code's settings.
- **A Codex plugin.** Codex plugins can carry hooks, but a plugin is installed from a marketplace, and its hooks still need trust. Not worth a package for three hooks.
