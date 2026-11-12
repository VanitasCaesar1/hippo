---
name: day
description: Show what the coding agents on this machine did on the last active day, or on a given date, from hippo's index.
argument-hint: "[YYYY-MM-DD]"
disable-model-invocation: true
allowed-tools: Bash(hippo), Bash(hippo show day *)
---

The date asked for, if any: $ARGUMENTS

- With no date, run `hippo`. It reads any new transcripts, then prints the overview of the last active day.
- With a date, run `hippo show day <date>` with the date as `YYYY-MM-DD` (convert "yesterday" or a weekday to a date first).
- If `hippo` is not found or says it is not set up, tell the user to run `/hippo:setup` and stop.

Show the output as printed, then summarise it in at most five lines: what got done, what went wrong (the corrections, quoted), and what is still open. Mention one of its suggested commands only if it answers something the user is likely to ask next.
