# Environment and config

Keeps an environment or configuration note current: it adds the settings that coding sessions dealt with, at the end of the note.

## What it does

For each matching note that a session touched or cited, it adds one entry per variable, flag or setting a session introduced, renamed or removed, under a final `## Logged settings` section: the date, the name, what changed and what it controls. It records names only. A value, a default that looks like a secret or a connection string is never written. A setting the note already records is skipped.

It proposes `append` changes only. Each entry cites the transcript turns it comes from, and hippocamp's reviewer checks every entry against the evidence in a fresh context before anything is written. 

## Notes it works on

- `**/Environment*.md`
- `**/Config*.md`
- `**/Env vars*.md`

## How it gets work

The pack acts when sessions touch or cite the notes above: a note named with `hippocamp session end --note`, or linked as `[[...]]` in a session's handoff. A handoff that links `[[Environment]]` is enough to bring the note in.

## What it writes

It appends cited entries at the end of the note, under `## Logged settings`. The workspace maintainer curates that section: move entries up, merge them or delete them. The pack never edits what is already in the note, and a second run does not repeat an entry the note already records.

## Setup

Copy `templates/Environment.md` into your notes directory, under any subdirectory, and edit it. The pack works on any note whose path matches the patterns above, so a note you already have under one of those names works as it is.

## What it reads

- `sessions`: the task, the notes touched, the handoff and the date of each session that named the note.
- `notes`: the environment note itself, so it can skip settings it already holds.
- `turns`: the transcript turns that mention the note, which are the evidence each entry cites.

## Cost

One model call for each matching note that a session touched or cited that day, plus one reviewer call for each proposal. A day on which no session names an environment or configuration note costs nothing.
