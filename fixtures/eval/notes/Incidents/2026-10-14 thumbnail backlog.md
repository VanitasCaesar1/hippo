---
type: evidence
status: current
updated: 2026-10-15
---
# 2026-10-14 thumbnail backlog

On 14 October the thumbnail queue grew to about 9,000 jobs after a worker crash loop, and new uploads showed no previews for three hours.

## Timeline

- 13:05 a bad release of the worker started crashing on HEIC files.
- 13:40 queue depth passed 500 and the on-call person was paged.
- 14:20 the worker was rolled back; the queue drained by 16:10.

## Cause

- A decoder update returned an empty buffer for some HEIC files, and the worker did not handle it.

## Follow-up

- Add a decode test with sample HEIC files to [[CI]].
- Keep the page threshold in [[On-call]] at 500.
- Pipeline details are in [[Thumbnail pipeline]].
