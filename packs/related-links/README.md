# Related links

Connects notes: when a session's handoff ties two notes together and the notes do not link each other, it adds a `Related:` line to each.

## What it does

For every note that a session touched or cited, it looks at the handoffs of that day. Where a handoff connects two notes that do not link each other, it appends a `Related: [[...]]` line to the note, under a final `## Related` section. A link the note already has, anywhere in its text, is not added again.

It proposes `append` changes only. Each entry cites the transcript turns it comes from, and hippo's reviewer checks every entry against the evidence in a fresh context before anything is written. It makes one call a day, with all of that day's notes together, rather than one call for each note.

## Notes it works on

- every note that a session touched or cited

## How it gets work

The pack acts when sessions touch or cite the notes above: a note named with `hippo session end --note`, or linked as `[[...]]` in a session's handoff. It has no filter: it considers every note that a session touched or cited, and a handoff that links two notes with `[[...]]` is what connects them.

## What it writes

It appends cited entries at the end of the note, under `## Related`. The workspace maintainer curates that section: move entries up, merge them or delete them. The pack never edits what is already in the note, and a second run does not repeat an entry the note already records.

## Setup

There is nothing to copy: the pack works on any note that a session touched or cited. Ask your agents to end sessions with `hippo session end`, naming the notes they changed and linking related notes in the handoff with `[[...]]`.

## What it reads

- `sessions`: the task, the notes touched and the handoff of each session, which say what connects two notes.
- `notes`: the notes that sessions touched or cited, so it can see which already link each other.

It does not read transcript turns. Each entry cites the last turn of a session, which the sessions input gives.

## Cost

One model call a day for each day on which a session touched or cited a note, however many notes that is, plus one reviewer call for each proposal.
