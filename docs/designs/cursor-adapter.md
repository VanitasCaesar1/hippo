# An adapter for Cursor

Status: accepted, to build in 0.4.2.

## The problem

[Cursor](https://cursor.com) is the most used AI editor, and its agent runs in the editor and as the `cursor-agent` CLI. hippo read neither: `hippo detect` listed `~/.cursor` as found but not supported, so a person who uses Cursor next to Claude Code or Codex got only part of their day in hippo, and Cursor's agent started every chat without the notes.

## What changes for a user

- `~/.cursor/projects` joins the default `transcript_roots`. A configuration that lists its own roots does not change: `hippo start --add-root ~/.cursor/projects` adds it.
- Cursor sessions appear wherever the other tools' sessions do, with the tool name `cursor` and ids `cursor-<id>`; a sub-agent is `cursor-<id>-agent-<sub>`.
- `hippo hooks install --agent cursor` adds one hook to `~/.cursor/hooks.json`, after which a new Cursor chat in the workspace starts with the start block. `hippo mcp install --client cursor` ([MCP server](mcp-server.md)) gives the agent the relevant notes for its task and the rest of hippo's tools.

## Where this comes from

Cursor documents its hooks ([cursor.com/docs/hooks](https://cursor.com/docs/hooks)), rules ([cursor.com/docs/context/rules](https://cursor.com/docs/context/rules)) and MCP configuration, but not its transcripts. The transcript layout below comes from public descriptions (a [Cursor forum thread](https://forum.cursor.com/t/richer-agent-transcripts-lifecycle-data-for-observability-langfuse-stop-hooks/166592) in which Cursor's staff describe the files, and open-source tools that read them), and was confirmed against the structure of a real `~/.cursor` in November 2026: folder and file names, JSON keys and the values of enumerations only. No content from it is in this repository; the fixtures are written by hand.

Cursor keeps the editor's chat history in a different place, a SQLite key-value store (`state.vscdb`), and the `cursor-agent` CLI keeps its own chat store (`~/.cursor/chats/<hash>/<id>/store.db`, content-addressed blobs, partly protobuf). Both are undocumented and change between versions. The agent transcripts are plain JSON Lines that both the editor's agent and the CLI write, so hippo reads those and only those.

## What Cursor writes

**Where.** `~/.cursor/projects/<slug>/agent-transcripts/<id>/<id>.jsonl`, one folder per agent conversation, with sub-agents in `<id>/subagents/<sub>.jsonl`. An older layout put `<id>.jsonl` directly in `agent-transcripts/`. `<slug>` is the project's absolute path with the leading `/` dropped and every other run of characters that are not letters or digits written as `-` (`/work/my.app` is `work-my-app`); a window with no folder gets a number. Beside `agent-transcripts/` are `agent-tools/` (tool output Cursor kept aside), `mcps/`, `terminals/` and others; hippo reads none of them.

**Every line** is a message or a marker:

```json
{"role": "user", "message": {"content": [{"type": "text", "text": "<timestamp>Sunday, Nov 1, 2026, 9:00 AM (UTC-7:00)</timestamp>\n<user_query>\nFix the retry backoff.\n</user_query>"}]}}
{"role": "assistant", "message": {"content": [{"type": "text", "text": "I'll read it."}, {"type": "tool_use", "name": "Read", "input": {"path": "/work/src/retry.ts"}}]}}
{"type": "turn_ended", "status": "success"}
```

- Content blocks are `text` and `tool_use` (`name`, `input`). There are no tool results, no ids, no token counts and no per-line timestamps.
- The person's message wraps what they typed in `<user_query>` and adds the time as `<timestamp>`, in the editor's locale and with its UTC offset, and sometimes other context Cursor attached (`<image_files>`, `<manually_attached_skills>`).
- `turn_ended` marks the end of an agent turn with `success`, `error` (with an `error` string) or `aborted`.

## Mapping onto hippo

- **The session** is `cursor-<id>`, from the file's name; a sub-agent is `cursor-<parent id>-agent-<sub id>`, the shape Claude Code's sub-agents have, so `hippo fleet`, `xray` and `show` treat it as a sub-agent.
- **Turns**, one per block in file order: `text` in a `user` message a `user` turn, holding only what is inside `<user_query>` when there is one (else the text without its `<timestamp>`); `text` in an `assistant` message an `assistant` turn; `tool_use` a `tool` turn named by `name`, whose text is `<name> <input as compact JSON>`. `turn_ended` and other lines are skipped.
- **Times.** Each turn takes the time of the latest person's message before it. Turns before the first message with a time take that first time; a file with no time at all (sub-agents, whose prompt comes from the agent) takes its modification time. The `<timestamp>` format is `<weekday>, <Mon> <day>, <year>, <h>:<mm> AM|PM (UTC±H:MM)`; a time that does not parse is ignored. So a session's turns are as precise as its prompts, to the minute.
- **The directory** the session ran in is the slug read back against the file system: from `/`, follow each directory whose name, written as a slug, matches the next parts of the slug, trying longer names first, and backtrack. A project that was moved or deleted resolves to nothing; its sessions are indexed but belong to no workspace, as a Claude Code session whose project folder is gone.
- **Tool names** are stored as Cursor writes them (`Read`, `Grep`, `StrReplace`, `Shell`, `Task`), not mapped.
- **The handoff** of a derived record is the last assistant message, as for Codex and the others.
- **Redaction** as for every adapter, before anything reaches the index.
- **Claude Code's adapter** takes every other `*.jsonl` under a root; it now leaves anything under an `agent-transcripts` folder to this one.

## Context at the start of a chat

Cursor offers three ways in: rules files, hooks and MCP.

- **Rules** (`.cursor/rules/*.mdc` with `alwaysApply: true`, or `AGENTS.md`) are static files in the project. hippo would have to rewrite them whenever a note changes, and they cannot carry the notes relevant to a task. User rules live in Cursor's settings, not in a file.
- **Hooks.** `sessionStart` runs when a conversation is created and may return `{"additional_context": "…"}`, which Cursor adds to the conversation's initial system context. Its input carries `workspace_roots`. No prompt hook can add context: `beforeSubmitPrompt` can only allow or block.
- **MCP.** The agent can call `hippo_context` with its task.

hippo uses the hook for the start block and MCP for the rest. `hippo hooks install --agent cursor` adds, once, to `~/.cursor/hooks.json` (creating it with `"version": 1` when missing, keeping every other hook and key in order):

```json
{ "version": 1, "hooks": { "sessionStart": [ { "command": "hippo hook session-start --agent cursor" } ] } }
```

`hippo hook session-start --agent cursor` reads the hook's JSON and prints `{"additional_context": "<the required parts>"}` when one of `workspace_roots` is inside hippo's workspace, and nothing otherwise (no notes folder, another project, a window with no folder). It is read-only: it writes no session record, because the hook's ids are not documented to match the transcript's, and ingest derives the record from the transcript anyway. `uninstall` and `status` work as for Codex.

Cursor documents `sessionStart` as fire-and-forget: the agent does not wait for it. The hook only compiles notes it already has on disk, which takes milliseconds, but a first reply could in principle start before the context lands. The MCP tool covers that case.

## What is not supported

- **The editor's chat store and the CLI's `store.db`.** Undocumented, versioned and partly binary; the agent transcripts hold the same conversations in a readable form.
- **Tool results.** Cursor does not write them to the transcript (some are in `agent-tools/`, unlinked to the call). Incidents that need a tool's output, such as a failed check, are not found in Cursor sessions; incidents from the person's words are.
- **Per-turn times** finer than the prompt's minute.
- **`turn_ended` errors** are not turned into incidents yet.
- **Fleet live state**: Cursor sessions show from their indexed turns, without a running or waiting state.

## Fixtures and tests

`fixtures/cursor/projects/work-example/` in the shape above, written by hand: a session with two prompts at UTC−7, text and tool calls, `turn_ended` markers and every line of `redaction/leaks.txt`; its sub-agent; an older flat transcript; and an `agent-tools` sidecar that is never read. Tests: the parse into turns and times; the sub-agent and flat layouts; which files are claimed (and that Claude Code's adapter leaves them); the `<timestamp>` format; the slug read back against a temporary tree and from `/`; the redaction test every adapter has; an ingest whose project folder is named after a temporary workspace, with the session's directory read back to it; the hook's install, status and uninstall in Cursor's format; and `hippo hook session-start --agent cursor` in and out of the workspace, in a scratch `HOME`.

## Alternatives

- **A rules file** written by `hippo hooks install`. Simpler and synchronous, but stale as soon as a note changes, and per project. The hook reads the notes at each start.
- **Recording a declared session from the hook.** It would give Cursor sessions a task and a start-block size, as Claude Code's have, but needs the hook's `conversation_id` to equal the transcript's id, which Cursor does not document. Worth adding once it is confirmed.
- **Reading `state.vscdb`.** It holds chats from before agent transcripts existed, but its schema is internal and its size can reach gigabytes.
