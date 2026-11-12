---
type: topic
status: current
updated: 2026-10-25
---
# Sharing links

Public share links let someone without an account open an album or a single photo.

## Expiry

- Share links expire after 30 days by default.
- The owner can choose a shorter time for each link, or revoke it at any moment.

## Behaviour

- A link carries a random token and no user id, so it leaks nothing about the owner.
- Opening an expired link shows a plain "link expired" page, not an error.
- The viewer only loads thumbnails and the original of shared photos, see [[Thumbnail pipeline]].

## Known gaps

- The viewer page is slow to load on very old phones.
- Download of a whole shared album as a zip is not built yet.
