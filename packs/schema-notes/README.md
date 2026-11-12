# Schema notes

Keeps a schema or data-model note current: it adds the tables, columns and fields that coding sessions added, renamed or dropped.

## What it does

For each schema, data-model or database note that a session touched or cited, it adds one entry per change under a final `## Schema changes` section: the date, whether it was added, renamed or dropped, the table or record, the column or field, its type and the reason when the session states them. Only changes the turns show made are recorded, never designs. It writes names and types, never data. A change the note already describes is skipped.

It proposes `append` changes only. Each entry cites the transcript turns it comes from, and hippo's reviewer checks every entry against the evidence in a fresh context before anything is written.

## Notes it works on

- `**/Schema*.md`
- `**/Data model*.md`
- `**/Database*.md`

## How it gets work

The pack acts when sessions touch or cite the notes above: a note named with `hippo session end --note`, or linked as `[[...]]` in a session's handoff. A handoff that links `[[Schema]]` is enough to bring the note in.

## What it writes

It appends cited entries at the end of the note, under `## Schema changes`. The workspace maintainer curates that section: move entries up, merge them or delete them. The pack never edits what is already in the note, and a second run does not repeat an entry the note already records.

## Setup

Copy `templates/Schema.md` into your notes directory, under any subdirectory, and edit it. The pack works on any note whose path matches the patterns above, so a note you already have under one of those names works as it is.

## What it reads

- `sessions`: the task, the notes touched, the handoff and the date of each session that named the note.
- `notes`: the schema note itself, so it can skip changes it already describes.
- `turns`: the transcript turns that mention the note, which are the evidence each entry cites.

## Cost

One model call for each matching note that a session touched or cited that day, plus one reviewer call for each proposal. A day on which no session names a schema note costs nothing.
