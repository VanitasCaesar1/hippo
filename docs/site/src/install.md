# Install

hippo is one binary. Install it with the script, with Cargo, or build it from a checkout.

## With the install script

On macOS (Apple silicon or Intel) and Linux (x86_64 or arm64):

```sh
curl -fsSL https://raw.githubusercontent.com/VanitasCaesar1/hippo/main/install.sh | sh
```

The script picks the binary for your machine from the latest [release](https://github.com/VanitasCaesar1/hippo/releases), checks its SHA-256, and copies it to `~/.local/bin`, without `sudo`. If that folder is not on your `PATH`, it prints the line that adds it. Set `HIPPO_INSTALL_DIR` to install somewhere else, or `HIPPO_VERSION` to pick a release. The Linux binaries are static (musl), so they need no system libraries.

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

Then `/hippo:setup` installs the binary with the script above if it is missing (after asking you) and runs `hippo start`. `/hippo:ask <question>` asks whether an agent already solved something, and `/hippo:day [date]` shows a day's work. Once you have a notes folder (`hippo init`), the plugin gives every session in that workspace the start block; it stays silent everywhere else, and when `hippo hooks install` has already added the workspace's own hooks.

## Requirements

- For dreaming, and for `hippo ask`'s written answers, a model command. The default is the `claude` CLI (Claude Code), signed in. You can set another command; see [Configuration](configuration.md).
- Every command is meant to run on macOS and Linux; [Limits](limits.md) says how far Linux is tested. Scheduling installs launchd jobs on macOS. Elsewhere `hippo schedule install` prints cron and systemd lines for you to copy.

## macOS notes

`hippo schedule install` writes launchd jobs that run the absolute path of the binary that ran the install. If you later move or reinstall hippo to another path, run `hippo schedule install` again.

If your workspace or your transcripts are on an external volume, the first launchd run asks for access to "files on a removable volume". See [Scheduling on macOS](scheduling.md) for what that means and how to answer it.

## Next

Run `hippo`. It finds the history your agents left on this machine (Claude Code, Codex, DeepSeek Harness, OpenCode and Muse Code), indexes it and shows the last active day. Then go to the [Quick start](quick-start.md).
