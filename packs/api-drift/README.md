# API drift

Keeps API notes honest: it flags endpoints, fields and status codes that the note documents and that transcript turns contradict.

## What it does

For each API note that a session touched or cited, it adds one entry per documented endpoint, field or status code that a session's turns contradict, under a final `## Drift` section: what the note says, what the turn shows instead, and the turn quoted in a few words. It does not rewrite the documented text; the workspace maintainer decides which side is right.

It proposes `append` changes only. Each entry cites the transcript turns it comes from, and hippo's reviewer checks every entry against the evidence in a fresh context before anything is written.

## Notes it works on

- `**/API*.md`
- `**/api/**`
- `**/Endpoints*.md`

## How it gets work

The pack acts when sessions touch or cite the notes above: a note named with `hippo session end --note`, or linked as `[[...]]` in a session's handoff. A handoff that links `[[API]]` is enough to bring the note in.

## What it writes

It appends cited entries at the end of the note, under `## Drift`. The workspace maintainer curates that section: move entries up, merge them or delete them. The pack never edits what is already in the note, and a second run does not repeat an entry the note already records.

## Setup

Copy `templates/API.md` into your notes directory, under any subdirectory, and edit it. The pack works on any note whose path matches the patterns above, so an API note you already have under one of those names works as it is.

## What it reads

- `sessions`: the task, the notes touched, the handoff and the date of each session that named the note.
- `notes`: the note itself, so it can skip what it already holds.
- `turns`: the transcript turns that mention the note, which are the evidence each entry cites.

## Cost

One model call for each matching note that a session touched or cited that day, plus one reviewer call for each proposal. A day on which no session names a matching note costs nothing.
