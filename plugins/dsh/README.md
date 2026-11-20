# dsh-plugin-hippo

[hippo](https://github.com/VanitasCaesar1/hippo) for [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) (`dsh`), the way hippo's Claude Code plugin is for Claude Code:

- **A start block.** A new dsh session whose working directory is inside hippo's workspace starts with the same block a Claude Code session gets: your entry note and required notes, under hippo's byte budget. Outside the workspace, or without a notes folder, nothing is added.
- **Two tools the model can call.** `hippo_ask` answers "has an agent on this machine already solved this?" from `hippo ask --sources-only`, with each source's session id. `hippo_day` shows what the agents did today, or on a date, from `hippo show day`.

The plugin only runs the `hippo` binary. It reads nothing of hippo's itself, writes nothing, and opens no network connection. Design: [`docs/designs/dsh-plugin.md`](../../docs/designs/dsh-plugin.md). MIT.

## Install

You need hippo installed and set up (`hippo start`, then `hippo init --notes-dir <folder> --entry-note "<folder>/START HERE.md"` in your workspace for the start block), and dsh 0.2.

The plugin is not on npm. Install it from GitHub, pinned to a commit of this repository, into the dsh profile you use (`web` for `dsh web`):

```sh
dsh plugin --profile web add "github:VanitasCaesar1/hippo#v0.8.0&path:/plugins/dsh"
```

or from a clone of this repository:

```sh
dsh plugin --profile web add /path/to/hippo/plugins/dsh
```

The repository holds the built plugin (`lib/`), so installing it runs no build script. Start a new dsh session in your workspace; `dsh --profile web --dump-config` shows a `# == dsh-plugin-hippo` layer. `dsh plugin --profile web remove dsh-plugin-hippo` removes it.

The plugin runs the path in `$HIPPO_HOME/bin-path` (default `~/.hippo/bin-path`), which `hippo init`, `hippo hooks install` and the installer write, so another program named `hippo` earlier on `PATH` is not the one that runs. Without that file it looks for `hippo` on `PATH`, then in `~/.local/bin` and `~/.cargo/bin`, and `hippo doctor` says so.

## What reaches the model

hippo sends nothing for these commands. But the start block and each tool result become part of the dsh session, and dsh sends the session to the model provider it is set up with. `hippo_ask` searches every session hippo indexed on this machine, across projects, and the model calls it on its own. If you do not want that, set `tools: false`: the start block stays, the tools go.

## Settings

Override the plugin's row in your profile's `cordis.patch.yml` (`$DSH_HOME/profiles/<profile>/cordis.patch.yml`, `DSH_HOME` defaulting to `~/.dsh`). An override replaces the whole `config`, so give every key you set:

```yaml
- id: hippo
  config:
    command: hippo     # a name looked up as above, or an absolute path
    timeoutMs: 10000   # how long one hippo command may run
    tools: true        # register hippo_ask and hippo_day
    guard: false       # run hippo guard check before every tool call
```

**The guard.** With `guard: true` the plugin runs `hippo guard check --agent dsh` in dsh's `tools/pre-execute` gate with the tool's name, parsed arguments and the session's folder ([the guard](https://vanitascaesar1.github.io/hippo/docs/security.html#the-guard)). A refusal becomes dsh's `deny`; a warning lets the call run and reaches the model with the call's result (and dsh's log). In hippo 0.6 only hippo's self-protection rules refuse. When hippo cannot answer, the call goes on, unless `fail = "closed"` is in force. The contract was read from dsh's `dsh-tools` type definitions and checked against a running dsh 0.2.2-rc.5 by `scripts/agent-e2e` in the repository: a warned call runs and the model reads the warning with its result; a refused call does not run. dsh's headless profile prints only the final answer, so there the person sees a warning only in the session (dsh's web UI) or if the model passes it on.

## What it does not do

- It gives the start block only, not the notes relevant to your first prompt (which hippo's own Claude Code hooks add).
- It records nothing in hippo; dsh sessions reach hippo when it next ingests, through its dsh adapter.
- A sub-agent gets no start block, as in Claude Code. A session in one of dsh's SSH workspaces gets none either, since hippo runs on this machine.
- `hippo_day` knows the day as of hippo's last ingest.
- Not tried on Windows.

## Developing

```sh
cd plugins/dsh
npm ci
npm run typecheck
npm test          # Node's test runner, with a fake hippo on PATH (test/bin/hippo)
npm run check-lib # rebuilds lib/ and fails if it differs from what is committed
```

`lib/` is what users run: commit it with any change to `src/`.

The end-to-end check runs a real dsh from a checkout of its repository (built as its README says, plus `pnpm run build:native-system`, and with `pnpm` on `PATH`) against a canned OpenAI-compatible server and a hippo binary, all in a temporary `HOME`. No model and no key are used:

```sh
plugins/dsh/e2e/run.sh /path/to/deepseek-harness /path/to/hippo
```

It installs the plugin with `dsh plugin add`, runs one headless task whose canned model calls both tools, and checks that every model request held the start block once, ahead of the task, and that both tool results came back. Run it before a release that changes the plugin and before widening `engines.dsh`.
