---
type: topic
status: current
updated: 2026-10-22
---
# Upload API

The upload API accepts photos and videos from the web client and the mobile app and hands them to the thumbnail worker.

## Endpoints

- `POST /v1/upload` for a single request upload, `POST /v1/upload/chunks` for pieces.
- Both return an asset id as soon as the original is on disk.
- Thumbnails follow asynchronously, see [[Thumbnail pipeline]].

## Limits

- The upload limit is 50 MB per file.
- Requests above the limit get HTTP 413 with a plain error message.

## Chunked upload

- Uploads over 8 MB use chunked upload.
- Each chunk is acknowledged; a dropped connection resumes from the last chunk.
- Auth is the normal session or token, see [[Auth sessions]].
