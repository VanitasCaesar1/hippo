# Agent instructions

- `ROADMAP.md` says what is built and what is next (the docs site's `roadmap.md` includes it); `docs/site/src/architecture.md` says how it works.
- Design before code: a new feature or contract gets a design doc in `docs/designs/`, written for a public reader and reviewed before the code. Record formats, the pack format and the security model change only through one.
- Files are the contract (`HIPPO_V1`). Changing the shape of a note's frontmatter, a session record, a pack manifest, a dreaming report, the registry index or the web theme contract ([`docs/themes.md`](docs/themes.md)) is a design change, not a refactor.
- Rust, stable toolchain. `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings` and `cargo test --workspace --locked` must pass before a pull request. CI runs them on Linux and macOS, plus `gitleaks` and `cargo deny check`. For a change under `app/` (Hippo.app, its own Cargo workspace, macOS only), run `cargo fmt --all --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked` and `cargo deny check` in `app/`, and check the change by running the app (`app/scripts/demo-home.sh` seeds synthetic data), not only by its tests.
- No dreaming pass, pack loader or registry path may write without a fresh-context review recorded as a receipt. For packs, the receipt is the pack review: in `origin.json` for a pack `hippo add` or `hippo packs update` wrote, and in `registry/reviews/` for a listed pack. Never simulate a reviewer inside the writing context. In code, `notes::apply(&Confirmed)` is the only path by which dreaming writes a note, and only a confirming receipt yields a `Confirmed`.
- Redact on ingest: tokens, keys and personal data never enter the index. Every adapter ships with a redaction test.
- Packs do not execute. A pass may read only what its `reads` declares; the loader enforces it.
- Never copy private data, transcripts or notes from any other project into this repository, including as test fixtures.
- Keep `README.md` honest about status.
- One branch per change, pull request to `main`, squash merge, small diffs.
