# FAQ

Keeps a FAQ note growing from real questions: it adds the questions users asked in coding sessions and the answer each session established.

## What it does

For each FAQ note that a session touched or cited, it adds one entry per question a person asked and the session answered, under a final `## Asked in sessions` section: the question, the answer the turns established and the date. Questions left open, answers that are guesses and questions about one person's private setup are skipped. A question the note already answers is not repeated.

It proposes `append` changes only. Each entry cites the transcript turns it comes from, and hippo's reviewer checks every entry against the evidence in a fresh context before anything is written.

## Notes it works on

- `**/FAQ*.md`

## How it gets work

The pack acts when sessions touch or cite the notes above: a note named with `hippo session end --note`, or linked as `[[...]]` in a session's handoff. A handoff that links `[[FAQ]]` is enough to bring the note in.

## What it writes

It appends cited entries at the end of the note, under `## Asked in sessions`. The workspace maintainer curates that section: move entries up, merge them or delete them. The pack never edits what is already in the note, and a second run does not repeat an entry the note already records.

## Setup

Copy `templates/FAQ.md` into your notes directory, under any subdirectory, and edit it. The pack works on any note whose path matches the patterns above, so a note you already have under one of those names works as it is.

## What it reads

- `sessions`: the task, the notes touched, the handoff and the date of each session that named the note.
- `notes`: the FAQ itself, so it can skip questions it already answers.
- `turns`: the transcript turns that mention the note, which are the evidence each entry cites.

## Cost

One model call for each matching note that a session touched or cited that day, plus one reviewer call for each proposal. A day on which no session names a FAQ note costs nothing.
