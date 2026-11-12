# Entries awaiting review

These are the entries for the 26 first-party packs in `packs/` (23 pass packs and 3 theme packs). They are not listed yet: an entry pins a repository id and a commit, and these must pin the public repository, which needs its own id and history. Until then `check` and `build` do not read this folder, so the registry lists no pack.

To list them, from a checkout of `main` in the public repository:

```sh
scripts/registry-repin.sh VanitasCaesar1/hippocamp "$(git rev-parse HEAD)"
for f in registry/packs/*.toml; do cargo run -p hippocamp-registry --locked -- review --entry "$f"; done
```

The first command writes the repository's id, its owner's id and the commit into every entry here and moves them to `registry/packs/`. The second runs the fresh-context review of each entry and writes it to `registry/reviews/`. Then `cargo run -p hippocamp-registry -- check` passes, and the entries and reviews go in one pull request.
