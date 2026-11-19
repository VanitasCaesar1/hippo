# An adapter for Gemini CLI

Status: accepted, to build in 0.4.2.

## The problem

[Gemini CLI](https://github.com/google-gemini/gemini-cli) is Google's open-source coding agent for the terminal. hippo detected `~/.gemini` but did not read it, and gave a Gemini session nothing at its start.

## What changes for a user

- `~/.gemini/tmp` joins the default `transcript_roots`; an existing configuration adds it with `hippo start --add-root ~/.gemini/tmp`.
- Gemini CLI sessions appear wherever the other tools' do, with the tool name `gemini` and ids `gemini-<session id>`; a sub-agent's is `gemini-<parent>-agent-<id>`.
- `hippo hooks install --agent gemini` adds three hooks to `~/.gemini/settings.json`. A Gemini session in the workspace then starts with the start block, gets the relevant notes with its first prompt, and is recorded as a declared session with a derived handoff at its end, as a Codex session is.
- `hippo mcp install --client gemini` adds hippo's MCP tools ([MCP server](mcp-server.md)).

`hippo detect` no longer lists any agent folder as found but unsupported.

## Where this comes from

Gemini CLI's documentation ([session management](https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/session-management.md), [hooks reference](https://github.com/google-gemini/gemini-cli/blob/main/docs/hooks/reference.md), [configuration](https://github.com/google-gemini/gemini-cli/blob/main/docs/reference/configuration.md)) and its source on `main` in November 2026 (`packages/core/src/services/chatRecordingService.ts`, `packages/core/src/config/storage.ts`, `projectRegistry.ts`, `packages/core/src/hooks/types.ts`), with the fixtures written by hand from them. No real Gemini session was read or run: no model and no key were used.

The documentation and the source disagree in two places, and hippo reads both forms:

- **The project folder.** The documentation and older releases name it after the SHA-256 of the project's root path. Current source names it with a short slug of the folder's name (`my-app`, `my-app-1` on a collision), writes the root's path to a `.project_root` file in it, and moves old hashed folders over.
- **The session file.** The documentation and older releases (to 0.38) write one JSON document, `session-<time>-<id8>.json`. Current source appends JSON Lines to `session-<time>-<id8>.jsonl`.

## What Gemini CLI writes

**Where.** `~/.gemini/tmp/<project>/chats/session-<YYYY-MM-DDTHH-MM>-<first 8 of the id>.json[l]`. A sub-agent's session is `chats/<parent session id>/<id>.jsonl`. Beside `chats/` are `logs.json` (the person's prompts only), `checkpoint-<tag>.json` (a conversation saved with `/chat save`, as Gemini API `Content[]`) and `checkpoints/` (file snapshots for `/restore`); hippo reads none of them, because the session file already holds the same conversation.

**The older document:**

```json
{"sessionId": "<uuid>", "projectHash": "<sha256>", "startTime": "2026-11-01T09:00:00.000Z", "lastUpdated": "…",
 "messages": [
   {"id": "m1", "timestamp": "…", "type": "user", "content": "Fix the retry backoff."},
   {"id": "m2", "timestamp": "…", "type": "gemini", "content": "I'll read it.", "thoughts": […], "tokens": {…}, "model": "…",
    "toolCalls": [{"id": "c1", "name": "read_file", "args": {"absolute_path": "/work/src/retry.ts"}, "status": "success", "timestamp": "…",
                   "result": [{"functionResponse": {"id": "c1", "name": "read_file", "response": {"output": "…"}}}], "resultDisplay": "…"}]}]}
```

Message `type` is `user`, `gemini`, `info`, `error` or `warning`; `content` is a string or a list of parts with `text`.

**The JSON Lines file** starts with a metadata record (`sessionId`, `projectHash`, `startTime`, `lastUpdated`, `kind`: `main` or `subagent`) and then appends one record per message in the shape above, the same message again when it changes (a tool call finishing), and update records: `{"$set": {…}}` for metadata (a `messages` array in it is an older full rewrite), `{"$patch": {…}}` for edits to messages, and `{"$rewindTo": "<id>"}` when the person rewinds. The exact layout of `$patch` could not be read verbatim.

## Mapping onto hippo

- **The file.** hippo first tries the whole file as one JSON document; if that is not a session, it reads JSON Lines: metadata records give the id and start; a message record is added, or replaces the earlier message with the same `id`; `$set` updates the metadata and, with `messages`, replaces the messages read so far; other `$` records are skipped. A file with neither an id nor a message is reported as one hippo could not parse.
- **Rewinds and patches.** Skipped: hippo indexes every message the session wrote, as it does Command Code's branches. A rewound message is still something the person and the agent did.
- **The session** is `gemini-<sessionId>` (the file name when there is no id), started at `startTime`, else the first message's time; a sub-agent's file is `gemini-<parent>-agent-<sessionId>`.
- **Turns**, per message in order, stamped with the message's `timestamp` (a tool call with its own `timestamp` keeps it):
  - `user`: a `user` turn of its content;
  - `gemini`: an `assistant` turn of its content, then for each tool call a `tool` turn `<name> <args as compact JSON>` and a `tool` turn of its result (the `functionResponse.response.output`, or `error`, or the response as JSON; `resultDisplay` when there is none), both named by the call;
  - `thoughts`, token counts and `info`, `error` and `warning` messages: never indexed.
- **The directory** is the project folder's `.project_root`. An older hashed folder has none; its sessions are indexed and belong to no workspace (hashing every candidate directory to match would be guesswork).
- **Tool names** are stored as Gemini CLI writes them (`read_file`, `run_shell_command`, …).
- **Redaction** as for every adapter, before anything reaches the index.
- **Claude Code's adapter** leaves every file under a `.gemini` folder to this one.

## Context at the start

Gemini CLI has hooks with the shape Claude Code's and Codex's have, in `settings.json`, enabled by default (`hooksConfig.enabled`). `SessionStart` (with `source` `startup`, `resume` or `clear`) and `BeforeAgent` (with the `prompt`) may return `{"hookSpecificOutput": {"hookEventName": …, "additionalContext": "…"}}`: at the start the text becomes the first turn of the history, before a prompt it is added to that prompt. `SessionEnd` runs at exit. Every input carries `session_id`, `transcript_path`, `cwd`, `hook_event_name` and `timestamp`.

So hippo treats Gemini CLI as it treats Codex. `hippo hooks install --agent gemini` adds, once each and keeping every other setting (`mcpServers` among them) in order:

```json
{ "hooks": {
  "SessionStart": [{ "hooks": [{ "name": "hippo-session-start", "type": "command", "command": "hippo hook session-start --agent gemini" }] }],
  "BeforeAgent":  [{ "hooks": [{ "name": "hippo-prompt", "type": "command", "command": "hippo hook prompt --agent gemini" }] }],
  "SessionEnd":   [{ "hooks": [{ "name": "hippo-session-end", "type": "command", "command": "hippo hook session-end --agent gemini" }] }]
} }
```

`hippo hook <event> --agent gemini` does what `--agent codex` does: nothing unless hippo has a workspace and the input's `cwd` is in it; at the start, the required parts and a declared record `gemini-<session_id>`; on the first prompt, the relevant notes and the task; at the end, the last answer of the session file at `transcript_path` as a derived handoff. The output's `hookEventName` is `BeforeAgent` for the prompt. A settings file with comments (Gemini CLI strips them; a JSON parser does not) is refused and left as it is, with a message.

The declared id assumes the hook's `session_id` is the session file's `sessionId`; both come from the same session id in Gemini CLI's source. If a release ever differs, the declared record and the session's derived record would be two records, not a wrong one.

No `GEMINI.md` line is written: the hooks give per-task context, which a static file could not, and the MCP server covers the rest.

## What is not supported

- `logs.json`, `/chat save` checkpoints and `/restore` snapshots (the session file holds the conversation).
- Rewinds: rewound messages stay in the index.
- The project folder of an older, hashed layout: no directory, so no workspace.
- Fleet live state: Gemini sessions show from their indexed turns.

## Fixtures and tests

`fixtures/gemini/.gemini/tmp/work-example/`, written by hand from the source and documentation: a `.project_root`; a JSON Lines session with the synthetic marker, metadata, a prompt with every line of `redaction/leaks.txt`, a reply written twice as its two tool calls (one failing) finish, a `$set`, an `info` message, a `$patch` and a closing answer; its sub-agent; an older JSON session; and a `logs.json` and checkpoint that are never read. Tests: both formats into turns, replacement of a re-written message, `$set` with messages, a file that is not a session, which files are claimed (and that Claude Code's adapter leaves them), the directory from `.project_root`, the redaction test, an ingest with the directory moved to a temporary workspace, the hooks' install beside other settings, and the three hooks end to end in a scratch `HOME`.

## Alternatives

- **A `GEMINI.md` line pointing at `hippo brief`.** Needs the model to follow an instruction and run a tool; the hooks need neither.
- **Following rewinds.** Possible once the `$rewindTo` semantics are confirmed against a real file; the cost of reading everything is a few extra turns.
- **Reading `logs.json`** for sessions an older release did not record. It holds only prompts, with no answers or tool calls.
