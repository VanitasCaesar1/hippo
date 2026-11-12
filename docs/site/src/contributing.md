# Contributing and licence

## Contributing

Thanks for looking. The most useful contributions right now are design review and adapters for tools you use.

- Read the [roadmap](roadmap.md) and [Architecture](architecture.md) first.
- A new feature or contract starts as a design doc in `docs/designs/` in the repository, written for a public reader and reviewed before the code. Changing the shape of a note's frontmatter, a session record, a pass file, a dreaming report or the security model is a design change. Open an issue before a large change.
- One change per pull request, small enough to review in one sitting. Branch from `main`; pull requests are squash-merged.
- Before you open a pull request, these must pass:

  ```sh
  cargo fmt --all --check
  cargo clippy --workspace --all-targets --locked -- -D warnings
  cargo test --workspace --locked
  ```

  CI runs them on Linux and macOS, plus `gitleaks` and `cargo deny check`.
- A new adapter comes with a redaction test. A new record format comes with a round-trip test.
- A change that moves the eval numbers updates the pinned lines in `crates/hippo/tests/eval.rs` and says why. Answers in `fixtures/eval/set.toml` are never changed to match output.
- Add a line under *Unreleased* in `CHANGELOG.md` for anything a user would notice.
- Never put real transcripts, notes, keys or personal data from any project into the repository, including as test fixtures. Make fixtures up. Every `*.jsonl` fixture begins with the line `{"synthetic": true}`.
- Write plain language in docs and comments. Say what the code does.

## Principles

1. Files are the contract. The index can always be rebuilt from them.
2. Copy and own. Packs are source you keep and edit; hippo tracks where they came from.
3. Nothing promotes or publishes without a fresh-context review.
4. Measure it. An eval harness ships in the box.
5. Injected context has a hard budget that fails loudly.
6. One writer per artifact.

## Layout of the repository

```text
crates/hippo-core              the engine
crates/hippo-core/packs/core   the core pack that hippo init writes
crates/hippo                   the CLI
crates/hippo-theme             the web theme contract: stylesheet, built-in themes, CSS checks
crates/hippo-view              the data behind the desktop app's panels (no window code)
crates/hippo-registry          the registry tool: check, review, build the index and the site
crates/hippo-feed              the community feed server (not deployed)
app/                           Hippo.app, a macOS window on GPUI in its own Cargo workspace
packs/                         first-party packs
registry/                      the registry: entries, pack reviews, settings
fixtures/                      synthetic test data and the eval set
plugin/                        the Claude Code plugin
docs/site/                     this site: the docs (mdBook) and the landing page
docs/designs/                  design docs for new features, written before the code
docs/demo/                     a scripted demo on synthetic data and its recording
brand/                         mark and icon
```

Security problems go through private vulnerability reporting, not issues. See [Security and privacy](security.md#reporting-a-vulnerability).

## Licence

hippo is released under the MIT licence, copyright 2026 Vanitas Caesar. By contributing you agree that your contribution is licensed under the same terms.

The hippo mark is copyright Vanitas Caesar and may be used to refer to this project.
