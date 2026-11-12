# Roadmap

This is version 0.1.0. [Architecture](architecture.md) says how the built parts work.

## What is built

- The core and CLI: an index of Claude Code and Codex transcripts, `recall` and `ask`, the overview, `fleet`, `incidents`, `xray` and `card`.
- Notes, context and Claude Code hooks.
- Dreaming with a fresh-context reviewer, receipts, snapshots and revert, and its schedule: launchd on macOS, printed cron and systemd lines elsewhere.
- `hippocamp eval` with a synthetic question set.
- Packs: `hippocamp add` and `hippocamp packs`, the registry tool and its static site, and 26 first-party packs (23 pass packs and 3 theme packs).
- A Claude Code plugin, an install script and a release script.
- The desktop app, Hippocamp.app: macOS only, early, and checked on synthetic data.
- The community feed server (`crates/hippocamp-feed`): built and tested locally, not deployed. See [`docs/feed-local.md`](https://github.com/VanitasCaesar1/hippocamp/blob/main/docs/feed-local.md) in the repository.

## What is next

- Packs by other authors, and re-reviewing the registry entries against the public repository.
- Adapters for more agents. OpenCode and Cursor are detected but not read.
- Measuring whether dreaming improves agents' work on real notes.
- Deploying the feed.
- Hosted dreaming, team sync of a shared brain and private packs: ideas, not designed in public yet.
