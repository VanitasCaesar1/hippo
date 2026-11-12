---
type: topic
status: current
updated: 2026-10-20
---
# Thumbnail pipeline

The thumbnail worker turns every uploaded original into the small images the grid and the mobile app show.

## Sizes

- Thumbnails are generated at 256 and 1024 pixels on the long edge.
- The smaller size feeds the grid, the larger one feeds the photo viewer.

## Format

- Thumbnails are stored as WebP.
- Originals are never rewritten; only the derived files are.

## Workers

- The worker pulls jobs from the `thumbs` queue, two jobs at a time per process.
- A failed job is retried three times, then parked in the dead queue.
- Queue depth above 500 for ten minutes pages the on-call person, see [[On-call]].
- Past trouble: [[Incidents/2026-10-14 thumbnail backlog]].
