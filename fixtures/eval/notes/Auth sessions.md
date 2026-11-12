---
type: topic
status: current
updated: 2026-10-19
---
# Auth sessions

Sign-in gives a browser or phone a session token that the server checks on every request.

## Lifetime

- Login sessions last 7 days.
- Signing out on one device ends only that device's session.

## Two-factor

- Two-factor login uses TOTP codes.
- Ten one-time recovery codes are shown once when two-factor is switched on.

## Tokens

- Session tokens are random, stored hashed, and can be revoked from the account page.
- The mobile app keeps its token in the system keychain, see [[Mobile app]].
