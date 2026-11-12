# Open questions

Keeps an open-questions note current: it adds the questions coding sessions raised and left unanswered, and the answers sessions found to questions already listed.

## What it does

For each open-questions note that a session touched or cited, it adds under a final `## Raised in sessions` section one entry per question a session raised and left open, and one entry per listed question that a session answered, with the answer. It does not delete or edit the listed questions; the workspace maintainer closes them.

It proposes `append` changes only. Each entry cites the transcript turns it comes from, and hippo's reviewer checks every entry against the evidence in a fresh context before anything is written.

## Notes it works on

- `**/Open questions*.md`

## How it gets work

The pack acts when sessions touch or cite the notes above: a note named with `hippo session end --note`, or linked as `[[...]]` in a session's handoff. A handoff that links `[[Open questions]]` is enough to bring the note in.

## What it writes

It appends cited entries at the end of the note, under `## Raised in sessions`. The workspace maintainer curates that section: move entries up, merge them or delete them. The pack never edits what is already in the note, and a second run does not repeat an entry the note already records.

## Setup

Copy `templates/Open questions.md` into your notes directory, under any subdirectory, and edit it. The pack works on any note whose path matches the pattern above, so a note you already have under that name works as it is.

## What it reads

- `sessions`: the task, the notes touched, the handoff and the date of each session that named the note.
- `notes`: the note itself, so it can skip what it already holds.
- `turns`: the transcript turns that mention the note, which are the evidence each entry cites.

## Cost

One model call for each matching note that a session touched or cited that day, plus one reviewer call for each proposal. A day on which no session names a matching note costs nothing.
