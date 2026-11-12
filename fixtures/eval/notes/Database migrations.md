---
type: policy
status: current
updated: 2026-10-16
sources: ["notes/Decisions/Use Postgres 17.md"]
---
# Database migrations

Schema changes to the Postgres database are plain SQL files that are applied in order.

## Execution

- Migrations run automatically on deploy.
- The runner records each applied file in a table, so a file is never applied twice.

## Rollback

- Every migration has a down step.
- Down steps are tested in CI against a copy of last night's schema, see [[CI]].

## Naming

- Files are numbered, for example `0042_backfill_covers.sql`, and live in `db/schema/`.
- One change per file; never edit a file that has already shipped.
- The database engine is described in [[Decisions/Use Postgres 17]].
