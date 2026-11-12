# An adapter for OpenCode

Status: accepted, to build in 0.3.0.

## The problem

hippo reads what Claude Code, Codex and DeepSeek Harness (dsh) did. [OpenCode](https://github.com/anomalyco/opencode) (MIT) is another coding agent, with sub-agents, forks and a terminal and desktop UI. Until now `hippo detect` listed its folder as "detected, no adapter", and OpenCode sessions were missing from the overview, Fleet, Incidents, the x-ray, `recall`, `ask`, `brief`, lessons and dreaming.

The other three tools write one file per session. OpenCode 2 keeps every session of every project in one SQLite database, which OpenCode itself may be writing to while hippo reads it, and which on a busy machine is many gigabytes. So this adapter has two jobs the others do not: read a database that belongs to a running program without disturbing it, and read only what changed since the last ingest without loading the database.

## What changes for a user

- `hippo` with no arguments and `hippo detect` count OpenCode sessions among what they found. OpenCode leaves the "detected, not read" list; Cursor and Gemini stay on it.
- OpenCode's data folder joins the default `transcript_roots`: `$XDG_DATA_HOME/opencode` when `XDG_DATA_HOME` is set, else `~/.local/share/opencode` (OpenCode uses that path on macOS too). A configuration that already lists its roots does not change: `hippo start --add-root ~/.local/share/opencode` adds it. A database moved with `OPENCODE_DB` is added the same way, by its folder.
- OpenCode sessions appear everywhere the other tools' sessions do, with the tool name `opencode`. Sub-agents appear under the session that started them.
- `hippo doctor` gets an `OpenCode database` row: whether the database is in a schema hippo reads, and what to do when it is not.
- The session record format gains one value: `tool` may be `opencode`. Nothing else in a record changes.

No hook and no setting is written for OpenCode. The [OpenCode plugin](opencode-plugin.md) gives OpenCode sessions the start block and two tools; it is separate and optional.

## What OpenCode writes

All of this was read from OpenCode's source at tag `v2.2.7` (commit `fedc6f2`, November 2026), and checked against the schema (not the content) of a real database written by that version.

**Where.** `<data>/opencode.db`, where `<data>` is `$XDG_DATA_HOME/opencode` or `~/.local/share/opencode`. A build on another release channel writes `opencode-<channel>.db` beside it. `OPENCODE_DB` replaces the file name, or the whole path. OpenCode opens the database in WAL mode, so a live database has `opencode.db-wal` and `opencode.db-shm` beside it. The same folder holds `opencode-next.db` (a preview database that OpenCode 2 imports once), logs, git snapshots of projects (`snapshot/`) and cloned repositories (`repos/`).

**Tables hippo reads.**

| Table | What it holds |
|---|---|
| `session_v2` | One row per session: `id` (`ses_…`), `parent_id` (the session a sub-agent was started from), `fork_session_id` and `fork_boundary` (for a fork), `directory` (the working directory), `title`, `time_created`, `time_updated`, `time_busy` and `time_idle` (the start and end of the newest run), in Unix milliseconds. |
| `session_message` | One row per message: `session_id`, `type`, `seq` (its place in the session), `time_created`, `time_updated`, and `data`, the message as JSON without its id and type. |
| `session_part` | Assistant content as separate rows, written only when OpenCode's `experimental.part_rows` is on. Usually empty. |
| `event_sequence` | Per session, the sequence number of its newest change. |
| `migration` | One row per schema migration OpenCode has applied, by id (`20261030054400_job`). |

**Messages.** The `type` column says what a message is:

| `type` | `data` |
|---|---|
| `user` | `text`, what the person typed, and attached files. |
| `assistant` | One model step: `content[]` of `text`, `reasoning` and `tool` items, `model` (`id`, `providerID`, `variant`), token counts and `time`. |
| `synthetic`, `system`, `skill` | Text OpenCode adds for the model; not the person's words. |
| `shell` | A command the person ran with `!`. |
| `compaction` | A summary that replaced older messages. |
| `idle` | The end of a run, with its `outcome`. |
| `model-switched`, `agent-switched`, `location-switched` | A change of model, agent or folder. |

A `tool` item is `{id, name, state, time}`. `state.status` is `streaming` or `running` while the call runs, then `completed` with `content[]` (text and files) or `error` with `error.message`. A tool's result lives in the same item as its call: there is no separate result message.

**Sub-agents.** OpenCode's `subagent` tool creates a new session whose `parent_id` is the caller, and records the child's id in the call's `state.metadata.sessionID`.

**Forks.** A fork has no `parent_id`. Its `fork_session_id` names the session it was forked from, and OpenCode copies that session's messages up to `fork_boundary` into the fork's own rows, keeping their `seq`.

**Live state.** A session is running when `time_busy` is later than `time_idle`. A question or a permission request waiting on the person is held in OpenCode's memory, not in the database; the database shows only a tool call that has not finished.

**Older formats.** OpenCode 1 kept sessions in tables named `session`, `message` and `part` in the same file, and before that in JSON files under `<data>/storage/`. OpenCode 2 copies the version 1 tables into `session_v2` and `session_message` when it first starts, renaming tools as it goes, and never writes them again. It has no reader for the JSON files.

## Reading without getting in the way

hippo opens the database read-only (`SQLITE_OPEN_READ_ONLY`, a `file:` URI with `mode=ro`), with a busy timeout of two seconds, and never writes to it: no schema change, no `VACUUM`, no checkpoint, no `PRAGMA` that writes. It does not use `immutable=1`, which is only safe for a file nobody is writing.

A WAL database is read through its `-shm` file, as every reader of it is. When OpenCode is running, hippo's reader takes the same shared locks OpenCode's own readers take. While a read transaction is open, OpenCode can keep writing, but a checkpoint cannot move past the snapshot hippo is reading, so the `-wal` file can grow until hippo is done. hippo keeps each read short: one query lists the sessions, then each changed session is read by its own statement, so no transaction spans the ingest. When the `-shm` and `-wal` files are missing (OpenCode is not running and shut down cleanly), SQLite creates an empty `-wal` and a `-shm` beside the database as it opens it, as it does for any reader of a WAL database, read-only or not; the database file itself is not changed. OpenCode uses and resets both files when it next starts. In a folder hippo cannot write to, such a database cannot be opened, and the ingest records that as a parse failure.

If OpenCode holds a lock longer than the busy timeout, that ingest skips the database with a warning and the next ingest tries again.

## Reading only what changed

The ingest treats each session in the database as a transcript of its own, at the path `<database>#<session id>`. For each one, the listing query gives a stamp: the session's `event_sequence` number, its `time_updated` and its count of messages. OpenCode raises the sequence number on every change it commits to a session, so a session whose stamp is the one the index holds is not read. The stamp takes the place of the hash the index keeps for a transcript file.

A session whose stamp changed is read whole, by its own `SELECT … WHERE session_id = ? ORDER BY seq`, one row at a time, and replaces what the index held for it, as a changed transcript file does. Memory holds one session's turns at a time, never the database.

The listing query reads the session table, the sequence table and the message index, not message content: on a 17.6 GB database with 3,066 sessions it takes under half a second. A first index reads every session once. On a synthetic 536 MB database of 2,000 sessions and 80,000 messages, on a laptop, the first index took 20 seconds with at most 35 MB of memory, an ingest with nothing changed 0.07 seconds, and one with ten changed sessions 0.26 seconds.

## Mapping onto hippo

**Which files.** The adapter claims `opencode.db` and `opencode-<channel>.db` (not `opencode-next.db`, whose sessions OpenCode imports into `opencode.db`). The ingest does not walk the folders below a folder named `opencode` that holds one of these: snapshots and cloned repositories are not transcripts, and the Claude Code adapter would otherwise claim any `*.jsonl` file in them.

**The session.** Its id is `opencode-<id>`, for a sub-agent `opencode-<top>-agent-<id>`, where `<top>` is the session at the top of its `parent_id` chain. That is the shape the other tools' sub-agents have, so the index, the rollup and the views put OpenCode sub-agents under their session without new code. A fork is a session of its own (`opencode-<id>`), as in dsh. An id with characters outside letters, digits, `_` and `-` is replaced by a hash of it. The session starts at `time_created`, and its working directory is `directory`.

**Turns.** Messages are read in `seq` order. Each turn is stamped with its own time: a message's `time.created`, a tool call's `time.created`, a tool result's `time.completed`, else the message's.

- `user`: one `user` turn of `text`. Attached files are not read.
- `assistant`: in content order, one `assistant` turn per non-empty `text` item; for each `tool` item, one `tool` turn for the call, `<name> <input as JSON>`, and, once it has finished, one `tool` turn for its result. Reasoning is never indexed. When `session_part` rows exist for the message, they come first in `ordinal` order, then any inline content, as OpenCode reads them.
- A tool result's text is its `content[]` text items, joined by newlines; files are left out. A result with `status: error` is its `error.message`, then any content text.
- `synthetic`, `system`, `skill`, `shell`, `compaction`, `idle` and the `…-switched` messages are not turns.
- In a fork, the messages copied from the session it was forked from (those up to its boundary, by `seq`) are skipped. When that session is gone, the copied messages are known by being older than the fork.

**Tool names.** hippo's readers (the x-ray's claims, incidents, the rollup's edits and agents, Fleet's activity line, dreaming's observed reads and edits) recognise a tool by its name and its arguments by Claude Code's names. OpenCode's core tools do the same things under other names, and name some arguments in camel case, so the adapter stores them as Claude Code would have written them:

| OpenCode | stored as | arguments renamed |
|---|---|---|
| `shell` (`bash` before 2.0) | `Bash` | |
| `read` | `Read` | `path` or `filePath` → `file_path` |
| `write` | `Write` | `path` or `filePath` → `file_path` |
| `edit` | `Edit` | `path` or `filePath` → `file_path`, `oldString` → `old_string`, `newString` → `new_string`, `replaceAll` → `replace_all` |
| `patch` (`apply_patch` before 2.0) | `apply_patch` | `patchText` → `input` |
| `glob`, `grep` | `Glob`, `Grep` | |
| `webfetch`, `websearch` | `WebFetch`, `WebSearch` | |
| `todowrite` | `TodoWrite` | |
| `question` | `AskUserQuestion` | `multiple` → `multiSelect` in each question |
| `skill` | `Skill` | `id` → `skill` |
| `subagent` (`task` before 2.0) | `Agent` | `agent` → `subagent_type` |

Every other tool, MCP tools included, keeps OpenCode's name and arguments. OpenCode's patch format is the one Codex's `apply_patch` uses, so hippo reads it the same way. The cost, as for dsh: `recall` shows `Bash` where OpenCode said `shell`.

**Failed results.** hippo's readers of failures know a failed shell command by a result that starts `Exit code N`, and a refused call by `<tool_use_error>`. OpenCode records a non-zero exit as a completed call whose last text item is `Exit code: N` (with a signal name after it when there is one). The adapter writes such a result `Exit code N`, a newline, then the output without that line. A call with `status: error` is stored inside `<tool_use_error>` … `</tool_use_error>`. Nothing else in a result changes.

**Redaction.** Turn text goes through the same `redact()` as every other adapter before it reaches the index. A title read from the database is redacted before it is shown. The adapter ships with the redaction test every adapter has: a synthetic database plants each line of `fixtures/redaction/leaks.txt` in a user message, an assistant text, a tool input and a tool result, and the test checks none survives.

**Live and finished.** Fleet lists the OpenCode sessions changed today, by `time_updated`, `time_busy` and `time_idle`, and reads each one's newest messages.

- Live: the newest message is at most a few minutes old, as for the other tools.
- Running: a tool call that has not finished, or `time_busy` later than `time_idle`.
- Waiting on the person: a `question` call (stored as `AskUserQuestion`) that has not finished. A permission prompt is not in the database, so a call waiting for permission shows as running.
- Finished, interrupted or open: hippo's usual reading of the last turns, as for the other tools.

**Model.** The session tab shows the newest assistant message's `model.id`, and its `model.variant` as the effort.

## Version compatibility

OpenCode changes its schema through numbered migrations, several a month, and records each one it applied in the `migration` table. Most do not touch the tables hippo reads. So hippo uses the migration table to place the database, and checks the tables themselves before it reads:

- **Older than OpenCode 2.** No `session_v2` table, or a newest migration before `20260804233008_loose_psylocke` (the one that named it). Not read. Running a current OpenCode once upgrades the database.
- **Known.** A newest migration hippo was checked against (up to `20261030054400_job`, OpenCode 2.2.7). Read.
- **Newer.** A newest migration hippo does not know. Read when every table and column hippo reads is there with the type it expects. `hippo doctor` then says the database is newer than this hippo was checked against.
- **Changed.** Any table or column hippo reads is missing. Not read, whatever its version.

A database that is not read is recorded as a parse failure with a message naming its newest migration, and `hippo doctor`'s `OpenCode database` row repeats it with what to do. It is checked again at every ingest.

Within a database that is read, a message `type` hippo does not know is skipped, and a message whose `data` is not the shape expected is skipped with a warning that names the session and the message's `seq`, never its content.

## What is not supported

- **OpenCode 1 storage.** The `session`, `message` and `part` tables and the JSON files under `storage/` are not read. OpenCode 2 copies the tables into the ones hippo reads; a session OpenCode 2 has not copied yet appears once it has. This loses a little detail OpenCode's own copy drops (tool titles, step markers).
- **`opencode-next.db`** and databases on another machine (OpenCode's remote workspaces).
- **Attachments.** Files a person attached and files a tool returned are not indexed.
- **Messages that are not the person's or the model's.** Text OpenCode adds (`synthetic`, `system`, `skill`), commands the person ran with `!`, and compaction summaries are not turns.
- **Permission prompts**, which OpenCode keeps in memory. Fleet cannot tell a call waiting for permission from one that is running.
- **How a run ended.** `idle`'s `outcome` is not read; the ending comes from the last turns, as for the other tools.
- **Usage.** Token counts and cost are not read into the usage view.
- **Hooks.** OpenCode sessions are always derived records. The [plugin](opencode-plugin.md) gives them the start block but does not record them.
- **Sessions deleted in OpenCode** stay in hippo's index, as a transcript file deleted from disk does.

## Fixtures and tests

Fixtures are made up and small. They are SQL, not database files: `fixtures/opencode/schema.sql` creates the tables as OpenCode 2.2.7 does, and each scenario is a `.sql` file of `INSERT`s that a test helper loads into a fresh database in a temporary folder. Each begins with a `-- synthetic` comment and inserts a `synthetic` row into OpenCode's `kv` table, the SQL counterpart of the `{"synthetic": true}` line the other fixtures start with. The scenarios: a normal session with every leak, a failed command and a refused edit; a sub-agent, and a sub-agent of that; a fork; a live session that stops mid-call and at a question; and a database whose newest migration is unknown and whose `session_message` lacks a column.

Tests: adapter unit tests, the redaction test, an ingest round trip, an incremental re-ingest that reads only the session that changed, the doctor row, and the first-run check for `hippo` alone. The eval set does not change.

## Alternatives

- **Copy the database, then read the copy.** Safe from locks, but copying many gigabytes at every ingest is slower than reading what changed, and the copy needs disk the machine may not have. Rejected.
- **Open with `immutable=1`.** No locks and no `-shm`, but SQLite then assumes nothing changes the file, and OpenCode does. Rejected.
- **Read OpenCode's event log.** OpenCode 2 records events in an `event` table only in its cloud server; a local install leaves it empty. Rejected.
- **Ask a running OpenCode over its HTTP API.** Works only while OpenCode runs, and needs its server and port. Rejected; the database is there whether OpenCode runs or not.
- **One index row for the whole database.** Fits the one-file-one-session model as it is, but every change to any session would read them all. Rejected for a path per session.
- **Refuse any migration hippo does not know.** Strictest, but OpenCode adds migrations every few days, most of them to tables hippo never reads, so hippo would stop reading OpenCode after nearly every OpenCode update. Rejected for a check of the tables hippo reads, with the version shown in `hippo doctor`.
- **Keep OpenCode's tool names.** As for dsh: more honest in `recall`, but every reader would need a second spelling and camel-case arguments. Rejected.
