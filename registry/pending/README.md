# Entries awaiting review

An entry here is not listed: `check` and `build` do not read this folder. It waits for a change its fresh-context review asked for.

- `incident-timeline`: its template has an imperative line that an agent reading an unedited copy would take as an instruction, and its test fixture folds three moments into one entry that the pack's own reviewer would reject. Reword the template and fix the fixture, then list it again.

To list an entry once its pack is fixed and the fix is on `main`:

```sh
scripts/registry-repin.sh VanitasCaesar1/hippocamp "$(git rev-parse HEAD)"
cargo run -p hippocamp-registry --locked -- review --entry registry/packs/<name>.toml
cargo run -p hippocamp-registry --locked -- check
```

The first command pins every entry in this folder to that commit and moves it to `registry/packs/`, so run it when only fixed entries are here. The review is written to `registry/reviews/`. The entry and its review go in one pull request.
