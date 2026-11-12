# A DeepSeek Harness (`dsh`) plugin

Status: accepted, to build after 0.2.0.

## Background

hippo indexes what coding agents did on a machine (their session transcripts) and keeps a folder of plain-Markdown notes that agents start from. The folder belongs to one *workspace*, set up with `hippo init`. The *start block* is the part of those notes every new session should see (the entry note and any required notes), compiled by `hippo context` under a byte budget the workspace sets (`budget_bytes`, 25 600 bytes by default). hippo reads new transcripts when it *ingests*, on a schedule or when a person runs `hippo`.

[DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) (`dsh`) is a coding agent built so that everything is a plugin. hippo 0.2.0 reads dsh's session logs ([the adapter](dsh-adapter.md)).

## The problem

hippo gives a dsh session nothing back. A Claude Code session with hippo's plugin starts with the start block and has `/hippo:ask` and `/hippo:day`. A dsh session starts with none of that, and its agent learns about hippo only if someone tells it to run `hippo` in a shell.

A dsh plugin can add text to a session's context and can give the model a tool. So hippo can ship a dsh plugin that does for dsh what its Claude Code plugin does for Claude Code.

## What changes for a user

A dsh user installs the plugin into a dsh profile (see [Installing](#installing)). After that:

- A new dsh session whose working directory is inside hippo's workspace starts with the same start block a Claude Code session gets. A session outside the workspace, or on a machine where hippo has no notes folder, gets nothing, as with the Claude Code plugin.
- The model has two tools and decides itself when to call them:
  - `hippo_ask`: "has an agent on this machine already solved this?" It runs `hippo ask --sources-only` and returns the ranked past sessions and notes, each with its session id or note path.
  - `hippo_day`: "what did the agents do today?" (or on a given day). It runs `hippo show day`.

In the Claude Code plugin, `/hippo:ask` and `/hippo:day` run only when the person types them. Here the model calls the tools on its own, and what they return goes to the model the dsh session uses, like any other tool result (see [What leaves the machine](#what-leaves-the-machine)).

The plugin is a separate package in this repository, `plugins/dsh/`, named `dsh-plugin-hippo`, under MIT. hippo itself changes in one place: `hippo ask` and the x-ray learn to recognise a dsh `hippo_ask` call (see [Changes in hippo](#changes-in-hippo)).

## How a dsh plugin works

All of this was read from dsh's source at commit `e8f7b93` (version 0.2.2-rc.2, November 2026), the same commit the adapter was written against.

- **A plugin** is a JavaScript module that exports `name`, `inject` (the services it needs), an optional `Config` and `apply(ctx, config)`. dsh is built on [Cordis](https://github.com/cordiverse/cordis), which validates the plugin's configuration with `Config` (any [Standard Schema](https://standardschema.dev/)) before `apply`, and removes whatever the plugin registered through `ctx` when the plugin unloads.
- **Context at the start of a turn.** Before each model request dsh calls the `agent/pre-step` listeners in turn; each can change the list of messages that enter the session's log before the request. dsh's own context plugins (`agent-instructions` for `AGENTS.md`, `tmux-context`) add a user-role message there with a `source.kind` of their own. The message is written to the session log and replayed on resume, like any other message.
- **A tool** is registered with `ctx.tools.register({ name, description, parameters, output, execute })`. `parameters` is a JSON Schema the model sees; `execute` returns one JSON value; `output.render` turns it into the text the model reads. A thrown error becomes a failed tool result. dsh asks the person for approval only for actions its sandboxed shell or a `tools/pre-execute` policy flags, so a plugin's tool runs without asking.
- **Session projections** (`ctx.sessionProjections.register`) fold a session's committed events into a small state. They are how a plugin asks "has this session already had X?" without reading the log itself.

## The plugin

### The start block

The plugin joins `agent/pre-step`. It passes a rejected step, an aborted one, a step after the first of a turn, and a first step with no messages through untouched. Otherwise it gives the start block when all of these hold:

1. the session is not a sub-agent (its header's `origin` is not `subagent`), as a Claude Code sub-agent gets no `SessionStart` hook either;
2. the part of the session the model still sees holds no start block. A projection over the session's events records the log position of the last start block, and forgets it when a compaction (`compaction/summary` or `compaction/prune`) shadows that position. So a resumed session, or a fork that copied its parent's history, gets no second block, and a compacted one gets it again, as Claude Code's `SessionStart` hook fires again after `/compact`;
3. this process has not decided for this session since its last compaction. The plugin asks hippo once per session, at its first turn; a session outside the workspace does not run hippo again at every turn;
4. `hippo config show --json` names a workspace (hippo is installed and set up with a notes folder), and the session's working directory (`ctx.workingDirectory`), with symbolic links resolved, is that workspace or inside it;
5. `hippo context --parts required` prints something.

It then adds one user-role message ahead of the turn's messages:

```text
<system-reminder>
hippo's start block for this workspace (`hippo context --parts required`):

<the block as hippo printed it>
</system-reminder>
```

Its `source` is `{ kind: "hippo-start", form: "snapshot", sections: [{ name: "hippo-start", text }] }`. A literal `</system-reminder>` in the block is escaped so a note cannot close the frame. The block's size is hippo's to keep: `hippo context` already holds it under the workspace's `budget_bytes`, and the plugin does not cut it.

When a step fails (hippo is missing, a command fails or times out, its output is not what the plugin expects), the session gets no block and dsh's log gets a warning: one per process when hippo is missing, one per session otherwise. The turn goes on.

hippo's dsh adapter indexes only `user/message` events whose `source.kind` is `user`, so the start block is never read back into hippo's index as something the person typed.

**Why not `hippo hook session-start --plugin`.** That is the Claude Code plugin's command. Before it prints, it checks whether a Claude Code settings file for the folder already runs hippo's own hook, and prints nothing if one does, so a Claude Code session does not get the block twice. That question is about Claude Code: a workspace with hippo's Claude Code hooks installed would give a dsh session nothing. `hippo config show --json` and `hippo context --parts required` are the two read-only steps that command takes, without that check, and both exist in hippo 0.2.0. The plugin reads one field of `config show --json`, `workspace`; that output also feeds Hippo.app, so it changes only with care.

### The tools

| Tool | Arguments | Runs |
|---|---|---|
| `hippo_ask` | `question` (required) | `hippo ask --sources-only --json -- <question>` |
| `hippo_day` | `date` (optional, `YYYY-MM-DD`) | `hippo show day [<date>] --offset <the machine's UTC offset on that day>` |

The tools check their own arguments: a raw `ctx.tools.register` does not validate them against `parameters`. `--` keeps a question that starts with `-` from being read as a flag.

`hippo_ask` asks for `--json` so it can show each source's session id (the text output prints none) and writes it as text:

```text
searched 41 sessions and 12 notes for: thumbnail sizes
[1] the grid thumbnails look soft  (session · claude-code · tern · 2026-11-02 10:31)
    agent: The thumbnail pipeline now writes 320 and 1280 pixels on the long edge.
    session eval-02
[2] Thumbnails  (note · 2026-10-30 09:12)
    notes/Thumbnails.md
```

Both are read-only. `--sources-only` ranks past sessions and notes without calling a model, and `hippo show day` reads the rollup the last ingest wrote, so `hippo_day` knows the day only as of the last ingest; its description says so. Neither reads new transcripts or writes anything. The offset for a date is the machine's offset at noon that day, so a day across a change to or from daylight saving time is still the user's day.

When hippo is not installed, or says it is not set up, the tool fails with that message and the next step (`hippo start`), so the model can tell the person. A long result is left to dsh's spill policy, which caps what reaches the model and keeps the rest in a file.

### Running hippo

The plugin runs the `hippo` binary as a child process of dsh, with arguments passed as an array (never through a shell), dsh's environment and a timeout, and stops it when dsh cancels the call. It finds `hippo` on `PATH`, then in `~/.local/bin` (where hippo's installer puts it) and `~/.cargo/bin`, as the Claude Code plugin does. It reads only hippo's standard output and exit status.

hippo runs in a local directory the plugin chooses, not in the session's: the workspace root for `hippo context`, the home folder otherwise. None of these commands needs the session's directory, and a session in one of dsh's SSH workspaces has a working directory that is not on this machine. Such a session gets no start block unless its remote path happens to name a folder inside the local workspace.

The plugin never reads hippo's index or notes itself, never writes a file, and never opens a network connection.

### What leaves the machine

hippo sends nothing for these commands. But the start block and every tool result become part of the dsh session, and dsh sends the session to the model provider it is set up with, as it does with every file the agent reads. `hippo_ask` searches every indexed session on the machine, across projects, so its result can carry another project's history into this session. In Claude Code the same text reaches Claude only when the person types `/hippo:ask`; in dsh the model can call the tool on its own.

A person who does not want that sets `tools: false`, which keeps the start block and drops the tools. The plugin's README and hippo's security page say this.

### Configuration

The plugin's row in `cordis.patch.yml` takes:

| Field | Default | Meaning |
|---|---|---|
| `command` | `hippo` | The hippo binary: a name looked up as above, or an absolute path |
| `timeoutMs` | `10000` | How long one hippo command may run, as long as the Claude Code plugin's hook may |
| `tools` | `true` | Register `hippo_ask` and `hippo_day` |

### Dependencies and compatibility

The plugin imports nothing from dsh at run time and has no dependencies. It uses dsh's services through the shapes it reads from `ctx` (`ctx.on`, `ctx.tools.register`, `ctx.sessionProjections.register`, `ctx.workingDirectory.get`, `ctx.logger`), declared as local TypeScript types; its `inject` is `agents`, `tools`, `sessionProjections` and `workingDirectory`. It writes by hand what it would otherwise import: a Standard Schema `validate` for its `Config`, a `parse` for its projection's state, and its message, built as dsh's `createUserMessage` builds one. So it installs without fetching dsh's packages, and its tests run without dsh.

The cost is that a change to those shapes in dsh is not caught by the compiler. dsh says it will make breaking changes. `engines.dsh` is `>=0.2.0 <0.3.0-0` (dsh compares ranges with prereleases included, so `<0.3.0` would admit 0.3.0's release candidates), so a dsh outside it refuses the plugin instead of loading it and failing later. The range moves only after the end-to-end check below passes against the newer dsh.

## Changes in hippo

`hippo ask` leaves its own earlier output out of its sources: it skips a tool call whose text matches `hippo ask`, `hippo recall` or `hippo context`, and that call's result. The dsh adapter stores a tool call as `<name> <arguments>`, so a `hippo_ask` call is stored as `hippo_ask {"question": …}` and would not match; earlier answers would come back as past sessions. The pattern learns `hippo_ask`. The x-ray's list of searches learns it too, so a dsh session that asked hippo shows that search. `hippo_day`'s result is indexed like any tool result, as the Claude Code plugin's `hippo show day` through Bash is.

## Installing

The plugin is not published to npm. A git install of a package that builds on install needs the person to allow that build in pnpm, so the repository holds the plugin's built `lib/` next to its TypeScript source, and installing it runs no build. What users run is `lib/`, so a pull request that changes the source shows the regenerated `lib/` too, and reviewers read it. CI deletes `lib/`, rebuilds it, and fails when the result differs from what is committed, new files included.

From GitHub, pinned to a commit:

```sh
dsh plugin --profile <profile> add "github:VanitasCaesar1/hippo#v0.2.1&path:/plugins/dsh"
```

From a clone of this repository:

```sh
dsh plugin --profile <profile> add /path/to/hippo/plugins/dsh
```

`dsh plugin --profile <profile> remove dsh-plugin-hippo` removes it. The plugin needs hippo installed and set up (`hippo start`, and `hippo init --notes-dir …` for the start block), as the Claude Code plugin does.

## Tests

- **Unit tests** (`npm test` in `plugins/dsh/`, Node's own test runner) load the plugin against a fake `ctx` and a fake `hippo` script put first on `PATH`. They cover: the start block given once, with its frame, source and place; nothing outside the workspace, with no workspace, to a sub-agent, on a later step or turn, after a resume or fork that holds one, or when hippo is missing, fails or times out; the block given again after a compaction shadows it; the escape of `</system-reminder>`; each tool's arguments as hippo receives them, the rendered `hippo_ask` sources, and the tools' errors; the configuration's checks.
- **hippo's own tests** cover `ask` and the x-ray recognising a stored dsh `hippo_ask` call.
- **CI** runs the unit tests on Linux and macOS and checks that `lib/` is current.
- **End to end**, by hand before a release that changes the plugin and before `engines.dsh` is widened: dsh run from a checkout of its repository, in a scratch `HOME`, against a small local server that answers like an OpenAI-compatible endpoint with canned replies, so no model and no key are used. The canned model calls both tools; the check reads the requests the server received and confirms the start block reached the model once and the tool results came back. `plugins/dsh/e2e/` holds the server and the script.

## What is not supported

- **The relevant notes.** A Claude Code session with hippo's hooks installed also gets, on its first prompt, the notes relevant to that prompt (`hippo hook prompt`). The plugin gives only the start block, as the Claude Code plugin does.
- **Recording the session in hippo.** hippo's own hooks start and end a session record; the plugin does not. dsh sessions reach hippo through the adapter, when hippo next ingests.
- **Remote workspaces**, as above.
- **Windows.** Not tried.
- **A model's written answer.** `hippo_ask` never asks hippo for one; the model in the session reads the sources and answers itself.
- **Reading a whole past session.** There is no tool for `hippo show session <id>`; the model can run it in dsh's shell, which may ask for approval.

## Alternatives

- **Instructions only.** dsh already loads `AGENTS.md` into every session, so a line there could tell the agent to run `hippo context` and `hippo ask` in the shell. No code, but nothing makes the agent run them, and each command is a shell call that dsh's approval policy may stop. Rejected; it remains what a person can do without the plugin.
- **Reuse `hippo hook session-start --plugin`**, or add a flag to it that skips the Claude Code check. One command instead of two, but the first gives nothing in a workspace whose Claude Code settings run hippo's hook, and the second needs a hippo newer than 0.2.0. Rejected for now; the plugin works with the hippo people have.
- **An MCP server.** dsh speaks MCP, so `hippo` could serve the two tools over it. hippo has no MCP server, and it would still give no start block. Not now.
- **Import dsh's packages** (`defineTool`, `createUserMessage`, `zod`). Type-checked against dsh and closer to its examples, but the plugin would then depend on dsh's packages resolving from the profile at run time and from npm at test time. Rejected for now; revisit if the shapes the plugin uses start to drift.
- **Plain JavaScript with JSDoc types.** No build and no committed `lib/`, at the cost of writing types in comments. TypeScript was chosen for the source; the committed build and its CI check are the price.
- **Publish to npm.** The shortest install (`dsh plugin add dsh-plugin-hippo`). Not now: installing from GitHub works, and publishing is a separate decision.
