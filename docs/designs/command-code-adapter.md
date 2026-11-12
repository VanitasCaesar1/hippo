# An adapter for Command Code

Status: accepted, to build in 0.4.0. Reading `taste.md` into the Inbox is considered here and not built.

## The problem

hippo reads what Claude Code, Codex, DeepSeek Harness and OpenCode did. [Command Code](https://commandcode.ai/docs) is a coding agent CLI (`npm i -g command-code`) that learns a person's preferences ("taste") from their sessions. People who use it next to the others get only part of their day in hippo: its sessions are missing from the overview, Fleet, Incidents, the x-ray, `recall`, `ask`, `brief`, lessons and dreaming.

Command Code keeps each session as an append-only JSON Lines file, so hippo can read it like the others.

## What changes for a user

- `hippo` with no arguments counts Command Code history among what it found, before setup.
- `~/.commandcode/projects` joins the default `transcript_roots`. A configuration that already lists its roots does not change: `hippo start --add-root ~/.commandcode/projects` adds the folder.
- Command Code sessions appear everywhere the other tools' sessions do, with the tool name `command-code` and ids `command-code-<session id>`.

Nothing else changes. No hook or start block is written for Command Code.

## Where this comes from

The `command-code` package, version 1.88.5 (November 2026), installed into a scratch prefix with install scripts off. Its `dist/cli.mjs` is bundled but not minified, so the code that writes sessions can be read. The format below was then checked against a real session that package wrote in a scratch `HOME`, using a provider in `~/.commandcode/providers.json` that points at a local server answering like the OpenAI chat completions API with canned replies, `--local-only`, and a placeholder key: no model, no key and nothing sent to Command Code's service. The old public repository, `CommandCodeAI/cmd-old-public`, is no longer available and held no code.

## What Command Code writes

**Where.** `~/.commandcode/projects/<slug>/<session id>.jsonl`, where `<slug>` is the working directory lowercased with every run of other characters as `-`. The slug cannot be turned back into a path, so the directory comes from the file itself. No setting moves the folder. Beside each session are `<id>.meta.json`, `<id>.checkpoints.jsonl`, `<id>.prompts.jsonl`, `<id>.share.json` and `<id>.scheduled-tasks.json`, and after a migration `<id>.v2.bak`; hippo reads none of them. Nothing is written until the first assistant reply.

**The first line** is the header:

```json
{"type": "session", "version": 3, "id": "<uuid>", "timestamp": "2026-11-12T18:04:29.986Z", "cwd": "/abs/work"}
```

with an optional `parentSession` when the session continues another. Command Code rewrites a version 1 or 2 file into version 3 when it opens one, and refuses a newer version.

**Every other line** is an entry, `{"type", "id", "parentId", "timestamp", …}`, with an 8-character `id`. The entries form a tree through `parentId`: going back to an earlier point in a session starts a new branch from there, in the same file. Entry types are `message`, `model_change`, `effort_change`, `compaction`, `branch_summary`, `custom`, `custom_message`, `label` and `session_info`. A `message` holds one message in Anthropic's shape:

```json
{"type": "message", "id": "c662b03e", "parentId": "9d466d55", "timestamp": "2026-11-12T18:04:30.063Z",
 "message": {"role": "assistant", "content": [
   {"type": "text", "text": "Let me read it."},
   {"type": "tool_use", "id": "call_1", "name": "read_file", "input": {"file_path": "/abs/work/notes.txt"}}],
  "meta": {"source": "model", "createdAt": 1794506670057, "messageId": "<uuid>"}},
 "usage": {"inputTokens": 5, "outputTokens": 7, "cacheReadTokens": 0, "cacheWriteTokens": 0}, "model": "<provider>/<model>"}
```

A tool result is a `user` message with `meta.source` `tool` and content blocks `{"type": "tool_result", "tool_use_id", "content": [{"type": "text", "text"}]}`. The person's own messages have `meta.source` `user`.

## Mapping onto hippo

- **The session** is `command-code-<header id>`; its start is the header's `timestamp`, its directory the header's `cwd`. A file whose first line is not a version 3 header is not read: ingest reports it as a file it could not parse, with the reason.
- **Turns**, one per content block of each `message` entry, stamped with the entry's `timestamp`, in file order:
  - `text` in a `user` message: a `user` turn; in an `assistant` message: an `assistant` turn;
  - `tool_use`: a `tool` turn named by `name`, whose text is `<name> <input as compact JSON>`;
  - `tool_result`: a `tool` turn named by the `tool_use` with the same id (unnamed when none), whose text is its text blocks joined;
  - `thinking` and other block types: never indexed.
- Every other entry type is skipped.
- **Tool names** are stored as Command Code writes them (`read_file`, …), not mapped to Claude Code's.
- **The handoff** of a derived record is the last assistant message, as for Codex, dsh and OpenCode.
- **Redaction** as for every adapter: before anything is written to the index.

## What is not supported

- **Branches.** hippo reads every message in file order, so a branch the person went back from is indexed along with the one they kept. Following the tree to the newest leaf is possible later; the file does not mark which branch was kept.
- **`parentSession`.** A continued session is its own session in hippo.
- **Compaction summaries**, `custom_message` and `branch_summary` entries: text Command Code wrote for the model, not the person or the agent.
- **Sub-agents.** Not seen in a real file; not read.
- **Older versions.** Version 1 and 2 files are reported, not read; Command Code converts them itself the next time it opens them.
- **Fleet live state**: Command Code sessions show from their indexed turns, without a running or waiting state.

## Taste

Command Code also learns a person's preferences into Markdown files: `<project>/.commandcode/taste/taste.md` and `~/.commandcode/taste/taste.md`, each with categories under `# ` headings and one bullet per preference ending in `Confidence: <0..1>`, a category moved to `<category>/taste.md` once it has more than five. Its learner rewrites these files.

These are the closest thing another agent has to hippo's rules, so hippo could offer them to the Inbox as proposals. Not built in 0.4.0, for three reasons. A proposal in the Inbox carries the sessions and turns it came from, and a taste bullet carries none, so the person could not check it against what happened. Its format is Command Code's to change and is rewritten by its learner, so a proposal would need a stable identity hippo cannot give it. And the learner can itself import Claude Code and Codex sessions, so the same correction could come back twice. A later design can add taste as a source with its own record kind, never kept without the person.

## Fixtures and tests

A synthetic fixture under `fixtures/transcripts/command-code/` in the shape above: a header, a prompt, an assistant message with text and a tool call, its result, a closing answer, and skipped entries (`model_change`, `compaction`, `thinking`). Tests: the parse into turns; skipped entries; a file with another version or no header; the redaction test every adapter has; an ingest through the default roots in a scratch `HOME`.

## Alternatives

- **Read only the newest branch.** Truer to what the person kept, but the file does not mark the kept branch, and the guess (the last-written leaf) is wrong after a person goes back and forth. Reading everything is honest about what happened in the session.
- **Read `<id>.prompts.jsonl`** for the person's prompts. The session file already holds them with their place in the conversation.
- **Map tool names to Claude Code's.** Worth doing once Command Code's tool set is seen in real sessions.
