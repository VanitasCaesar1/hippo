# Limits and quotas

Keeps a limits or quotas note current: it adds the rate limits, quotas and usage caps that coding sessions ran into, quoting the error.

## What it does

For each limits or quotas note that a session touched or cited, it adds one entry per limit a session ran into under a final `## Hit in sessions` section: the date, the service, the limit with its numbers when the error gives them, the error quoted and what the session did about it. A limit nothing in the turns shows is not recorded, and the account, project or key it applied to is left out. A limit the note already lists is skipped.

It proposes `append` changes only. Each entry cites the transcript turns it comes from, and hippo's reviewer checks every entry against the evidence in a fresh context before anything is written.

## Notes it works on

- `**/Limits*.md`
- `**/Quotas*.md`

## How it gets work

The pack acts when sessions touch or cite the notes above: a note named with `hippo session end --note`, or linked as `[[...]]` in a session's handoff. A handoff that links `[[Limits]]` is enough to bring the note in.

## What it writes

It appends cited entries at the end of the note, under `## Hit in sessions`. The workspace maintainer curates that section: move entries up, merge them or delete them. The pack never edits what is already in the note, and a second run does not repeat an entry the note already records.

## Setup

Copy `templates/Limits.md` into your notes directory, under any subdirectory, and edit it. The pack works on any note whose path matches the patterns above, so a note you already have under one of those names works as it is.

## What it reads

- `sessions`: the task, the notes touched, the handoff and the date of each session that named the note.
- `notes`: the limits note itself, so it can skip limits it already lists.
- `turns`: the transcript turns that mention the note, which are the evidence each entry cites.

## Cost

One model call for each matching note that a session touched or cited that day, plus one reviewer call for each proposal. A day on which no session names a limits note costs nothing.
