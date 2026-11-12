# ADR status

Keeps decision records honest: it adds a dated status update when a later coding session reversed, replaced or revisited a decision the record holds.

## What it does

For each note of type `decision` that a session touched or cited, it adds one entry per decision the session reversed, replaced or revisited, under a final `## Status updates` section: the date, what changed (reversed, replaced or revisited), and the reason the turns give. A change the note already records is skipped.

It proposes `append` changes only. Each entry cites the transcript turns it comes from, and hippocamp's reviewer checks every entry against the evidence in a fresh context before anything is written.

## Notes it works on

- every note whose frontmatter has `type: decision` (an architecture decision record), wherever it is stored

## How it gets work

The pack acts when sessions touch or cite the notes above: a note named with `hippocamp session end --note`, or linked as `[[...]]` in a session's handoff. A handoff that links `[[Use SQLite for the cache]]`, if that note has `type: decision`, is enough to bring it in. A note without that frontmatter type is not read.

## What it writes

It appends cited entries at the end of the note, under `## Status updates`. The workspace maintainer curates that section: move entries up, merge them or delete them. The pack never edits what is already in the note, and a second run does not repeat an entry the note already records.

## Setup

Copy `templates/Decision record.md` into your notes directory, under any subdirectory, and edit it. The pack works on any note whose frontmatter has `type: decision`, so records you already have work as they are, whatever their names.

## What it reads

- `sessions`: the task, the notes touched, the handoff and the date of each session that named the note.
- `notes`: the note itself, so it can skip what it already holds.
- `turns`: the transcript turns that mention the note, which are the evidence each entry cites.
- `human_confirmed`: text a person restored with `hippocamp revert`, which the pack treats as settled and does not contradict.

## Cost

One model call for each matching note that a session touched or cited that day, plus one reviewer call for each proposal. A day on which no session names a matching note costs nothing.
