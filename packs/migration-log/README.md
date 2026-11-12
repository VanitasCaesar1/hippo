# Migration log

Keeps a migrations note current: it adds the database migrations that coding sessions dealt with, at the end of the note.

## What it does

For each matching note that a session touched or cited, it adds one entry per database migration a session ran, under a final `## Logged migrations` section: the date, the migration, the environment it ran in, the command as the session ran it, and the outcome. A migration the note already records for that environment is skipped.

It proposes `append` changes only. Each entry cites the transcript turns it comes from, and hippocamp's reviewer checks every entry against the evidence in a fresh context before anything is written. 

## Notes it works on

- `**/Migrations*.md`
- `**/Migration log*.md`

## How it gets work

The pack acts when sessions touch or cite the notes above: a note named with `hippocamp session end --note`, or linked as `[[...]]` in a session's handoff. A handoff that links `[[Migrations]]` is enough to bring the note in.

## What it writes

It appends cited entries at the end of the note, under `## Logged migrations`. The workspace maintainer curates that section: move entries up, merge them or delete them. The pack never edits what is already in the note, and a second run does not repeat an entry the note already records.

## Setup

Copy `templates/Migrations.md` into your notes directory, under any subdirectory, and edit it. The pack works on any note whose path matches the patterns above, so a note you already have under one of those names works as it is.

## What it reads

- `sessions`: the task, the notes touched, the handoff and the date of each session that named the note.
- `notes`: the migrations note itself, so it can skip migrations it already holds.
- `turns`: the transcript turns that mention the note, which are the evidence each entry cites.

## Cost

One model call for each matching note that a session touched or cited that day, plus one reviewer call for each proposal. A day on which no session names a migrations note costs nothing.
