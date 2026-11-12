---
type: policy
status: current
updated: 2026-10-18
review_after: 2026-12-31
sources: ["notes/Decisions/Use Postgres 17.md"]
---
# Backups

Tern backs up the database and the originals every night, and the backups can be restored on a fresh server.

## Retention

- Nightly backups are kept for 14 days.
- Older dumps are pruned by the backup job after it finishes.

## Encryption

- Backups are encrypted with the repository key.
- The key file lives outside the backup target and is rotated once a year.

## Restore

- `tern backup verify --latest` checks that the newest dump decrypts.
- A full restore drill happens once a quarter; the last one took 50 minutes.
- Where the originals sit is described in [[Storage backends]].
