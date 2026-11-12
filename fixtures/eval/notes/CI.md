---
type: topic
status: current
updated: 2026-10-27
---
# CI

Continuous integration runs on every pull request and keeps the main branch green.

## Jobs

- `lint` runs the formatter check and the linter.
- `test` runs unit tests, then integration tests against a throwaway Postgres.
- `build` builds the server image and the mobile app bundle.

## Speed

- A full run takes about 14 minutes; the install step is the slowest part.
- Integration tests are split over three runners.

## Rules

- A pull request needs a green run before it merges.
- Flaky tests are quarantined within a day and fixed within a week.
- The release branch runs the same jobs plus `tern release check`, see [[Release process]].
