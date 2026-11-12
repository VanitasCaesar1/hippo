# The hippo pack registry

This directory lists packs from public GitHub repositories. A pack is text that hippo copies into a workspace and never runs. Nothing is listed without a review.

- **Submit a pack.** Put the pack in a public GitHub repository with a `LICENSE` (and fixture tests, for a pass pack). Open a pull request that adds only one file, `registry/packs/<name>.toml`, with the ids from `gh api repos/<owner>/<repo> --jq '.id, .owner.id'`. Outside pull requests that change anything else fail.
- **What `check` does.** `cargo run -p hippo-registry -- check` reads each entry, fetches the pinned commit, validates the pack, checks its name, licence and `LICENSE` file, runs its fixture tests against a stub model, looks for a confirming review of exactly that content, rejects names confusable with another entry's and pass names used twice, and (unless `--offline`) asks GitHub that the repository and its owner are still the ones the entry names.
- **The review.** A registry maintainer runs the fresh-context review from a checkout of `main` (`hippo-registry review --entry <file>`), commits the review it writes to `registry/reviews/`, and deletes any review file a submitter adds. A review file is a claim that `check` can verify only in form, so only a maintainer's own run counts.
- **Update a pack.** Change the entry's `commit`. A pin whose content changed needs a new review; a pin with identical content keeps its reviews.
- **An entry that starts failing** (its repository moved, its pin is gone, its content no longer passes) is unlisted, and every registry pull request fails until the entry is re-pinned or deleted.
- **A version found bad** is unlisted by deleting its entry, or re-pinning it to fixed content with a new review, in a reviewed pull request. The next `build` leaves it out.
- **Nothing is listed without a review.** A rejected review blocks that content until it changes or a maintainer deletes the reject in a pull request.

- **Entries not listed yet** wait in `registry/pending/`, which `check` and `build` do not read. An entry stays there until its review confirms it; [`pending/README.md`](pending/README.md) says how to list one, with `scripts/registry-repin.sh` and a review.

`registry.toml` holds the settings: the licences a pack may use, the owners whose repositories are first-party, the reviewer persona hashes whose reviews count, and the name pairs accepted as not confusable. `cargo run -p hippo-registry -- build --out registry/out` writes the index that `hippo add <name> --registry <index.json>` reads.
