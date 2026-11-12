---
type: topic
status: current
updated: 2026-10-17
---
# Storage backends

Tern stores originals and thumbnails through a small storage interface with two backends.

## Backends

- `local` writes under a directory on the server and is the default for small installs.
- `s3` talks to any S3-compatible bucket and is the choice for large libraries.

## Layout

- Originals sit under `orig/<year>/<month>/`, thumbnails under `thumb/<size>/`.
- File names are the asset id plus the original extension.

## Moving data

- `tern storage copy --from local --to s3` copies without deleting the source.
- Backups of this data are described in [[Backups]].
