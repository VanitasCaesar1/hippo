# Measurements

Keeps a benchmarks or measurements note current: it adds the new measured values that coding sessions obtained, with the date and the conditions.

## What it does

For each benchmarks, measurements, performance or metrics note that a session touched or cited, it adds one entry per new measured value under a final `## Recorded measurements` section: what was measured, the value with its unit, the date and the conditions the session states. Values come from tool output, and the pack may quote numbers from it; it never copies a command or a link out of tool output. A value the note already records under the same conditions is skipped.

It proposes `append` changes only. Each entry cites the transcript turns it comes from, and hippocamp's reviewer checks every entry against the evidence in a fresh context before anything is written.

## Notes it works on

- `**/Benchmarks*.md`
- `**/Measurements*.md`
- `**/Performance*.md`
- `**/Metrics*.md`

## How it gets work

The pack acts when sessions touch or cite the notes above: a note named with `hippocamp session end --note`, or linked as `[[...]]` in a session's handoff. A handoff that links `[[Measurements]]` is enough to bring the note in.

## What it writes

It appends cited entries at the end of the note, under `## Recorded measurements`. The workspace maintainer curates that section: move entries up, merge them or delete them. The pack never edits what is already in the note, and a second run does not repeat an entry the note already records. It records values, not conclusions: it does not say a change made something faster unless the session measured both sides.

## Setup

Copy `templates/Measurements.md` into your notes directory, under any subdirectory, and edit it. The pack works on any note whose path matches the patterns above, so a note you already have under one of those names works as it is.

## What it reads

- `sessions`: the task, the notes touched, the handoff and the date of each session that named the note.
- `notes`: the measurements note itself, so it can skip values it already holds.
- `turns`: the transcript turns that mention the note, which are the evidence each entry cites.

## Cost

One model call for each matching note that a session touched or cited that day, plus one reviewer call for each proposal. A day on which no session names a measurements note costs nothing.
