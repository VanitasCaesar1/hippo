# MCP server

Status: accepted, to build in 0.4.2.

## Background

hippo gives an agent its notes in two ways today: hooks that print the start block and the relevant notes into a Claude Code, Codex, OpenCode or DeepSeek Harness session, and commands (`hippo ask`, `hippo recall`, `hippo context`) that an agent runs from a shell when its instructions tell it to. Both need hippo to know the agent: its hook format, or a shell tool and an instruction file that mentions hippo.

Most coding agents now speak the [Model Context Protocol](https://modelcontextprotocol.io/specification/2025-11-25) (MCP): Cursor, Windsurf, Zed, VS Code's Copilot agent, Gemini CLI, Claude Desktop and Claude Code all start local MCP servers from a small JSON entry and offer their tools to the model. One server reaches all of them without an adapter per agent.

## The problem

An agent hippo has no hook for cannot ask hippo anything unless it has a shell and has been told to. Most of the agents people use are in that position.

## What changes for a user

```text
hippo mcp [--allow-write]
hippo mcp install --client <cursor|claude-desktop|gemini|windsurf|zed|vscode|claude-code> [--write] [--allow-write]
```

`hippo mcp` is started by the client, not by the person. It speaks MCP over stdio until the client closes its stdin. `hippo mcp install --client <client>` prints the entry to add to that client's configuration, with the absolute path of the `hippo` binary; `--write` merges it into the client's user file instead. [`docs/mcp.md`](../mcp.md) lists each client's file and format with its source.

The read tools need nothing but an index (`hippo start` or `hippo ingest`); `hippo_context` also needs a notes folder (`hippo init`).

## Tools

Every tool returns one text content block, the text the matching command prints, so an agent sees what a person would see in a terminal. Each is marked `readOnlyHint` except the write tool.

| Tool | Arguments | Returns the text of |
|---|---|---|
| `hippo_context` | `task`, `cwd` | `hippo context --task <task>`, run in `cwd` |
| `hippo_recall` | `query` (required), `limit` | `hippo recall <query> --limit <limit>` |
| `hippo_ask` | `question` (required), `limit` | `hippo ask --sources-only <question>` |
| `hippo_day` | `date`, `offset` | `hippo show day [<date>] --offset <offset>` |
| `hippo_incidents` | `days`, `all` | `hippo incidents --days <days> [--all]` |
| `hippo_note_correction` | `said`, `rule` (required), `title`, `note`, `section` | queues a lesson; only with `--allow-write` |

- `hippo_context` takes `cwd` because a client starts the server in a directory of its choosing, often not the project. The directory only ranks the notes that name its repository higher, as the command's current directory does.
- `hippo_ask` never makes hippo's model call. The client is a model already; the ranked sources, with why each matched and how to resume it, are what it needs, and a second model call per question would cost the person twice.
- `hippo_day` and `hippo_incidents` give an agent the same view of the fleet's day that `hippo show day` and `hippo incidents` give a person, so it can say what other agents did or broke today.

### The write tool

`hippo_note_correction` exists for the moment a person corrects an agent in a way future sessions should keep ("no, previews run against staging"). The agent passes the person's words (`said`) and a rule it drafts from them. hippo treats the agent as the drafter of a lesson and nothing more: the `lesson-reviewer` checks the rule against the words in a fresh context, as for every lesson the lessons pass drafts. A confirmed rule is saved as a pending lesson and waits in the Inbox (`hippo lessons`, Hippo.app) until the person keeps or dismisses it; a rejected one is saved as rejected, with the reviewer's notes in the reply. No note changes in this call. Keeping it later writes it as any kept lesson is written, with a receipt `hippo revert` undoes.

The tool is listed only when the server runs with `--allow-write`, because each call runs the reviewer (a model call through `model_command`) and writes a record. The record is an ordinary lesson (`kind: correction`, key `reported: <said>`), so no record format changes. It has no evidence entries: the words came through the agent, not from an indexed turn. If a later lessons scan finds the same correction in the index, it merges into this record as it would into any correction with the same terms.

## Redaction

Ingest redacts what enters the index, but notes are the person's own files and are not redacted on disk, and a CLI user reading their own notes in a terminal needs no redaction. A tool result leaves the machine: the client sends it to a model provider. So every tool's text, and every error text, goes through `hippo_core::redact::redact` (the function ingest uses: keys, tokens, JWTs, `key=value` secrets, emails, Aadhaar and phone numbers) and then the control-character escaping the CLI applies to errors, before it is returned. The cost is that a note's email address or phone number reaches the client as `[redacted:email]`; that is the same trade ingest makes. The integration test puts a synthetic key in a note and checks it never comes back.

## Resources and prompts

None. Resources would expose notes as files; the client already reads the workspace's files, and `hippo_context` returns the ones that matter within the byte budget, which a resource listing would not. Prompts are user-invoked templates; nothing in hippo is a template a person picks from a menu. Both can be added when there is a use that a tool does not already cover.

## Protocol

- **Transport.** stdio: one JSON-RPC 2.0 message per line on stdin and stdout, logs on stderr only. A batch (a JSON array, allowed by 2025-03-26) gets an array of replies.
- **Versions.** The server speaks 2024-11-05, 2025-03-26, 2025-06-18 and 2025-11-25, each of which opens with `initialize`. It answers `initialize` with the version the client asked for when it is one of those, and with 2025-11-25 otherwise, as the specification's version negotiation requires; the client then continues or disconnects. 2026-07-28, which replaces `initialize` with per-request metadata, is not supported yet.
- **Methods.** `initialize`, `ping`, `tools/list`, `tools/call`. Notifications (`notifications/initialized`, `notifications/cancelled`) get no reply. Any other method gets `-32601`. An unknown tool is a protocol error (`-32602`); a bad argument or a failed command is a tool result with `isError: true` and the error's text, so the model can correct itself. A panic in a tool is caught and reported the same way.
- **State.** None between calls. Each call reads `config.toml`, the workspace and the index afresh, so the server sees a new ingest without a restart.

## SDK or hand-written

The official Rust SDK, `rmcp` 3.11, was tried first. It depends on `chrono` unconditionally, which `cargo deny` fails as unmaintained (RUSTSEC-2026-0543), and it adds tokio, schemars and about forty other crates to a CLI that is synchronous today. The server is instead a small synchronous loop over `serde_json`, with unit tests for each protocol rule and an end-to-end conversation test. It implements only what the table above lists; if hippo later needs HTTP transport, sampling or elicitation, moving to the SDK is the way, once it passes `cargo deny`.

## Installing into a client

`hippo mcp install --client <client>` prints the entry by default. With `--write` it:

1. reads the client's user file, or starts an empty object when there is none;
2. refuses, leaving the file untouched, when the file is not plain JSON (Zed's and VS Code's settings may hold comments, which a JSON rewrite would drop) or the servers key is not an object, and says to add the entry by hand;
3. does nothing when an identical `hippo` entry is there;
4. otherwise copies the file to `<file>.hippo-backup-<UTC time>`, sets the `hippo` entry under the client's servers key, keeping every other server and every other key in their order, and writes the file through a temporary file and a rename.

The entry runs the `hippo` on `PATH` when that is this binary (a Homebrew link outlives an upgrade, the versioned path it points to does not), and this binary's path otherwise.

Claude Code is the exception. It keeps user-scope servers in `~/.claude.json`, a state file it rewrites while it runs, so hippo prints the `claude mcp add` command and refuses `--write`. Claude Code already gets the start block from hippo's hooks; the server adds the other tools.

## Alternatives

- **A tool per CLI command.** `xray`, `fleet`, `brief`, `lessons` and the rest could all be tools. More tools cost every request context in every client and make the model's choice harder; the five cover starting a task, finding past work and seeing the day. Others can follow when an agent needs them.
- **Shelling out to `hippo <command>` for each call.** It would guarantee identical text with no refactor, but costs a process per call and loses the typed errors. The commands now write their text to any writer, and the CLI and the server share that code.
- **Writing notes directly from the write tool.** It would break the rule that nothing reaches a note without a fresh-context review and a person's keep.

## Tests

- Unit tests: version negotiation, notification and response handling, the JSON-RPC error codes, batches, the write tool's listing, redaction of returned text, and the install merge (order kept, backup, idempotence, a commented file refused).
- `crates/hippo/tests/mcp.rs`: one stdio conversation with `hippo mcp --allow-write` against a synthetic workspace and an index of `fixtures/`: `initialize`, `tools/list`, `tools/call` for every tool, `ping`. Each read tool's text is compared with the CLI command's output; a synthetic key in a note is checked to be redacted; the correction is checked to land as a pending lesson with no note changed; `hippo mcp install` is checked to print by default and to merge with `--write` in a temporary `HOME`.

No real client is run in the tests. The JSON-RPC test plays the client's part as the specification describes it.
