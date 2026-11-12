# Security

hippocamp reads agent transcripts and writes to files agents load at session start. Both are sensitive, so security reports are taken seriously.

## Reporting

Use GitHub's private vulnerability reporting on this repository (*Security* → *Report a vulnerability*). Do not open a public issue for a security problem.

You will get an acknowledgement within three days. Fixes for confirmed issues are released before any public discussion, and reporters are credited unless they prefer not to be.

## Scope

In scope:

- Anything that lets a pack read or write outside its declared permissions.
- Anything that gets secrets or personal data past redaction into the index.
- Anything that lets a dreaming pass write without a recorded review.
- Hidden instructions in a pack that affect an agent's behaviour.

Out of scope: vulnerabilities in the agents themselves (Claude Code, Codex and others), which belong to their vendors.
