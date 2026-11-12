# Security log

Keeps a security note current: it adds the security-relevant changes that coding sessions made, such as authentication, permissions, secret handling and advisories. It records changes and never secrets.

## What it does

For each security note that a session touched or cited, it adds one entry per security-relevant change under a final `## Logged changes` section: the date, the area, what changed and the reason when the session states it. It records names and kinds, never values: no password, token, key, connection string or personal data goes into an entry, even when a transcript contained one. An unfixed risk is logged only as an observation that says so. A change the note already logs is skipped.

It proposes `append` changes only. Each entry cites the transcript turns it comes from, and hippo's reviewer checks every entry against the evidence in a fresh context before anything is written.

## Notes it works on

- `**/Security*.md`

## How it gets work

The pack acts when sessions touch or cite the notes above: a note named with `hippo session end --note`, or linked as `[[...]]` in a session's handoff. A handoff that links `[[Security log]]` is enough to bring the note in.

## What it writes

It appends cited entries at the end of the note, under `## Logged changes`. The workspace maintainer curates that section: move entries up, merge them or delete them. The pack never edits what is already in the note, and a second run does not repeat an entry the note already records. If a session exposed a credential, the entry says that one was exposed and rotated or not, and never shows it.

## Setup

Copy `templates/Security log.md` into your notes directory, under any subdirectory, and edit it. The pack works on any note whose path matches the patterns above, so a note you already have under one of those names works as it is.

## What it reads

- `sessions`: the task, the notes touched, the handoff and the date of each session that named the note.
- `notes`: the security note itself, so it can skip changes it already logs.
- `turns`: the transcript turns that mention the note, which are the evidence each entry cites.

## Cost

One model call for each matching note that a session touched or cited that day, plus one reviewer call for each proposal. A day on which no session names a security note costs nothing.
