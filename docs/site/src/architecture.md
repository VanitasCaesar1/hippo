# Architecture

This page is for someone who wants to know how hippo works inside before reading the code or changing it. The pages it links say the same things in more detail, one feature at a time.

## Files are the contract

hippo is a command-line program (`hippo`), a desktop app for macOS (`Hippo.app`) and a set of plain files. Everything a person or an agent needs to read is a file: Markdown notes, JSON records, TOML settings and pass files. The one exception is the SQLite index, and it can be rebuilt from the transcripts at any time with `hippo ingest --full`. Changing the shape of a record, a pass file, a pack manifest or a report is a change to the contract, not a refactor.

There are two places on disk:

| Place | What it holds | Shared? |
|---|---|---|
| `$HIPPO_HOME` (default `~/.hippo/`) | `config.toml`, the index (`index.sqlite`), the rollup (`rollup.sqlite`), usage figures, lessons, logs | Never: it belongs to one machine |
| The workspace | The notes under `notes_dir`, and `.hippo/` beside them | Partly, through git ([below](#what-is-shared-and-what-is-per-person)) |

## The layers

| Layer | What it is | Kept in |
|---|---|---|
| Transcripts | What agents said and did, turn by turn | The agents' own files, read by `hippo ingest` into the index |
| Notes | Markdown files people and agents edit | `notes_dir` |
| Wiki | The `[[links]]` between notes | The notes |
| Injected context | The block an agent gets at the start of a session | Printed by `hippo context` and the hooks; never stored |

Reading goes up the layers: an agent starts from the injected context, which is built from notes, and notes are kept true against transcripts. Dreaming is the only thing that writes notes on its own, and only through a second reviewer ([below](#dreaming)).

## The code

| Part | What it does |
|---|---|
| `crates/hippo-core` | The engine: adapters, redaction, the index, records, notes and links, context, dreaming, packs, lessons, incidents, the rollup |
| `crates/hippo` | The command-line program and its tests, including the end-to-end runs on synthetic history |
| `crates/hippo-view` | What the desktop app's panels show, computed from the engine, with no window code |
| `crates/hippo-registry` | Checks the pack registry's entries, reviews a pack and builds the registry index |
| `crates/hippo-theme` | The web theme contract: the base stylesheet, built-in themes and the checks for custom CSS |
| `crates/hippo-feed` | A community site for packs; built and tested, not deployed |
| `app/` | `Hippo.app`, its own Cargo workspace, macOS only |

The engine makes no network calls of its own. Data leaves the machine only through two kinds of child process: the model command, and `git` fetching a pack ([Security and privacy](security.md)).

## Invariants

Four rules hold everywhere in the engine, and the code refers to them by number:

1. **No note is written by dreaming without a confirming receipt.** A write needs a receipt that names the proposal, whose `proposal_hash` matches the proposal file, whose verdict is `confirm`, and whose `before_hash` equals the note's current hash. In code, the write function takes a value that only those checks can produce.
2. **No raw transcript text reaches the index.** Redaction is the only way to make text the index accepts. Reasoning blocks are never indexed.
3. **A proposal's note path is checked before review.** As written, it is relative, has no `..`, and names a visible `.md` file below `notes_dir`; after links are resolved it is still a regular file inside `notes_dir` and not under `.hippo/`. A proposal that fails is reported and never reviewed.
4. **A model run is isolated and leaves no transcript.** hippo runs a model only through the argv in `model_command`, with the prompt on stdin. The default runs `claude -p` with no tools, no settings files, no MCP servers and no saved session, from a scratch working directory under `$HIPPO_HOME/model-runs/`, which ingest never reads. A test checks that such a run cannot read a file in its working directory or the workspace.

## The index and redaction

`hippo ingest` finds transcripts under `transcript_roots` (Claude Code, Codex, DeepSeek Harness and OpenCode), parses each with an adapter, and writes turns into `index.sqlite`, a full-text index of every turn. A file whose hash has not changed is skipped. OpenCode keeps every session in one SQLite database, which hippo opens read-only and reads session by session: a session whose change stamp has not moved is skipped, so an ingest reads only what changed. Before any text is written, redaction replaces keys, tokens, emails and a few other patterns with `[redacted:<kind>]`. Each adapter has a test that plants a fake secret of every kind and checks that none reaches the index. Reasoning blocks are never indexed. Redaction is pattern matching, so it is a safeguard and not a guarantee.

After indexing, ingest brings the **rollup** up to date: `rollup.sqlite` holds per-session facts computed from the index (work time, edits, pull requests, waits, claims and their evidence, incidents). It is a cache. When its format changes, the old file is deleted and rebuilt at the next ingest. The overview, `fleet`, `incidents`, `xray`, `card` and the app read it.

## Sessions, and which ones belong to a workspace

A session is one run of an agent, and the workspace keeps a JSON record of it in `.hippo/sessions/<id>.json`.

- A **declared** record is written by the Claude Code hooks (`SessionStart`, `UserPromptSubmit`, `SessionEnd`) or by `hippo session start` and `end`. It holds the task, the bytes of context the session was given, the notes it touched and a handoff.
- A **derived** record is written by ingest for a session with no declared record, but only when that session belongs to the workspace: it ran inside the workspace's folder or one of its git worktrees, or it read, edited or cited one of its notes. Every other session is indexed in `$HIPPO_HOME`, where search and the overview see it, and leaves nothing in the workspace. Ingest removes records an older version wrote for other projects, and says how many.

A handoff names notes with `[[links]]`. Dreaming reads handoffs to learn which notes a session's work bears on.

## Context

`hippo context` (and the `session-start` and `prompt` hooks) build the injected block from the entry note, the required notes, a session line, up to five relevant notes for the task, and linter counts, within `budget_bytes`. Optional parts are left out whole when they do not fit; when the required parts alone do not fit, the command fails rather than grow silently ([Concepts](concepts.md#the-context-budget)). `hippo brief` builds a start block for any agent, adding past sessions that already did part of the task.

## Dreaming

`hippo dream` runs over a window of time, from the last successful run to now, in chunks of 24 hours. Each chunk runs the passes of every pack in the workspace. A pass is a TOML file that names a prompt, what the pass may read (`sessions`, `notes`, `turns`, `human_confirmed`), an input budget and the reviewer persona. A pass only ever sees the notes that the chunk's sessions touched or cited.

1. **Propose.** hippo builds the prompt, marks every piece of transcript and note text as untrusted data, and runs the model command. The model replies with proposals: a `replace` or an `append` on one note, with evidence that names transcript turns.
2. **Validate.** Before any review, each proposal is checked in code: the note exists and was in the prompt, the evidence turns exist, a `replace` names text that occurs once and quotes its evidence, the change is not larger than half the note. A proposal that fails is reported and goes no further.
3. **Review.** A valid proposal goes to a second model run, the reviewer. Its system prompt is the persona file, and it sees only the note, the change and the evidence turns, never the first run's reasoning. It answers `confirm` or `reject`; anything else is a rejection. For a pass outside the core pack, the persona is one that ships inside hippo, not the pack's own.
4. **Apply.** Only a confirming receipt lets the change through (in code, `notes::apply` takes a value that only a confirming receipt can produce). The note must be unchanged since the prompt was built.

The window advances only when a chunk completes, so a failed run is picked up where it stopped. Every run writes a report in `.hippo/reports/`.

The **lessons** pass runs after dreaming (or with `hippo lessons scan`). It finds corrections a person gave and failures that repeat, drafts a rule for each, and has a second run review the draft. A confirmed lesson waits in the Inbox until a person keeps or dismisses it; keeping writes it into a note with a receipt.

## Receipts, snapshots and revert

Every reviewed proposal gets a receipt in `.hippo/receipts/`: who reviewed it, the verdict, hashes of the note before and after, and how the reviewer was invoked. Before a confirmed change is applied, the note is copied to `.hippo/snapshots/<proposal>/`. `hippo revert <receipt>` restores the note from that snapshot and writes a revert receipt; it refuses when the note was edited since, unless forced. Reverted text is shown to later `verify` runs as confirmed by a human, so the same change is not proposed again. A snapshot exists only on the machine that dreamed the change; elsewhere, git's history is the undo.

## Signals read from the transcripts

Some views read the rollup with rules and no model:

- **Claims.** `xray` lists what an agent said it did (tests pass, pushed, merged, deployed) beside the evidence: a run of the check after the last edit that could change it, CI on the pushed head, or the output of a wrapper, a background task or a delegated session. A check claimed with no such run is listed as a hint to check, not counted as an incident, because the rule is not precise enough for that.
- **Incidents.** Corrections, blocked and failed steps, work redone by another session, collisions between sessions and destructive commands, each with the step that caused it when a rule can say.

## Packs and their review

A pack is a directory of passes, prompts, reviewer personas, note templates and fixture tests. It holds no code, and hippo runs nothing a pack contains. `hippo add` copies a pack into `.hippo/packs/`, and from then on the files belong to the workspace; `origin.json` records where they came from, so updates can be shown as a diff and never overwrite local edits silently.

Before a pack is written, it is reviewed once in a fresh context: a persona that ships inside hippo reads the pack's files as untrusted data and confirms only a pack that does what its README says and nothing else. The verdict goes into `origin.json`; a rejection stops the add. Listed packs are reviewed the same way in the [registry](registry.md), with the review stored under `registry/reviews/` and pinned to exactly the content it read.

Themes for the desktop app and the web pages are packs too: a `theme.json` and a `theme.css` instead of passes ([Themes](themes.md)).

## What is shared and what is per person

A team shares a workspace through git; nothing syncs live and there are no accounts. Each person's dreaming learns from their own sessions.

| In the workspace | Shared or per person |
|---|---|
| Notes, `.hippo/hippo.toml`, `.hippo/packs/` | Shared |
| `.hippo/proposals/`, `receipts/`, `reviews/` | Shared: a teammate can see why a note changed |
| `.hippo/sessions/`, `snapshots/`, `reports/`, `dream-state.json`, the session log | Per person, ignored by the rules `hippo init` writes |

`hippo doctor` warns when git tracks a per-person file and names the commands that fix it. hippo itself never commits or pushes. [Sharing a brain in a team](sharing.md) has the details.

## Proposing a change

New designs are written for public readers under `docs/designs/` before code, as the [contributing guide](contributing.md) says. A change to any record format, the pack format, the registry index or the security model starts there.
