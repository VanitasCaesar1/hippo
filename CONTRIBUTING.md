# Contributing

Thanks for looking. hippo is one person's project at version 0.2.0. The core, CLI, dreaming, eval, packs and the desktop app are built. What helps most now: packs for the notes your team keeps, adapters for agents hippo does not read yet, docs that are wrong or unclear, and bug reports from machines that are not mine.

## Get it running

You need Rust (the version in `rust-toolchain.toml`) and git. On macOS the synthetic home below also uses the system `sqlite3`.

```sh
git clone https://github.com/VanitasCaesar1/hippo
cd hippo
cargo build -p hippo
scripts/first-run-check.sh
```

`first-run-check.sh` walks a new user's first run on synthetic data in a temporary home and fails on any dead end. It sends nothing anywhere and touches nothing outside its temporary folder.

To poke at hippo by hand without touching your own agents' history or `~/.hippo`, point `HOME` and `HIPPO_HOME` at a scratch folder and build a synthetic home there:

```sh
scratch=$(mktemp -d)
HOME=$scratch HIPPO_BIN=$PWD/target/debug/hippo app/scripts/demo-home.sh "$scratch/demo"
export HOME=$scratch HIPPO_HOME=$scratch/demo/home
target/debug/hippo            # the overview
target/debug/hippo doctor
target/debug/hippo recall export
```

The home's model command is a stub in `fixtures/stub-model/`, so nothing goes to a real model. `hippo ask` still prints its ranked sources; the written answer fails with a stub error, which is expected. Do not run `hippo schedule install` or `hippo hooks install` against a scratch home: they write to launchd and to your agent's settings.

Hippo.app (macOS only) runs against the same synthetic home. Build it and start it as [`app/README.md`](app/README.md) says, under "Trying it on synthetic data".

## Where to start

- **A pack.** A pack lives in your own public GitHub repository, and a pull request here adds one entry file that pins it. Read [`registry/README.md`](registry/README.md) and [Writing a pack](docs/site/src/writing-packs.md). No pack by another author exists yet, so the first one will find the rough edges.
- **An adapter.** hippo reads Claude Code and Codex transcripts, DeepSeek Harness (dsh) session logs ([its design](docs/designs/dsh-adapter.md) shows what an adapter decides) OpenCode's database ([its design](docs/designs/opencode-adapter.md) covers a database that holds many sessions) and Muse Code session logs ([its design](docs/designs/muse-adapter.md) covers a closed-source tool). Cursor and Gemini folders are detected but not read. Open an issue first to agree on the record shape. A new adapter comes with a redaction test.
- **Docs.** The docs site's source is [`docs/site/src/`](docs/site/src/SUMMARY.md). If a page says one thing and hippo does another, that is a bug in one of them.
- Issues labelled [`good first issue`](https://github.com/VanitasCaesar1/hippo/labels/good%20first%20issue) or [`help wanted`](https://github.com/VanitasCaesar1/hippo/labels/help%20wanted), when there are any.

Read the [roadmap](ROADMAP.md) and [Architecture](docs/site/src/architecture.md) before a larger change.

## Reporting a bug

Open an issue with the bug template. Include:

- `hippo --version`, your OS and CPU, and how you installed hippo.
- Which agent and version wrote the transcripts.
- The output of `hippo doctor`. It prints paths on your machine; edit out anything you do not want public.
- What you ran, what happened and what you expected.

Never paste transcripts, notes, keys or tokens into an issue. If a bug only shows with real data, describe its shape or make up a small file that shows it.

Security problems go through private vulnerability reporting, not issues. See [`SECURITY.md`](SECURITY.md).

## Before a large change

Open an issue first. A new feature or contract starts as a design doc in [`docs/designs/`](docs/designs/README.md), written for a public reader and reviewed before the code. Record formats, the pack format and the security model change only through one.

## Pull requests

- One change per pull request, small enough to review in one sitting.
- Branch from `main`. Pull requests from contributors are squash-merged.
- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings` and `cargo test --workspace --locked` must pass. CI runs the same commands on Linux and macOS, plus `gitleaks` and `cargo deny check`.
- For a change under `app/` (Hippo.app), run `cargo fmt --all --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked` and `cargo deny check` in `app/` on a Mac, and look at the running window on the synthetic home.
- A change to what a new user sees (`hippo` alone, `start`, `init`, `doctor`, errors and empty results) also runs `scripts/first-run-check.sh` after `cargo build -p hippo`.
- New adapters include a redaction test. New record formats include a round-trip test.
- Describe what changed and why in the pull request. Link the issue or design doc.
- A change that moves the eval numbers updates the pinned lines in `crates/hippo/tests/eval.rs` and says why in the pull request. Answers in `fixtures/eval/set.toml` are never changed to match output.
- Before merging a change to `crates/hippo-core/packs/core/`, a maintainer runs `cargo run --release -p hippo -- eval fixtures/eval --with-model`, with `model_command` pinned to the latest baseline's `--model`, and compares the `dreaming:` line with that baseline in `docs/eval/`. CI does not measure prompts or personas.
- Add a line under *Unreleased* in [`CHANGELOG.md`](CHANGELOG.md) for anything a user would notice.

## Style

- Plain language in docs and comments. Say what the code does, not how clever it is.
- No fixtures that contain real transcripts, notes, keys or personal data from any project. Make them up. Every `*.jsonl` fixture begins with the line `{"synthetic": true}`.
- Prefer deleting code to adding it.

## Conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md).

## Licence

By contributing you agree your contribution is licensed under the [MIT licence](LICENSE).
