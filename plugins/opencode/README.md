# opencode-plugin-hippo

[hippo](https://github.com/VanitasCaesar1/hippo) for [OpenCode](https://github.com/anomalyco/opencode) 2, the way hippo's Claude Code plugin is for Claude Code:

- **A start block.** A top-level OpenCode session whose directory is inside hippo's workspace gets the same block a Claude Code session gets (your entry note and required notes, under hippo's byte budget) in its system prompt. Outside the workspace, in a sub-agent, or without a notes folder, nothing is added.
- **Two tools the model can call.** `hippo_ask` answers "has an agent on this machine already solved this?" from `hippo ask --sources-only`, with each source's session id. `hippo_day` shows what the agents did today, or on a date, from `hippo show day`.

The plugin only runs the `hippo` binary. It reads nothing of hippo's itself, writes nothing, and opens no network connection. Design: [`docs/designs/opencode-plugin.md`](../../docs/designs/opencode-plugin.md). MIT.

## Install

You need hippo installed and set up (`hippo start`, then `hippo init --notes-dir <folder> --entry-note "<folder>/START HERE.md"` in your workspace for the start block), and OpenCode 2.2 or later. OpenCode 1 plugins and this one do not load in each other's versions.

The plugin is not on npm. Add it to `plugins` in `~/.config/opencode/opencode.json` (every project) or in a project's `opencode.json`, pinned to a release of this repository:

```json
{
  "plugins": [
    { "package": "github:VanitasCaesar1/hippo#v0.3.0::path:plugins/opencode", "options": { "tools": true } }
  ]
}
```

`::path:` is npm's syntax for a package in a subfolder of a git repository; OpenCode fetches it into its cache (`$XDG_CACHE_HOME/opencode`) on the next start. The repository holds the built plugin (`lib/`), and its `package.json` has no `build`, `prepare` or install script, so OpenCode installs it without running anything. (npm's installer runs `npm install` in a git dependency that has one of those scripts, and inside OpenCode's single binary that step fails.) From a clone of this repository, use the folder instead: `"plugins": ["/path/to/hippo/plugins/opencode"]`.

Remove the entry to remove the plugin. The plugin looks for `hippo` on `PATH`, then in `~/.local/bin` and `~/.cargo/bin`.

## What reaches the model

hippo sends nothing for these commands. But the start block is in the system prompt of every model request of the session, and each tool result becomes part of the session, and OpenCode sends both to the model provider it is set up with. `hippo_ask` searches every session hippo indexed on this machine, across projects, and the model calls it on its own. If you do not want that, set `"tools": false`: the start block stays, the tools go.

## Options

| Option | Default | Meaning |
|---|---|---|
| `command` | `hippo` | The hippo binary: a name looked up as above, or an absolute path |
| `timeoutMs` | `10000` | How long one hippo command may run |
| `tools` | `true` | Add `hippo_ask` and `hippo_day` |

A wrong or unknown option is reported in OpenCode's log, and its default is used.

## What it does not do

- It gives the start block only, not the notes relevant to your first prompt (which hippo's own Claude Code hooks add).
- It records nothing in hippo; OpenCode sessions reach hippo when it next ingests, through its OpenCode adapter.
- A session is decided once per OpenCode location, at its first request: moving it to another folder later does not change its block.
- `hippo_day` knows the day as of hippo's last ingest.
- Not tried on Windows.

## Developing

```sh
cd plugins/opencode
npm ci
npm run typecheck
npm test          # Node's test runner, with a fake hippo on PATH (test/bin/hippo)
npm run check-lib # rebuilds lib/ and fails if it differs from what is committed
```

`lib/` is what users run: commit it with any change to `src/`.

The end-to-end check runs a real `opencode` against a canned OpenAI-compatible server and a hippo binary, all in a temporary `HOME` with its own `XDG_*` folders, so your own OpenCode configuration and data are not touched. No model and no key are used:

```sh
plugins/opencode/e2e/run.sh "$(command -v opencode)" /path/to/hippo
plugins/opencode/e2e/run.sh "$(command -v opencode)" /path/to/hippo "github:VanitasCaesar1/hippo#v0.3.0::path:plugins/opencode"
```

It writes an `opencode.json` with the canned provider and the plugin, runs one task whose canned model calls both tools, and checks that every model request held the start block once, in its system prompt, and that both tool results came back. Then hippo ingests the database OpenCode wrote and must find the session. The second form installs the plugin from GitHub (it needs the network). Run it before a release that changes the plugin and after a new OpenCode minor version.
