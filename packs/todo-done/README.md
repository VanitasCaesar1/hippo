# Todo done

Keeps a TODO or backlog note honest: it adds a `Done:` line for each listed task that a coding session finished, and never adds a task.

## What it does

For each TODO, tasks or backlog note that a session touched or cited, it adds one line per listed task the session completed, under a final `## Done` section: `Done:`, the date and the task's text as the note words it. A task counts only when the turns show it finished and working. It never invents a task, and never rewrites the list.

It proposes `append` changes only. Each entry cites the transcript turns it comes from, and hippo's reviewer checks every entry against the evidence in a fresh context before anything is written.

## Notes it works on

- `**/TODO*.md`
- `**/Tasks*.md`
- `**/Backlog*.md`

## How it gets work

The pack acts when sessions touch or cite the notes above: a note named with `hippo session end --note`, or linked as `[[...]]` in a session's handoff. A handoff that links `[[TODO]]` is enough to bring the note in.

## What it writes

It appends cited entries at the end of the note, under `## Done`. The workspace maintainer curates that section: move entries up, merge them or delete them. The pack never edits what is already in the note, and a second run does not repeat an entry the note already records. The listed task itself stays where it is: the pack only adds the `Done:` line, and you move or delete the task when you curate.

## Setup

Copy `templates/TODO.md` into your notes directory, under any subdirectory, and edit it. The pack works on any note whose path matches the patterns above, so a note you already have under one of those names works as it is.

## What it reads

- `sessions`: the task, the notes touched, the handoff and the date of each session that named the note.
- `notes`: the TODO note itself, so it can match each finished task to a listed one and skip tasks already marked.
- `turns`: the transcript turns that mention the note, which are the evidence each entry cites.

## Cost

One model call for each matching note that a session touched or cited that day, plus one reviewer call for each proposal. A day on which no session names a TODO note costs nothing.
