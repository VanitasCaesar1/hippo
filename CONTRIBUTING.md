# Contributing

Thanks for looking. hippocamp's core, CLI, dreaming, eval, packs and the desktop app are built, so the most useful contributions right now are packs for the notes your team keeps, design review, and adapters for tools you use.

To submit a pack to the registry, read [`registry/README.md`](registry/README.md): a pack lives in your own public GitHub repository, and a pull request here adds one entry file that pins it.

## Before you start

- Read the [roadmap](docs/site/src/roadmap.md) and [Architecture](docs/site/src/architecture.md).
- Open an issue before a large change. A new feature or contract starts as a design doc in [`docs/designs/`](docs/designs/README.md), written for a public reader and reviewed before the code. Record formats, the pack format and the security model change only through one.

## Pull requests

- One change per pull request, small enough to review in one sitting.
- Branch from `main`; pull requests are squash-merged.
- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings` and `cargo test --workspace --locked` must pass. CI runs the same commands on Linux and macOS, plus `gitleaks` and `cargo deny check`.
- For a change under `app/` (Hippocamp.app), run `cargo fmt --all --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked` and `cargo deny check` in `app/` on a Mac, and look at the running window: `app/scripts/demo-home.sh` seeds a synthetic home to run it against. See [`app/README.md`](app/README.md).
- A change to what a new user sees (`hippocamp` alone, `start`, `init`, `doctor`, errors and empty results) also runs `scripts/first-run-check.sh` after `cargo build -p hippocamp`: it walks the first run on synthetic data in a temporary home and fails on any dead end.
- New adapters include a redaction test. New record formats include a round-trip test.
- Describe what changed and why in the pull request. Link the issue or design doc.
- A change that moves the eval numbers updates the pinned lines in `crates/hippocamp/tests/eval.rs` and says why in the pull request. Answers in `fixtures/eval/set.toml` are never changed to match output.
- Before merging a change to `crates/hippocamp-core/packs/core/`, a maintainer runs `cargo run --release -p hippocamp -- eval fixtures/eval --with-model`, with `model_command` pinned to the latest baseline's `--model`, and compares the `dreaming:` line with that baseline in `docs/eval/`; CI does not measure prompts or personas.
- Add a line under *Unreleased* in [`CHANGELOG.md`](CHANGELOG.md) for anything a user would notice.

## Style

- Plain language in docs and comments. Say what the code does, not how clever it is.
- No fixtures that contain real transcripts, notes, keys or personal data from any project. Make them up.
- Prefer deleting code to adding it.

## Licence

By contributing you agree your contribution is licensed under the [MIT licence](LICENSE).
