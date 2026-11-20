# Configuration

hippo has two configuration files and one environment variable.

| File | Scope | Written by |
|---|---|---|
| `$HIPPO_HOME/config.toml` | The machine: which workspace, where transcripts are, which model, when to schedule | `hippo start`, `hippo init`, `hippo config` |
| `<workspace>/.hippo/hippo.toml` | The workspace: where notes are and how big the context may be | `hippo init`, `hippo config` |

`HIPPO_HOME` is the machine directory. If it is unset or empty, hippo uses `~/.hippo`. Besides `config.toml` it holds `index.sqlite`, `ingest.lock`, `ingest-state.json`, `logs/` and `model-runs/`.

`.hippo/hippo.toml` is meant to be committed with the workspace, so a new machine that clones the workspace needs only `hippo init` (with no flags) to pick it up.

A value that breaks the rules below makes the file fail to load, and the command fails with the reason. `hippo doctor` then reports the failure as its one row.

`hippo config show` prints the settings, and `hippo config set`, `unset`, `add` and `remove` change one. Each prints the change and writes it only with `--yes`.

## `$HIPPO_HOME/config.toml`

`hippo start` and `hippo init` write this file with every key set to its default except `workspace`, which a history-only setup leaves out until `hippo init` names one. In this file only, a leading `~` is expanded to your home directory.

| Key | Type | Default | Meaning |
|---|---|---|---|
| `workspace` | path | none; required for notes, context, dreaming and hooks | The one workspace every command uses. Absolute once `~` is expanded. `hippo init` writes the canonical path of the directory you ran it in, or of `--workspace`. |
| `transcript_roots` | list of glob patterns | `["~/.claude*/projects", "~/.codex/sessions", "~/.dsh/sessions", "~/.local/share/opencode", "~/.local/share/muse/sessions", "~/.commandcode/projects", "~/.cursor/projects", "~/.gemini/tmp"]` | Where transcripts live. hippo walks every root. A `*.jsonl` file whose name starts with `rollout-` is read as a Codex rollout; a `session.v<N>.jsonl` or `session.v<N>.jsonl.zstd` in a session folder of a dsh project folder (`--<project>--`) is read as a DeepSeek Harness log, the newest generation only; a `*.jsonl` file under an `agent-transcripts` folder is read as a Cursor agent transcript; a `session-*.json` or `session-*.jsonl` in a `chats` folder under `.gemini` is read as a Gemini CLI session; any other `*.jsonl` file is read as a Claude Code transcript. dsh keeps its sessions in `$DSH_HOME/sessions`, `~/.dsh/sessions` by default. An `opencode.db` or `opencode-<channel>.db` is read as OpenCode's database, each session in it on its own, and the folders below it are not walked; OpenCode keeps it in `$XDG_DATA_HOME/opencode`, `~/.local/share/opencode` by default, which a new configuration lists instead when `XDG_DATA_HOME` is set. A `session.jsonl` in `<YYYY>/<MM>/<DD>/<session id>/` is read as a Muse Code log; Muse keeps them in `$XDG_DATA_HOME/muse/sessions`, `~/.local/share/muse/sessions` by default, listed the same way. A `<session id>.jsonl` in a project folder of a `.commandcode/projects` folder is read as a Command Code session (and no file under `.commandcode` as a Claude Code transcript); Command Code keeps them in `~/.commandcode/projects`. |
| `model_command` | list of strings | the `claude` command below | The command hippo runs for dreaming and for the reviewer. Each element `{system_prompt}` is replaced by the system prompt. The user prompt is written to the command's standard input. |
| `ingest_interval_minutes` | integer, at least 1 | `10` | Minutes between scheduled ingests. Read by `hippo schedule install`, and by `hippo doctor` to decide when the index is stale (older than three intervals). |
| `dream_at` | string `HH:MM` | `"03:30"` | The time of day the scheduled dreaming run starts, on a 24-hour clock. Read by `hippo schedule install`. |
| `member` | string | unset | Your handle in a shared workspace's files (`.hippo/inbox/`): letters, digits, `-` and `_`, at most 32 characters. A label, not an identity: anyone with push access can write any name, and hippo shows each file's git commit author beside it. `hippo lessons share` asks for it. |
| `share_lessons` | `"off"` or `"rules"` | `"off"` | What of yours may be written to a [shared Inbox](sharing.md#the-shared-inbox): nothing, or your lessons' titles, rules, match terms and counts (no quotes). Your consent: `hippo lessons share` (or `hippo config set share_lessons rules`) needs you at a terminal and refuses an agent. |
| `auto_keep` | `true` or `false` | `false` | Keep a reviewed rule seen in 3 or more sessions without asking, recorded as kept by `auto`; never one from an agent memory or one that replaces another. `hippo lessons undo` takes it back. You, at a terminal: the guard blocks an agent's `hippo config set auto_keep`. |
| `memory_sources` | list of source ids | `[]` | The agent memories hippo reads rules from (`hippo lessons sources`). Turned on with `hippo lessons sources --on <id>`, by you at a terminal. |
| `memory_sources_off` | list of source ids | `[]` | The ones you turned off; a source in neither list was never asked about. |
| `rules_budget_bytes` | integer | `2000` | The most the start block's Rules section takes. Rules that do not fit are named by count, and a long session can get them when its topic changes. |
| `reinject_every` | integer, at least 1 | `5` | The fewest prompts between two topic-change injections in one session. |
| `reviewer_command_override` | list of strings | unset | Advanced. A command to run the reviewer with instead of `model_command`. It follows the same rules. `init` does not write it. It exists for testing with a stand-in model, and it lets the reviewer run a different command from the proposing run. |

After you change `ingest_interval_minutes` or `dream_at`, run `hippo schedule install` again. The values are written into the launchd job files (or the printed cron and systemd lines) when you install.

### The default model command

```toml
model_command = [
  "claude", "-p", "--no-session-persistence", "--setting-sources", "", "--tools", "",
  "--strict-mcp-config", "--system-prompt", "{system_prompt}", "--output-format", "json",
]
```

This runs Claude Code in print mode with no tools, no settings files, no MCP servers and no saved session, so the model sees only what hippo sends. A replacement command must:

- read the user prompt from standard input;
- print one JSON object whose `result` field is the model's reply, as `claude -p --output-format json` does;
- finish within 600 seconds. After that hippo stops it and counts a model error.

hippo runs each call in its own new folder, `$HIPPO_HOME/model-runs/<ulid>/`, and removes it afterwards.

`model_command` and `reviewer_command_override` still work as before: the command inherits hippo's whole environment and runs without the sandbox. [Runners](#runners-which-cli-model-and-account-hippos-own-calls-use) replace them; `hippo runners migrate` moves them.

Set it with `hippo config set model_command '<JSON array>' --yes`, for example `'["my-model","--json"]'`, or with words separated by spaces. hippo writes the key as a one-line array and replaces a multi-line array whole. `hippo config unset model_command --yes` restores the default.

When the command exits non-zero, hippo shows its reason: the `result` of the JSON it printed (such as `Not logged in · Please run /login`), else the first non-empty line of stderr, else of stdout. When the program is not on `PATH`, the error says so.

### Runners: which CLI, model and account hippo's own calls use

hippo calls a model when it dreams, drafts and reviews lessons, answers `hippo ask`, and reviews a pack in `hippo add`. A **runner** names a CLI, a model, an effort and an account once; **roles** say which runner each kind of call uses. They live in `config.toml`, never in the shared `hippo.toml`. With none set, the default is the `claude` preset with the default command above, argument for argument. The design is [runners.md](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/runners.md).

```toml
[runners.claude]
preset = "claude"

[runners.work]
preset = "claude"
account = "~/.claude-work"       # passed as CLAUDE_CONFIG_DIR
model = "claude-opus-5-5"        # passed as --model
effort = "high"                  # passed as --effort
data_domain = "acme"

[roles]
default = "work"
ask = "claude"

[fallback]
default = ["claude"]
```

| Key | Meaning |
|---|---|
| `preset` | `claude` or `command`. Required. |
| `model`, `effort`, `account` | `claude` only. `effort` is `low`, `medium`, `high`, `xhigh` or `max`. Without `model` the CLI's default applies. |
| `data_domain` | A label for where this runner's prompts may go. Default `anthropic` for `claude`; required for `command`. |
| `timeout_secs` | Default 600, at most 1800. |
| `argv` | `command` only. Placeholders: `{system_prompt}`, `{system_prompt_file}`, `{reply_file}`, `{run_dir}`. |
| `reply` | `command` only: `json-result` (one JSON object with a string `result`; the default), `stdout` (the whole stdout, trimmed) or `file` (the file at `{reply_file}`). |
| `env_pass` | Extra environment variable names to pass on. |
| `limit_exit_codes` | `command` only: exit codes that mean the account is at its limit, so the call may fall back. |
| `allow_tools` | `command` only: let a runner that may have tools serve `propose`, `review` and `lessons`. Default `false`. |
| `allow_unsandboxed` | Let a `command` runner serve those roles where this machine has no sandbox. Default `false`. |

**Roles.** `default` (required once there is more than one runner), and `propose` (each dreaming pass), `review` (the dreaming reviewer, the lessons reviewer, the pack review), `lessons` (the lessons drafter) and `ask`, which fall back to `default`.

**Fallback.** A role's runner is skipped, before the call, when its account's meter (as Fleet reads it) shows a 5-hour or 7-day window at 95% or more and not yet reset, and the call moves on when the CLI answers with its usage-limit message (or, for a `command` runner, one of its `limit_exit_codes`). Any other failure stops the call. A fallback runner must have the same `data_domain` as the role's runner, so a work account never falls back to a personal one; a role named in `[fallback] cross_domain = ["ask"]` may cross. The receipt records which runner answered and what it fell back from (`runner.fallback_from`), and `hippo runners` shows each role's fallbacks and those refused.

**What every runner gets.** Each call runs in its own run folder, inside an OS sandbox where this machine has one (`sandbox-exec` on macOS, `bwrap` on Linux): it can read the run folder, its own install and its account folder (for Claude Code also `~/.claude.json` and, on macOS, the login keychain it signs in with), and everything outside your home folder, `$HIPPO_HOME` and the workspace; it can write only the run folder, the account folder and the system's temporary folders. Network is allowed. Its environment is a base list (`PATH`, `HOME`, `USER`, `LOGNAME`, `SHELL`, `LANG`, `LC_*`, `TERM`, `TZ`, `XDG_*`, the proxy and certificate variables), `TMPDIR` set to the run folder, the preset's sign-in variables (for `claude`: `CLAUDE_CONFIG_DIR`, `ANTHROPIC_*` keys and URL, Bedrock and Vertex variables, `AWS_*`) and the runner's `env_pass`. Nothing else in the shell that started hippo reaches it.

**The `command` contract.** The user prompt is on standard input, never in an argument. The system prompt goes as an argument (`{system_prompt}`), in a file (`{system_prompt_file}`), or, with neither, at the top of standard input between `<<<SYSTEM>>>` and `<<<END SYSTEM>>>` lines, with those markers in the user text broken (`<<\<SYSTEM>>>`) so a transcript cannot close the system part early. A runner with an inline system prompt cannot be the `review` role. A reply hippo cannot read is a model error naming the runner. A `command` runner is assumed to give the model tools until `hippo doctor --model --canary --runner <name>` shows it cannot read either of two planted files; until then it cannot serve `propose`, `review` or `lessons` unless it sets `allow_tools = true`.

**Changing them.** `hippo runners add`, `hippo runners remove`, `hippo config set role.<role> <runner>`, `hippo config set fallback.<role> 'a,b'` and their `unset` print the change and write it with `--yes` ([commands](commands.md#hippo-runners)). A mistake in these tables does not stop `config.toml` from loading: hooks and `context` keep working, and only the commands that call a model fail, naming it. Both the older keys and `[runners]` at once is such a mistake; `hippo runners migrate` fixes it, and `hippo runners add` migrates first.

#### Other CLIs: candidate recipes, not yet checked

These `command` runners were written from each CLI's `--help` on 20 November 2026. None has been run against a model by hippo's maintainers; run `hippo doctor --model --canary --runner <name>` with a throwaway account before you rely on one.

```toml
# Codex 0.170: the reply is the last message, in a file. It has no real
# system prompt flag here, so it cannot be the reviewer.
[runners.codex]
preset = "command"
argv = ["codex", "exec", "-", "--ephemeral", "--sandbox", "read-only", "--skip-git-repo-check", "--ignore-user-config", "--ignore-rules", "-C", "{run_dir}", "-o", "{reply_file}"]
reply = "file"
data_domain = "openai"
env_pass = ["CODEX_HOME", "OPENAI_API_KEY"]

# Muse Code 1.4: prompt from a file in the run folder is not possible
# (hippo writes only the system prompt there), so the prompt stays on stdin.
[runners.muse]
preset = "command"
argv = ["muse", "exec", "--disable-shell", "--disable-write", "--disable-web-tools", "--approval-mode", "never", "--no-session-log"]
reply = "stdout"
data_domain = "muse"

# OpenCode 2.3: a private server, plain output.
[runners.opencode]
preset = "command"
argv = ["opencode", "run", "--standalone"]
reply = "stdout"
data_domain = "opencode"
```

DeepSeek Harness (`dsh --profile headless`) and Command Code (`cmd -p`) have no recipe yet: whether they read the task from standard input is unchecked, and hippo never puts the prompt in an argument.

## `<workspace>/.hippo/hippo.toml`

Paths in this file are relative to the workspace, must not be absolute and must not contain `..`.

| Key | Type | Default | Meaning |
|---|---|---|---|
| `notes_dir` | path | none; required | The directory of Markdown notes. |
| `entry_note` | path | none; required | The note whose opening section, the text before its first `## ` heading, starts every context block. |
| `required_notes` | list of paths | `[]` | Notes every session must read. The context block lists each as a `required: <path>` line. hippo prints the path, not the note's text. |
| `budget_bytes` | integer | `25600` | The hard limit on the context block, in bytes. |
| `log_path` | path | `<notes_dir>/Session log.md` | Where `hippo log` writes the session log. Must stay inside the workspace; hippo checks this when it writes. |
| `sample_tasks` | list of strings | `[]` | Tasks `hippo doctor` compiles a context block for, to check that each stays within `budget_bytes`. |
| `workspace_id` | string (a ULID) | written by `hippo init` | Names this workspace's file of accepted settings on each machine (`$HIPPO_HOME/accepted/<id>.toml`). Written once; do not change it. `hippo config set` of a security setting adds it to an older workspace. |
| `guard_note` | path | `<notes_dir>/Guard.md` | The note whose fenced `hippo-guard` blocks hold the team's [guard](security.md#the-guard) rules. A change waits for `hippo accept` on each machine. |
| `[guard] fail` | `"open"` or `"closed"` | `"open"` | What a call does when the guard cannot answer: proceeds (and is logged), or is refused. `closed` applies at once; back to `open` waits for `hippo accept`. Run `hippo guard install --yes` after a change so Claude Code's hook entry matches. |
| `[guard] timeout_ms` | integer, 1 to 60000 | `2000` | How long the guard may take before `fail` applies. Any change waits for `hippo accept`. |
| `[team] inbox` | `"local"` or `"shared"` | `"local"` | Whether lessons are shared through `.hippo/inbox/` ([Sharing](sharing.md#the-shared-inbox)). `shared` waits for `hippo accept` on each machine, and each person still decides what of theirs is written (`share_lessons`). |
| `min_hippo_version` | string `X.Y.Z` | unset | The oldest hippo that may write a note or a shared record in this workspace, or call a model for it. An older hippo refuses `dream`, `lessons` (scan, keep, dismiss), `add`, `packs update` and `remove`, `review`, `revert` and `ask` (unless `--sources-only`), naming the version it needs; hooks, `context` and `brief` keep working, so a teammate's agents are not stranded. Read from hippo 0.5.0. |

**Security settings.** `guard_note`, `[guard]`, `[team]` and `min_hippo_version` follow the [accept rule](security.md#the-accept-rule): a change that makes hippo stricter applies at once, one that makes it looser waits until each person accepts it on their machine. `hippo config set guard.fail closed`, `guard.timeout_ms`, `guard_note`, `team.inbox` and `min_hippo_version` edit them in place; setting one also raises `min_hippo_version` to 0.6.0, the first release that understands them.

A key or table this hippo does not know (one a newer release added) does not stop the file from loading. Neither does a key it does not know inside a table it does (`[team] dreamer`, which a later release adds). It is a `hippo doctor` problem, and the commands above refuse while it is there, as they do below `min_hippo_version`. Set `min_hippo_version` to the release that introduced a setting before relying on it in a team.

`hippo init` writes `log_path` out in full as `"<notes_dir>/Session log.md"` using the directory you gave.

## `$HIPPO_HOME/guard/machine.toml`

Your own guard rules, for every session on this machine, in the format of a `hippo-guard` block. `hippo guard allow host|origin <value>` adds an allow entry here (or to the team's note with `--note`); like every loosening it waits for `hippo accept`.

```toml
[allow]
hosts = ["raw.githubusercontent.com/Homebrew/", "github.com/example-org/"]
clone_origins = ["github.com/example-org/"]

[[rule]]
kind = "credential-read"        # remote-script, fresh-clone-exec, credential-read, guard-bypass
action = "ask"                  # warn, ask, block (all warn in 0.6.0)

[[rule]]
name = "prod deploys need a tag"
match = 'kubectl .*--context[= ]prod'   # a regular expression over a shell command
action = "block"
reason = "Production deploys go through CI with a release tag."

[[rule]]
name = "no edits to the lockfile"
path = "**/package-lock.json"           # a glob over a file tool's paths
action = "warn"
```

A rule has exactly one of `kind`, `match` or `path`. A block or rule hippo cannot use (a bad expression, an unknown kind or key) is left out, and `hippo doctor`'s `guard` row names it.

## The `.hippo/` directory

| Path | Written by | Notes |
|---|---|---|
| `hippo.toml` | `hippo init` | Settings above. |
| `.gitignore` | `hippo init` | Contains `snapshots/`. |
| `packs/core/` | `hippo init` | The [core pack](concepts.md#the-core-pack). Yours to edit. |
| `sessions/<id>.json` | `hippo session`, `hippo hook`, `hippo ingest` | One record per session. |
| `proposals/<id>.json` | `hippo dream` | Immutable, kept even when rejected. |
| `receipts/<id>.json` | `hippo dream`, `hippo revert` | Review and revert receipts. |
| `reports/<date>-dream.md` | `hippo dream` | One per UTC day. |
| `dream-state.json` | `hippo dream` | `{"protocol": "HIPPO_V1", "last_success": "<time>"}`. |
| `snapshots/` | `hippo dream` | Pre-change copies for `hippo revert`. Gitignored. |
| `inbox/L-<id>/` | `hippo lessons` with a shared Inbox | `lesson-<member>.json`, `seen-<member>.json`, `decision-<member>-<ulid>.json`, `same-as-<other>-<member>.json`. Committed. |

Every JSON record carries `"protocol": "HIPPO_V1"`. A record without it, or with another value, is rejected when read.
