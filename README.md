<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="brand/hippocamp-mark-dark.svg">
    <img src="brand/hippocamp-mark-light.svg" width="180" alt="hippocamp">
  </picture>
</p>

<h1 align="center">hippocamp</h1>

<p align="center"><b>One brain for all your coding agents.</b></p>

Your Claude Code sessions start from the same plain-Markdown notes and write back to them, and at night a second, fresh-context reviewer checks every proposed change to those notes against what your agents actually did. Underneath, hippocamp indexes every Claude Code and Codex session on your machine, so you can search them, see your agents' day, and share it.

<p align="center"><img src="docs/assets/card.png" width="600" alt="A card dated Tuesday 3 March 2026: My agents did 3 hours 18 minutes of work on Tuesday, with threads, sub-agents, pull requests, waits and corrections"></p>

<p align="center"><img src="docs/assets/first-run.gif" width="700" alt="hippocamp, then hippocamp start --yes, on synthetic history: what it found, then the last active day"></p>

<p align="center"><sub>Both from synthetic data. The card shows numbers and a date, never a project, title or path.</sub></p>

## Install

macOS (Apple silicon or Intel), one line, no `sudo` (installs to `~/.local/bin`):

```sh
curl -fsSL https://raw.githubusercontent.com/VanitasCaesar1/hippocamp/main/install.sh | sh
```

Inside Claude Code, as a plugin (then run `/hippocamp:setup`):

```text
/plugin marketplace add VanitasCaesar1/hippocamp
/plugin install hippocamp@hippocamp
```

On Linux, or anywhere with Rust 1.89 or later: `cargo install --locked --git https://github.com/VanitasCaesar1/hippocamp hippocamp`. Prebuilt Linux binaries are not published yet.

## The first two minutes

```sh
hippocamp                  # lists the agent history it finds; nothing is written yet
hippocamp start --yes      # indexes it (history only, nothing sent), then shows your last active day
hippocamp card             # that day as a 1200 x 675 PNG to share, and the same lines as text
```

The overview gives the day's sessions and sub-agents, the agents' work time, how long they waited on you, the pull requests they merged, the times you corrected them (the first one quoted), the busiest sessions, and four commands to try next. It reads `~/.claude*/projects` and `~/.codex/sessions`; `hippocamp start --add-root <dir>` adds another folder.

## What it does

1. **Search every past session.** `hippocamp ask --sources-only "<question>"` ranks the past sessions that dealt with it, with the command to resume each; without `--sources-only` it also writes an answer that cites them. `hippocamp recall "<words>"` searches every indexed turn.
2. **See the day.** `hippocamp` shows the last active day; `fleet` today's sessions (live, waiting on you, stopped at a usage limit); `incidents` what went wrong and at which step; `xray <session>` what context a session was given and what it used; `card` the day, or `--week`, as one image.
3. **A shared brain your agents start from and write back to.** `hippocamp init --notes-dir notes --entry-note "notes/START HERE.md"` sets up a notes folder; `hippocamp hooks install` gives every Claude Code session in it the notes at start and keeps a record of each session; `hippocamp brief --task "<task>"` is the start block for any agent, with the past sessions that already did part of it.
4. **Kept true at night, with a second reviewer and an undo.** `hippocamp dream` checks notes against transcripts and folds finished sessions' handoffs into them. A second model run that never sees the first one's reasoning must confirm each change; every applied change has a receipt and a snapshot, and `hippocamp revert <receipt>` undoes it. `hippocamp schedule install` runs it nightly.
5. **Packs you copy and own.** `hippocamp add <path or git URL>` copies a pack of dreaming passes, prompts and reviewer personas into your workspace after a fresh-context review; it is plain files, never code, and `hippocamp packs update` merges upstream changes without losing your edits.

## How it differs

- **Your notes are files.** Plain Markdown in your own folder: read them, edit them, commit them. The index is a local SQLite file you can delete and rebuild.
- **No change lands without a fresh-context review.** The model that proposes a change never grades it.
- **Every change can be undone.** A snapshot and a receipt per change, and one command to revert it.

## What leaves your machine

hippocamp's own code makes no network calls: it has no HTTP client. Data leaves only through two kinds of child process it starts:

- **The model command**, by default `claude -p` under your own login, run by `dream` (also `--dry-run`), `lessons scan`, `ask` without `--sources-only`, the pack review in `add` and `packs update`, `eval --with-model`, `doctor --model`, and the scheduled dream.
- **Git**, to fetch a pack: `add` from a URL or a registry, `packs update`, and `packs diff` of a git pack.

Everything else, including `hippocamp`, `start`, `card`, `recall`, `ask --sources-only`, `fleet`, `incidents` and `xray`, is local. Transcript text is redacted on ingest (keys, tokens, emails and more, by pattern; best effort). Details: [Security and privacy](docs/site/src/security.md).

## What it does not do yet

- **Claude Code and Codex only.** OpenCode and Cursor folders are detected, not read. Only Claude Code gets the notes at session start through hooks; a Codex session is indexed and searchable, and can run `hippocamp brief` itself if your instructions ask it to.
- **Scheduling** is launchd on macOS. Elsewhere `hippocamp schedule install` prints cron and systemd lines for you to install.
- **Linux is not tested yet.** This release publishes macOS binaries only; on Linux, `cargo install` builds it, and the tests have not run on Linux for this version. It is used and tested on macOS on Apple silicon (the Intel build is built, not run).
- **The desktop app**, Hippocamp.app in [`app/`](app/README.md), is macOS only, built from source, and early: checked on synthetic data, not in daily use.
- **Whether dreaming makes an agent's work better is not measured.** `hippocamp eval` measures a small, invented question set, not your notes.
- **No pack by another author exists.** The 26 first-party packs in [`packs/`](packs/) await their registry review against this repository.
- **Sharing one brain between people is through git.** `hippocamp init` ignores what is per person, so the notes can be committed with `git add -A`; nothing syncs live, and each person's dreaming learns from their own sessions ([Sharing a brain in a team](docs/site/src/sharing.md)).

## Where it came from

I built hippocamp to keep my own coding agents on the same page. On one day in November 2026 that was 13 threads and 538 sub-agents merging 66 pull requests on one 16 GB Mac, and they needed one memory they could all start from and that something checked. It is one person's project, built in the open from here.

## Docs, packs, contributing, licence

- **Docs**: [vanitascaesar1.github.io/hippocamp](https://vanitascaesar1.github.io/hippocamp/) (source in [`docs/site/src/`](docs/site/src/SUMMARY.md)): [install](docs/site/src/install.md), [quick start](docs/site/src/quick-start.md), [commands](docs/site/src/commands.md), [concepts](docs/site/src/concepts.md), [limits](docs/site/src/limits.md), [roadmap](docs/site/src/roadmap.md), [architecture](docs/site/src/architecture.md).
- **Packs**: [`packs/`](packs/) and the [registry](registry/README.md); [writing a pack](docs/site/src/writing-packs.md).
- **Contributing**: [`CONTRIBUTING.md`](CONTRIBUTING.md). Security reports go through [private vulnerability reporting](SECURITY.md), not issues.
- **Licence**: MIT. The card's embedded DejaVu fonts are under their own [licence](crates/hippocamp/assets/fonts/LICENSE-DejaVu.txt). The hippocamp mark is © Vanitas Caesar and may be used to refer to this project.
