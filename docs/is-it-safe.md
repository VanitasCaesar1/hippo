# Is it safe?

hippo puts hooks in your agents, can serve an MCP server, runs a binary on a schedule if you ask, and runs a model at night. This page lists exactly what each of those does, so you can decide. It is kept true by a test, `crates/hippo/tests/is_it_safe.rs`, which fails when the code gains a hook, an MCP tool, a program it starts or an outbound host that this page does not name.

Short version: hippo's own code has no HTTP client and opens no network connection. Everything it writes is under `~/.hippo`, your project's `.hippo/` and notes folder, and the agent settings files listed below. Data leaves your machine only through the model command you configure (by default your own `claude -p`, with no tools), `git` when you fetch a pack, and `curl` when you install hippo or Hippo.app.

## Hooks

Hooks are added only by commands you run (`hippo setup --yes`, `hippo hooks install`, `hippo guard install --yes`) or by installing the Claude Code plugin. Every hook runs the hippo binary by its absolute path, never a bare `hippo` from `PATH`, and a hook never fails your session: on any error it prints one line to stderr and exits 0. No hook calls a model.

### Session hooks (`hippo hooks install`, `hippo setup`)

| Agent | File | Event | Runs |
|---|---|---|---|
| Claude Code | `<project>/.claude/settings.local.json` (personal, not committed; never the shared `settings.json`) | `SessionStart` | `hippo hook session-start` |
| | | `UserPromptSubmit` | `hippo hook prompt` |
| | | `SessionEnd` | `hippo hook session-end` |
| Codex | `$CODEX_HOME/hooks.json` (`~/.codex/hooks.json`); runs only once you trust it in Codex's review | `SessionStart` | `hippo hook session-start --agent codex` |
| | | `UserPromptSubmit` | `hippo hook prompt --agent codex` |
| | | `SessionEnd` | `hippo hook session-end --agent codex` |
| Gemini CLI | `~/.gemini/settings.json` | `SessionStart` | `hippo hook session-start --agent gemini` |
| | | `BeforeAgent` | `hippo hook prompt --agent gemini` |
| | | `SessionEnd` | `hippo hook session-end --agent gemini` |
| Cursor | `~/.cursor/hooks.json` | `sessionStart` | `hippo hook session-start --agent cursor` |
| Claude Code, user (`hippo hooks install --user`) | `$CLAUDE_CONFIG_DIR/settings.json` (`~/.claude/settings.json`) | `SessionStart` | `hippo hook session-start --rules-only` |
| Claude Code plugin | `plugin/hooks/hooks.json`, inside the plugin | `SessionStart` | `hippo hook session-start --plugin` (the binary named in `~/.hippo/bin-path`) |

What they read and write:

- **session-start** reads the hook's JSON on stdin (session id, working directory), your notes and `~/.hippo`. It prints the start block (the entry note, the required notes, the rules you kept) for the agent to add to the session. For Claude Code, Codex and Gemini CLI it writes one session record, `<project>/.hippo/sessions/<id>.json`, which is ignored by git, and a small state file, `~/.hippo/hook-state/<id>.json` (prompt count, the words of recent prompts, what was given; removed after 14 days). For a session outside hippo's workspace it prints only your machine-wide rules and writes no record; `--rules-only` does the same for the Claude Code user hook, and prints nothing inside the workspace when the workspace hook is there.
- **prompt** on the first prompt prints the notes relevant to the task and adds the task to the same record. On a later prompt it prints something only when the topic changed (new words, at least 5 prompts since the last time): the notes and kept rules for the new topic not given yet.
- **Every output** adds one line to `~/.hippo/logs/injections.jsonl`: the session id, the agent, which rules and notes it gave and its size, never your prompt.
- **session-end** marks the record ended. For Codex and Gemini CLI it reads the session's transcript (read only) to keep the last answer as the handoff.
- **The Cursor hook and the plugin's hook** print the start block (outside the workspace, the machine rules) and write only the delivery log line. The plugin's hook stays silent when a settings file already runs hippo's own hook, so a session gets the block once.

### The guard (`hippo guard install --yes`)

Off until you install it. Warn-only: it flags, it does not stop a tool call, except that it refuses calls that would switch the guard itself off.

| Agent | File | Event | Runs |
|---|---|---|---|
| Claude Code | each account's `settings.json` (`$CLAUDE_CONFIG_DIR`, `~/.claude`, `~/.claude-*`) | `PreToolUse` | `hippo guard check --agent claude-code` |
| Codex | `$CODEX_HOME/hooks.json` | `PreToolUse` | `hippo guard check --agent codex` |
| Gemini CLI | `~/.gemini/settings.json` | `BeforeTool` | `hippo guard check --agent gemini` |
| Cursor | `~/.cursor/hooks.json` | `preToolUse` | `hippo guard check --agent cursor` |

`guard check` reads the tool call on stdin and the compiled rules, answers within its time limit, uses no model, and writes only under `~/.hippo/guard/` (its log, failures and per-day call counts). It keeps a command and a path, never a secret's value. The OpenCode and DeepSeek Harness plugins can run the same check if you turn `guard` on in their settings.

### The other plugins

- **Codex plugin** (`plugins/codex`): one skill, no hooks and no MCP server. The skill runs hippo's own commands and asks before installing or scheduling anything.
- **OpenCode and DeepSeek Harness plugins**: read-only. They run `hippo context` for the start block and `hippo ask --sources-only` and `hippo show day` as tools, with `execFile`, never a shell.

## The MCP server (`hippo mcp`)

JSON-RPC over stdio: it listens on no port. `hippo mcp install --client <client>` prints the configuration; with `--write` it adds it to that client's file and keeps a backup copy.

| Tool | Does |
|---|---|
| `hippo_context` | The start block. Read-only. |
| `hippo_recall` | Searches indexed turns. Read-only. |
| `hippo_ask` | Ranked past sessions and notes, with no model call. Read-only. |
| `hippo_day` | A day's sessions. Read-only. |
| `hippo_incidents` | What went wrong. Read-only. |
| `hippo_note_correction` | Listed only with `--allow-write`. Drafts a lesson from a correction you report and runs the lesson reviewer (one model call); it lands in your Inbox, never straight in a note. |

Every answer is redacted before it is sent to the client.

## Programs hippo starts

hippo starts no program from a hook except itself. The commands below start other programs, each for the reason given.

| Program | Started by | For |
|---|---|---|
| The model command (`claude` by default) | `hippo dream` (also `--dry-run` and the scheduled one), `lessons scan`, `lessons propose`, `ask` without `--sources-only`, the pack review in `add` and `packs update`, `eval --with-model`, `doctor --model`, MCP `hippo_note_correction` | The night pass, rule drafts, their reviews and written answers |
| `sandbox-exec` (macOS) or `bwrap` (Linux) | the model command | A file sandbox around it (see below) |
| `git` | `hippo add` and `packs update` from a URL; `doctor`; team lessons; Hippo.app's Brief | Fetching a pack over https with hooks off (`init --bare`, `fetch --depth 1`, `ls-tree`, `cat-file`, `diff --no-index`); `rev-parse`, `ls-files`, `check-ignore` for checks; `log -1` and `config user.email` to say who shared a lesson; `remote get-url` |
| `launchctl`, `id` | `hippo schedule install`, `uninstall`, `status` | Loading and unloading the two launchd jobs, `dev.hippo.ingest` and `dev.hippo.dream`, for your user |
| `plutil` | `hippo add`, Hippo.app | Reading which binary the scheduled night pass runs |
| `curl`, `tar`, `pgrep` | `hippo app install` | Downloading Hippo.app, unpacking it, telling you to reopen it if it is running |
| hippo itself | `hippo setup` (`init`, `hooks install`, `ingest`), Hippo.app (the commands behind its buttons) | Running hippo's own steps |
| `open`, `ps`, `lsof`, `defaults`, `kill` | Hippo.app | Opening a session's thread, terminal or T3 Code; finding a running Codex session's folder; reading Reduce motion; stopping a hippo command it started that ran past its time limit |
| `claude` or `codex`, in Terminal | Hippo.app's Brief and Guard's Fix with, when you press the button | A new session started with the brief it showed you first |

Programs hippo starts by a name it does not hard-code: the model command and any `[runners]` you configure (`model.rs`), the hippo binary (`setup.rs`, and the app's `cli.rs`, which also runs the `plutil` it is given).

### The model call

By default the night pass and written answers run:

```text
claude -p --no-session-persistence --setting-sources "" --tools "" --strict-mcp-config --system-prompt <hippo's prompt> --output-format json
```

under your own Claude login: no tools, no MCP servers, none of your settings, nothing saved to your history. The prompt goes in on stdin and marks every piece of transcript and note text as untrusted data. The environment is cleared to a short list (`PATH`, `HOME`, locale, proxy and certificate variables, and for `claude` its own login and provider variables). On macOS it runs inside `sandbox-exec`, which denies reading your home folder, `~/.hippo` and the workspace and writing anywhere but its run folder, temporary folders and Claude's own account files; on Linux inside `bwrap` when available. The sandbox narrows what an injected instruction could reach; it is not a security boundary, and network stays open because the model needs it. A model command you set yourself in the older `model_command` key runs without the sandbox and with your whole environment, as before 0.5.0; `[runners]` sandbox a `command` runner unless you set `allow_unsandboxed`.

What the model is sent: excerpts of the day's sessions (redacted at index time, by pattern: keys, tokens, emails and more; best effort) and the notes being checked.

## Network

hippo's own code opens no connection; none of the CLI's or Hippo.app's crates depend on an HTTP client. Through the programs above:

- **The model command** reaches its provider (Anthropic for `claude`), under your login.
- **`git`** reaches the pack's host over https only (`protocol.allow=never`, `https` allowed).
- **`curl`** reaches `github.com` (and the release-asset hosts it redirects to) for `hippo app install`; `install.sh` reaches `github.com` and `raw.githubusercontent.com`.

Hosts that appear in hippo's code: `github.com` (release downloads and pack and pull-request links), `www.apple.com` (the doctype line of a launchd plist; never fetched), and `example.com` in placeholders. There is no telemetry, no crash reporting and no update check.

`crates/hippo-feed` is a separate web server for a hosted pack feed; it is not part of the `hippo` binary or Hippo.app and is not installed by any of the above.

## Agent memories hippo can read

`hippo lessons sources` lists the rules files and memories of your agents that hippo finds by name: each Claude Code account's `CLAUDE.md` and its auto-memory files, `$CODEX_HOME/AGENTS.md`, `~/.cursor/rules/`, `~/.gemini/GEMINI.md`. Listing them reads folder names only. hippo reads a source's files only after you turn it on (`hippo lessons sources --on <id>`, in a terminal; the guard blocks an agent's attempt), never writes them, and sends their rule lines, redacted, only to your lessons and review runners, as it sends transcript quotes ([remember.md](designs/remember.md#rules-from-where-they-already-live)).

## Files written outside `~/.hippo` and your project

- The hooks files above, merged with what is there (written atomically; a file that is not a JSON object is refused and left alone). `hippo hooks uninstall` removes only hippo's entries.
- `~/Library/LaunchAgents/dev.hippo.ingest.plist` and `dev.hippo.dream.plist`, by `hippo schedule install` only.
- MCP client files, by `hippo mcp install --write` only, with a backup.
- `~/Applications/Hippo.app` (or `--dir`), by `hippo app install` only.
- `~/.local/bin/hippo`, by `install.sh`.
- In your project, by `hippo init` (and `hippo setup`): the notes folder and a starter entry note if missing, `.hippo/` with its own `.gitignore`, one line in `.gitignore` and one in `.gitattributes`.

## Checksums

`install.sh` checks the downloaded release against its published `.sha256` before installing, then records in `~/.hippo/install.json` (mode 0600) the version, the binary's path and its sha256, and the asset's sha256. `hippo app install` adds an `app` entry with the bundle's path and its binary's sha256. `hippo doctor` compares both with what is on disk (its `install` and `app` rows), and warns when a hook or `bin-path` names a different binary.

To check by hand:

```sh
shasum -a 256 ~/.local/bin/hippo            # compare with "sha256" in ~/.hippo/install.json
curl -fsSL https://github.com/VanitasCaesar1/hippo/releases/download/v<version>/hippo-<version>-<target>.tar.gz.sha256
hippo doctor
```

Hippo.app is signed ad hoc and not notarised: the checksum, not Apple, is what says it is the release's ([app-download.md](designs/app-download.md)).

## What hippo never does

- It never runs `git commit`, `git push`, `git add` or `git pull`, and never commits your notes for you.
- It never writes into an agent's transcript or session; it reads them.
- It never schedules anything you did not run `hippo schedule install` for, and `hippo setup` never does.
- It never changes a file's quarantine attribute or other extended attributes, and never needs `sudo`.
- It never sends telemetry or checks for updates.
- No hook calls a model, and no change reaches a note without a second model's confirmation recorded as a receipt.
- It never writes an agent's memory or rules files (`CLAUDE.md`, `AGENTS.md`, `GEMINI.md`, Cursor rules, auto-memory).
- It never writes Codex's `config.toml` or the trust Codex keeps for hooks: trusting hippo's hooks is your decision in Codex.

Found something this page gets wrong? Report it privately: [SECURITY.md](../SECURITY.md).
