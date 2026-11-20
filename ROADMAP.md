# Roadmap

This is version 0.8.1. hippo is one person's project, so there are no dates here. "Planned" means I intend to do it and know roughly how. "Idea" means it is not designed yet and may not happen.

The text lives in `ROADMAP.md` at the root of the repository, and the docs site shows the same file. How the built parts work is in [Architecture](https://vanitascaesar1.github.io/hippo/docs/architecture.html); what is missing or rough today is in [Limits and known gaps](https://vanitascaesar1.github.io/hippo/docs/limits.html).

## What is built

- The core and CLI: an index of Claude Code, Codex, Cursor, Gemini CLI, DeepSeek Harness (`dsh`), OpenCode, Muse Code and Command Code transcripts, `recall` and `ask`, the overview, `fleet`, `incidents`, `xray` and `card`.
- An adapter for DeepSeek Harness (`dsh`): its session logs, compressed or not, with sub-agents and forks ([design](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/dsh-adapter.md)).
- An adapter for OpenCode: OpenCode 2's SQLite database, read-only and session by session, with sub-agents and forks ([design](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/opencode-adapter.md)).
- An adapter for Muse Code: its session logs, as Muse 1.4.18 writes them ([design](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/muse-adapter.md)).
- An adapter for Command Code: its version 3 session files ([design](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/command-code-adapter.md)).
- An adapter for Cursor: its agent transcripts, from the editor and `cursor-agent`, with sub-agents ([design](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/cursor-adapter.md)).
- An adapter for Gemini CLI: its saved sessions in both the older JSON and the newer JSON Lines form, with sub-agents ([design](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/gemini-adapter.md)).
- An MCP server, `hippo mcp`, so any agent that speaks MCP can ask hippo for the start block, past sessions, the day and incidents, and `hippo mcp install` for seven clients ([design](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/mcp-server.md), [docs](https://github.com/VanitasCaesar1/hippo/blob/main/docs/mcp.md)).
- Notes, context, and hooks for Claude Code, Codex and Gemini CLI, and a start hook for Cursor, each running hippo by its absolute path from per-person files.
- Runners: which CLI, model, effort and account hippo's own model calls use, per role, with fallback inside a data domain, each call in an OS sandbox with a limited environment ([design](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/runners.md)).
- The guard's incidents after the fact: remote scripts, a fresh clone's code run, credential reads and changes to hippo's hooks, read from every agent's transcripts ([design](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/guard.md)).
- A guard before the fact, warn-only and machine-wide: `hippo guard check` in Claude Code's, Gemini CLI's, dsh's and OpenCode's pre-tool hooks, team rules in a note, machine rules, self-protection, `guard test` over your own history ([design](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/guard.md#part-b-before-the-fact-a-guard-in-the-agent)).
- The accept rule: a shared security setting that makes hippo stricter applies at once, one that loosens it waits for `hippo accept` on each machine ([design](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/common.md#rule-3-tightening-takes-effect-at-once-loosening-needs-a-local-accept-on-each-machine)).
- A shared Inbox for teams: lesson identities and decisions through git, per-person opt-in, no quotes, markers on kept bullets ([design](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/team-brain.md)).
- Secrets that reached a model, from redaction rows that name the kind and the turn, never the value ([design](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/secrets.md)).
- A private `$HIPPO_HOME` (0700 folders, 0600 files), a version floor for shared workspaces, and an install script that records the checksum it checked ([design](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/common.md)).
- Dreaming with a fresh-context reviewer, receipts, snapshots and revert, and its schedule: launchd on macOS, printed cron and systemd lines elsewhere.
- Rules that are learned once and given everywhere: corrections grouped by meaning across the whole history, rules from agent memories you turn on, reversals that retire the older rule, kept rules in every start block (machine-wide ones outside the workspace too) and again when a long session changes topic, an in-session question for pending rules, and `hippo stats repeats` ([design](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/remember.md)).
- `hippo eval` with a synthetic question set.
- Packs: `hippo add` and `hippo packs`, the registry tool and its static site, and 26 first-party packs (23 pass packs and 3 theme packs), all reviewed and listed in the registry.
- A Claude Code plugin and a Codex plugin that set hippo up with no terminal (`hippo setup`, [design](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/setup.md)), an install script, release binaries for macOS and Linux, and a Homebrew formula attached to the release.
- A DeepSeek Harness (`dsh`) plugin, installed from GitHub: the start block in each new session, and `hippo_ask` and `hippo_day` as tools ([design](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/dsh-plugin.md)).
- An OpenCode plugin, installed from GitHub: the start block in the system prompt of each top-level session, and `hippo_ask` and `hippo_day` as tools ([design](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/opencode-plugin.md)).
- The desktop app, Hippo.app: macOS only, a release download (`hippo app install`, ad-hoc signed, not notarised) or built from source, early, and checked on synthetic data.
- The community feed server (`crates/hippo-feed`): built and tested locally, not deployed. See [`docs/feed-local.md`](https://github.com/VanitasCaesar1/hippo/blob/main/docs/feed-local.md).

## Planned

- **The guard's `ask` and `block`.** Once `hippo guard test` counts on real history show which defaults are quiet enough; then `unknown-egress` and `command-from-fetched` ([design](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/guard.md#rollout)).
- **The shared Inbox in Hippo.app,** with who saw and who decided, and the Share dialog.
- **`[secrets]` references** and the guard's secrets rules ([design](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/secrets.md#rollout)).
- **More runner presets.** Codex, Muse and OpenCode as checked `command` recipes, then presets; a reviewer on another vendor ([design](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/runners.md#rollout)).

- **Checking the Cursor and Gemini CLI adapters on real sessions.** Cursor's transcripts were confirmed by their structure only, and Gemini CLI's format comes from its source; both need a run against real history, as Codex's hooks had.
- **History from more agents.** Windsurf, Zed and other MCP clients use hippo's tools but their own sessions are not read. An adapter needs a redaction test before it lands.
- **Measuring whether dreaming helps.** `hippo eval` measures a small invented set. It does not show whether the nightly pass makes an agent's work better on real notes. I want a measurement that does, before claiming it.
- **Packs by other authors.** The registry takes outside entries today ([`registry/README.md`](https://github.com/VanitasCaesar1/hippo/blob/main/registry/README.md)). None exists yet.
- **Deploying the community feed.** The server is built and tested locally. Nothing is hosted.
- **Signed and notarised macOS builds.** Neither the release binaries nor Hippo.app are signed with a Developer ID or notarised. The app is signed ad hoc: `hippo app install` fetches it with curl, so it opens; a copy downloaded in a browser needs Open Anyway in System Settings once.
- **Hippo.app beyond synthetic data.** The app is checked on synthetic data only. The next step is using it day to day and fixing what that shows, before calling it more than early.

## Ideas

- **Hosted dreaming, live team sync of a shared brain, and private packs.** Not designed in public yet. Today a team shares a brain through git ([Sharing a brain in a team](https://vanitascaesar1.github.io/hippo/docs/sharing.html)).

To suggest something, open an issue. A new feature or contract starts as a design doc in [`docs/designs/`](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/README.md) before any code.
