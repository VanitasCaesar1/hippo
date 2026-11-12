## What

## Why

## Checks

- [ ] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings` and `cargo test --workspace --locked` pass locally
- [ ] For `app/`: the same four commands in `app/` (with `cargo deny check`), and the change seen in the running app
- [ ] CI is green, including `gitleaks` and `cargo deny check`
- [ ] Linked issue or design doc
- [ ] `CHANGELOG.md` updated if a user would notice
