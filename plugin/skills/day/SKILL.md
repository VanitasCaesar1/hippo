---
name: day
description: Show what the coding agents on this machine did on the last active day, or on a given date, from hippocamp's index.
argument-hint: "[YYYY-MM-DD]"
disable-model-invocation: true
allowed-tools: Bash(hippocamp), Bash(hippocamp show day *)
---

The date asked for, if any: $ARGUMENTS

- With no date, run `hippocamp`. It reads any new transcripts, then prints the overview of the last active day.
- With a date, run `hippocamp show day <date>` with the date as `YYYY-MM-DD` (convert "yesterday" or a weekday to a date first).
- If `hippocamp` is not found or says it is not set up, tell the user to run `/hippocamp:setup` and stop.

Show the output as printed, then summarise it in at most five lines: what got done, what went wrong (the corrections, quoted), and what is still open. Mention one of its suggested commands only if it answers something the user is likely to ask next.
