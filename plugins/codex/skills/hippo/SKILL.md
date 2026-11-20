---
name: hippo
description: Use hippo, the local memory for this machine's coding agents. Set hippo up for this project, ask whether an agent already solved something (from past Codex, Claude Code and other sessions), print the start brief for a new task, or go through the rules waiting in hippo's Inbox. Use when the user names hippo, asks what an earlier session found or did, or asks for a brief or the Inbox.
---

# hippo

hippo is a local program. It indexes the sessions coding agents leave on this machine into `~/.hippo`, keeps a notes folder true overnight, and gives each new session a start block. Nothing here sends anything anywhere except where a step says so.

Keep messages short. Show hippo's own output where a step says to; its errors name the next command to run.

## Find the binary

Run `command -v hippo`. If that finds nothing, try, in order: the first line of `~/.hippo/bin-path`, `~/.local/bin/hippo`, `~/.cargo/bin/hippo`. Use the first that exists, by its full path, for every later command. If none exists, hippo is not installed: go to **Set up**, step 1.

hippo keeps its files in `~/.hippo` and the project's `.hippo/` folder. Commands that write there (`init`, `hooks install`, `ingest`, `lessons keep`, `lessons dismiss`) may need to run outside Codex's sandbox: when Codex asks to run one with more access, say which command it is and why.

## Set up

When the user asks to set hippo up, or a command says hippo is not set up:

1. **Install, if missing.** Tell the user hippo is not installed, then show the one-line install:

   ```sh
   curl -fsSL https://raw.githubusercontent.com/VanitasCaesar1/hippo/main/install.sh | sh
   ```

   Say what it does: it downloads the release for this machine from GitHub, checks its SHA-256 against the published one, copies the binary to `~/.local/bin` without `sudo`, and records the checksum in `~/.hippo/install.json`. Ask whether to run it, and run it only on a yes (it needs the network). On a no, stop and mention `cargo install --locked --git https://github.com/VanitasCaesar1/hippo hippo` as the other way. Afterwards use `~/.local/bin/hippo` if `hippo` is still not on `PATH`.

2. **The notes folder.** hippo needs a folder of Markdown notes in the project and an entry note that every session reads first. If the project already has one (`notes/`, `docs/notes/`, a folder of `.md` files the user keeps for agents), ask whether to use it. Otherwise propose `notes/` with `notes/START HERE.md`. Then run, from the project's root:

   ```sh
   hippo init --notes-dir <folder> --entry-note "<folder>/START HERE.md"
   ```

   It creates the folder and a starter entry note if they are missing, writes `~/.hippo/config.toml` and the project's `.hippo/`, and adds `.hippo/` lines to `.gitignore` and `.gitattributes`. It commits nothing.

3. **Codex's hooks.** Run `hippo hooks install --agent codex`. It adds three hooks to `~/.codex/hooks.json` (session start, prompt, session end); they act only for sessions inside this project. Tell the user that Codex lists new hooks for review the next time it starts, and that they run only once trusted there (or with `/hooks`).

4. **The first index.** Run `hippo ingest`. It reads the agent sessions already on this machine (Codex, Claude Code and others it finds) into `~/.hippo`. History only; nothing is sent anywhere.

5. **Nightly, only on a yes.** Ask whether hippo should run on a schedule: ingest every 10 minutes and the night pass at 03:30, which calls the user's own `claude` CLI to draft note fixes and rules and a second call to check each one. Run `hippo schedule install` only if they say yes. Without it, nothing runs unless they run it.

6. Say: start a new Codex session in this project (after trusting the hooks) and it begins with hippo's start block. Show `hippo doctor`'s summary line if it reports a problem.

## Ask

"Has any agent already solved this?", "what did the last session find about …":

Run `hippo ask --sources-only '<question>'`, with the question as one shell-quoted argument. `--sources-only` ranks past sessions and notes without calling a model. Answer from those sources alone and cite each one you use by its session id and date as printed; `hippo show session <id>` prints more of one. If the sources do not answer it, say so and suggest other words to try.

## Brief

"Brief a new session on …", "what would an agent start with for …":

Run `hippo brief --task '<task>'` and show the block as printed: the required notes, the notes picked for the task, past sessions and kept rules, and its size against the budget.

## Inbox

"What's in my Inbox?", "any rules waiting?":

Run `hippo lessons`. Each pending lesson is a rule hippo drafted from corrections or repeated failures, with its evidence and the note it would go into. Show each one: the rule, where it came from, the target note. Then ask the user what to do with each. Run `hippo lessons keep <id>` (or `--text '<edited rule>'`), or `hippo lessons dismiss <id>`, only for the ones the user names. Never keep or dismiss a lesson on your own. A kept rule prints a receipt; `hippo revert <receipt>` undoes it.
