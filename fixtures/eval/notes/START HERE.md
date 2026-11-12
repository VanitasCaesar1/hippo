---
type: policy
status: current
updated: 2026-10-30
review_after: 2027-01-31
---
# START HERE

Tern is an open-source, self-hosted photo library, and this note is the entry point to its engineering notes.

## What Tern is

- A server with an upload API, a thumbnail worker, a search index and public share links.
- A mobile app for backup and browsing, and nightly backups of the library.
- Run by a small team; every note here is written for people who deploy and change it.

## Reading order

1. [[Upload API]] and [[Thumbnail pipeline]] for how a photo enters the system.
2. [[Storage backends]] and [[Backups]] for where it lives.
3. [[Search index]], [[Sharing links]] and [[Auth sessions]] for how people reach it.
4. [[Release process]], [[CI]] and [[Database migrations]] before shipping a change.
5. [[On-call]] and [[Incidents/2026-10-14 thumbnail backlog]] when something breaks.

## Also read

- [[Style guide]] and [[Mobile app]]
- [[Decisions/Use Postgres 17]] for the database choice
