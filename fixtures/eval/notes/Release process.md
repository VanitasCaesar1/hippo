---
type: policy
status: current
updated: 2026-10-12
---
# Release process

Tern ships numbered releases on a fixed rhythm so that self-hosters know when to expect an update.

## Schedule

- Releases are cut every second Tuesday.
- A release that misses its day waits for the next one unless it fixes a security problem.
- Release candidates soak on the beta channel for three days.

## Checklist

- Every release gets a changelog entry.
- The release check (`tern release check`) must pass on the release branch.
- Tag the commit, publish the container image, then post the notes.

## After release

- Watch error rates for an hour, see [[On-call]].
- Check the schema files that ship with the release, see [[Database migrations]].
