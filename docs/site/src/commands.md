# Commands

`hippo` has 28 commands, in the groups `hippo --help` shows. Run `hippo <command> --help` for the same flags in short form.

Run `hippo` with no arguments first. With no `config.toml` it prints what `hippo detect` finds and offers to index that history (history only, no notes folder, nothing sent anywhere). On a terminal it asks `Index it now? [Y/n]`; with no terminal it prints `run hippo start --yes to index it` and exits 0. Once hippo is set up, `hippo` on its own runs an incremental ingest when the last one is older than `ingest_interval_minutes`, then shows the last active day: its sessions, merged pull requests, corrections, and four commands to try next.

Commands read `$HIPPO_HOME/config.toml` (by default `~/.hippo/config.toml`) and work on the one workspace named there, never on the current directory. Before setup they print `hippo is not set up yet` and exit 1, naming `hippo` as the way to start. The exceptions are `detect`, `start`, `eval` without `--with-model`, `packs check`, `packs test`, `schedule uninstall` and `schedule status`, which read no configuration (`init` writes it). A history-only setup has no workspace; commands that need notes say so.

Unless a command says otherwise, an error prints to stderr and the exit code is 1. Set `RUST_LOG` (for example `RUST_LOG=debug`) to change the log level; the default is `warn`, and logs go to stderr.

**Start here**

| Command | What it does |
|---|---|
| `hippo` | On its own: find your agents' history and index it; once set up, show the last active day |
| [`start`](#hippo-start) | Find your agents' history and index it (history only) |
| [`detect`](#hippo-detect) | List the agent history on this machine, before anything is indexed |

**Every day**

| Command | What it does |
|---|---|
| [`ask`](#hippo-ask) | Has any agent already solved this? Ranked sources, and an answer that cites them |
| [`recall`](#hippo-recall) | Search every indexed turn |
| [`fleet`](#hippo-fleet) | Today's sessions and headroom per profile |
| [`incidents`](#hippo-incidents) | What went wrong, which agent did it, at which step |
| [`xray`](#hippo-xray) | What context a session was given and used |
| [`show`](#hippo-show) | Print a session, a day, a proposal or a note's receipts |
| [`resume`](#hippo-resume) | Print the command that resumes a session, with a short handoff |
| [`brief`](#hippo-brief) | The start block for a new agent on a task |
| [`card`](#hippo-card) | The day as one image to share: numbers and a date, no names |

**Notes and dreaming**

| Command | What it does |
|---|---|
| [`context`](#hippo-context) | Print the context block |
| [`dream`](#hippo-dream) | Run the dreaming passes once |
| [`lessons`](#hippo-lessons) | Corrections and repeated failures proposed as rules |
| [`review`](#hippo-review) | Mark applied dreaming changes as read and right |
| [`revert`](#hippo-revert) | Undo an applied change |
| [`eval`](#hippo-eval) | Measure on an eval set |

**Packs**

| Command | What it does |
|---|---|
| [`add`](#hippo-add) | Add a pack from a path or git, after a review |
| [`packs`](#hippo-packs) | Check, test, list, diff, update and remove packs |

**Setup and checks**

| Command | What it does |
|---|---|
| [`init`](#hippo-init) | Write `config.toml` and the workspace's `.hippo/` |
| [`ingest`](#hippo-ingest) | Index the transcripts |
| [`config`](#hippo-config) | Show or change one setting |
| [`doctor`](#hippo-doctor) | Check the setup |
| [`schedule`](#hippo-schedule) | Schedule ingest and dreaming |
| [`hooks`](#hippo-hooks) | Install or check the three Claude Code hooks |

**Run by hippo itself.** Hooks and jobs call these. You rarely type them.

| Command | What it does |
|---|---|
| [`hook`](#hippo-hook) | Run as a Claude Code hook |
| [`session`](#hippo-session) | Start or end a declared session |
| [`log`](#hippo-log) | Write the session log |

## hippo start

```text
hippo start [--yes] [--add-root <path>]...
```

Finds your agents' history and indexes it, history only: no notes folder is needed and nothing is sent anywhere. It prints what it found, asks `Index it now? [Y/n]` on a terminal, writes a history-only `config.toml`, ingests with progress, then shows the last active day. It never adopts a workspace from the current directory. When hippo is already set up it says so and shows the day.

| Flag | Meaning |
|---|---|
| `--yes` | Index the history without asking. |
| `--add-root <path>` | Another folder of transcripts to read, absolute or under `~/`. Repeatable. |

With no terminal and no `--yes` it prints `run hippo start --yes to index it` and exits 0. When it finds no history it names the folders it looked in and exits 0.

## hippo detect

```text
hippo detect [--json]
```

Lists the agent history on this machine without indexing anything and without a configuration: each folder with its files (for OpenCode, the sessions in its database), size and newest day. `--json` prints one JSON object. hippo reads Claude Code and Codex transcripts, DeepSeek Harness (dsh) session logs and OpenCode's database. It also reports Cursor and Gemini folders when it sees them, as agents with no adapter: hippo does not read them.

## hippo ask

```text
hippo ask <question> [--sources-only] [--limit <n>] [--json]
```

Has any agent already solved this? It ranks the indexed sessions that match the question, then asks the model for an answer that cites them.

| Flag | Meaning |
|---|---|
| `--sources-only` | Skip the model: print the ranked sources only. Local. |
| `--limit <n>` | The most sources to list. Default 6. |
| `--json` | Print the result as one JSON object. |

Without `--sources-only` it runs your `model_command` ([Security and privacy](security.md#when-data-leaves-your-machine)).

## hippo fleet

```text
hippo fleet [--json]
```

Today's sessions: live, stopped at a usage limit, or finished, and the headroom left on each Claude profile. With no session today it says so and names `hippo show day <date>`. `--json` prints one JSON object.

## hippo incidents

```text
hippo incidents [--days <n>] [--all] [--json]
```

What went wrong, which agent did it, at which step, and why when a rule says so.

A check an agent said passed with no run of it after its last edit is listed apart, after the incidents, under `Hints to check: a check claimed with no run after the last edit`. It is a hint, not an incident: the rule is not yet precise enough to count, so these rows are not counted anywhere. The check counts a run that came through a wrapper script, a remote sandbox, a background task, CI on the pushed head, or a session the agent delegated to; `hippo xray <session>` shows the evidence it found. `--json` lists them with the incidents, kind `unverified`.

| Flag | Meaning |
|---|---|
| `--days <n>` | Days to cover, 1 to 14. Default 7. |
| `--all` | Also list destructive commands that nothing harmful followed. |
| `--json` | Print one JSON array. |

## hippo xray

```text
hippo xray <session> [--json]
hippo xray --fleet [--days <n>] [--json]
hippo xray unrequire <note> [--yes]
```

Shows what context a session was given and used, what it searched for, and the evidence for its claims. `<session>` is a full id or a unique prefix, as `hippo fleet` prints it.

| Flag | Meaning |
|---|---|
| `--fleet` | The fleet's use of its context over the last days, with tips to tune it. |
| `--days <n>` | Days the fleet stats cover, 1 to 30. Default 7. |
| `--json` | Print one JSON object. |

`xray unrequire` removes a note from `required_notes` in `.hippo/hippo.toml`. It prints the change and writes it only with `--yes`.

## hippo resume

```text
hippo resume <id> [--on <profile>] [--json]
```

Prints the command that resumes a session, with a short handoff. `<id>` is a full id or a unique prefix of any indexed top-level session, not only today's.

| Flag | Meaning |
|---|---|
| `--on <profile>` | Resume on this Claude profile instead (`default`, `work`, ...). The command copies the transcript there first, never over a file. |
| `--json` | Print one JSON object. |

## hippo brief

```text
hippo brief --task <text> [--off <id>]... [--json]
```

The start block for a new agent on a task: required and relevant notes, past sessions and kept rules. `--off <id>` leaves out the part with that id (repeatable; `--json` lists the ids). `--json` prints the parts and the block as one JSON object.

## hippo card

```text
hippo card [--day <YYYY-MM-DD>] [--week] [--out <path>]
```

Your agents' day as one 1200 x 675 PNG to share, and the same words on stdout. It shows the work time the overview shows, the threads and sub-agents, the pull requests merged, how long the agents waited on you and over how many questions, and how many times you corrected them. Nothing else: no project, session title, path, repository, account or profile name is ever drawn or printed, only numbers and a date. It reads the index and the rollup, calls no model and sends nothing anywhere.

| Flag | Meaning |
|---|---|
| `--day <date>` | The day, in local time. Default: the day the overview shows (today, or the last day with a session). |
| `--week` | The seven days that end on that day. Work time, wait and questions are summed day by day; threads, sub-agents, pull requests and corrections are counted once over the seven days. |
| `--out <path>` | Where to write the PNG. Default: `$HIPPO_HOME/cards/card-<date>.png` (`card-week-<date>.png`), or the system temporary folder when that folder is inside a git repository. |

A day with no session prints the last active day and exits 1. The card is drawn with an embedded subset of DejaVu Sans, so it looks the same on every machine; the font's licence is `LICENSE-DejaVu.txt` beside the binary in a release.

## hippo init

```text
hippo init [--workspace <path>] [--notes-dir <dir>] [--entry-note <file>] [--add-root <path>]... [--force]
```

Writes `$HIPPO_HOME/config.toml` and, if it does not exist, the workspace's `.hippo/`. With no flags and outside a workspace it writes a history-only configuration, the same one `hippo start` writes.

| Flag | Meaning |
|---|---|
| `--workspace <path>` | The workspace. Default: the current directory when it holds `.hippo/` or the notes flags are given. It must be a directory. hippo stores its canonical path. |
| `--notes-dir <dir>` | The notes directory, relative to the workspace, without `..`. Needed to create `.hippo/`. |
| `--entry-note <file>` | The entry note, relative to the workspace, without `..`. Needed to create `.hippo/`. |
| `--add-root <path>` | Another folder of transcripts to read, absolute or under `~/`. Repeatable. |
| `--force` | Overwrite `config.toml` and the files `init` writes under `.hippo/` (including the core pack). |

`--notes-dir` and `--entry-note` must be given together. Without `--force`, `init` refuses (exit 1, naming the path) if `config.toml` exists, or if you give the two flags for a workspace that already has a `.hippo/`. It checks everything before it writes anything. It prints `wrote <path>` or `kept <path>` for each path. If `.hippo/` exists and you give neither flag, it keeps `.hippo/` and writes only `config.toml`. In a workspace that is already set up (its `config.toml` names this workspace), `init` with no flags does not refuse: it only adds the git rules below and says so.

Whenever it writes or keeps a workspace, `init` adds the lines that are missing (and only those) to `.hippo/.gitignore` (`snapshots/`, `sessions/`, `reports/`, `dream-state.json`), the workspace's `.gitignore` (`/<log_path>`) and its `.gitattributes` (`/<notes_dir>/**/*.md merge=union`), printing `updated <path>` for a file that gained lines. See [Sharing a brain in a team](sharing.md).

If the notes folder does not exist, `init` creates it (`created <dir>/ (it did not exist)`). If the entry note does not exist, it writes a starter entry note you can edit (`wrote <note> (a starter entry note: edit it)`). It never overwrites a file. When you add a workspace to a history-only setup whose index already holds sessions, it prints `<n> sessions are indexed (sub-agents included): run hippo ingest --full to give the ones that belong to this workspace a session record`.

## hippo add

```text
hippo add <source> [--registry <index.json>] [--dry-run]
```

Copies a pack into `.hippo/packs/<name>/` after checking it and having it reviewed in a fresh context. `<source>` is a path (`./dir`, `../dir`, `/dir`, `~/dir`) or an https git URL, `https://host/repo[//dir][@ref]`, or the name of a pack listed in a registry index.

| Flag | Meaning |
|---|---|
| `--registry <index.json>` | The `index.json` that `hippo-registry build` wrote. Required for a bare name; `hippo packs diff` and `hippo packs update` take it too ([The registry](registry.md)). |
| `--dry-run` | Validate and print the summary; write nothing and make no model call. |

It prints the summary (`pack`, `source`, a `pass` line for each pass, a `template` line for each template, `files`), then `review <id> <verdict>: <notes>`, then `wrote .hippo/packs/<name>/`. Exit 1 when the pack does not validate, when its name or a pass name is already used, when the reviewer rejects it, and when the review cannot run. [Packs](packs.md#using-a-pack) shows the output and what each step does.

## hippo ingest

```text
hippo ingest [--full]
```

Walks `transcript_roots`, indexes every transcript whose content changed, and creates a derived session record for every transcript that has no declared one and belongs to the workspace: the session ran inside the workspace's folder (or one of its git worktrees), or read, edited or cited one of its notes. Other projects' sessions are indexed but leave nothing in the workspace. Output: `indexed <n> files, skipped <m>, derived <k> sessions`; when it removed records of other projects' sessions (a workspace written by an older version is cleaned once), a second line says how many and why. If another ingest is running it prints `ingest already running` and exits 0.

| Flag | Meaning |
|---|---|
| `--full` | Build a new index from every transcript and swap it in. Use it to rebuild the index from scratch, or to give already-indexed sessions their records after you add a workspace. |
| `--progress` | Write one JSON line per file to stderr as it is read. |

A transcript that fails to parse is recorded, skipped until it changes, and shown by `hippo doctor`. A line that is not valid JSON is skipped with a warning. Redaction runs before any text is indexed ([Security and privacy](security.md)).

## hippo recall

```text
hippo recall <query> [--limit <n>] [--json] [--raw]
```

Searches every indexed turn: user text, assistant text, tool calls and tool results. Results are ranked by relevance, newer turns first among equals.

| Flag | Meaning |
|---|---|
| `<query>` | A phrase to find. hippo quotes it, so words must appear together in order. |
| `--limit <n>` | The most turns to print. Default 20, at most 200. |
| `--json` | One JSON object per line: `session`, `tool`, `ts`, `role`, `tool_name`, `snippet`, `score`. |
| `--raw` | Pass the query to the index as written, in SQLite FTS5 syntax. A syntax error exits 1 with the index's message. |

Text output is one line per turn: `<session> <tool> <ts> <role> [<tool_name>] <snippet>`. The snippet has line breaks and runs of spaces folded into single spaces. `<tool_name>` appears only for tool turns. With no index, the command fails; run `hippo ingest` first.

## hippo context

```text
hippo context [--task <text>] [--session <id>] [--budget <bytes>] [--parts required|optional|all] [--json]
```

Prints the context block described in [The context budget](concepts.md#the-context-budget).

| Flag | Meaning |
|---|---|
| `--task <text>` | The task. Adds the relevant-notes part. |
| `--session <id>` | A declared or derived session id. Adds the session line. An unknown id exits 1 with `no session <id>`. |
| `--budget <bytes>` | The limit. Default: `budget_bytes` from `.hippo/hippo.toml`. |
| `--parts` | `required`, `optional` or `all` (the default). |
| `--json` | Print one JSON line instead of the block. |

Parts that did not fit go to stderr as `omitted: <part> (<bytes>)`. If the required parts exceed the budget, the command prints `context over budget: <required> > <budget>` and exits 1.

With `--json` the command prints one line with the block and how it was built: `text` (the block), `bytes` (its size), `budget`, `omitted` (a list of `{"part", "bytes"}` for each part that did not fit, which `--json` does not also print to stderr), `required` (the notes the block's `required:` part names, as paths relative to the workspace) and `relevant` (the notes its `relevant:` part lists, in order, as paths). Each is empty when its part was not compiled or was omitted.

```text
$ hippo context --task "how long do we keep the nightly backups" --json
{"budget":25600,"bytes":400,"omitted":[],"relevant":["notes/Search index.md","notes/Backups.md"],"required":[],"text":"# START HERE\n\nTern is an open-source, ... \nlinter: overdue=0 dangling=0\n"}
```

(The `text` is shortened here.)

## hippo show

```text
hippo show session <id> [--json]
hippo show proposal <id> [--json]
hippo show note <path> [--json]
hippo show day [YYYY-MM-DD] [--offset +01:00] [--json]
```

Read-only. It prints what the desktop work needs to show: a session's turns, a dreaming proposal with its receipts and evidence, and a note's lint findings and receipts. It needs an index (run `hippo ingest` first). Text output folds each text onto one line; with `--json` each record is printed compact, as its file holds it.

| Subcommand | Text output | `--json` |
|---|---|---|
| `session <id>` | The session's log line, then one line per indexed turn: `<ordinal> <role> [<tool_name>] <text>`. | The session record as its file holds it (`null` when there is none), then one object per turn: `ordinal`, `role`, `ts`, `tool_name`, `text`. |
| `proposal <id>` | The proposal (`proposal <id> <pass> <note> <kind>`, then `old:`, `new:` and `reason:`), one `receipt` line for each receipt, and one `evidence` line for each cited turn. | One object with `proposal`, `receipts` and `evidence`. |
| `note <path>` | One `lint <kind> <detail>` line for each finding, then one `receipt` line for each change receipt that names the note. A note with neither prints nothing. | One object with `note`, `lint` and `receipts`. |
| `day [date]` | The day's totals (`2026-03-03: 3 sessions, 4 agents, 210 turns, 12 edits, 1 pull request`), one line per top-level session with a turn that day (`<title> · <turns> turns · <edits> edits · <ending> · <id>`, most turns first; the ending is `completed`, `usage limit`, `rate-limited`, `interrupted` or `still open`), then one line per failure along the way. The day is today when left out, in the `--offset` time zone (UTC by default). It reads the rollup `hippo ingest` writes (`$HIPPO_HOME/rollup.sqlite`, a cache rebuilt from the index); without one it exits 1 saying to run `hippo ingest`. | One object with `date`, `sessions`, `agents`, `turns`, `edits`, `pulls`, `events` and `rows`. |

`<path>` is relative to the workspace. An unknown session id exits 1 with `no session <id>`; an unknown proposal id exits 1 with `no proposal <id>`.

```text
$ hippo show session eval-01
- 09:12 claude-code eval-01 — check how long backups are kept — ended 09:15 — turns 13 — notes: [[Backups]] — handoff: no — dreaming: 1 confirmed, 0 rejected
0 user can you check how long nightly backups are kept? i think the cron job prunes old dumps
1 assistant Looking at the prune job now. Nightly backups are still kept for 14 days in the config I remember.
...
$ hippo show proposal 01M6X6VZNF8X666HVGCVJZD4FE
proposal 01M6X6VZNF8X666HVGCVJZD4FE consolidate notes/Backups.md replace
old: Nightly backups are kept for 14 days.
new: Backups are now kept for 30 days after the retention change.
reason: stub
receipt 01M6X6VZNTGPYXYWFBCW5RQFXH confirm consolidate-reviewer applied 2026-11-08T09:12:13.258708Z stub
receipt 01M6X6VZSSTVQA135WF8HE5VW3 revert human applied 2026-11-08T09:12:13.363188Z 
evidence eval-01 7 assistant Backups are now kept for 30 days after the retention change.
$ hippo show note notes/Backups.md
receipt 01M6X6VZNTGPYXYWFBCW5RQFXH confirm 01M6X6VZNF8X666HVGCVJZD4FE 2026-11-08T09:12:13.258708Z
receipt 01M6X6VZSSTVQA135WF8HE5VW3 revert 01M6X6VZNF8X666HVGCVJZD4FE 2026-11-08T09:12:13.363188Z
```

(That run used the synthetic eval set and a stand-in model.)

## hippo session

```text
hippo session start --tool <claude-code|codex|dsh|opencode|muse> --session <id> [--task <text>]
hippo session end --session <id> [--note <path>]... [--handoff <file>|-] [--no-change <reason>]
```

Both subcommands print nothing on success.

`start` writes a declared session record, or reopens an existing declared one with the same id (its `ended` is cleared, and everything else, including the task, is kept). An id that is not 1 to 128 letters, digits, `_` or `-` exits 1 with `invalid session id`.

| Flag of `end` | Meaning |
|---|---|
| `--session <id>` | The declared session to end. A missing or derived id exits 1 with `no declared session <id>`. |
| `--note <path>` | A note the session touched, relative to the workspace. Repeatable. |
| `--handoff <file>` | A Markdown handoff naming notes with `[[...]]` links. A file path, or `-` to read stdin. |
| `--no-change <reason>` | Why the session changed no note. |

If you give neither `--note` nor `--handoff` nor `--no-change`, the session records the reason `unspecified`.

## hippo config

```text
hippo config show
hippo config set <key> <value> [--yes]
hippo config unset <key> [--yes]
hippo config add <key> <value> [--yes]
hippo config remove <key> <value> [--yes]
```

Shows or changes one setting in `config.toml` or `.hippo/hippo.toml`. It prints the change and writes it only with `--yes`.

| Subcommand | Does |
|---|---|
| `show` | Prints the settings the desktop app's Manage reads. |
| `set` | Sets `ingest_interval_minutes`, `dream_at`, `budget_bytes`, `lessons` (`on` or `off`), `snapshot_keep_days` or `model_command`. |
| `unset` | Removes a key's line, so its default applies. |
| `add`, `remove` | Add or remove a value of a list: `transcript_roots`, `exclude_roots`, `required_notes`, `passes_off`. |

`model_command` takes a JSON array of strings or words separated by spaces, and is written as a one-line array:

```sh
hippo config set model_command '["claude","-p","--output-format","json"]' --yes
hippo config unset model_command --yes
```

The second command restores the default.

## hippo hooks

```text
hippo hooks install [--agent <claude-code|codex>] [--workspace <dir>]
hippo hooks uninstall [--agent <claude-code|codex>] [--workspace <dir>]
hippo hooks status [--agent <claude-code|codex>] [--workspace <dir>]
```

Adds, removes or checks hippo's three hooks (`SessionStart`, `UserPromptSubmit`, `SessionEnd`). For Claude Code, the default, they go in the workspace's `.claude/settings.json` (`--workspace` names another workspace). With `--agent codex` they go in `$CODEX_HOME/hooks.json` (`~/.codex/hooks.json`), which Codex runs for every session; the hooks themselves act only for sessions in the workspace. `install` adds each once and keeps everything else in the file. `uninstall` removes only hippo's three. `status` says which of the three are there.

Codex runs a new hook only after you trust it: it lists hippo's hooks for review the next time it starts, or use `/hooks`. hippo never writes that trust for you.

## hippo hook

```text
hippo hook session-start [--plugin] [--agent <claude-code|codex>]
hippo hook prompt [--agent <claude-code|codex>]
hippo hook session-end [--agent <claude-code|codex>]
```

For Claude Code hooks, and with `--agent codex` for Codex hooks (below). Each reads the hook's JSON on stdin (it needs `session_id`; `prompt` also needs `prompt`). See [Quick start](quick-start.md#6-add-the-claude-code-hooks) for the settings to add. A hook always exits 0. On any error it prints `hippo hook: <error>` on one line to stderr.

| Subcommand | Does |
|---|---|
| `session-start` | Starts a declared Claude Code session and prints the required parts of the context. |
| `session-start --plugin` | The Claude Code plugin's hook ([Install](install.md#in-claude-code)): prints the required parts read-only, with no session record, and prints nothing at all, not even an error, when there is no notes folder, the session is outside the workspace, or a settings file already runs `hippo hook session-start`. |
| `prompt` | On the first prompt of a session only: records it as the task and prints the relevant notes and linter counts, within what is left of the budget. Later prompts print nothing. |
| `session-end` | Ends the session with no notes, no handoff and no reason (recorded as `unspecified`). |

With `--agent codex` each reads Codex's hook input and does nothing at all unless hippo has a workspace and the input's `cwd` is in it. The session id is `codex-<session_id>`, the id the Codex adapter gives the same session. `session-start` and `prompt` print the same parts as JSON Codex reads (`{"hookSpecificOutput": {"hookEventName": …, "additionalContext": …}}`); `prompt` passes over a sub-agent's prompt (one with an `agent_id`). `session-end` records the last assistant message of the session's transcript as a derived handoff, or ends it `unspecified` when the transcript cannot be read.

## hippo log

```text
hippo log [--stdout]
```

Writes the session log to `log_path` and prints the path. With `--stdout` it prints the log and writes nothing. The log lists sessions newest first, grouped by day, with their task, turn count, notes, whether a handoff exists, and how many dreaming changes were confirmed and rejected. A session with no indexed transcript, such as a declared one whose transcript is not under `transcript_roots`, shows `?` for its turns.

## hippo doctor

```text
hippo doctor [--model]
```

Prints a table `check | status | detail`, one row per check. Status is `ok`, `warn` or `problem`. Exits 1 if any row is a problem, else 0. A healthy new setup passes: a schedule or a dreaming run that does not exist yet is an `ok` row whose detail starts `not set up yet:`. See [Troubleshooting](troubleshooting.md).

| Flag | Meaning |
|---|---|
| `--model` | Also send one word through `model_command` to check that it answers. This is one model call. A nightly check should pass it ([Scheduling](scheduling.md#a-nightly-check)). |

Without `--model` the `model` row says `not checked`, or warns when the program is not on `PATH`.

The `git` row reads git (it never writes through it): when the workspace is in a git work tree, it warns when git tracks a per-person file (`.hippo/sessions/`, `reports/`, `snapshots/`, `dream-state.json`, the session log), naming them and the two commands that stop tracking them, or when one is not ignored, naming `hippo init`. See [Sharing a brain in a team](sharing.md).

## hippo dream

```text
hippo dream [--pass <name>] [--dry-run] [--no-lessons]
```

Runs the dreaming passes over the window since the last success. See [Dreaming](concepts.md#dreaming).

| Flag | Meaning |
|---|---|
| `--pass <name>` | Run only the pass with this file name, without `.toml`, such as `verify`. An unknown name exits 2 with `no pass named <name>`. |
| `--dry-run` | Ingest, run the model and the reviewer, and write only the report. No proposal, receipt, snapshot, note or `dream-state.json` is written. |
| `--no-lessons` | Skip the [lessons](#hippo-lessons) pass that follows dreaming. |

On success it prints one line: the report's path, then the counts of the run.

```text
/home/you/work/.hippo/reports/2026-11-08-dream.md: 9 chunks, 1 proposals, 0 validation failures, 1 confirmed, 0 rejected, 1 applied
```

It prints each pass's skip reason (`consolidate: no note to work on`) and, when nothing was proposed, what dreaming needs: notes that the window's sessions touched. The window is the last 24 hours on a first run.

Exit codes: 0 on success; 2 when the run fails, for example when the model cannot be reached. A failed run still writes its report, with `status: failed`.

## hippo lessons

```text
hippo lessons [--all] [--json]
hippo lessons scan [--days <n>] [--dry-run] [--candidates] [--json]
hippo lessons keep <id> [--text <text>] [--title <title>]
hippo lessons dismiss <id>
hippo lessons propose --question <q> --answer <a> [--note] [--json]
hippo lessons propose --miss <session>:<turn> [--json]
```

Corrections and repeated failures, proposed as rules. With no subcommand it lists the lessons waiting in the Inbox; `--all` lists every lesson and `--json` prints one JSON object.

| Subcommand | Does |
|---|---|
| `scan` | Runs the lessons pass: finds candidates, drafts rules, reviews them. Calls the model, except with `--candidates`, which only lists what the pass would draft. `--days` looks back 1 to 30 days (default 7); `--dry-run` writes no record. Exits 1 when it tried to draft a lesson and every draft failed. `hippo dream` runs this pass afterwards unless you pass `--no-lessons`. |
| `keep <id>` | Writes a pending lesson into its note, with `--text` and `--title` to edit it first. It gets a receipt, so `hippo revert` undoes it. |
| `dismiss <id>` | Dismisses a pending lesson. It returns only if it recurs in 3 more sessions. |
| `propose` | Proposes a rule, or with `--note` a new note, from an Ask answer. `--miss` instead proposes a rule saying where a search that found nothing would have found its answer. |

## hippo review

```text
hippo review --ok <receipt>...
```

Marks applied dreaming changes as read and right. `<receipt>` is the id of a confirm receipt. `--ok` is the only mark there is.

## hippo revert

```text
hippo revert <receipt> [--force]
```

Restores a note from the snapshot of an applied change and writes a revert receipt. `<receipt>` is the id of the confirm receipt (the file name in `.hippo/receipts/` without `.json`, also listed under `## Applied` in the report).

| Flag | Meaning |
|---|---|
| `--force` | Restore the snapshot even if the note was edited after the change. |

```text
restored notes/Topic A.md from .hippo/snapshots/01M6X4WX5356BKSXXW2CDBY5Z8/notes/Topic A.md; revert receipt 01M6X4WX8PG69HNXDWZAQK7H5X
```

Exit 1 when the id is not a receipt id, when the note was edited since the change (without `--force`), and when the receipt is not an applied confirm receipt, its snapshot is outside `.hippo/snapshots/`, or the snapshot does not match its recorded hash (even with `--force`).

## hippo eval

```text
hippo eval <set> [--with-model]
```

Measures context, recall and dreaming on an eval set directory and prints a Markdown report. See [Measuring with hippo eval](eval.md).

| Flag | Meaning |
|---|---|
| `<set>` | The eval set directory, `fixtures/eval` in the repository. |
| `--with-model` | Also run the dreaming suite with the `model_command` from your `config.toml`. |

Exit 1 after printing the report if a suite failed. Without `--with-model` it reads no configuration and runs no model.

## hippo schedule

```text
hippo schedule install [--job ingest|dream]
hippo schedule uninstall [--job ingest|dream]
hippo schedule status
```

Schedules ingest and dreaming. On macOS it manages two launchd jobs; see [Scheduling](scheduling.md). On any other system it writes nothing and prints what to copy. `--job` limits `install` and `uninstall` to one job.

| Subcommand | On macOS | Elsewhere |
|---|---|---|
| `install` | Writes both jobs to `~/Library/LaunchAgents` and loads them. Prints `installed <path>` per job. | Prints a crontab block and a systemd user timer pair for the same two jobs, with the commands that enable them. |
| `uninstall` | Unloads both jobs and removes their files. Prints `removed <path>` or `<path> was not installed`. | Prints the commands that remove what you added. |
| `status` | Prints `<label>: launchctl print exit status <n>` for each job. | Prints the commands that check cron and systemd. |

Exit 0 elsewhere. Without a notes folder, `install` schedules the ingest job only.

## hippo packs

```text
hippo packs check <dir>
hippo packs test <dir> [--case <name>]
hippo packs list
hippo packs diff <name> [--ref <ref>]
hippo packs update <name> [--ref <ref>] [--keep-local] [--accept-reads]
hippo packs remove <name> [--force]
```

`check` and `test` read only the pack directory `<dir>`: no configuration and no workspace, and `test` uses a stand-in model, never your `model_command`. `check` validates the pack and prints `skipped`, `error` and `warning` lines, then `ok <name> <version>`; it exits 1 on any error and 0 with warnings alone. `test` runs the pack's fixture tests, prints `ok <pack>/<case>` or `FAIL <pack>/<case>: <reason>` for each, and exits 1 on any failure. `--case <name>` runs one case.

`list`, `diff`, `update` and `remove` read `config.toml` and work on `.hippo/packs/` of the workspace. They finish an interrupted update first.

| Subcommand | Does |
|---|---|
| `list` | One line for each installed pack: name, version, state (`builtin`, `local`, `clean` or `edited:<n>`) and source (`path:<path>` or `git:<url>//<dir>@<commit>`). |
| `diff <name>` | Prints what an update would change, as `take`, `keep` or `conflict` for each file, then a diff of what the source changed. Writes nothing and makes no model call. |
| `update <name>` | Applies that update after a review. Refuses a result that does not validate, a pass name another pack uses, a pass that now reads or works on more (unless `--accept-reads`) and a conflict (unless `--keep-local`, which keeps your file). |
| `remove <name>` | Deletes `.hippo/packs/<name>/`. Refuses `core`, and a pack with no `origin.json` unless `--force`. |

`--ref <ref>` reads another branch, tag or commit of a git source. `diff`, `update` and `remove` exit 1 on a refusal. [Packs](packs.md#using-a-pack) shows the output of each, and [Writing a pack](writing-packs.md) gives the rules for `check` and `test`.
