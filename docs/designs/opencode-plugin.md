# An OpenCode plugin

Status: accepted, to build in 0.3.0.

## Background

hippo indexes what coding agents did on a machine and keeps a folder of plain-Markdown notes that agents start from. The folder belongs to one *workspace*, set up with `hippo init`. The *start block* is the part of those notes every new session should see (the entry note and any required notes), compiled by `hippo context` under the workspace's byte budget (`budget_bytes`, 25 600 bytes by default).

hippo 0.3.0 reads OpenCode's sessions ([the adapter](opencode-adapter.md)). This document is about the other direction: giving an OpenCode session what hippo knows. It follows the [dsh plugin](dsh-plugin.md), which does the same for DeepSeek Harness, and differs from it only where OpenCode's plugin system does.

## The problem

A Claude Code session with hippo's plugin starts with the start block and has `/hippo:ask` and `/hippo:day`; a dsh session with hippo's dsh plugin has the block and two tools. An OpenCode session has none of that, and its agent learns about hippo only if someone tells it to run `hippo` in a shell.

An OpenCode plugin can add text to what the model is sent and can give the model tools, so hippo can ship one.

## What changes for a user

A user adds the plugin to OpenCode's configuration (see [Installing](#installing)). After that:

- A top-level OpenCode session whose directory is inside hippo's workspace gets the same start block a Claude Code session gets. A session outside the workspace, a sub-agent, or a machine where hippo has no notes folder gets nothing, as with the Claude Code plugin.
- The model has two tools and decides itself when to call them:
  - `hippo_ask`: "has an agent on this machine already solved this?" It runs `hippo ask --sources-only --json` and returns the ranked past sessions and notes, each with its session id or note path.
  - `hippo_day`: "what did the agents do today?" (or on a given day). It runs `hippo show day`.

The plugin is a separate package in this repository, `plugins/opencode/`, named `opencode-plugin-hippo`, under MIT. hippo itself does not change for it: `hippo ask` already leaves stored `hippo_ask` calls out of its sources and the x-ray already lists them as searches (from the dsh plugin), and the OpenCode adapter stores a tool call the way the dsh adapter does.

## How an OpenCode plugin works

All of this was read from OpenCode's source at tag `v2.2.7` (commit `fedc6f2`, November 2026), and checked by running that version (see [Tests](#tests)).

- **A plugin** is a module whose default export is `{ id, setup(ctx) }`. OpenCode 2 does not load OpenCode 1 plugins (a function returning hooks). OpenCode runs one instance of a plugin for each location (a directory, or a workspace) it serves, shared by every session there, and runs `setup` again after the plugin or the configuration changes.
- **Options** come from the configuration entry, `{ "package": "<spec>", "options": { … } }`, as `ctx.options`.
- **The system prompt.** `ctx.session.hook("context", event)` runs before every model request of a session's agent loop, tool continuations included, with `event.system`, the system prompt parts of that request. OpenCode builds the system prompt again for every request and does not store it, so a part added there reaches that request only, survives compaction, and is never written into the session's history.
- **The session.** The `context` event names the session (`sessionID`) but not its directory or its parent. `ctx.session.get({ sessionID })` returns both: `location.directory`, and `parentID` for a sub-agent.
- **A tool** is added with `ctx.tool.transform((editor) => editor.add({ name, description, input, execute, options }))`. `input` may be a plain JSON Schema. `execute(input, { sessionID, signal, … })` returns `{ content }`; a thrown error becomes a failed tool result. A tool from a plugin is, by default, reachable only through OpenCode's code-mode tool (the model writes a script that calls it); `options: { codemode: false }` makes it an ordinary tool the model calls directly.
- **Hooks fail loudly.** A hook whose promise rejects can stop the model step, so every hook body catches its own errors.

## The plugin

### The start block

On each `context` event the plugin adds one system part, `{ type: "text", text }`, when the session has a block, and nothing otherwise. Whether a session has a block, and the block, are decided once per session per plugin instance, at its first model request, and kept in memory:

1. `ctx.session.get` gives the session; a session with a `parentID` (a sub-agent) gets nothing, as a Claude Code sub-agent gets no `SessionStart` hook either;
2. `hippo config show --json` names a workspace (hippo is installed and set up with a notes folder), and the session's `location.directory`, with symbolic links resolved, is that workspace or inside it;
3. `hippo context --parts required` prints something.

The part's text is:

```text
<system-reminder>
hippo's start block for this workspace (`hippo context --parts required`):

<the block as hippo printed it>
</system-reminder>
```

A literal `</system-reminder>` in the block is escaped so a note cannot close the frame. The block's size is hippo's to keep: `hippo context` already holds it under the workspace's `budget_bytes`, and the plugin does not cut it.

Because the part is added to every request of the session and the system prompt is not stored, the block stays in front of the model after a compaction without the plugin doing anything, and it never enters the session's messages, so the OpenCode adapter never reads it back as something the person typed. The text is the same for every request of a session, so a provider's prompt cache keeps working.

When a step fails (hippo is missing, a command fails or times out, its output is not what the plugin expects), the session gets no block and OpenCode's log gets a warning: one per plugin instance when hippo is missing, one per session otherwise. The request goes on.

The same reasons as for the dsh plugin keep it off `hippo hook session-start --plugin`: that command checks Claude Code's settings, which say nothing about OpenCode.

### The tools

| Tool | Arguments | Runs |
|---|---|---|
| `hippo_ask` | `question` (required) | `hippo ask --sources-only --json -- <question>` |
| `hippo_day` | `date` (optional, `YYYY-MM-DD`) | `hippo show day [<date>] --offset <the machine's UTC offset on that day>` |

Both are added with `codemode: false`, so the model calls them as it calls OpenCode's own tools. They check their own arguments. `hippo_ask` writes hippo's JSON as text with each source's session id or note path, exactly as the dsh plugin does. Both are read-only: `--sources-only` calls no model, and `hippo show day` reads the rollup the last ingest wrote, so `hippo_day` knows the day only as of the last ingest; its description says so. When hippo is not installed, or says it is not set up, the tool fails with that message and the next step (`hippo start`), so the model can tell the person.

### Running hippo

As in the dsh plugin: the `hippo` binary runs as a child process, arguments as an array (never through a shell), OpenCode's environment, a timeout, stopped when OpenCode cancels the call. It is found on `PATH`, then in `~/.local/bin` and `~/.cargo/bin`. hippo runs in the workspace root for `hippo context` and in the home folder otherwise. Only its standard output and exit status are read. The plugin never reads hippo's index or notes itself, never writes a file, and never opens a network connection.

### What leaves the machine

hippo sends nothing for these commands. But the start block goes into the system prompt of every model request of the session, and every tool result becomes part of the session, and OpenCode sends both to the model provider it is set up with, as it does with every file the agent reads. `hippo_ask` searches every indexed session on the machine, across projects, so its result can carry another project's history into this session. In Claude Code the same text reaches Claude only when the person types `/hippo:ask`; in OpenCode the model can call the tool on its own.

A person who does not want that sets `"tools": false` in the plugin's options, which keeps the start block and drops the tools. The plugin's README and hippo's security page say this.

### Configuration

Options, in the plugin's entry in `opencode.json`:

| Option | Default | Meaning |
|---|---|---|
| `command` | `hippo` | The hippo binary: a name looked up as above, or an absolute path |
| `timeoutMs` | `10000` | How long one hippo command may run |
| `tools` | `true` | Add `hippo_ask` and `hippo_day` |

An option that is not one of these, or has the wrong type, is reported in OpenCode's log once, and the plugin runs with the defaults for it.

### Dependencies and compatibility

The plugin imports nothing from OpenCode and has no dependencies. It uses OpenCode's services through the shapes it reads from `ctx` (`ctx.session.hook`, `ctx.session.get`, `ctx.tool.transform`, `ctx.options`, `ctx.location`), declared as local TypeScript types, so it installs without fetching OpenCode's packages and its tests run without OpenCode. OpenCode forbids a plugin from bundling its own copy of `@opencode/plugin` in any case.

The cost is that a change to those shapes is not caught by the compiler. The package's `engines.opencode` is `>=2.2.0 <3.0.0`. The end-to-end check below is run before a release that changes the plugin and after a new OpenCode minor version.

## Installing

The plugin is not published to npm. OpenCode installs a package spec with npm's own resolver and runs no install scripts, so the repository holds the plugin's built `lib/` next to its TypeScript source, as for the dsh plugin. The package has no `build`, `prepare` or install script (its build script is named `compile`): npm's installer runs `npm install` inside a git dependency that has one, and inside OpenCode's single binary that step fails, so the plugin would not load. CI rebuilds `lib/` and fails when it differs from what is committed.

From GitHub, pinned to a release tag, in `~/.config/opencode/opencode.json` (every project) or a project's `opencode.json`:

```json
{
  "plugins": [
    { "package": "github:VanitasCaesar1/hippo#v0.4.1::path:plugins/opencode", "options": { "tools": true } }
  ]
}
```

`::path:` is npm's syntax for a package in a subfolder of a git repository. A plain string entry works too, with the default options. From a clone of this repository, the entry is the folder: `"plugins": ["/path/to/hippo/plugins/opencode"]`. Removing the entry removes the plugin. The plugin needs hippo installed and set up (`hippo start`, and `hippo init --notes-dir …` for the start block).

## Tests

- **Unit tests** (`npm test` in `plugins/opencode/`, Node's own test runner) load the plugin against a fake `ctx` and a fake `hippo` script put first on `PATH`. They cover: the block given to a top-level session in the workspace, on every request, from one run of hippo per session; nothing outside the workspace, with no workspace, to a sub-agent, or when hippo is missing, fails or times out; one warning per cause; the escape of `</system-reminder>`; the tools added with `codemode: false`, or not at all with `tools: false`; each tool's arguments as hippo receives them, the rendered `hippo_ask` sources, and the tools' errors; the options' checks.
- **CI** runs the unit tests on Linux and macOS and checks that `lib/` is current.
- **End to end**, by hand before a release that changes the plugin: the real `opencode` binary in a scratch `HOME` with scratch `XDG_*` folders, against a small local server that answers like an OpenAI-compatible endpoint with canned replies, so no model and no key are used. The canned model calls both tools; the check reads the requests the server received and confirms the start block was in the system prompt of each request and the tool results came back, and then that hippo's adapter reads the scratch database the run left. `plugins/opencode/e2e/` holds the server and the script.

## What is not supported

- **The relevant notes** for a prompt (`hippo hook prompt`): the plugin gives only the start block, as the Claude Code and dsh plugins do.
- **Recording the session in hippo.** OpenCode sessions reach hippo through the adapter, when hippo next ingests.
- **A session's working directory that changes** (OpenCode's `location-switched`): the decision made at the first request stands for the session in that plugin instance.
- **Windows.** Not tried.
- **A model's written answer.** `hippo_ask` never asks hippo for one; the model in the session reads the sources and answers itself.

## Alternatives

- **A message instead of a system part** (`ctx.session.synthetic`, or the `prompt` hook). The block would be stored in the session once, as in dsh, but a compaction would summarise it away and the plugin would have to watch for that, and the adapter would have to tell it from the person's words. Rejected: the system prompt is rebuilt for every request, which is what a start block wants.
- **Leave the tools in code mode.** Fewer tools in front of the model, but the model reaches them only by writing a script, which smaller models do badly. Rejected; `codemode: false` is the default here, and a person who wants fewer tools sets `tools: false`.
- **Import `@opencode/plugin`.** Typed against OpenCode, but it would have to resolve at run time from OpenCode's host and at test time from npm. Rejected for local types, as in the dsh plugin.
- **One plugin for dsh and OpenCode.** The two load plugins differently and give them different services; sharing the hippo-running code would need a third package. Rejected for now; the two copies are small.
- **Publish to npm.** Not now, for the reasons the dsh plugin gives.
