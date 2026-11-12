# Changelog

All notable changes to this project are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

- **A workspace that is safe to commit.** `hippocamp ingest` writes a session record into the workspace only for sessions that ran in it or touched its notes; other projects' sessions stay in your own index, and a workspace written by an older version is cleaned on the next ingest. `hippocamp init` writes the git ignore rules for per-person files and `merge=union` for notes, without touching your own lines, and also runs in a set-up workspace to add them. `hippocamp doctor` has a `git` row that names tracked per-person files and the fix, and no longer warns for ever after a merged note. `hippocamp <command> | head` ends quietly.

- **Claims that hold, as hints.** The "claimed without checking" rule counts the runs it used to miss: CI on the pushed head, wrapper scripts, remote and background runs, delegated sessions, commands in order inside one call, and edits that cannot change the result. It is still not precise enough to count, so `hippocamp incidents` and the app list these apart, after the incidents, under "Hints to check", and count them nowhere.

- **`hippocamp card`**: your agents' day, or `--week`, as one 1200 x 675 image to share and the same lines as text; numbers and a date only, the same pixels on every machine (an embedded DejaVu Sans subset).
- **Docs**: [Sharing a brain in a team](docs/site/src/sharing.md), tried with two people and one git remote: what to commit, what to keep per person, and what does not work yet.

## [0.1.0]

The first release. hippocamp is one brain for all your coding agents: it reads what they did, keeps your notes true, and briefs every new session. Everything runs on your machine.

- **History.** `hippocamp` finds Claude Code and Codex transcripts on the machine, indexes them (redacting keys, tokens and personal data) and shows the last active day: sessions, sub-agents, time agents waited, merged pull requests and corrections. `recall`, `ask`, `fleet`, `incidents`, `xray`, `show` and `resume` search and explain that history.
- **Notes.** With a notes folder (`hippocamp init`), sessions get a context block at start under a hard byte budget, through three Claude Code hooks (`hippocamp hooks install`).
- **Dreaming.** A scheduled pass checks notes against transcripts and proposes fixes; a fresh-context reviewer confirms every change before it is written, each change keeps a snapshot, and `hippocamp revert` undoes it.
- **Packs.** Copy-and-own bundles of passes, prompts, personas and themes, added after a review (`hippocamp add`, `hippocamp packs`). Packs never run code.
- **Eval.** `hippocamp eval` measures context, recall and dreaming on a synthetic set.
- **Install** in one line on macOS and Linux (`install.sh`: the release binary for the machine, checked against its SHA-256, into `~/.local/bin` without `sudo`), or with `cargo install --locked --git https://github.com/VanitasCaesar1/hippocamp hippocamp`. Release binaries for macOS (arm64, x86_64) and static Linux (x86_64, aarch64) come from `scripts/release.sh`.
- **A Claude Code plugin** from this repository's marketplace: the start block at session start when hippocamp has a notes folder (and no workspace hook gives it already), and `/hippocamp:setup`, `/hippocamp:ask` and `/hippocamp:day`.
- **Hippocamp.app**, a macOS window over the same files, built from `app/`.
