---
type: policy
status: current
updated: 2026-10-24
review_after: 2027-01-15
---
# On-call

One person per week carries the pager and decides whether a problem is an incident.

## Pages

- Thumbnail queue depth above 500 for ten minutes.
- Upload API error rate above 5 percent for five minutes.
- Backup job missing for a night.

## First steps

1. Acknowledge the page within 15 minutes.
2. Check the status dashboard, then the server logs for the failing service.
3. Restart a stalled worker before anything bigger.

## Afterwards

- Write an incident note, as in [[Incidents/2026-10-14 thumbnail backlog]].
- Hand the pager over on Monday with a short list of open issues.
