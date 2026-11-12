# Changelog

All notable changes to this project are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

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

[Unreleased]: https://github.com/VanitasCaesar1/hippo/compare/v0.2.1...HEAD
[0.2.1]: https://github.com/VanitasCaesar1/hippo/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/VanitasCaesar1/hippo/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/VanitasCaesar1/hippo/releases/tag/v0.1.0
