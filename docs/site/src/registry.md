# The registry

The registry is a list of packs in the repository's `registry/` directory. Each entry names a public GitHub repository, a directory in it and one commit. Nothing is listed without a passing check and a fresh-context review of exactly that content.

Nothing is hosted. There is no public index and no public registry site. You build both from a checkout.

No pack is listed yet. The entries for the 26 first-party packs (23 pass packs and 3 theme packs) are in `registry/pending/`, waiting to be pinned to this repository and reviewed; its README gives the two commands that list them.

No pack by another author is listed yet. Outside authors are welcome: see [Submit a pack](#submit-a-pack).

## What is in `registry/`

| Path | What it holds |
|---|---|
| `registry.toml` | The settings: the licences a pack may use, the owners whose repositories count as first-party, the reviewer persona hashes whose reviews count, and the name pairs accepted as not confusable. |
| `packs/<name>.toml` | One entry for each listed pack: its repository and the repository's id, the owner's id, the directory, the pinned commit and the maintainers. |
| `reviews/*.json` | The fresh-context review of each listed content, written by a maintainer's run of `hippocamp-registry review`. |
| `pending/<name>.toml` | Entries not listed yet: the first-party packs, waiting to be pinned to this repository and reviewed. `check` and `build` do not read them. |

The tool is the `hippocamp-registry` crate. It is not installed with `hippocamp`; you run it with `cargo run -p hippocamp-registry --` from a checkout. It has three commands: `check`, `review` and `build`.

## What `check` verifies

```sh
cargo run -p hippocamp-registry -- check
```

For each entry, `check`:

- reads the entry and fetches the pinned commit;
- validates the pack, as [`hippocamp packs check`](writing-packs.md) does;
- checks the pack's name, its licence against the settings, and that a `LICENSE` file is present;
- runs the pack's fixture tests against a stand-in model;
- looks for a confirming review of exactly that content;
- rejects a name that is confusable with another entry's, and a pass name used by two packs;
- unless you pass `--offline`, asks GitHub that the repository and its owner are still the ones the entry names.

It exits 1 on any error. A fixture test uses a stand-in model, so it shows the wiring and nothing about quality.

## Build the index and the site

```sh
cargo run -p hippocamp-registry -- build --out registry/out
```

This runs the same checks, then writes into `registry/out/`:

- `index.json`, the list of packs that `hippocamp add` reads;
- `packs.html`, `themes.html` and one `packs/<name>.html` page for each pack, and, with `--landing <file>`, that HTML fragment in the same page frame as `index.html` (the site's landing page);
- `hippocamp.css`, `registry.css` and `icon.svg`.

The pages use Hacker News's dense layout. They hold no JavaScript and load nothing from another host. Every file of a pack is shown as text. Add `--offline` to skip the GitHub owner check and the entries outside this repository.

To build the whole site (the landing page, the registry pages and these docs) run `docs/site/build.sh`. It writes it under `docs/site/out/`, with relative links so it works at <https://vanitascaesar1.github.io/hippocamp/> and from a folder on disk, and fails if a page has a script, loads from another host, or links a file of the site that does not exist. `docs/site/publish.sh` puts that folder on the `gh-pages` branch.

## Install a listed pack by name

```sh
hippocamp add decision-log --registry registry/out/index.json
```

A name with no path or URL in front of it is looked up in the index you give with `--registry`. `hippocamp packs diff` and `hippocamp packs update` take the same flag. hippocamp then:

1. checks the index's entry against the registry's rules for an entry;
2. fetches the pack from the entry's repository at the pinned commit;
3. refuses the pack if its content hashes to anything but the entry's `pack_hash`, or if its `pack.toml` names another pack;
4. validates and reviews it, and copies it, as [Packs](packs.md#add-it) describes.

Step 2 uses git over https, so the repository must be reachable. Without `--registry`, a bare name is an error that says so.

## Submit a pack

The steps are in [`registry/README.md`](https://github.com/VanitasCaesar1/hippocamp/blob/main/registry/README.md). In short: put the pack in a public GitHub repository with a `LICENSE` and, for a pass pack, fixture tests. Then open a pull request that adds one file, `registry/packs/<name>.toml`. A pull request that changes anything else fails.

A maintainer runs the review from a checkout of `main` and commits the review file. A review file a submitter adds is deleted, because a review is a claim that `check` can verify only in form.

To update a pack, change the entry's `commit`. If the content changed, it needs a new review. An entry that starts failing is unlisted, and every registry pull request fails until the entry is re-pinned or deleted.

## What is not there

- No hosted index and no hosted site. Reading an index by URL is not built; `--registry` takes a file on your disk.
- No pack by another author.
- Reviews are by a language model. A confirming review is not proof that a pack is good, and none of the first-party packs has been run against real notes with a real model ([Limits](limits.md#packs)).
