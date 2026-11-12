# Roadmap

This is version 0.3.0. hippo is one person's project, so there are no dates here. "Planned" means I intend to do it and know roughly how. "Idea" means it is not designed yet and may not happen.

The text lives in `ROADMAP.md` at the root of the repository, and the docs site shows the same file. How the built parts work is in [Architecture](https://vanitascaesar1.github.io/hippo/docs/architecture.html); what is missing or rough today is in [Limits and known gaps](https://vanitascaesar1.github.io/hippo/docs/limits.html).

## What is built

- The core and CLI: an index of Claude Code, Codex, DeepSeek Harness (`dsh`) and OpenCode transcripts, `recall` and `ask`, the overview, `fleet`, `incidents`, `xray` and `card`.
- An adapter for DeepSeek Harness (`dsh`): its session logs, compressed or not, with sub-agents and forks ([design](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/dsh-adapter.md)).
- An adapter for OpenCode: OpenCode 2's SQLite database, read-only and session by session, with sub-agents and forks ([design](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/opencode-adapter.md)).
- Notes, context and Claude Code hooks.
- Dreaming with a fresh-context reviewer, receipts, snapshots and revert, and its schedule: launchd on macOS, printed cron and systemd lines elsewhere.
- `hippo eval` with a synthetic question set.
- Packs: `hippo add` and `hippo packs`, the registry tool and its static site, and 26 first-party packs (23 pass packs and 3 theme packs), all reviewed and listed in the registry.
- A Claude Code plugin, an install script, release binaries for macOS and Linux, and a Homebrew formula attached to the release.
- A DeepSeek Harness (`dsh`) plugin, installed from GitHub: the start block in each new session, and `hippo_ask` and `hippo_day` as tools ([design](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/dsh-plugin.md)).
- An OpenCode plugin, installed from GitHub: the start block in the system prompt of each top-level session, and `hippo_ask` and `hippo_day` as tools ([design](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/opencode-plugin.md)).
- The desktop app, Hippo.app: macOS only, built from source, early, and checked on synthetic data.
- The community feed server (`crates/hippo-feed`): built and tested locally, not deployed. See [`docs/feed-local.md`](https://github.com/VanitasCaesar1/hippo/blob/main/docs/feed-local.md).

## Planned

- **Adapters for more agents.** Cursor and Gemini folders are detected but not read. Reading them is the next adapter work. An adapter needs a redaction test before it lands.
- **Measuring whether dreaming helps.** `hippo eval` measures a small invented set. It does not show whether the nightly pass makes an agent's work better on real notes. I want a measurement that does, before claiming it.
- **Packs by other authors.** The registry takes outside entries today ([`registry/README.md`](https://github.com/VanitasCaesar1/hippo/blob/main/registry/README.md)). None exists yet.
- **Deploying the community feed.** The server is built and tested locally. Nothing is hosted.
- **Signed and notarised macOS builds.** Neither the release binaries nor Hippo.app are signed with a Developer ID or notarised. The app is signed ad hoc, so its first launch needs Open Anyway in System Settings.
- **Hippo.app beyond synthetic data.** The app is checked on synthetic data only. The next step is using it day to day and fixing what that shows, before calling it more than early.

## Ideas

- **Hosted dreaming, live team sync of a shared brain, and private packs.** Not designed in public yet. Today a team shares a brain through git ([Sharing a brain in a team](https://vanitascaesar1.github.io/hippo/docs/sharing.html)).

To suggest something, open an issue. A new feature or contract starts as a design doc in [`docs/designs/`](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/README.md) before any code.
