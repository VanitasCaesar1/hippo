# Limits and known gaps

hippo is early software at version 0.2.0. This page lists what you should expect to go wrong or be missing.

## Scope

- **Three tools.** Only Claude Code and Codex transcripts and DeepSeek Harness (dsh) session logs are read. There are no adapters for other agents.
- **One workspace per configuration.** `config.toml` names one workspace.
- **Sharing a brain is through git.** Several people can commit one workspace; `hippo init` keeps per-person state out of git. Nothing syncs live, each person's dreaming learns only from their own sessions, and `hippo revert` works only on the machine that dreamed the change (git is the undo elsewhere). See [Sharing a brain in a team](sharing.md).
- **The desktop app is macOS only and early.** Hippo.app (in `app/`, built from source) has been checked on synthetic data, not in daily use. Everything it shows is also on the command line.
- **The feed is not live.** The `hippo-feed` crate and its deployment files exist. Nothing is deployed, so there is no feed to sign in to, vote on or read.
- **Hooks only for Claude Code.** Codex and dsh sessions are always derived records: no task, no declared notes. Nothing should depend on a Codex or dsh session being declared. The [dsh plugin](https://github.com/VanitasCaesar1/hippo/tree/main/plugins/dsh) gives a dsh session the start block, but records nothing.
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
- **Index size.** The index of a large transcript history can reach several gigabytes under `$HIPPO_HOME`. `hippo ingest --full` is the only maintenance it needs.

## Redaction

Redaction is best-effort pattern matching, in both directions: it can miss a secret, and it can redact harmless text. See [Security and privacy](security.md).

## Dreaming

- **A model reviews a model.** The reviewer is isolated from the proposer, but both are language models. Dreaming can propose and confirm a wrong change. Every change has a snapshot, a receipt and a revert, and the first runs should use `--dry-run`.
- **Prompt injection is reduced, not removed.** Transcript text reaches the model. See [Security and privacy](security.md).
- **Notes have many writers.** Agents edit notes at any time. hippo guards a write by checking the note has not changed since the prompt was built, but an agent can still overwrite a dreamed change later. The change keeps its receipt and snapshot.
- **The model channel can fail.** The default `claude` command depends on your login and your usage limits. A failed run exits with code 2, writes a failed report, and `hippo context` warns every agent within 36 hours. The window does not advance past a failed day.
- **It is not measured on your notes.** The eval harness measures the shipped set, which is small, invented and has one author. Its numbers describe that set, not your workspace.
- **Whether hippo's results hold on real daily use is not established.** It is early; treat results as unproven until you have measured them on your own notes.

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
