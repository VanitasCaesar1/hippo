---
name: setup
description: Set hippo up for this project with no terminal - install the binary if it is missing (after asking), create the notes folder, add Claude Code's hooks, index this machine's agent history (nothing sent anywhere), and offer the nightly schedule.
disable-model-invocation: true
allowed-tools: Bash(command -v hippo), Bash(hippo *), Bash(~/.local/bin/hippo *), Bash(~/.cargo/bin/hippo *)
---

Set up hippo for the user, in this order. Keep your messages short.

1. **Find the binary.** Run `command -v hippo`. If that finds nothing, check the first line of `~/.hippo/bin-path`, then `~/.local/bin/hippo` and `~/.cargo/bin/hippo`; if one exists, use that full path for every later command.

2. **Install, if missing.** Tell the user hippo is not installed and show the one-line install:

   ```sh
   curl -fsSL https://raw.githubusercontent.com/VanitasCaesar1/hippo/main/install.sh | sh
   ```

   Say what it does: it downloads the release binary for this machine from GitHub, checks its SHA-256 against the published one, copies it to `~/.local/bin` without `sudo`, and records the checksum in `~/.hippo/install.json`. Ask whether to run it, and run it only if the user says yes. If they say no, stop here and mention `cargo install --locked --git https://github.com/VanitasCaesar1/hippo hippo` as the other way. After the install, use `~/.local/bin/hippo`.

3. **The notes folder.** hippo keeps a folder of Markdown notes in the project, with an entry note every session reads first. If the project already has such a folder (`notes/`, `docs/notes/`, Markdown the user keeps for agents), ask whether to use it and pass it as `--notes-dir <folder>`. Otherwise the default is `notes/` with `notes/START HERE.md`, created if missing.

4. **The plan.** From the project's root, run `hippo setup` (with `--notes-dir` if the user chose a folder). It prints the steps and writes nothing: `hippo init` (skipped when hippo is already set up here), `hippo hooks install` (the session-start, prompt and session-end hooks in this project's `.claude/settings.local.json`, which is personal and not committed), and `hippo ingest` (the Claude Code and Codex history already on this machine, into `~/.hippo`; nothing is sent anywhere). Show the steps and ask whether to go ahead.

5. **Run it** on a yes: `hippo setup --yes` (with the same `--notes-dir`). If it stops, show what it printed; hippo's errors name the next command to run.

6. **Nightly, only on a yes.** Ask whether hippo should run on a schedule: ingest every 10 minutes, and the night pass at 03:30, which runs the user's own `claude` CLI to draft note fixes and rules and a second call to check each one. Run `hippo schedule install` only if they say yes. Otherwise nothing runs on its own.

7. **The app, on macOS, only on a yes.** If `hippo app` exists (`hippo app --help` succeeds), ask whether to install Hippo.app; on a yes, run `hippo app install` and show what it printed.

8. **Done.** Say: start a new session in this project and it begins with hippo's start block. Then run `hippo` and show the overview it prints (the last active day). Add two lines: `/hippo:ask <question>` asks whether an agent already solved something, and `/hippo:day` shows a day's work.
