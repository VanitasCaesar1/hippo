# Shared rules and release plan for runners, team brain, guard and secrets

Status: accepted for v0.5.0 (rules 1, 2 and 4) and v0.6.0 (rule 3). Written 12 November 2026.

The four designs ([runners.md](runners.md), [team-brain.md](team-brain.md), [guard.md](guard.md), [secrets.md](secrets.md)) each add settings, stores or model calls that need the same protections. This file states those protections once. Each design links here instead of restating them.

## Rule 1: one folder per model run

Today `model::run` empties `$HIPPO_HOME/model-runs/` before every call (`crates/hippo-core/src/model.rs`, `run_with_timeout` → `empty_dir`) and nothing serialises calls. A nightly dream and an `ask` that overlap can delete each other's files. With today's `claude` argv nothing is written there, so the harm is small; any runner that writes a reply file or a prompt file makes it real.

The rule, for every model call hippo makes:

- The call runs in `$HIPPO_HOME/model-runs/<ulid>/`, created with mode 0700, and removed when the call ends, whether it succeeded or not. This is what the pack review already does with `review-runs/<ulid>/`.
- Nothing empties `model-runs/` as a whole. A call that starts removes sibling folders older than 24 hours, which only a crashed call leaves behind.
- Ingest already skips every file under `model-runs/`, nested folders included (`ingest.rs`, `MODEL_RUNS_DIR`); that stays.
- A test runs two calls at once with a stub CLI that writes `last.txt` and sleeps; each call reads its own reply and neither folder is touched by the other.

Ships in v0.5.0 as part of [the gate](#rule-4-the-hardening-gate), because it fixes today's code and every later runner relies on it.

## Rule 2: security settings in the shared `hippo.toml` need a version floor

`.hippo/hippo.toml` is read into `config::Workspace`, which does not deny unknown fields. An older hippo silently ignores any table it does not know. The four designs add security or privacy settings there: `[team]`, `[review] independent`, `[guard]`, `guard_note` and `[secrets]`. A teammate on an older hippo would ignore `dreamer = "alice"` and apply changes directly, or ignore `independent = "vendor"`.

The rule:

- **v0.5.0 adds `min_hippo_version`** to `hippo.toml`. A hippo older than the value refuses every command that writes a note or a shared record or calls a model (`dream`, `lessons`, `add`, `packs update`, `review`, `revert`), naming the version it needs. Commands that agents depend on (`hook`, `context`, `brief`, `guard check`) keep working, so an outdated teammate's agents are not stranded.
- **v0.5.0 also reports unknown keys.** A key or table in `hippo.toml` that this hippo does not know is a `doctor` problem, and the refusing commands above refuse on it too, with the same message. This covers a table added in a release after the reader's.
- **Writing a security setting raises the floor.** `hippo config set` of any setting in the [table below](#which-direction-is-loosening) also sets `min_hippo_version` to the first release that understands it, and prints both changes.
- **Records name their writer.** Shared records that lack it (proposals, receipts, review marks, pack reviews, lesson files) gain `hippo_version`, written right after `protocol`; readers ignore it. Machine-local state (`dream-state.json`, ingest state, session records, `origin.json`) does not carry it. From v0.6.0, `doctor`'s `team` row names a member whose latest record was written by a hippo below the floor.
- **`ask` refuses too** when it would call a model; `ask --sources-only` makes no model call and keeps working.

What this cannot do: a hippo older than v0.5.0 has neither check. For every table in these designs an older hippo behaves as if the feature did not exist, which is the state before the feature: no guard, no shared Inbox, no secrets part, a same-vendor reviewer. The one exception is `dreamer`: a pre-0.5 teammate applies changes directly, which brings back today's note conflicts, not a leak. The release notes for v0.5.0 say "everyone on a shared workspace should be on 0.5.0 before anyone turns on a team, guard or secrets setting".

## Rule 3: tightening takes effect at once; loosening needs a local accept on each machine

Shared settings have many writers: every teammate with push access, and every agent that can edit files in the workspace. An agent misled by text it fetched could loosen a rule. So:

- Each machine keeps `$HIPPO_HOME/accepted/<workspace-id>.toml` (0600): the last accepted value of every setting in the table below, and for the guard the hash of the last accepted compiled rule set. `<workspace-id>` is a ULID in `hippo.toml`'s `workspace_id`: `hippo init` writes it for a new workspace, and `hippo config set` of a setting in the table adds it to an older one. Until a workspace has one, its file is named `path-<hash of the workspace path>`, and it moves to the ULID the first time hippo reads the workspace with one. A machine with no workspace keeps the guard's hash in `accepted/machine.toml`.
- When hippo loads a setting, it compares the file's value with the accepted one, setting by setting. If the file is stricter or equal, the file's value applies at once and becomes the accepted value. If the file is looser, the accepted value stays in force and `doctor`'s `accept` row, `hippo accept --list` and Hippo.app (Manage › Health) say "loosening waiting: <setting>". A file that tightens one setting and loosens another gets the tightening at once; only the loosening waits. With no accepted file, every setting is compared with its default, so a cloned workspace whose file loosens a setting waits too.
- **Accepting.** Hippo.app's native dialog is the primary path: the `accept` row in Manage › Health has Accept…, which shows the difference with Accept and Keep current, and writes the accepted file through hippo's own code. The CLI's `hippo accept` shows the same difference and asks; it requires a terminal on stdin and stdout, and refuses when any agent marker is in its environment (the variables each supported agent sets in its children; the list lives in code, `accept::AGENT_MARKERS`, with a test per agent). There is no `--yes`. `hippo accept --list` only lists, and anything may run it. dsh, Muse and Command Code set no marker hippo has found, so a command they run is refused only for the missing terminal.
- **The guard protects the accept path** with built-in rules no note can lower ([guard.md](guard.md#self-protection)): an agent's call to `hippo accept`, `hippo guard accept` or `hippo guard allow`, and an agent's write under `$HIPPO_HOME/accepted/` or `$HIPPO_HOME/guard/`, are blocked.
- **The limit, stated plainly.** An agent with a shell runs as the person. It can unset its markers, run `script` to fake a terminal, or edit `$HIPPO_HOME` with a tool hippo does not see. This rule stops the casual and the injected-by-text cases and makes the rest visible in transcripts (the `guard-bypass` incident); it is not a security boundary.

### Which direction is loosening

| Setting | Tighter (applies at once) | Looser (waits for accept) |
|---|---|---|
| Guard rule set, compiled from every source (built-ins, machine rules, the note, packs) | a rule added; an action raised; an allow entry removed | an allow entry added; an action lowered; a rule removed; any edit to a `match` rule (counted as remove + add) |
| `guard_note` | — | any change |
| `[guard] fail` | `open` → `closed` | `closed` → `open` |
| `[guard] timeout_ms` | — | any change |
| `[review] independent` | `none` → `model` → `vendor` | the reverse |
| `[team] inbox` | `shared` → `local` | `local` → `shared` (also needs the person's own opt-in, [team-brain.md](team-brain.md#consent)) |
| `[team] dreamer` | — | any change, including setting it |
| `[team] apply_after_review` | `false` → `true` | `true` → `false` |
| `[secrets] runner`, its arguments | — | any change |
| `[secrets.names]` `use` entries, and their pinned hashes | an entry removed | an entry added; a pinned file's hash changed |
| `min_hippo_version` | raised | lowered |

A setting that is not in the table is not security-relevant and applies at once.

The accept machinery ships in v0.6.0, with its first users (guard rules and the shared Inbox). Before then no setting in the table exists. In v0.6.0 the rows that exist are the guard rule set, `guard_note`, `[guard] fail`, `[guard] timeout_ms`, `[team] inbox` and `min_hippo_version`; `[review] independent`, `[team] dreamer`, `[team] apply_after_review` and `[secrets]` arrive with their features, and until then a v0.6.0 hippo reports them as unknown keys (rule 2). A key this release does not know inside a table it does (`[team] dreamer`) counts as unknown too.

## Rule 4: the hardening gate

The guard log, the redaction rows and the shared Inbox each add a store of transcript-derived text, and the guard runs hippo on every tool call. Two things must be true first. Both ship in v0.5.0, and none of the four designs' features ships before them.

**`$HIPPO_HOME` permissions.** Today new files take the umask (usually 0644) and `write_atomic` keeps whatever mode the old file had.

- Every directory hippo creates under `$HIPPO_HOME` is 0700 and every file 0600, the index included. The CLI and Hippo.app set their umask to 077 first, which also covers files SQLite and child processes create. `write_atomic` creates its temporary file 0600 when there is no old file, and never widens an old mode.
- On start, a command that finds `$HIPPO_HOME` itself looser than 0700 tightens it and says so once.
- `doctor` reports any file or directory under `$HIPPO_HOME` that is group- or world-readable, and `hippo doctor --fix-permissions` tightens them.

**Hooks call hippo by absolute path, from per-person files.** Today `hooks install` writes the command `hippo hook …` into the workspace's `.claude/settings.json` (`crates/hippo-core/src/hooks.rs`, `settings_path`), which teams commit, and the Claude Code plugin's hook finds `hippo` through `PATH`. A fake `hippo` earlier on `PATH` would see every session start, and with the guard every tool call.

- `hooks install` for Claude Code writes to `.claude/settings.local.json` in the workspace, which is per person, instead of `settings.json`. It writes the absolute path of the binary that ran the install: the `hippo` link on `PATH` when that link resolves to this binary (a Homebrew or `~/.local/bin` link stays valid across upgrades, its target does not), the canonical path of the binary otherwise. `doctor` warns if git tracks `settings.local.json`.
- An existing hippo entry in `.claude/settings.json` is moved: `hooks install` prints the removal from `settings.json` and the addition to `settings.local.json`, and writes both with `--yes` (Hippo.app passes `--yes` after its own confirmation). A committed absolute path would break for teammates and name the owner's home folder, so hippo never writes one there.
- Codex's hooks are in the user's `$CODEX_HOME/hooks.json` ([codex-hooks.md](codex-hooks.md)), Cursor's in `~/.cursor/hooks.json` ([cursor-adapter.md](cursor-adapter.md)) and Gemini CLI's in `~/.gemini/settings.json` ([gemini-adapter.md](gemini-adapter.md)). All three are per person already; they get the absolute path too, and `hooks install` rewrites an earlier bare `hippo` command in them to the absolute path.
- `hippo mcp install` already writes the binary's absolute path into each MCP client's user file ([mcp-server.md](mcp-server.md)). `doctor` checks those entries against the checksum like the hooks.
- The plugins (Claude Code, dsh, OpenCode) ship in repositories, so they cannot carry a path. `hippo init`, `hippo hooks install` and `install.sh` write the binary's absolute path to `$HIPPO_HOME/bin-path` (0600). The plugins read that file and run the path in it; they fall back to `PATH` only when the file is missing, and `doctor` reports the fallback.
- `install.sh` verifies the release asset against the published checksum file before it writes the binary, and records the checksum in `$HIPPO_HOME/install.json`. `doctor` compares the binary the hooks name with that checksum. Signed and notarised releases stay a separate, later gate ([secrets.md](secrets.md#rollout) needs them; this does not).

## Release plan

Each line is one step that can ship on its own; a release is the set of steps that are ready. Steps inside a release are listed in build order.

### v0.5.0: the gate, and the parts that need nothing new from anyone

| Step | Design | Depends on |
|---|---|---|
| Per-run model folders | [Rule 1](#rule-1-one-folder-per-model-run) | — |
| `$HIPPO_HOME` permissions | [Rule 4](#rule-4-the-hardening-gate) | — |
| Hooks by absolute path, in per-person files; `bin-path`; checksum in `install.sh` and `doctor` | [Rule 4](#rule-4-the-hardening-gate) | — |
| `min_hippo_version`, `hippo_version` in records, unknown-key reporting | [Rule 2](#rule-2-security-settings-in-the-shared-hippotoml-need-a-version-floor) | — |
| Runners step 1: `claude` preset with accounts and fallback, `command` contract, OS sandbox, environment rule, migration | [runners.md](runners.md#rollout) | per-run folders |
| Guard Part A: transcript incidents `remote-script`, `fresh-clone-exec`, `credential-read`, `guard-bypass` | [guard.md](guard.md#rollout) | — |
| Secrets detection: redaction rows, `secret-to-model` | [secrets.md](secrets.md#rollout) | permissions (the rows are a new store) |

### v0.6.0: the first shared security settings

| Step | Design | Depends on |
|---|---|---|
| The accept machinery | [Rule 3](#rule-3-tightening-takes-effect-at-once-loosening-needs-a-local-accept-on-each-machine) | v0.5.0's version floor |
| Guard Part B, warn-only, machine-wide: `guard check`, `guard test`, self-protection rules, Claude Code accounts, then the dsh and OpenCode plugins, Codex if its contract is checked | [guard.md](guard.md#rollout) | accept machinery; hooks by absolute path |
| Team: `member`, lesson markers and backfill, shared Inbox with lesson identities and decisions, no quotes | [team-brain.md](team-brain.md#rollout) | accept machinery; version floor |

### Later, each when its trigger is met

| Step | Design | Trigger or gate |
|---|---|---|
| `[secrets]` references, start-block part, the guard's three secrets rules | [secrets.md](secrets.md#rollout) | guard warn-only in use |
| Other runner presets from verified `command` recipes; `[review] independent`; the app's Models card | [runners.md](runners.md#rollout) | each recipe passes the end-to-end check |
| Guard `ask` and `block` actions | [guard.md](guard.md#rollout) | `guard test` counts on real history |
| `unknown-egress`, `command-from-fetched` | [guard.md](guard.md#rollout) | tuning on real history |
| Quotes in the shared Inbox | [team-brain.md](team-brain.md#rollout) | a team asks for them |
| The team dreamer | [team-brain.md](team-brain.md#rollout) | a team reports note conflicts from dreaming |
| Brain templates | [team-brain.md](team-brain.md#rollout) | none; unscheduled |
| `hippo-secrets` helper | [secrets.md](secrets.md#rollout) | signed and notarised releases |

### Documentation each release updates

- v0.5.0: `security.md` list A (runners, accounts, fallback, the sandbox), "What is written where" (permissions, `bin-path`, `install.json`, redaction rows); `configuration.md` (`[runners]`, `[roles]`, `[fallback]`, `min_hippo_version`); `concepts.md` incident kinds.
- v0.6.0: `sharing.md` stops calling `git add -A` safe once `[team] inbox = "shared"` is set, and says to read `git status` first; `security.md` gains the guard and the accept rule; `configuration.md` gains `[team]`, `[guard]`, `guard_note`, `member`.
