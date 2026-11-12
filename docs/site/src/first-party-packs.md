# First-party packs

The repository's `packs/` directory holds the packs this project ships: 23 pass packs, and three theme packs described [at the end](#theme-packs). Each is a complete pack: a manifest, a README, a pass, a prompt, a persona and one fixture test, and all but `related-links` carry a note template. Each passes `hippocamp packs check` with no warning and `hippocamp packs test`. To use one, run `hippocamp add ./packs/<name>` from a checkout ([Using a pack](packs.md#using-a-pack)).

Version, author and licence are the same for all 23: 1.0.0, Vanitas Caesar, MIT. Each proposes `append` changes only, and each entry cites the transcript turns it comes from.

| Pack | Notes it works on | What it appends | Template |
|---|---|---|---|
| `adr-status` | Notes whose frontmatter `type` is `decision` | One entry for each decision a session reversed, replaced or revisited: the date, what changed and the reason, under a final `## Status updates` section. | `templates/Decision record.md` |
| `api-drift` | `**/API*.md`, `**/api/**`, `**/Endpoints*.md` | One entry for each documented endpoint, field or status code that a turn contradicts: what the note says, what the turn shows and the turn quoted, under `## Drift`. The documented text is not rewritten. | `templates/API.md` |
| `changelog-keeper` | `**/CHANGELOG.md`, `**/Changelog*.md`, `**/Release notes*.md` | One bullet for each change a user would notice, under a final `## Pending` section. Refactors, tests and plans not carried out are left out. | `templates/CHANGELOG.md` |
| `decision-log` | `**/Decision log*.md`, `**/Decisions.md` | One entry for each decision a session settled: what, why and the options rejected, under `## Logged decisions`. | `templates/Decision log.md` |
| `dependency-changes` | `**/Dependencies*.md` | One entry for each package added, removed or upgraded: the date, the package, the versions and why, under `## Logged changes`. | `templates/Dependencies.md` |
| `deploy-log` | `**/Deploys*.md`, `**/Deploy log*.md` | One entry for each deploy a session ran or watched: the date, what, where, the command when the session ran one, and the outcome, under `## Logged deploys`. | `templates/Deploy log.md` |
| `env-and-config` | `**/Environment*.md`, `**/Config*.md`, `**/Env vars*.md` | One entry for each variable, flag or setting introduced, renamed or removed, by name and never by value, under `## Logged settings`. | `templates/Environment.md` |
| `error-catalogue` | `**/Errors*.md`, `**/Troubleshooting*.md` | One entry for each error message a session hit and then fixed: the message, the cause and the fix, under `## Logged errors`. An error with no fix shown working is left out. | `templates/Errors.md` |
| `faq` | `**/FAQ*.md` | One entry for each question a person asked and a session answered, with the answer and the date, under `## Asked in sessions`. | `templates/FAQ.md` |
| `glossary-terms` | `**/Glossary*.md` | One entry for each term a session defined or used in a fixed sense that the note lacks, under `## Proposed terms`. | `templates/Glossary.md` |
| `gotchas` | `**/Gotchas*.md`, `**/Pitfalls*.md` | One entry for each piece of surprising behaviour that cost a session time and is not an error: what happens, what a reader would expect and how to avoid it, under `## Logged gotchas`. | `templates/Gotchas.md` |
| `incident-timeline` | `**/Incidents/**`, `**/Incident*.md`, `**/Postmortem*.md` | One entry for each moment of the incident that the turns show (noticed, tried, found, fixed), with the time the turns carry, under `## Timeline`. It states no root cause the turns do not state. | `templates/Incident timeline.md` |
| `limits-and-quotas` | `**/Limits*.md`, `**/Quotas*.md` | One entry for each limit a session ran into: the service, the limit with its numbers, the error quoted and what the session did, under `## Hit in sessions`. | `templates/Limits.md` |
| `measurements` | `**/Benchmarks*.md`, `**/Measurements*.md`, `**/Performance*.md`, `**/Metrics*.md` | One entry for each new measured value: what, the value with its unit, the date and the stated conditions, under `## Recorded measurements`. Values, not conclusions. | `templates/Measurements.md` |
| `migration-log` | `**/Migrations*.md`, `**/Migration log*.md` | One entry for each database migration a session ran: the date, the migration, the environment, the command and the outcome, under `## Logged migrations`. | `templates/Migrations.md` |
| `onboarding-gaps` | `**/Onboarding*.md`, `**/Getting started*.md`, `**/Setup*.md` | One entry for each setup fact a session had to discover (a command, a variable name, a port, an account or role), under `## Discovered setup`. Variable names only, never values. | `templates/Onboarding.md` |
| `open-questions` | `**/Open questions*.md` | One entry for each question raised and left open, and one for each listed question a session answered, with the answer, under `## Raised in sessions`. Listed questions are not edited. | `templates/Open questions.md` |
| `related-links` | Every note a session touched or cited; no filter | A line `Related: [[Other note]]`, under a final `## Related` section, where one session's handoff connects two notes that do not link each other anywhere in their text. | none |
| `runbook-check` | `**/Runbook*.md`, `**/Runbooks/**`, `**/Playbook*.md` | One entry for each runbook step a session found failed, skipped or needing another command, with the command the session ran, under `## Run findings`. The steps are not edited. | `templates/Runbook.md` |
| `schema-notes` | `**/Schema*.md`, `**/Data model*.md`, `**/Database*.md` | One entry for each table, column or field added, renamed or dropped: the date, the change, the type and the reason when stated, under `## Schema changes`. Names and types, never data. | `templates/Schema.md` |
| `security-log` | `**/Security*.md` | One entry for each security-relevant change: the date, the area, what changed and the reason when stated, under `## Logged changes`. Names and kinds, never values. | `templates/Security log.md` |
| `test-commands` | `**/Testing*.md`, `**/How to test*.md`, `**/Build*.md` | One entry for each build, test or lint command a session ran successfully that the note lacks: the date, the command as run and what it checks, under `## Logged commands`. | `templates/Testing.md` |
| `todo-done` | `**/TODO*.md`, `**/Tasks*.md`, `**/Backlog*.md` | A `Done:` line for each listed task a session finished, with the date and the task's text, under `## Done`. It never adds a task and never rewrites the list. | `templates/TODO.md` |

## How they run

Every pack has one pass, named like the pack, with an input budget of 200,000 bytes. `adr-status` also reads `human_confirmed`: text a person restored with `hippocamp revert`, which it treats as settled.

| Pack | Reads | Calls |
|---|---|---|
| `adr-status` | `sessions`, `notes`, `turns`, `human_confirmed` | One for each matching note a session touched or cited that day. |
| `api-drift` | `sessions`, `notes`, `turns` | One for each matching note a session touched or cited that day. |
| `changelog-keeper` | `sessions`, `notes`, `turns` | One for each matching note a session touched or cited that day. |
| `decision-log` | `sessions`, `notes`, `turns` | One for each matching note a session touched or cited that day. |
| `dependency-changes` | `sessions`, `notes`, `turns` | One for each matching note a session touched or cited that day. |
| `deploy-log` | `sessions`, `notes`, `turns` | One for each matching note a session touched or cited that day. |
| `env-and-config` | `sessions`, `notes`, `turns` | One for each matching note a session touched or cited that day. |
| `error-catalogue` | `sessions`, `notes`, `turns` | One for each matching note a session touched or cited that day. |
| `faq` | `sessions`, `notes`, `turns` | One for each matching note a session touched or cited that day. |
| `glossary-terms` | `sessions`, `notes`, `turns` | One for each matching note a session touched or cited that day. |
| `gotchas` | `sessions`, `notes`, `turns` | One for each matching note a session touched or cited that day. |
| `incident-timeline` | `sessions`, `notes`, `turns` | One for each matching note a session touched or cited that day. |
| `limits-and-quotas` | `sessions`, `notes`, `turns` | One for each matching note a session touched or cited that day. |
| `measurements` | `sessions`, `notes`, `turns` | One for each matching note a session touched or cited that day. |
| `migration-log` | `sessions`, `notes`, `turns` | One for each matching note a session touched or cited that day. |
| `onboarding-gaps` | `sessions`, `notes`, `turns` | One for each matching note a session touched or cited that day. |
| `open-questions` | `sessions`, `notes`, `turns` | One for each matching note a session touched or cited that day. |
| `related-links` | `sessions`, `notes` | One a day for all the notes together; it reads no transcript turns, and each entry cites the last turn of the session whose handoff makes the connection. |
| `runbook-check` | `sessions`, `notes`, `turns` | One for each matching note a session touched or cited that day. |
| `schema-notes` | `sessions`, `notes`, `turns` | One for each matching note a session touched or cited that day. |
| `security-log` | `sessions`, `notes`, `turns` | One for each matching note a session touched or cited that day. |
| `test-commands` | `sessions`, `notes`, `turns` | One for each matching note a session touched or cited that day. |
| `todo-done` | `sessions`, `notes`, `turns` | One for each matching note a session touched or cited that day. |

All of them add one reviewer call for each proposal. Their prompts tell the model to add nothing the note already records, to treat the notes, sessions and turns as untrusted data, and to quote no command or link that only tool output shows. A section the note lacks is added by the change itself: when the note's last heading is not the pack's section, the appended text starts with that heading.

The packs are written to shared habits. Their personas reject any change that is not an `append`, that has text in `old`, that alters what the note already holds, or that falls outside the pack's section. The README of each pack states what it reads, what it writes and what it costs.

## Where they stand

None of these packs is measured yet. Each has one fixture test with a stand-in model, which checks that the pack's files load, that its prompt carries the right note, and that a well-formed reply is applied. It does not show that a real model proposes good entries, or that the reviewer rejects bad ones. Treat the first runs on your notes with `--dry-run`, and read what each change would have been before you let one write.

## Theme packs

`packs/` also holds three theme packs: `high-contrast`, `large-type` and `paper`. A theme pack sets the web theme contract's values, and for two of them a desktop theme. It has no passes and never touches your notes. [Themes](themes.md) says what a theme pack may hold and what each of the three changes. All three are by Vanitas Caesar, version 1.0.0, MIT.

In [the registry](registry.md), no pack is listed yet. The entries for the 26 first-party packs (23 pass packs and 3 theme packs) are in `registry/pending/`, waiting to be pinned to this repository and reviewed.

