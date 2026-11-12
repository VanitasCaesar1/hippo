---
type: decision
status: current
updated: 2026-10-20
---
# Use SQLite for the cache

The sample app keeps its cache in a local SQLite file, because several workers write at once.

## Status updates
