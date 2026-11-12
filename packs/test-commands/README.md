# Test commands

Keeps a testing or build note current: it adds the build, test and lint commands that coding sessions dealt with, at the end of the note.

## What it does

For each matching note that a session touched or cited, it adds one entry per build, test or lint command a session ran successfully that the note does not list, under a final `## Logged commands` section: the date, the command exactly as the session ran it and what it checks. A command that failed, or that the note already lists, is left out.

It proposes `append` changes only. Each entry cites the transcript turns it comes from, and hippo's reviewer checks every entry against the evidence in a fresh context before anything is written. 

## Notes it works on

- `**/Testing*.md`
- `**/How to test*.md`
- `**/Build*.md`

## How it gets work

The pack acts when sessions touch or cite the notes above: a note named with `hippo session end --note`, or linked as `[[...]]` in a session's handoff. A handoff that links `[[Testing]]` is enough to bring the note in.

## What it writes

It appends cited entries at the end of the note, under `## Logged commands`. The workspace maintainer curates that section: move entries up, merge them or delete them. The pack never edits what is already in the note, and a second run does not repeat an entry the note already records.

## Setup

Copy `templates/Testing.md` into your notes directory, under any subdirectory, and edit it. The pack works on any note whose path matches the patterns above, so a note you already have under one of those names works as it is.

## What it reads

- `sessions`: the task, the notes touched, the handoff and the date of each session that named the note.
- `notes`: the testing note itself, so it can skip commands it already lists.
- `turns`: the transcript turns that mention the note, which are the evidence each entry cites.

## Cost

One model call for each matching note that a session touched or cited that day, plus one reviewer call for each proposal. A day on which no session names a testing or build note costs nothing.
