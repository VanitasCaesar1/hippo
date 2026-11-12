---
name: setup
description: Install hippocamp if it is missing, index this machine's coding-agent history (history only, nothing sent anywhere) and show the last active day.
disable-model-invocation: true
allowed-tools: Bash(command -v hippocamp), Bash(hippocamp *), Bash(~/.local/bin/hippocamp *), Bash(~/.cargo/bin/hippocamp *)
---

Set up hippocamp for the user, in this order. Keep your messages short.

1. Find the binary. Run `command -v hippocamp`. If that finds nothing, check `~/.local/bin/hippocamp` and `~/.cargo/bin/hippocamp`; if one exists, use that full path for every later command.

2. If there is no binary, tell the user hippocamp is not installed and show the one-line install:

   ```sh
   curl -fsSL https://raw.githubusercontent.com/VanitasCaesar1/hippocamp/main/install.sh | sh
   ```

   Say what it does: it downloads the release binary for this machine from GitHub, checks its SHA-256, and copies it to `~/.local/bin` without `sudo`. Ask whether to run it, and run it only if the user says yes. If they say no, stop here and mention `cargo install --locked --git https://github.com/VanitasCaesar1/hippocamp hippocamp` as the other way. After the install, use `~/.local/bin/hippocamp` if `hippocamp` is still not on `PATH`, and repeat the `PATH` line the script printed so the user can add it.

3. Say in one sentence what comes next: hippocamp reads the Claude Code and Codex transcripts already on this machine and indexes them into `~/.hippocamp`, history only, with nothing sent anywhere. Then run `hippocamp start --yes`. If hippocamp was already set up, the same command says so and shows the overview.

4. Show the overview it printed as it is (it ends with commands to try). Then add two lines: `/hippocamp:ask <question>` asks whether an agent already solved something, and `/hippocamp:day` shows a day's work. If the user wants their notes given to every session, point them at `hippocamp init --notes-dir <folder> --entry-note "<folder>/START HERE.md"` run in their workspace; this plugin then gives sessions in that workspace the start block.

If a command fails, show its message; hippocamp's errors name the next command to run.
