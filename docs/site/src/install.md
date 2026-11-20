# Install

hippo is one binary. Install it with the script, with Cargo, or build it from a checkout.

## With the install script

On macOS (Apple silicon or Intel) and Linux (x86_64 or arm64):

```sh
curl -fsSL https://raw.githubusercontent.com/VanitasCaesar1/hippo/main/install.sh | sh
```

The script picks the binary for your machine from the latest [release](https://github.com/VanitasCaesar1/hippo/releases), checks it against the release's published SHA-256 (and installs nothing when it does not match), and copies it to `~/.local/bin`, without `sudo`. It records the binary's path and checksum in `~/.hippo/install.json`, which `hippo doctor` checks the binary against later, and the path in `~/.hippo/bin-path`, which the plugins run (both 0600; `HIPPO_HOME` moves them). If that folder is not on your `PATH`, it prints the line that adds it. Set `HIPPO_INSTALL_DIR` to install somewhere else, or `HIPPO_VERSION` to pick a release. The Linux binaries are static (musl), so they need no system libraries.

## With Cargo

With Rust 1.89 or later and a C compiler (hippo builds SQLite from source; on macOS the Xcode command line tools provide one):

```sh
cargo install --locked --git https://github.com/VanitasCaesar1/hippo hippo
```

This puts a `hippo` binary in `~/.cargo/bin`. From a checkout of the repository, `cargo install --locked --path crates/hippo` does the same, and `cargo build --release -p hippo --locked` builds `target/release/hippo` without installing it.

Check it:

```sh
hippo --version
```

```text
hippo 0.2.0
```

## In Claude Code

The repository is also a Claude Code plugin marketplace. In a Claude Code session, run:

```text
/plugin marketplace add VanitasCaesar1/hippo
/plugin install hippo@hippo
```

Then `/hippo:setup` does the first run with no terminal: it installs the binary with the script above if it is missing (after asking you), shows what [`hippo setup`](commands.md#hippo-setup) will do (the notes folder, Claude Code's hooks in the project's personal `.claude/settings.local.json`, the first index) and runs it on your yes, and offers the nightly schedule, which it adds only on a yes. Then a new session in the project starts with hippo's start block. `/hippo:ask <question>` asks whether an agent already solved something, and `/hippo:day [date]` shows a day's work. Once you have a notes folder (`hippo init`), the plugin gives every session in that workspace the start block; it stays silent everywhere else, and when `hippo hooks install` has already added the workspace's own hooks. The plugin's hook runs the binary named in `~/.hippo/bin-path`, and looks on `PATH` only when that file is missing.

## In Codex

The repository is also a Codex plugin marketplace. In the Codex app, open **Plugins**, choose **Add a marketplace**, enter `VanitasCaesar1/hippo` as the source (leave the Git ref empty), then click **+** on hippo and start a new chat. From a terminal:

```sh
codex plugin marketplace add VanitasCaesar1/hippo
codex plugin add hippo@hippo
```

The app and the CLI share their plugins. The plugin adds one skill, `$hippo`: ask it to *set up hippo for this project* and it installs the binary with the script above if it is missing (after asking you), creates the notes folder, adds hippo's Codex hooks, indexes the history already on the machine, and offers the nightly schedule, which it adds only on a yes. Codex lists the new hooks for review at its next start; they run once you trust them there (or with `/hooks`). The same skill asks past sessions (*has any agent already solved this?*), prints a brief for a task, and walks the Inbox. The plugin carries no hooks or MCP server of its own. To update it: `codex plugin marketplace upgrade hippo`; to remove it: `codex plugin remove hippo@hippo` and `codex plugin marketplace remove hippo`.

## In Cursor, Gemini CLI and other agents

Install the binary as above, then give the agent hippo's notes:

```sh
hippo hooks install --agent gemini            # Gemini CLI: start block and first-prompt notes
hippo hooks install --agent cursor            # Cursor: the start block in each new chat
hippo mcp install --client cursor --write     # any MCP client: hippo's tools (prints the entry without --write)
```

`hippo mcp install` takes `cursor`, `claude-desktop`, `gemini`, `windsurf`, `zed`, `vscode` or `claude-code`; [Any MCP client](mcp.md) lists each client's file and what the tools return. Restart the agent afterwards.

## Requirements

- For dreaming, and for `hippo ask`'s written answers, a model command. The default is the `claude` CLI (Claude Code), signed in. You can set another command; see [Configuration](configuration.md).
- Every command is meant to run on macOS and Linux; [Limits](limits.md) says how far Linux is tested. Scheduling installs launchd jobs on macOS. Elsewhere `hippo schedule install` prints cron and systemd lines for you to copy.

## macOS notes

`hippo schedule install` writes launchd jobs that run the absolute path of the binary that ran the install. If you later move or reinstall hippo to another path, run `hippo schedule install` again.

If your workspace or your transcripts are on an external volume, the first launchd run asks for access to "files on a removable volume". See [Scheduling on macOS](scheduling.md) for what that means and how to answer it.

## Next

Run `hippo`. It finds the history your agents left on this machine (Claude Code, Codex, DeepSeek Harness, OpenCode, Muse Code, Command Code, Cursor and Gemini CLI), indexes it and shows the last active day. Then go to the [Quick start](quick-start.md).
