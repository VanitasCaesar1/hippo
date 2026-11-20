# Setup with no terminal

Status: accepted, to build in 0.7.1.

## The problem

Getting from "I installed the plugin" to "my next session starts with my notes" took four commands typed in a terminal: the install script, `hippo init` with two flags, `hippo hooks install` with the right `--agent`, and `hippo ingest`. The Claude Code plugin's `/hippo:setup` ran `hippo start`, which indexes history but sets up no notes folder and no hooks, so a new session still got nothing. Codex users had no way in at all before the [Codex plugin](codex-plugin.md).

## What changes for a user

In Claude Code, `/hippo:setup`; in Codex, *set up hippo for this project* (the `$hippo` skill). Either one, in the agent, with no terminal:

1. Finds the binary, or shows the one-line install, says what it does (download from GitHub, SHA-256 check against the published sum, `~/.local/bin`, no `sudo`, the checksum recorded in `~/.hippo/install.json`) and runs it only on a yes.
2. Asks about an existing notes folder; the default is `notes/` with `notes/START HERE.md`.
3. Runs `hippo setup --agent <agent>`, which prints the steps and writes nothing, shows them, and on a yes runs `hippo setup --agent <agent> --yes`: `hippo init`, `hippo hooks install --agent <agent>`, `hippo ingest`.
4. Asks whether to schedule ingest and the night pass, and runs `hippo schedule install` only on a yes. Nothing is scheduled otherwise.
5. On macOS, offers Hippo.app ([app-download.md](app-download.md)), only on a yes.
6. Says: start a new session here and it begins with the start block. For Codex: after trusting hippo's hooks in Codex's review.

## hippo setup

One command composes the existing ones, so the skills stay short and the first run can be tested without a model: `hippo setup [--agent claude-code|codex] [--workspace <dir>] [--notes-dir <dir>] [--entry-note <note>] [--yes]`. It runs each step as the same binary (`current_exe`), from the project, and stops at the first failure with that command's own message. It follows hippo's rule for commands that write: print without `--yes`, write with it.

- **The project** is `--workspace`, else the Git root around the current directory, else the current directory.
- **Init is skipped** when the configured workspace is this project. When it is another folder, setup exits 1 and names the `hippo init … --force` that would move it, rather than moving it: hippo has one workspace per machine, and replacing it silently would leave the first project's sessions without a start block.
- **Hooks** are the agent's: Claude Code's go to the project's `.claude/settings.local.json` (personal; the plugin's own hook then stays silent, so the block is given once), Codex's to `$CODEX_HOME/hooks.json`.
- **No schedule.** `hippo schedule install` writes launchd jobs that run a model every night; it stays a separate step the person agrees to.

No record format changes.

## Tests

- `crates/hippo/tests/setup.rs`: in a scratch home with synthetic history and a Git project with no notes. Without `--yes`, nothing is written. With `--yes` for Codex and for Claude Code, a stub agent then starts a session the way the agent does: it reads the hooks file setup wrote and runs the session-start command with the hook's JSON on stdin; the output carries the entry note. The Claude Code plugin's own hook stays silent there. A second run adds nothing; a second project is named, not adopted.
- `scripts/setup-e2e/run.sh <hippo> [codex] [claude]`, by hand before a release: a scratch machine with no hippo; `install.sh` from a release served on 127.0.0.1 (the checksum checked and recorded), `hippo setup --yes` per agent, then the real `codex exec` (with `--dangerously-bypass-hook-trust` standing in for the person's trust) and `claude -p --plugin-dir plugin` against stub models; the first request each agent sends must carry the entry note. Run on 20 November 2026 with codex-cli 0.170.0 and Claude Code 2.1.328: both passed.
