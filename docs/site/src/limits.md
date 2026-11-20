# Limits and known gaps

hippo is early software at version 0.8.0. This page lists what you should expect to go wrong or be missing.

## Scope

- **Eight tools.** Only Claude Code and Codex transcripts, DeepSeek Harness (dsh) session logs, OpenCode's database, Muse Code session logs, Command Code sessions, Cursor agent transcripts and Gemini CLI sessions are read. Other agents can use hippo through its [MCP server](mcp.md), but their history is not indexed.
- **One workspace per configuration.** `config.toml` names one workspace.
- **Sharing a brain is through git.** Several people can commit one workspace; `hippo init` keeps per-person state out of git. Nothing syncs live, each person's dreaming learns only from their own sessions, and `hippo revert` works only on the machine that dreamed the change (git is the undo elsewhere). See [Sharing a brain in a team](sharing.md).
- **The desktop app is macOS only and early.** Hippo.app (in `app/`; `hippo app install` downloads it, ad-hoc signed and not notarised) has been checked on synthetic data, not in daily use. Everything it shows is also on the command line.
- **The feed is not live.** The `hippo-feed` crate and its deployment files exist. Nothing is deployed, so there is no feed to sign in to, vote on or read.
- **Hooks only for Claude Code, Codex and Gemini CLI, and a start hook for Cursor.** dsh, OpenCode, Muse Code, Command Code and Cursor sessions are always derived records: no task, no declared notes, and so is a Codex session that did not run hippo's hooks. Nothing should depend on one being declared. The [dsh plugin](https://github.com/VanitasCaesar1/hippo/tree/main/plugins/dsh) gives a dsh session the start block, and the [OpenCode plugin](https://github.com/VanitasCaesar1/hippo/tree/main/plugins/opencode) an OpenCode session, but neither records anything.
- **Codex runs hippo's hooks only once you trust them.** `hippo hooks install --agent codex` writes them to `$CODEX_HOME/hooks.json`; Codex lists them for review at its next start, and skips them until you trust them there or with `/hooks`. `codex exec` shows no review. A Codex session started before that is read by ingest as before. Windows is not tried.
- **Scheduling is installed only on macOS** (launchd). Elsewhere `hippo schedule install` prints cron and systemd lines for you to install; hippo cannot see whether they run.
- **Not on crates.io.** Install with the install script, the Claude Code plugin's setup, or `cargo install --git` ([Install](install.md)).
- **Linux is tested in CI, not in daily use.** The release has static Linux binaries (x86_64 and arm64) and the test suite passes on Linux in CI, but hippo is used day to day on macOS on Apple silicon (the Intel build passes the first-run check under Rosetta).

## Search

- **Words, not meaning.** `hippo recall` matches the words of your query. A paraphrase that shares no words with the turn finds nothing. The eval set's paraphrase queries hit 0 of 4. hippo has no embeddings or vector search.
- **Context ranking is by title and headings.** The relevant-notes part scores a note by whether words of the task appear in its title or headings. A note whose title does not contain the task's words may be missed.
- **Lone surrogates.** A transcript line holding half of a UTF-16 surrogate pair, such as `\ud83d` with no partner, is rejected by the JSON parser and its turns are not indexed. hippo logs a warning.
- **Codex tool calls are unconfirmed.** The mapping of Codex tool calls and their results follows the OpenAI Responses API schema, but the Codex rollouts checked so far contained no tool calls. Messages were checked against a real rollout. If a rollout's tool calls do not appear in `hippo recall`, that is a bug worth reporting.
- **dsh is a developer preview.** DeepSeek Harness says its log format will change. hippo reads log formats 3 and 4; a log in any other format is not read, and `hippo doctor`'s `dsh logs` row names it. Only dsh's JSONL storage is read, and only the newest generation of each session. A compressed (`.zstd`) log is read whole each time it changes.
- **What hippo leaves out of a dsh log.** Why a turn ended (`turn/end`), token counts and rate limits, team mode, workflows, jobs and `plugin:` records are not read. dsh's core tools are stored under Claude Code's names (`bash` as `Bash`, `read` as `Read`, …), and a failed result is written the way hippo reads a failure (`Exit code N`, `<tool_use_error>`), so `recall` shows those spellings. See the [design](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/dsh-adapter.md).
- **OpenCode 2 only.** hippo reads OpenCode 2's tables (`session_v2`, `session_message`) and checks them before it reads; `hippo doctor`'s `OpenCode database` row says whether a database is read and, when its schema is newer than this hippo knows, says so. OpenCode 1's tables and JSON files are not read (OpenCode 2 copies them into its own), nor is `opencode-next.db`. When OpenCode is not running, opening its database makes SQLite create an empty `-wal` and a `-shm` file beside it; the database itself is never written.
- **What hippo leaves out of OpenCode.** Attachments' contents (their name, type, size and path are indexed), text OpenCode adds for the model, commands run with `!`, cost, token counts other than a sub-agent run's (which Fleet shows, as for Claude Code), and permission prompts (OpenCode keeps those in memory, so Fleet shows a call waiting for permission as running). OpenCode does not record who interrupted a run, so every interruption reads as stopped by you; a reply cut off at the model's output limit is not shown. Core tools are stored under Claude Code's names and argument names (`shell` as `Bash`, `filePath` as `file_path`, …). See the [design](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/opencode-adapter.md).
- **Muse Code's log is undocumented.** Muse is closed source, and hippo reads what Muse 1.4.18 wrote in real sessions: prompts, assistant messages, tool calls and their results. A record in a version other than 1 is skipped with a warning in the ingest log. Sub-agent logs, prompts typed while a run is going, and large tool outputs Muse moves to `tool-outputs/` are not read, and tool names are kept as Muse writes them. See the [design](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/muse-adapter.md).
- **Command Code: every branch, version 3 only.** hippo reads every message of a Command Code session in file order, so a branch the person went back from is indexed with the one they kept. Version 1 and 2 files are reported as files hippo could not read; Command Code converts them when it next opens them. Compaction summaries and sub-agents are not read, tool names are kept as Command Code writes them, and its learned preferences (`taste.md`) are not read. See the [design](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/command-code-adapter.md).
- **Cursor: agent transcripts only, no tool results.** hippo reads the agent transcripts under `~/.cursor/projects/<project>/agent-transcripts/`, which Cursor's agent writes in the editor and in `cursor-agent`; the editor's older chat store (`state.vscdb`) and the CLI's `store.db` are not read. The transcripts hold no tool results and no per-line times: each turn has the time of the prompt before it, to the minute, and incidents that need a tool's output are not found. A session's folder is found by reading the project folder's name back against the disk, so a moved or deleted project's sessions belong to no workspace. Cursor's `sessionStart` hook gives a new chat the start block; Cursor does not wait for it, and has no prompt hook, so the notes for a task come through the MCP server. See the [design](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/cursor-adapter.md).
- **Gemini CLI: from its source, not from real sessions.** The format was read from Gemini CLI's documentation and source, which disagree on the project folder (hashed or named) and the file (one JSON document or JSON Lines); hippo reads both. Rewinds are not followed, so a rewound message stays indexed. An older, hashed project folder names no directory, so its sessions belong to no workspace. `logs.json` and `/chat save` checkpoints are not read. See the [design](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/gemini-adapter.md).
- **Index size.** The index of a large transcript history can reach several gigabytes under `$HIPPO_HOME`. `hippo ingest --full` is the only maintenance it needs.

## Redaction

Redaction is best-effort pattern matching, in both directions: it can miss a secret, and it can redact harmless text. See [Security and privacy](security.md).

- **`secret-to-model` is only as good as the patterns.** A secret no pattern matches (a short password, a custom token format) is not found. It counts only text ingested by 0.5.0 or later until `hippo ingest --full`, and it cannot tell whether a key was live or whether the agent's client filtered it before sending.

## Runners and the sandbox

- **The sandbox on macOS and Linux with `bwrap` only.** Landlock without `bwrap` is not used, and Windows has none. Where there is none, `claude` runs as before (no tools) and a `command` runner may not write notes, receipts or lessons unless it sets `allow_unsandboxed`.
- **The profile is checked on startup, not on a signed-in call.** `claude --version` runs inside the generated macOS profile; no signed-in model call was made to build hippo. A Claude Code update can need a path the profile lacks; the symptom is a failed call, and `hippo doctor --model` shows it.
- **The account folder stays readable.** The sandbox narrows what a CLI can read to its run folder, its install and its account folder, which holds its own credentials. It is not a security boundary.
- **The older keys run as before.** `model_command` and `reviewer_command_override` keep the whole environment and no sandbox until `hippo runners migrate`.
- **Recipes for other CLIs are unchecked.** Codex, Muse and OpenCode have candidate `command` recipes read from their `--help`; dsh and Command Code have none.

## The guard's incidents

- **Text, not behaviour.** The guard's kinds read a command's text as the transcript records it. A script written to a file and then run, a base64-encoded command, a read through a language runtime, or a shell that is not one of the agents' shell tools are not found.
- **Built-in allow lists only.** The incidents use hippo's built-in allow list; the machine and team lists apply to the guard before the fact.
- **Measured on synthetic data only.** Precision was measured on a labeled synthetic set written by the rules' author and on the synthetic demo history, where nothing fired. Real history may be noisier; `hippo incidents --kind <kind>` shows what fires.

## The guard

- **Warn-only.** Every rule warns in 0.6.0; only the self-protection rules refuse a call. `ask` and `block` wait for counts on real history.
- **Not every agent shows a warning to you.** Claude Code, Codex, Cursor, Gemini CLI, dsh and OpenCode run the guard. Codex and Cursor cannot ask a question from their pre-tool hook, so once `ask` takes effect a question there is a refusal (Cursor's shell commands will get its approval prompt). `codex exec` does not print a warning (its TUI does); Cursor gives it to the model only; Gemini CLI shows it to you but gives the model none; dsh's headless profile does not show you one (the model gets it, and the session keeps it).
- **Cursor has no end-to-end check.** Its hook contract was read from the code `cursor-agent` 2026.11.19 ships and is tested by replaying payloads; `cursor-agent` cannot run against a local model, and Cursor's IDE was not read.
- **One call at a time.** A clone is followed within one call, and across calls only through the disk: a git repository outside the workspace created in the last day. A clone older than a day, or code run from elsewhere, is not counted.
- **`guard test` cannot tell where a session ran.** The index does not keep a session's folder, so the test applies the team note's rules to every call.
- **Not a boundary.** An agent with a shell can remove the hook, unset its markers or edit hippo's files with a tool the guard does not see. The accept rule's terminal and marker checks stop casual and injected attempts, not a determined one.
- **No guard in the desktop app yet.** Hippo.app shows the guard's rows in Manage › Health and accepts there; it has no guard panel per agent.

## Teams

- **Corrections only, no quotes.** The shared Inbox shares correction lessons whose evidence all ran in the workspace, with no quotes. Failure lessons stay local: their signatures carry paths and commands from a person's machine.
- **A session's folder must be known.** A session whose transcript does not name its folder counts as outside the workspace, so its lessons stay local.
- **Hippo.app does not show shared lessons yet.** `hippo lessons` does.
- **Git is the transport.** Nothing syncs until people pull and push; two people who scan before pulling can make two entries for one lesson, which `dismiss --reason duplicate --of` links.

## Dreaming

- **A model reviews a model.** The reviewer is isolated from the proposer, but both are language models. Dreaming can propose and confirm a wrong change. Every change has a snapshot, a receipt and a revert, and the first runs should use `--dry-run`.
- **Prompt injection is reduced, not removed.** Transcript text reaches the model. See [Security and privacy](security.md).
- **Notes have many writers.** Agents edit notes at any time. hippo guards a write by checking the note has not changed since the prompt was built, but an agent can still overwrite a dreamed change later. The change keeps its receipt and snapshot.
- **The model channel can fail.** The default `claude` command depends on your login and your usage limits. A failed run exits with code 2, writes a failed report, and `hippo context` warns every agent within 36 hours. The window does not advance past a failed day.
- **It is not measured on your notes.** The eval harness measures the shipped set, which is small, invented and has one author. Its numbers describe that set, not your workspace.
- **Whether hippo's results hold on real daily use is not established.** It is early; treat results as unproven until you have measured them on your own notes.

## Rules and the repeat count

- Grouping corrections by meaning depends on your lessons runner: two people's `hippo stats repeats` share definitions, not exactly the same grouping. A correction the local comparison finds no candidate for starts its own group without a model call, so a rule said in entirely different words can stay two groups.
- Until `hippo lessons backfill` runs, groups reach back 30 days from the first scan of 0.8.0, and `hippo stats repeats` leaves earlier weeks out.
- A correction said in one session only is grouped but not drafted; it becomes a rule when it comes back.
- Delivery is counted from the hooks' log. Sessions no hook saw (an agent without hooks, a session before 0.8.0 with no recorded start block) count as not given the rule.
- Rules typed into Cursor's settings live in its database, which hippo does not read; files in `~/.cursor/rules/` are read.
- With many kept rules the Rules section is cut at `rules_budget_bytes`. A session with one prompt gets only what fits; a long session gets more when its topic changes. On the eval set, a budget that holds one rule delivered 4 of 6 needed rules.
- An agent can record your answer to hippo's question about a pending rule. It could misread a mixed reply; `hippo lessons undo` takes a rule back, and Morning lists rules kept by an agent or by `auto_keep` apart.

## Packs

- **Not measured.** None of the 23 first-party packs has been run against real notes with a real model, and the eval set does not cover them. A pack's fixture test uses a stand-in model, so it shows the wiring and nothing about quality.
- **No hosted index of packs.** The 26 first-party packs are listed in the registry, and no hosted index is published. You build the index and the site from a checkout, and `hippo add <name> --registry <index.json>` reads a file on your disk ([The registry](registry.md)). Without one, `hippo add` takes a path or a git URL.
- **No pack by another author.** Every pack so far is by the project's author.
- **A review is a model's review.** A pack is listed only with a fresh-context review. It is a language model's judgement and not a guarantee.
- **Web themes load nowhere yet.** The three theme packs install with `hippo add`, and Hippo.app's theme picker lists their desktop themes, but no running site offers their web themes ([Themes](themes.md)).
- **A pack's reviewer is hippo's, not the pack's.** A pass outside the core pack is reviewed by hippo's own persona, and the pack's persona can only add reasons to reject. Its appends are capped at 2,048 bytes.
- **Validation is pattern matching.** `hippo packs check` warns about links, shell-shaped text and phrases that look like prompt injection. It cannot tell whether a prompt is wise. Read a pack's prompt before you copy it.

## Operations

- **Session handoffs depend on your instructions.** hippo cannot make an agent run `hippo session end --handoff`. Without handoffs, `consolidate` has nothing to work from and `hippo doctor` reports a problem.
- **A one-time prompt on external volumes.** macOS may block the first scheduled run until you answer a permission prompt. See [Scheduling](scheduling.md#the-removable-volume-prompt).
- **`hippo init --force` overwrites your core pack edits.**
- **A crash between applying a change and writing its receipt** leaves a snapshot with no receipt. `hippo doctor` reports it as a problem.
- **The YAML parser is unmaintained upstream.** Note frontmatter is parsed with `serde_yaml_ng`, which uses `unsafe-libyaml`, listed by RustSec as unmaintained (RUSTSEC-2024-0449). The repository's `deny.toml` ignores that one advisory until the project decides whether to keep or replace it.
