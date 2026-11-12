# Glossary terms

Keeps a glossary complete: it adds the terms coding sessions defined, or used in one fixed sense, that the glossary does not hold.

## What it does

For each glossary note that a session touched or cited, it adds one entry per term a session defined or used in a fixed sense and the note lacks, under a final `## Proposed terms` section: the term in bold, its meaning in one sentence as the turns give it. A term the note already defines is skipped.

It proposes `append` changes only. Each entry cites the transcript turns it comes from, and hippocamp's reviewer checks every entry against the evidence in a fresh context before anything is written.

## Notes it works on

- `**/Glossary*.md`

## How it gets work

The pack acts when sessions touch or cite the notes above: a note named with `hippocamp session end --note`, or linked as `[[...]]` in a session's handoff. A handoff that links `[[Glossary]]` is enough to bring the note in.

## What it writes

It appends cited entries at the end of the note, under `## Proposed terms`. The workspace maintainer curates that section: move entries up, merge them or delete them. The pack never edits what is already in the note, and a second run does not repeat an entry the note already records.

## Setup

Copy `templates/Glossary.md` into your notes directory, under any subdirectory, and edit it. The pack works on any note whose path matches the pattern above, so a glossary you already have under that name works as it is.

## What it reads

- `sessions`: the task, the notes touched, the handoff and the date of each session that named the note.
- `notes`: the note itself, so it can skip what it already holds.
- `turns`: the transcript turns that mention the note, which are the evidence each entry cites.

## Cost

One model call for each matching note that a session touched or cited that day, plus one reviewer call for each proposal. A day on which no session names a matching note costs nothing.
