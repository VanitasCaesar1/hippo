# An adapter for Muse Code

Status: accepted, to build in 0.4.0.

## The problem

hippo reads what Claude Code, Codex, DeepSeek Harness and OpenCode did. [Muse Code](https://dev.meta.ai/docs/muse-code/) is Meta's coding agent CLI. People who run it next to the others get only part of their day in hippo: Muse sessions are missing from the overview, Fleet, Incidents, the x-ray, `recall`, `ask`, `brief`, lessons and dreaming.

Muse keeps each session as an append-only JSON Lines log on disk, so hippo can read it like the others. Muse is not open source and its public documentation does not describe the log. The adapter therefore reads only what was seen in real logs, checks the version fields Muse writes on every record, and says plainly what it has not seen.

## What changes for a user

- `hippo` with no arguments counts Muse history among what it found, before setup.
- `~/.local/share/muse/sessions` (or `$XDG_DATA_HOME/muse/sessions`) joins the default `transcript_roots`. A configuration that already lists its roots does not change: `hippo start --add-root ~/.local/share/muse/sessions` adds the folder.
- Muse sessions appear everywhere the other tools' sessions do, with the tool name `muse` and ids `muse-<session id>`.

Nothing else changes. No hook or start block is written for Muse.

## Where this comes from

Muse Code 1.4.18 (build `1.4.18-R8587.1`, November 2026) is a single closed-source binary. Three primary sources, nothing else:

1. The help text and the `read-session` skill embedded in that binary, which state where sessions live and which records hold what.
2. Real sessions written by that binary in a scratch `HOME` and `XDG_*` folders, against a local server that answers like the OpenAI Responses API with canned replies (`muse exec --provider meta --base-url http://127.0.0.1:<port>/v1 …`): one with a tool call and its result, and plain ones. No model and no key were used.
3. `muse export --session <path>`, run offline on those sessions, which reads the same log and agreed with it.

The [documentation](https://dev.meta.ai/docs/muse-code/) mentions `session_retention_days`, `--no-session-log` and `muse exec --json`, but not the log's location or shape.

## What Muse writes

**Where.** `${XDG_DATA_HOME:-~/.local/share}/muse/sessions/<YYYY>/<MM>/<DD>/<session id>/session.jsonl`, the date being the session's local start date and the id a UUIDv7. Beside it a session folder holds `subagent/<child id>/session.jsonl` (sub-agents), `tool-outputs/` (large tool outputs), a lock file and a CLI log. `sessions/.msp-view-*/` holds binary caches. A session run with `--no-session-log` writes nothing. The runs wrote no `.muse/` folder in the working directory.

**A line** is a record envelope:

```json
{"schema_version": 1, "id": "<uuid>", "stream": {"kind": "session", "id": "<session id>"},
 "sequence": 25, "recorded_at": 1794506812431122, "record_type": "event",
 "payload_type": "runtime.session", "payload_schema_version": 1, "payload": { … }}
```

`recorded_at` is microseconds since the Unix epoch, UTC. A line may instead be a *retained frame*, `{"retained_frame": "<name>", "frame_schema_version": 1, "children": [{"child_index": 0, "record_json": "<an envelope, as a JSON string>"}, …], …}`, which wraps several envelopes written together; in every log seen it was the first line.

**Records hippo reads** (every other payload type is skipped):

| Payload | Meaning |
|---|---|
| `runtime.session.metadata`, `payload.record.workspace_root` | the session's working directory |
| `runtime.session`, `payload.kind` `run`, `payload.event.kind` `started`, `event.prompt` | the person's prompt that starts a run (a turn) |
| … `event.kind` `assistant_message_committed`, `event.text` | an assistant message |
| … `event.kind` `assistant_tool_calls_committed`, `event.tool_calls[]` `{call_id, name, args}` | tool calls; `args` is the arguments as a JSON string |
| … `event.kind` `tool_result_batch_committed`, `event.results[]` `{tool_call_id, text}` | tool results, in the order of the calls |

Records hippo skips include the session's permission profile, the system prompt Muse sends (`model_request_configured`, which alone makes a log about 200 KB), context blocks, tasks and their status, token usage, resource samples and `session.end`.

## Mapping onto hippo

- **The session** is `muse-<stream.id>`, from the first envelope; its start is that envelope's `recorded_at`. A file with no envelope naming a session is an error, as for the other adapters.
- **Turns**, each stamped with its envelope's `recorded_at`:
  - `started`: a `user` turn whose text is `prompt`;
  - `assistant_message_committed`: an `assistant` turn;
  - `assistant_tool_calls_committed`: one `tool` turn per call, named by `name`, whose text is `<name> <args>`;
  - `tool_result_batch_committed`: one `tool` turn per result, named by the call with the same `call_id` (unnamed when none), whose text is `text`.
- **Tool names** are stored as Muse writes them (`read_file`, `write_file`, `edit_file`, `apply_patch`, …), not mapped to Claude Code's.
- **The working directory** (for workspace membership, the x-ray and dreaming's observed paths) is `workspace_root`.
- **The handoff** of a derived record is the last assistant message, as for Codex, dsh and OpenCode.
- **Redaction** as for every adapter: before anything is written to the index.

## Version compatibility

Every envelope carries `schema_version` and `payload_schema_version`, and a frame `frame_schema_version`. hippo reads `1` for each. A record with another version is skipped, and ingest logs a warning naming the file and the versions it skipped. A log in which no record could be read is reported as a file it could not parse. Muse may change its log in any release; the version fields are what make that visible instead of a quiet misread.

## What is not supported

- **Sub-agents.** `subagent/<child>/session.jsonl` was not seen in a real log, so it is not read.
- **Prompts typed while a run is going** (Muse queues them as inbox items): not seen in a real log, not read.
- **Reasoning**, never indexed, as for every tool.
- **Large tool outputs** that Muse moves to `tool-outputs/`: hippo indexes the text the log holds, not the file.
- **Fleet live state**: Muse sessions show in Fleet from their indexed turns, without a running or waiting state.
- **`muse export`** is not used: hippo would have to run the Muse binary during an ingest.

## Fixtures and tests

A synthetic fixture under `fixtures/transcripts/muse/` in the shape above, written by hand from the real logs with all text replaced: a frame line, metadata, a prompt, a tool call and its result, an assistant message, skipped records, a record with an unknown version. Tests: the parse into turns; skipped records; a record in an unknown version skipped; a log without a session; the redaction test every adapter has; an ingest of the fixture through the default roots in a scratch `HOME`.

## Alternatives

- **Run `muse export`** and read its JSON. A versioned document, but it runs a closed binary during every ingest, and the export carries the same records. Rejected.
- **Wait for a published schema.** There is none yet. Reading the version fields and failing visibly is the safer middle.
- **Map tool names to Claude Code's**, as for dsh and OpenCode. Worth doing once Muse's tool set is seen in real use; with one tool seen, a guessed mapping would be wrong in ways hard to notice.
