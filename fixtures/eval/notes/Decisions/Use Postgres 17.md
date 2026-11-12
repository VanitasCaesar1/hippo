---
type: decision
status: current
updated: 2026-10-05
review_after: 2027-04-01
---
# Use Postgres 17

Tern moved its database to Postgres 17 in October 2026 and drops support for older versions.

## Decision

- New installs and the container image ship Postgres 17.
- Upgrades from 15 or 16 go through a dump and restore, described in [[Backups]].

## Reasons

- Faster vacuum on the large asset table.
- One supported version keeps the test matrix small.

## Consequences

- Schema files may use features new in 17; see [[Database migrations]].
- Self-hosters on older versions must upgrade before the next release.
