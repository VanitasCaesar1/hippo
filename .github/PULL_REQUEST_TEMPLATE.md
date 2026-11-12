## What

## Why

<!-- Link the issue or design doc. -->

## Checks run

- [ ] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings` and `cargo test --workspace --locked`
- [ ] For `app/`: the same commands and `cargo deny check` in `app/`, and the change seen in the running app
- [ ] For what a new user sees: `scripts/first-run-check.sh`
- [ ] A line under *Unreleased* in `CHANGELOG.md` if a user would notice
