---
type: topic
status: current
updated: 2026-10-23
---
# Mobile app

The Tern mobile app browses the library and backs up the phone's camera roll to the server.

## Backup behaviour

- New photos upload in the background over wifi, or over mobile data if the person allows it.
- Large videos use the chunked path of the [[Upload API]].

## Releases

- The app ships through the stores about a week after the server release.
- Server changes must stay compatible with the last two app versions.

## Debugging

- Settings has a "send logs" button that attaches the last 500 lines.
- Sign-in problems usually trace back to [[Auth sessions]].
