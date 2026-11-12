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
| `transcript_roots` | list of glob patterns | `["~/.claude*/projects", "~/.codex/sessions"]` | Where transcripts live. hippo walks every root. A `*.jsonl` file whose name starts with `rollout-` is read as a Codex rollout; any other `*.jsonl` file is read as a Claude Code transcript. |
| `model_command` | list of strings | the `claude` command below | The command hippo runs for dreaming and for the reviewer. Each element `{system_prompt}` is replaced by the system prompt. The user prompt is written to the command's standard input. |
| `ingest_interval_minutes` | integer, at least 1 | `10` | Minutes between scheduled ingests. Read by `hippo schedule install`, and by `hippo doctor` to decide when the index is stale (older than three intervals). |
| `dream_at` | string `HH:MM` | `"03:30"` | The time of day the scheduled dreaming run starts, on a 24-hour clock. Read by `hippo schedule install`. |
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

hippo runs the command in `$HIPPO_HOME/model-runs/`, which it empties before every run.

Set it with `hippo config set model_command '<JSON array>' --yes`, for example `'["my-model","--json"]'`, or with words separated by spaces. hippo writes the key as a one-line array and replaces a multi-line array whole. `hippo config unset model_command --yes` restores the default.

When the command exits non-zero, hippo shows its reason: the `result` of the JSON it printed (such as `Not logged in · Please run /login`), else the first non-empty line of stderr, else of stdout. When the program is not on `PATH`, the error says so.

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

`hippo init` writes `log_path` out in full as `"<notes_dir>/Session log.md"` using the directory you gave.

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

Every JSON record carries `"protocol": "HIPPO_V1"`. A record without it, or with another value, is rejected when read.
