# Changelog

All notable changes to this project are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- **The guard, end to end in real agents.** `scripts/agent-e2e/run.sh` runs Claude Code, Codex, Gemini CLI, OpenCode and dsh for real, each in a scratch home with the guard installed as a user installs it, against a stub model on 127.0.0.1 (no model, key or account). It checks that a warned call runs and who saw the warning, that a self-protection refusal holds, that an `ls` is untouched, and that the agent connected only to 127.0.0.1. CI runs the stub's own test and skips the agents it lacks.

- **The guard in Codex.** `hippo guard install --agent codex` adds a `PreToolUse` entry to `$CODEX_HOME/hooks.json`, beside the session hooks; Codex asks you to trust it at its next start. A warning reaches the model as a developer message and you in Codex's TUI; self-protection refuses with the reason. Codex cannot ask, so a question there will be a refusal. Contract read from Codex's source at 0.170.0 and checked end to end; see the [design](docs/designs/guard.md#codexs-pretooluse-contract).

- **The guard in Cursor.** `hippo guard install --agent cursor` adds a `preToolUse` script to `~/.cursor/hooks.json`. A warning reaches the model as `additional_context` and leaves Cursor's own approval as it was; self-protection denies. cursor-agent also runs Claude Code's `~/.claude/settings.json` hooks, so a Cursor call that reaches the Claude Code entry is answered in Cursor's format, and not at all when Cursor's own entry is installed. Read from the code cursor-agent 2026.11.19 ships; there is no end-to-end run, since cursor-agent cannot use a local model. See the [design](docs/designs/guard.md#cursors-pretooluse-contract).

- **Hippo.app: the team's shared Inbox.** With `[team] inbox = "shared"` in force, the Inbox lists the team's open and crossed lessons first, with who saw each and who kept or dismissed it beside who pushed the file. Keep and Dismiss write your decision; Share… shows the same consent as `hippo lessons share` and writes `member` and `share_lessons` on yes. `app/scripts/demo-home.sh` seeds three synthetic teammates.

### Fixed

- **The guard on a machine where hippo was never set up.** `hippo guard install` works without an index or workspace, but `guard check` then failed open on every call for want of `config.toml`. It now applies the built-in and machine rules.
- **OpenCode: warnings reach the model and you.** The plugin's guard put a warning only in OpenCode's log. It now puts it at the top of the call's result (`execute.after`), which the model reads and the tool's output shows. The hooks were checked against OpenCode's source at v2.3.2 and a running OpenCode; they are no longer marked unverified.

## [0.6.0] - 2026-11-20

**Upgrading.** Everyone on a shared workspace should be on 0.6.0 before anyone sets `[guard]`, `guard_note` or `[team]`: `hippo config set` of one raises `min_hippo_version` to 0.6.0, and a 0.5 hippo then refuses to write there. `hippo init` now writes a `workspace_id` into a new workspace's `hippo.toml`; an older workspace gets one the first time a security setting is set, and nothing else changes for it. Kept lesson bullets now end with a hidden marker; `hippo lessons markers --yes` adds it to bullets kept before. The guard is off until you run `hippo guard install --yes` (or set the dsh and OpenCode plugins' `guard: true`).

### Added

- **The guard, before the fact, warn-only.** `hippo guard check` runs in each agent's pre-tool hook, machine-wide: `hippo guard install` lists every Claude Code account's user `settings.json` and adds a `PreToolUse` entry with `--yes` (never a workspace's committed settings), `--agent gemini` adds Gemini CLI's `BeforeTool`, and the dsh and OpenCode plugins gain `guard: true`. It checks the incident kinds before a call runs (`remote-script`, `fresh-clone-exec`, including a repository cloned in the last day and set up in a later call, `credential-read`, `guard-bypass`) and the team's `match` and `path` rules, from four layers: built-ins, `$HIPPO_HOME/guard/machine.toml`, the team's guard note (`<notes_dir>/Guard.md`, fenced `hippo-guard` blocks; for sessions inside the workspace or its worktrees) and packs' `Guard.md`. Every rule warns in this release: the call runs and the agent is told to tell you. `hippo guard test` counts what the rules would have flagged on your indexed history; `guard allow`, `guard status` and `guard uninstall` round it out; `doctor` gains a `guard` row. Measured on a release build: p50 3.4 ms, p95 6.1 ms per check. Codex and Cursor are not covered yet. See the [design](docs/designs/guard.md#part-b-before-the-fact-a-guard-in-the-agent).
- **The accept rule.** A shared security setting that makes hippo stricter applies at once; one that loosens it (an allow entry added, a rule removed or an action lowered, `fail` back to `open`, a new `guard_note` or `timeout_ms`, `[team] inbox = "shared"`, a lower `min_hippo_version`) keeps the accepted value in force until you run `hippo accept` on your machine, or press Accept… in Hippo.app's Manage › Health. `hippo accept` needs a terminal and refuses when an agent's marker is set; there is no `--yes`. `doctor` gains an `accept` row; `hippo config set` edits `guard.fail`, `guard.timeout_ms`, `guard_note`, `team.inbox` and `min_hippo_version`. See [common.md](docs/designs/common.md#rule-3-tightening-takes-effect-at-once-loosening-needs-a-local-accept-on-each-machine).
- **A shared Inbox for teams.** With `[team] inbox = "shared"` accepted, a reviewed correction whose evidence all ran in the workspace goes to `.hippo/inbox/L-<id>/` once its person said yes (`hippo lessons share`, at a terminal; `member` and `share_lessons` in `config.toml`): their wording, match terms and counts, one file per person, no quotes and no session ids. Scans skip drafting what the team already has, so one entry is "seen by alice 3, bob 1". `hippo lessons keep L-…` and `dismiss L-… --reason …` write decisions as files; a lesson is kept if anyone kept it, and a keep against a dismissal is shown as crossed. `doctor` gains a `team` row. See [Sharing a brain in a team](docs/site/src/sharing.md#the-shared-inbox) and the [design](docs/designs/team-brain.md).
- **Lesson markers.** A kept bullet ends with `<!-- hippo:lesson L-… -->`, hidden by Obsidian and GitHub; keep refuses when the marker is already in a note. `hippo lessons markers` adds it to bullets kept before, with receipts.

### Security

- **Self-protection.** Whether or not anything else warns, the guard refuses an agent's `hippo accept`, `guard accept`, `guard allow`, `guard uninstall`, `hooks uninstall`, `lessons share`, and `hippo config set` of a security key or `share_lessons`, and an agent's write under `$HIPPO_HOME/guard/`, `$HIPPO_HOME/accepted/` or to `$HIPPO_HOME/config.toml`.
- **`fail` behaves as set.** If hippo is missing, crashes or does not answer within `[guard] timeout_ms` (2,000), the call proceeds and the failure is logged; with `fail = "closed"` it is refused (Claude Code's entry carries `onFailure: "block"`).
- Dreaming's validator and `hippo lessons keep` refuse the guard note.

### Changed

- `sharing.md` no longer calls `git add -A` safe once the shared Inbox is on: read `git status` first.
- A key `hippo.toml` does not know inside a table it does (`[team] dreamer`) counts as unknown, like an unknown table.

## [0.5.0] - 2026-11-19

**Upgrading.** Everyone on a shared workspace should be on 0.5.0 before anyone turns on a team, guard or secrets setting: older hippo silently ignores settings it does not know, and only 0.5.0 and later refuse to write under a `min_hippo_version` or unknown keys. On your machine: run `hippo hooks install --yes` once in each workspace to move hippo's Claude Code hooks from the shared `.claude/settings.json` to your own `.claude/settings.local.json` (until then they keep working where they are, and `hippo doctor` warns), and `hippo doctor --fix-permissions` once to close files an older hippo left readable by others. The rollup is rebuilt at the next ingest. `hippo ingest --full` counts secrets in sessions ingested before this release.

### Added

- **Runners: which CLI, model, effort and account hippo's own calls use.** `[runners.<name>]` in `config.toml` names a `claude` account (as `CLAUDE_CONFIG_DIR`), model and effort, or any CLI by its argv through the `command` contract (prompt on stdin; the system prompt as an argument, a file or inline; the reply as JSON, stdout or a file). `[roles]` picks a runner for `propose`, `review`, `lessons` and `ask`; `[fallback]` moves a role to another runner in the same data domain when its account is at 95% of a window or answers with its usage-limit message, so the nightly dream no longer fails when one account is out. Every runner runs in an OS sandbox (`sandbox-exec` on macOS, `bwrap` on Linux) with a limited environment. `hippo runners` lists them; `hippo runners add|remove|migrate` and `hippo config set role.<role>|fallback.<role>` change them; `hippo doctor --model` checks each runner, and `--canary` checks it cannot read two planted files. Receipts and lesson reviews record the runner. Manage in Hippo.app has a Models card that switches the default runner. With nothing configured the call is today's, argument for argument, now in the sandbox. Recipes for Codex, Muse and OpenCode are documented, unchecked. See the [design](docs/designs/runners.md).

### Security

- **Hooks run hippo by its absolute path, from per-person files.** `hippo hooks install` writes Claude Code's hooks to the workspace's `.claude/settings.local.json`, not the `.claude/settings.json` a team commits, and every hook (Claude Code, Codex, Cursor, Gemini CLI) runs the binary by its absolute path, so another program named `hippo` earlier on `PATH` never sees a session. Hooks an older hippo put in `.claude/settings.json` are moved: `hippo hooks install` says what it will move and moves hippo's entries only with `--yes`; it also rewrites a bare `hippo` in the other agents' hooks files. `hippo init`, `hippo hooks install` and `install.sh` write the binary's path to `$HIPPO_HOME/bin-path`, which the Claude Code, dsh and OpenCode plugins run before they look on `PATH`.
- **`$HIPPO_HOME` is private.** hippo runs with a umask of 077: folders it creates are 0700 and files 0600, the index included, and an atomic write never widens a file's mode. A command that finds `$HIPPO_HOME` open to other accounts closes it and says so once. `hippo doctor` lists anything inside still readable by others, and `hippo doctor --fix-permissions` tightens it.
- **`install.sh` records what it checked.** After checking the release against its published SHA-256 it records the binary's path and checksum in `$HIPPO_HOME/install.json`; `hippo doctor` reports a problem when that binary changed, and a warning when hooks, MCP entries or `bin-path` run another binary.
- **A version floor for shared workspaces.** `.hippo/hippo.toml` can set `min_hippo_version`. An older hippo, and a hippo that finds keys it does not know, refuse to write notes or records there or to call a model for it (`dream`, `lessons`, `add`, `packs update` and `remove`, `review`, `revert`, `ask` without `--sources-only`); hooks, `context` and `brief` keep working. Proposals, receipts, review marks, pack reviews and lessons carry `hippo_version`.

- **The guard's incidents, after the fact.** Four new incident kinds, read from every agent's transcripts with no model call: `remote-script` (a download piped into an interpreter, a non-Homebrew tap, a global install from a URL; the installers of Homebrew, rustup, uv, Bun, Deno, nvm, Claude Code and hippo do not count), `fresh-clone-exec` (a repository cloned in the session, then its install or build run inside it), `credential-read` (SSH private keys, cloud and registry credentials, keychain passwords, browser profiles, wallets; the path, never the contents) and `guard-bypass` (hippo's hooks or state changed by an agent). They show in `hippo incidents` (`--kind` filters), Hippo.app's Incidents and Fleet's marks. Nothing is blocked. Measured on a labeled synthetic set and the demo history; the numbers and their limits are in the [design](docs/designs/guard.md#part-a-after-the-fact-from-transcripts). The rollup is rebuilt once at the next ingest to find them in past sessions.

- **Secrets that reached a model.** Ingest now records a row for every match redaction replaces (the session, the turn, its role, the kind, the call whose result held it; never the value) in `index.sqlite`. A new incident kind, `secret-to-model`, lists the private keys, cloud and GitHub tokens and vendor keys (high confidence: rotate) and the authorization headers and JWTs (medium, with `--all`) that sat in a turn the agent's model saw, with where each came from. Emails, phone numbers, Aadhaar numbers and `key=value` matches never count, nor do published example keys and Stripe test keys. Rows exist only for text ingested from this release on: `hippo incidents` says since when, and `hippo ingest --full` counts older sessions.

### Changed

- A configuration that keeps `model_command` or `reviewer_command_override` works as before (the whole environment, no sandbox); `hippo runners migrate` moves it to runners. Both the older keys and `[runners]` at once no longer load silently wrong: model calls fail and name `hippo runners migrate`, while hooks and `context` keep working.
- A reviewer's notes and a proposal's reason are redacted and cut to 2,000 characters before they are written.
- The pack review runs in `model-runs/<ulid>/` like every other call; `review-runs/` is no longer used.

### Fixed

- Two model calls at once (a nightly dream and an `ask`) could delete each other's files: every call emptied the one `model-runs/` folder. Each call now runs in its own `model-runs/<ulid>/` (0700), removed when it ends; leftovers older than a day are swept.

## [0.4.2] - 2026-11-19

### Added

- **hippo in any MCP client.** `hippo mcp` is an MCP server over stdio. Any agent that speaks MCP (Cursor, Windsurf, Zed, VS Code's Copilot agent, Gemini CLI, Claude Desktop, Claude Code) can call `hippo_context` (the start block for a task), `hippo_recall`, `hippo_ask` (ranked sources, no model call), `hippo_day` and `hippo_incidents`. Each returns the text of the matching command, redacted before it leaves. `--allow-write` adds `hippo_note_correction`, which proposes a correction as a lesson; the lesson reviewer checks it, and it waits in the Inbox and never reaches a note on its own. `hippo mcp install --client <client>` prints the client's entry, and `--write` merges it into the client's file after a backup. The paths and formats, with their sources, are in [docs/mcp.md](docs/mcp.md). See the [design](docs/designs/mcp-server.md).
- **Cursor.** hippo reads Cursor's agent transcripts from `~/.cursor/projects`, now one of the default `transcript_roots`. That covers the editor's agent and `cursor-agent`, with sub-agents. Each turn takes the time of the prompt before it, and a session's folder is found by reading the project folder's name back against the disk. `hippo hooks install --agent cursor` adds a `sessionStart` hook that gives a new chat in the workspace the start block. A configuration that lists its own roots adds the folder with `hippo start --add-root ~/.cursor/projects`. See the [design](docs/designs/cursor-adapter.md).
- **Gemini CLI.** hippo reads Gemini CLI's saved sessions from `~/.gemini/tmp`, now one of the default `transcript_roots`. Both the older JSON documents and the newer JSON Lines files are read, with tool calls and their results and with sub-agents. `hippo hooks install --agent gemini` adds `SessionStart`, `BeforeAgent` and `SessionEnd` hooks to `~/.gemini/settings.json`, which work as Codex's do. The format comes from Gemini CLI's documentation and source and has not yet been checked against a real session. A configuration that lists its own roots adds the folder with `hippo start --add-root ~/.gemini/tmp`. See the [design](docs/designs/gemini-adapter.md).
- **The instrument look.** Hippo.app's built-in themes are neutral black and white, with colour only for a state; numbers, times and labels are SF Mono; areas are divided by hairlines, not cards; and each state has one square mark instead of an icon. The sidebar is navigation only (only Inbox shows a count); today's sessions are in Fleet, whose Rows now offer Settle and Snooze under the pointer. Home is one column of readouts, the scope band of the fleet's tool calls, what waits on you and last night's pass, with the prompt at the foot; its headroom and memory cards and the overnight line are gone. *Working* counts the sessions working now everywhere, the menu-bar item included. See the [design](docs/designs/app-instrument.md).
- **Fleet Pulse.** Hippo.app's Fleet opens on a new view: a header of how many sessions work and wait on you and each account's 5-hour meter (red, with a countdown, when it will run dry before its reset) over the scope band of the fleet's tool calls; a queue of the sessions that need you, then a ranked Next up (an account that runs dry, an unverified claim, a destructive command, a correction that is a rule candidate); and the selected item alone in a focus pane: a waiting session's question or plan steps with Open thread, T3 Code and Terminal, a table of what it is on and its last calls, an account's drain with the sessions that will stop and *Resume them on* another, or the one fact and action of the others. The working sessions, each with its current call and last 30 minutes, sit in a sheet (`F`); with nothing in the queue, Pulse shows the day's readouts and scope band. Rows and Lanes stay a click away. The engine gained what it reads, all computed at read time: 2-minute activity beats, usage drain and burn rate, the kept rules a session started with (`hippo fleet --json` shows them), correction and destructive marks, the nightly forecast, the ranking and a waiting plan's steps. See the [design](docs/designs/fleet-pulse.md).

### Fixed

- The day's incidents went missing from Lanes east of UTC: incident times were compared as text against a local midnight.
- A plan waiting for approval now says its last line in Fleet and the sidebar.

## [0.4.1] - 2026-11-12

### Added

- **More of OpenCode.** How a run ended: an assistant step's error and a run's `idle` outcome become the turns hippo already reads for Claude Code (`API Error: …`, `Failed to authenticate: …`, `[Request interrupted by user]`), so Fleet shows an OpenCode session that stopped on an error, a limit, a failed sign-in or a stop, and Incidents lists the errors. Attachments: a line per attached file or tool file with its name, type, size and path, never the bytes. A finished sub-agent call's result carries the sub-agent's tokens and tool calls, which Fleet shows as it does Claude Code's. A completed compaction is a summary turn the session view shows as the earlier conversation's summary. Cost is not recorded: hippo has no cost field, and Claude Code and Codex record no price. Every OpenCode session is read once more on the next ingest. See the [design](docs/designs/opencode-adapter.md#added-in-041).

## [0.4.0] - 2026-11-12

### Added

- **Codex hooks.** `hippo hooks install --agent codex` adds three hooks to `$CODEX_HOME/hooks.json` (`~/.codex/hooks.json`), keeping everything else in the file; `uninstall` and `status` take `--agent codex` too. Once you trust them in Codex's hook review, a top-level Codex session in hippo's workspace starts with the start block, its first prompt brings the relevant notes, and hippo records it as a declared session under the id the adapter gives it, `codex-<session id>`, ending with the last assistant message as a derived handoff. Sessions outside the workspace get nothing. `hippo hook session-start|prompt|session-end --agent codex` are the commands the hooks run. Checked against Codex 0.167.2 with `scripts/codex-e2e/`. See the [design](docs/designs/codex-hooks.md).
- **Muse Code.** hippo reads Muse Code's session logs from `~/.local/share/muse/sessions` (or `$XDG_DATA_HOME/muse/sessions`), now one of the default `transcript_roots`: prompts, assistant messages, tool calls and results, with the session's working directory. Muse is closed source, so the format follows logs Muse 1.4.18 wrote; a record in another version is skipped with a warning. A configuration that lists its own roots adds the folder with `hippo start --add-root ~/.local/share/muse/sessions`. See the [design](docs/designs/muse-adapter.md).
- **Command Code.** hippo reads Command Code sessions from `~/.commandcode/projects`, now one of the default `transcript_roots`: version 3 session files, every message in file order (branches included), with prompts, replies, tool calls and results, and the session's working directory from its header. Older versions are reported, not read. Its `taste.md` is not read. A configuration that lists its own roots adds the folder with `hippo start --add-root ~/.commandcode/projects`. See the [design](docs/designs/command-code-adapter.md).

## [0.3.0] - 2026-11-12

### Added

- **OpenCode.** hippo reads OpenCode 2's SQLite database (`~/.local/share/opencode/opencode.db`, or under `$XDG_DATA_HOME`) into the index, with sub-agents under their session and forks without their copied history. The database is opened read-only and read session by session; a session whose change stamp has not moved is not read again. OpenCode's core tools are stored under Claude Code's names, so the x-ray, incidents, the rollup and dreaming read them. `hippo doctor` has an `OpenCode database` row. A configuration that lists its own roots adds it with `hippo start --add-root ~/.local/share/opencode`. See the [design](docs/designs/opencode-adapter.md).
- **An OpenCode plugin**, `plugins/opencode/` (`opencode-plugin-hippo`), for OpenCode 2. A top-level OpenCode session in hippo's workspace gets the start block in its system prompt, and the model can call `hippo_ask` and `hippo_day`; `"tools": false` drops the tools. It only runs the `hippo` binary. Install it by adding `{ "package": "github:VanitasCaesar1/hippo#v0.3.0::path:plugins/opencode" }` to `plugins` in `opencode.json`; it is not on npm. See the [design](docs/designs/opencode-plugin.md) and [README](plugins/opencode/README.md).

## [0.2.1] - 2026-11-12

### Added

- **A DeepSeek Harness (`dsh`) plugin**, `plugins/dsh/` (`dsh-plugin-hippo`). A new dsh session in hippo's workspace starts with the same start block a Claude Code session gets, and the model can call `hippo_ask` (`hippo ask --sources-only`, with each source's session id) and `hippo_day` (`hippo show day`). It only runs the `hippo` binary. Install it with `dsh plugin --profile <profile> add "github:VanitasCaesar1/hippo#v0.2.1&path:/plugins/dsh"`; it is not on npm. See the [design](docs/designs/dsh-plugin.md) and [README](plugins/dsh/README.md).

### Fixed

- `hippo ask` leaves the dsh plugin's `hippo_ask` calls and their results out of its sources, as it does for `hippo ask` run in a shell, and the x-ray lists them as searches.

## [0.2.0] - 2026-11-12

### Added

- **DeepSeek Harness (`dsh`).** hippo reads dsh session logs, compressed (`.zstd`, dsh's default) or not, from `~/.dsh/sessions`, now one of the default `transcript_roots`. dsh sessions appear in the overview, Fleet, Incidents, the x-ray, `recall`, `ask`, `brief`, lessons and dreaming, with sub-agents and forks under the session that started them, and Fleet shows a call waiting for your approval as waiting. Log formats 3 and 4 are read; another format is refused and named in `hippo doctor`'s new `dsh logs` row. A configuration that lists its own roots adds the folder with `hippo start --add-root ~/.dsh/sessions`. See the [design](docs/designs/dsh-adapter.md).
- `hippo session start --tool dsh`.

### Changed

- The default `transcript_roots` add `~/.dsh/sessions`. A `config.toml` that already lists its roots keeps them.
- The Claude Code adapter no longer claims a dsh session log (`session.v<N>.jsonl` in a dsh session folder) as a Claude Code transcript.

## [0.1.0] - 2026-11-12

The first release. hippo reads what your coding agents did, keeps your notes true, and briefs every new session. Everything runs on your machine.

### Added

- **History.** hippo finds Claude Code and Codex transcripts, indexes them with keys, tokens and personal data redacted, and shows the last active day.
- **Search.** `recall`, `ask`, `fleet`, `incidents`, `xray`, `show` and `resume` search and explain that history.
- **Notes.** With a notes folder (`hippo init`), each Claude Code session gets a context block at start, under a hard byte budget, through three hooks (`hippo hooks install`).
- **Dreaming.** A scheduled pass checks notes against transcripts and proposes fixes. A fresh-context reviewer confirms each change before it is written, every change keeps a snapshot, and `hippo revert` undoes it.
- **Packs.** Copy-and-own bundles of passes, prompts, personas and themes, added after a review (`hippo add`, `hippo packs`). Packs never run code. 26 first-party packs are listed in the registry.
- **Eval.** `hippo eval` measures context, recall and dreaming on a small synthetic set.
- **`hippo card`.** Your agents' day, or `--week`, as one 1200 x 675 image and the same lines as text. Numbers and a date only.
- **A workspace that is safe to commit.** `hippo ingest` writes a session record into the workspace only for sessions that belong to it. `hippo init` writes git ignore rules for per-person files and `merge=union` for notes. `hippo doctor` has a `git` row that names tracked per-person files.
- **Hints, not incidents.** A check an agent claimed without a run after its last edit is listed under "Hints to check" and counted nowhere. The rule is not precise enough to count yet.
- **Release binaries** for macOS (arm64, x86_64) and static Linux (x86_64, aarch64), each with a SHA-256 file, on the [GitHub release](https://github.com/VanitasCaesar1/hippo/releases/tag/v0.1.0).
- **`install.sh`**: one line on macOS and Linux. It fetches the binary for the machine, checks its SHA-256 and puts it in `~/.local/bin` without `sudo`. `cargo install --locked --git https://github.com/VanitasCaesar1/hippo hippo` also works.
- **A Homebrew formula**, `hippo.rb`, attached to the release. There is no tap yet.
- **A Claude Code plugin** from this repository's marketplace: the start block at session start, and `/hippo:setup`, `/hippo:ask` and `/hippo:day`.
- **Hippo.app**, a macOS window over the same files. Built from source in `app/`; no binary is released. Checked on synthetic data, not in daily use.
- **Docs** at [vanitascaesar1.github.io/hippo](https://vanitascaesar1.github.io/hippo/), including [Sharing a brain in a team](docs/site/src/sharing.md), tried with two people and one git remote.

[Unreleased]: https://github.com/VanitasCaesar1/hippo/compare/v0.6.0...HEAD
[0.6.0]: https://github.com/VanitasCaesar1/hippo/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/VanitasCaesar1/hippo/compare/v0.4.2...v0.5.0
[0.4.2]: https://github.com/VanitasCaesar1/hippo/compare/v0.4.1...v0.4.2
[0.4.1]: https://github.com/VanitasCaesar1/hippo/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/VanitasCaesar1/hippo/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/VanitasCaesar1/hippo/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/VanitasCaesar1/hippo/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/VanitasCaesar1/hippo/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/VanitasCaesar1/hippo/releases/tag/v0.1.0
