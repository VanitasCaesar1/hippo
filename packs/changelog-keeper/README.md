# Changelog keeper

Keeps a changelog current: it adds the changes that coding sessions shipped to the end of a changelog or release-notes note.

## What it does

For each changelog note that a session touched or cited, it adds one bullet per change a user of the software would notice, under a final `## Pending` section: a new command, a changed default, a fixed bug, a removed option. Internal refactors, tests and plans that were not carried out are left out.

It proposes `append` changes only. Each entry cites the transcript turns it comes from, and hippocamp's reviewer checks every entry against the evidence in a fresh context before anything is written. 

## Notes it works on

- `**/CHANGELOG.md`
- `**/Changelog*.md`
- `**/Release notes*.md`

## How it gets work

The pack acts when sessions touch or cite the notes above: a note named with `hippocamp session end --note`, or linked as `[[...]]` in a session's handoff. A handoff that links `[[CHANGELOG]]` (or the note's own title) is enough to bring the note in.

## What it writes

It appends cited entries at the end of the note, under `## Pending`. The workspace maintainer curates that section: move entries up, merge them or delete them. The pack never edits what is already in the note, and a second run does not repeat an entry the note already records.

## Setup

Copy `templates/CHANGELOG.md` into your notes directory, under any subdirectory, and edit it. The pack works on any note whose path matches the patterns above, so a note you already have under one of those names works as it is.

## What it reads

- `sessions`: the task, the notes touched and the handoff of each session that named the note, so the pack knows what shipped.
- `notes`: the changelog itself, so it can skip what is already recorded, released sections included.
- `turns`: the transcript turns that mention the note, which are the evidence each entry cites.

## Cost

One model call for each matching note that a session touched or cited that day, plus one reviewer call for each proposal. A day on which no session names a changelog note costs nothing.
