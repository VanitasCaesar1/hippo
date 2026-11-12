# Onboarding gaps

Keeps onboarding notes complete: it adds the setup facts that coding sessions had to discover because the note did not say them.

## What it does

For each onboarding note that a session touched or cited, it adds one entry per setup fact a session had to discover, under a final `## Discovered setup` section: a command, an environment variable name, a port, or an account or role the setup needs. Variable names are recorded, never their values.

It proposes `append` changes only. Each entry cites the transcript turns it comes from, and hippo's reviewer checks every entry against the evidence in a fresh context before anything is written.

## Notes it works on

- `**/Onboarding*.md`
- `**/Getting started*.md`
- `**/Setup*.md`

## How it gets work

The pack acts when sessions touch or cite the notes above: a note named with `hippo session end --note`, or linked as `[[...]]` in a session's handoff. A handoff that links `[[Onboarding]]` is enough to bring the note in.

## What it writes

It appends cited entries at the end of the note, under `## Discovered setup`. The workspace maintainer curates that section: move entries up, merge them or delete them. The pack never edits what is already in the note, and a second run does not repeat an entry the note already records.

## Setup

Copy `templates/Onboarding.md` into your notes directory, under any subdirectory, and edit it. The pack works on any note whose path matches the patterns above, so a note you already have under one of those names works as it is.

## What it reads

- `sessions`: the task, the notes touched, the handoff and the date of each session that named the note.
- `notes`: the note itself, so it can skip what it already holds.
- `turns`: the transcript turns that mention the note, which are the evidence each entry cites.

## Cost

One model call for each matching note that a session touched or cited that day, plus one reviewer call for each proposal. A day on which no session names a matching note costs nothing.
