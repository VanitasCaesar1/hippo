# An adapter for DeepSeek Harness (`dsh`)

Status: accepted, to build in 0.2.0.

## The problem

hippo reads what Claude Code and Codex did. [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) (`dsh`, MIT, first released in August 2026) is another coding agent, with sub-agents, plan mode, approvals and a web UI. People who run it next to Claude Code or Codex get only part of their day in hippo: dsh sessions are missing from the overview, Fleet, Incidents, the x-ray, `recall`, `ask`, `brief`, lessons and dreaming.

dsh keeps every session as an append-only log of events, the same log it rebuilds the model's context from. That log is on disk, so hippo can read it the way it reads the other two tools' transcripts.

dsh is a developer preview and says it will make breaking changes. The adapter has to read what dsh writes today, and has to fail clearly, not quietly mis-read, when dsh writes something else.

## What changes for a user

- `hippo` with no arguments counts dsh history among what it found, before setup.
- `~/.dsh/sessions` joins the default `transcript_roots`. A configuration that already lists its roots does not change: `hippo start --add-root ~/.dsh/sessions` adds the folder. Someone who set `DSH_HOME` adds `$DSH_HOME/sessions` the same way.
- dsh sessions appear everywhere Claude Code and Codex sessions do, with the tool name `dsh`. Sub-agents appear under the session that started them.
- `hippo doctor` gets a `dsh logs` row. It says how many dsh logs are in a format version hippo does not read, which versions, and what to do.

Nothing else changes. No hook, no start block and no setting is written for dsh.

## What dsh writes

All of this was read from dsh's source at commit `e8f7b93` (November 2026), and checked against the session logs dsh records for its own snapshot tests (`snapshots/**/session.v4.jsonl` in its repository).

**Where.** dsh's shipped JSONL backend (`@deepseek-ai/dsh-session-persistence-jsonl`) keeps sessions under a configured root. The base bundle sets it to `$DSH_HOME/sessions`, and `DSH_HOME` defaults to `~/.dsh`. A profile can set another root in its `cordis.patch.yml`; hippo cannot see that, so such a root is added with `--add-root`.

```text
<root>/
  --<project path, separators as ->--/   (or _no-cwd/)
    <session id, escaped>/
      session.v4.jsonl.zstd   the current format, Zstandard-compressed (default)
      session.v4.jsonl        the same, with compression: none
      session.v3.jsonl.zstd   an older generation, kept after an upgrade
      session.lock            held by the process writing the session
```

A session directory can hold several generations. dsh reads the one with the highest version; so does hippo. By default each file is a run of independent Zstandard frames: one for the header line, then one per batch of events. With `compression: 'none'` it is plain JSON Lines.

**Format.** The first line is the header:

```json
{"type":"session","version":4,"id":"…","createdAt":1792123129533,"cwd":"/abs/path",
 "parentSession":"…","isSeeded":false,"origin":"subagent","delegationDepth":1}
```

`createdAt` is Unix milliseconds. `parentSession` and `origin: "subagent"` mark a sub-agent; a fork made by the person has neither. Every later line is one event, `{"type", "seq", "time", "data"}`, with `time` in Unix milliseconds. The events hippo reads:

| Event | What it holds |
|---|---|
| `user/message` | A message on the user side. `data.source.kind` is `user` for what the person typed; other kinds are text dsh adds (`runtime-context`, `skill-catalog`, `subagent-settled`, …). |
| `assistant/message` | One model reply: `data.message.content[]` of `text`, `reasoning` and `tool-call` blocks, plus the raw stream and token `usage`. |
| `tool/call` | `data.name`, `data.callId`, and `data.arguments`, the JSON string the model wrote. |
| `tool/result` | `data.message.content[]` (text and images), `data.message.isError`, and the `callId` it answers. |
| `request/header` | The provider, model and reasoning effort of the next request. |
| `session/title` | The session's title. The last one wins. |
| `approval/asked`, `approval/decided` | A tool call waiting for the person's approval, and the answer. |
| `subagent/descriptor` | In a sub-agent's own log: its label, mode and model. |
| `session/end-seed` | In a fork: `{"inherited": true}` marks where the copied history ends. |
| `turn/start`, `turn/end` | The boundaries of a turn, and why it ended. |

Everything else (system and developer messages, streams, attempts, compaction, jobs, todo lists, team messages, `plugin:` records) is skipped. A sub-agent is its own session directory, beside its parent's, whose header names its parent.

## Mapping onto hippo

**Which files.** The dsh adapter claims a file named `session.jsonl` or `session.v<N>.jsonl`, with or without `.zstd`, whose session directory sits in a project directory named `--…--` or `_no-cwd`, and only when no sibling has a higher version. The Claude Code adapter, which claims every other `*.jsonl`, stops claiming these. A Claude Code transcript is named after its session id, so it never matches.

**Reading.** A `.zstd` file is decompressed frame by frame, as a stream. A torn last frame, which a crash can leave, is dropped without an error; dsh repairs it the next time it writes the session, and hippo reads the file again then. An incomplete last line in a plain file is skipped, and so is anything before the header line. A line that is not JSON is skipped with a warning, as for the other tools.

**The session.** Its id is `dsh-<id>`, for a sub-agent `dsh-<top>-agent-<id>`, where `<top>` is the session at the top of its `parentSession` chain. That is the same `<parent>-agent-<child>` shape a Claude Code sub-agent has, so the index, the rollup and the views put dsh sub-agents under their session without new code. When a parent's log is not under the same root, the id uses the nearest parent it names. An id with characters outside letters, digits, `_` and `-` is replaced by a hash of it. The session starts at `createdAt`, and its working directory is the header's `cwd`.

**Turns.** Each turn is stamped with its event's `time`.

- `user/message` from `source.kind: "user"`: one `user` turn per text block. Text dsh adds is not the person's words and is not indexed.
- `assistant/message`: one `assistant` turn per text block. Reasoning is never indexed. Its `tool-call` blocks are not turns, because each call is also a `tool/call` event.
- `tool/call`: one `tool` turn, `<name> <arguments>` with the arguments as written.
- `tool/result`: one `tool` turn named by its call, whose text is the result's text blocks. Images are left out.
- In a fork, the events before the last `session/end-seed` with `inherited: true` are the parent's history and are skipped.

**Tool names.** hippo's readers (the x-ray's claims, incidents, the rollup's edits and agents, Fleet's activity line, dreaming's observed reads and edits) recognise a tool by name. dsh's core tools take the same arguments as Claude Code's (`file_path`, `command`, `old_string`, `new_string`, `content`, `pattern`, `description`, `prompt`), so the adapter stores them under Claude Code's names:

| dsh | stored as |
|---|---|
| `bash` | `Bash` |
| `read`, `write`, `edit` | `Read`, `Write`, `Edit` |
| `glob`, `grep` | `Glob`, `Grep` |
| `web_fetch` | `WebFetch` |
| `todo_write` | `TodoWrite` |
| `ask_user_question`, `exit_plan_mode` | `AskUserQuestion`, `ExitPlanMode` |
| `subagent`, `subagent_fork` | `Agent` |

Every other tool keeps its dsh name. `apply_patch` takes the same patch text as Codex's and is read the same way. The cost: `recall` shows `Bash` where dsh said `bash`. Teaching every reader a second spelling would touch a dozen places for no gain in what they understand.

**Failed results.** hippo's readers of failures (lessons, incidents, the x-ray) know a failed result by how it starts: `Exit code N` for a shell command, `<tool_use_error>` for a refused call. dsh says the same things differently, so the adapter writes them the way those readers know:

- a `bash` result whose last line is `[exit code: N]` is stored as `Exit code N`, a newline, then the output without that line;
- a result with `isError: true` is stored inside `<tool_use_error>` … `</tool_use_error>`.

Nothing else in a result's text changes.

**Redaction.** Turn text goes through the same `redact()` as every other adapter before it reaches the index: keys, tokens, emails and the other patterns become `[redacted:<kind>]`. A title or sub-agent label read from the log is redacted before it is shown. The adapter ships with the redaction test every adapter has: the fixture plants each leak in `fixtures/redaction/leaks.txt` and the test checks none survives.

**Live and finished.** Fleet reads the newest events of today's dsh logs.

- Live: the newest event is at most a few minutes old, as for the other tools.
- Running: a `tool/call` with no `tool/result` yet.
- Waiting on the person: an `approval/asked` with no `approval/decided`, or an `ask_user_question` or `exit_plan_mode` call with no result.
- Finished, interrupted or open: hippo's usual reading of the last turns, as for the other tools.

hippo never takes dsh's `session.lock`. Testing a lock means holding it for a moment, which could stop dsh from opening its own session.

**Model and effort.** The session tab shows the newest `request/header`'s `config.model` and `config.reasoningEffort`.

## Version compatibility

dsh numbers its log format in the header's `version` and in the file name. Format 4 is current; format 3 was released before it and differs only in fields hippo does not read (a tool result names its call in `message.source.callId` instead of `toolCallId`). hippo reads 3 and 4.

Any other version is refused, never guessed at. The file is recorded as a parse failure with the message `dsh log format v<N> is not one hippo reads (it reads v3 and v4)`, and is read again when it changes. `hippo doctor`'s `dsh logs` row counts them and names the versions:

- an older version (0 to 2): resuming the session in a current dsh upgrades its log;
- a newer version: a newer hippo is needed.

Within a supported version, an event type hippo does not know is skipped, and an event whose fields are not the expected shape is skipped with a warning. hippo indexes messages and calls; it does not rebuild the model's context, so an event it skips cannot change what the turns it keeps mean.

## What is not supported

- **Log formats before 3.** Formats 0 to 2 pack assistant replies differently. They are refused, as above.
- **Other storage.** Only the JSONL backend's files are read, not other persistence plugins or the web UI's server.
- **Hooks and the start block.** dsh has no hook hippo installs, so dsh sessions are always derived records. A dsh session can still run `hippo brief` if its instructions ask it to.
- **How a turn ended.** `turn/end`'s reason (`aborted`, `error`, `max-tokens`) is not read; the ending comes from the last turns, as for the other tools.
- **Usage and limits.** Token counts and provider rate limits are not read into the usage view.
- **Team mode, workflows, jobs, schedules and `plugin:` records.** Skipped. A team member's messages to another are not turns.
- **Tail reads of compressed logs.** A `.zstd` log is read whole, for ingest and for Fleet. A log that is not changing is not read again.

## Fixtures and tests

Fixtures are made up and small, and each begins with `{"synthetic": true}`, as every `*.jsonl` fixture does. They follow the shapes in dsh's snapshot logs exactly: a normal session (plain and compressed), a sub-agent and a fork, a live session that stops mid-call and at an approval, a tool error, and a header with an unknown version. Tests: adapter unit tests, the redaction test, an ingest round trip, and the first-run check for `hippo` alone. The eval set does not change.

## Alternatives

- **Run dsh's own reader.** dsh's backend is TypeScript; calling it would make hippo depend on Node and on dsh being installed. Rejected.
- **Read only `compression: 'none'` logs.** Simpler, but the default is compressed, so most people would see nothing. Rejected. hippo decodes Zstandard with `ruzstd`, a pure-Rust decoder under MIT.
- **Keep dsh's tool names and teach each reader both spellings.** More honest in `recall`, but about a dozen readers would need a second name, and every new reader would need to remember. Rejected for now; revisit if a dsh tool's arguments drift from Claude Code's.
- **Ask dsh for a stable export.** dsh's own README says the format will change; a reader that checks the version and refuses what it does not know is what can be built now.
