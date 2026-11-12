# Entries awaiting review

An entry here is not listed: `check` and `build` do not read this folder. It waits for a change its fresh-context review asked for.

The 26 first-party entries wait here while they are re-pinned to a commit on `main` and reviewed again: the rename to hippo changed every pack's content hash and the reviewer persona's hash.

To list an entry once its pack is fixed and the fix is on `main`:

```sh
scripts/registry-repin.sh VanitasCaesar1/hippo "$(git rev-parse HEAD)"
cargo run -p hippo-registry --locked -- review --entry registry/packs/<name>.toml
cargo run -p hippo-registry --locked -- check
```

The first command pins every entry in this folder to that commit and moves it to `registry/packs/`, so run it when only fixed entries are here. The review is written to `registry/reviews/`. The entry and its review go in one pull request.
