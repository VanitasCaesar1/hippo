# Changelog

All notable changes to this project are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

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

[Unreleased]: https://github.com/VanitasCaesar1/hippo/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/VanitasCaesar1/hippo/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/VanitasCaesar1/hippo/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/VanitasCaesar1/hippo/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/VanitasCaesar1/hippo/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/VanitasCaesar1/hippo/releases/tag/v0.1.0
