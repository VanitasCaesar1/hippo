---
type: topic
status: current
updated: 2026-10-21
sources: ["notes/Storage backends.md"]
---
# Search index

The search index lets people find photos by caption, place, date and the objects detected in them.

## How it is built

- Each new asset is added to the index a few seconds after its thumbnails exist.
- The search index is rebuilt with `tern reindex`.
- A full rebuild of 50,000 photos takes under five minutes on two workers.

## Operations

- `tern index status` shows document count and lag behind the database.
- Lag above one hour means the indexer has stalled; restart it before reindexing.

## Limits

- Search only covers photos the signed-in person may see, see [[Auth sessions]].
- Shared albums are searchable by their owner only, not by link holders.
