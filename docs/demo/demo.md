# Demo script

A two-minute terminal demo of hippocamp, run from a checkout on synthetic data. It needs no account, no network and no real model: dreaming and the pack review use the stub model in `fixtures/stub-model/`, so every run prints the same thing. Nothing here is published by the script itself.

## What it shows, in order

The demo uses the synthetic eval set, `fixtures/eval/`: 16 notes about "Tern", an imaginary photo library, eight Claude Code sessions and one Codex session, and the stub's `eval` scenario, which corrects one planted error.

1. **Set up** (`hippocamp init`): a workspace holding the 16 notes. `demo.sh` then writes the configuration: the transcripts under `/tmp/hippocamp-demo`, the stub as `model_command`, and the stub as reviewer through `reviewer_command_override` (`env HIPPOCAMP_STUB_REVIEW=1 HIPPOCAMP_STUB_VERDICT=confirm`), as `crates/hippocamp/tests/dream_stub_model.rs` does.
2. **Ingest** (`hippocamp ingest`): the nine transcripts are indexed.
3. **Recall** (`hippocamp recall "keyed on the hash of package-lock.json"`): the one turn, from the Codex session, that says so.
4. **Context** (`hippocamp context --task "how long do we keep the nightly backups and how do i restore one"`): the entry note, then the notes the task names, with the linter counts.
5. **A pack** (`hippocamp packs check`, `hippocamp add`, `hippocamp packs list`): `glossary-terms`, copied from `packs/`, is validated, reviewed in a fresh context (the stub confirms) and copied into the workspace with its origin recorded. `list` shows it as `clean`.
6. **Dreaming** (`hippocamp dream`): `notes/Backups.md` says backups are kept 14 days; a later session says 30. The demo prints the line, runs one real `hippocamp dream`, and prints the line again: the note now says 30 days. The run's summary line reports 1 applied change.
7. **Revert** (`hippocamp revert <receipt>`): the confirm receipt's snapshot restores the note, and the line is back to 14 days.
8. **Doctor** (`hippocamp doctor`): the setup checks in one table.

Each step prints one short `# comment` line first, so a viewer can follow without narration.

Two things on screen need a word:

- The dream summary says `1 validation failures`. The stub answers the `verify` pass with the same correction it gave `consolidate`; `consolidate` ran first and applied it, so the same text no longer occurs in the note and `verify`'s copy fails validation. The report under `.hippocamp/reports/` lists both.
- Doctor shows one `problem` row, `launchd`, and exits 1, because the demo installs no schedule. `demo.sh` accepts that row and fails on any other problem.

## Running it

```sh
cargo build -p hippocamp
HIPPOCAMP_BIN=target/debug/hippocamp docs/demo/demo.sh
```

- `HIPPOCAMP_BIN` is the hippocamp binary; it defaults to `hippocamp` on PATH.
- `DEMO_PAUSE` is the unit of the waits, in whole seconds, default 1: three units after each comment and seven after each output, about two minutes in all. `DEMO_PAUSE=0` removes the waits (a run takes a few seconds).
- It works in `/tmp/hippocamp-demo`, removed at the start and at the end, with `HIPPOCAMP_HOME` and `HOME` set there, so it never reads or writes the person's own hippocamp setup or transcripts. It copies from the repository the notes, transcripts and session records of `fixtures/eval/`, `fixtures/stub-model/model.sh` and `packs/glossary-terms`, so no path of the machine it runs on shows in the output.
- The copied transcripts and session records are dated 1 to 7 November 2026. Dreaming reads the window since the last run, so `demo.sh` rewrites those dates to yesterday (UTC) and sets the window's start to midnight before it.
- It exits non-zero if any step fails.

On macOS, `/tmp` resolves to `/private/tmp`, so `hippocamp add` and `hippocamp packs list` show the pack's source as `/private/tmp/hippocamp-demo/glossary-terms`.

## Recording

- Tool: [asciinema](https://asciinema.org) 3, which records a terminal to a text file (`.cast`) locally. The committed `demo.cast` was recorded with:

  ```sh
  cd docs/demo
  HIPPOCAMP_BIN=<a copy of the binary> asciinema rec --headless --window-size 100x30 --overwrite \
    --title "hippocamp demo" --command "sh ./demo.sh" demo.cast
  ```

  The binary copy sat in a plain directory, so that its path is not in the recording.
- Terminal: 100 columns by 30 rows, no shell prompt or theme (the command is `demo.sh` itself).
- Playback: `asciinema play docs/demo/demo.cast`. It can be turned into a GIF or video, for example with `agg`; nothing is uploaded.
- Before any recording is used, check it for paths, names or text from a real machine. The committed cast has no `/Volumes`, `/Users`, user name, project name or tool-session path in it (checked with `grep -c`), but a new recording needs the same check.
