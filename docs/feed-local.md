# Running the hippocamp feed locally

This page shows how to run `hippocamp-feed` on one machine with a mock sign-in, a fixture registry and demo data. The feed is not deployed, and there is no production procedure yet.

**Status, stated once.** The feed is built and tested locally. Nothing is deployed. Everything here runs on `127.0.0.1`, with no real GitHub account and no real secret. Dev mode (`FEED_DEV=1`) refuses a base URL that is not `http` on `127.0.0.1` or `localhost`, and a bind address that is not loopback.

Run every command from the repository root. You need a Rust toolchain, `python3` and `curl`. `sqlite3` is optional (see "Backdating a member").

## The environment

Put these in a file such as `feed-dev.env`, outside the repository or ignored, and load it with `set -a; . ./feed-dev.env; set +a`. Each subcommand reads only the variables it needs and ignores the rest.

```sh
FEED_DEV=1
FEED_BASE_URL=http://127.0.0.1:8080
FEED_DB=/tmp/hippocamp-feed-dev.db
FEED_GITHUB_CLIENT_ID=hippocamp-feed-dev
FEED_GITHUB_CLIENT_SECRET=dev-secret-not-real
FEED_OAUTH_AUTHORIZE_URL=http://127.0.0.1:9100/login/oauth/authorize
FEED_OAUTH_TOKEN_URL=http://127.0.0.1:9100/login/oauth/access_token
FEED_OAUTH_API_URL=http://127.0.0.1:9100
FEED_ADMINS=1001
FEED_REGISTRY_INDEX=fixtures/feed/registry-index.json
FEED_RAW_URL=http://127.0.0.1:9200
FEED_EVAL_SETS=000000000000:7:8
```

- `FEED_DB` is a path. Use a throwaway file. Delete it to start again.
- `FEED_ADMINS=1001` makes the mock user `ada` an admin.
- `FEED_REGISTRY_INDEX` is a file path or an `http` URL (dev mode only).
- `FEED_EVAL_SETS=000000000000:7:8` opens the measure page for the demo eval set (7 planted errors, 8 kept claims).
- `FEED_BIND` defaults to `127.0.0.1:8080`. Other variables keep their defaults.

## Order of commands

Use four terminals, or run the first two servers in the background.

1. Mock sign-in provider, on port 9100:

   ```sh
   cargo run -p hippocamp-feed -- mock-oauth
   ```

2. Raw file server for theme CSS, on port 9200:

   ```sh
   python3 -m http.server 9200 --directory fixtures/feed/raw
   ```

3. Create the database, read the registry index and add the demo data:

   ```sh
   cargo run -p hippocamp-feed -- migrate
   cargo run -p hippocamp-feed -- sync-registry
   cargo run -p hippocamp-feed -- seed-demo
   ```

   `seed-demo` refuses to run unless packs are listed, so `sync-registry` comes first. It also refuses to run twice. It adds eight demo members (`demo-1` to `demo-8`), 12 discussions whose titles hold `[demo] `, 40 comments, 150 votes, and measurements of `glossary-terms` (three), `decision-log` (three) and, when the index lists it, `review-dates` (one).

4. Start the server:

   ```sh
   cargo run -p hippocamp-feed -- serve
   ```

Open <http://127.0.0.1:8080/>. Stop the servers with Ctrl-C when you finish.

## Signing in as a mock user

Open <http://127.0.0.1:8080/login> and choose a user on the mock's page.

| Login | GitHub id | Notes |
|---|---|---|
| `ada` | 1001 | Admin, because of `FEED_ADMINS=1001`. Reach `/admin`, `/mod` and `/moderation`. |
| `grace` | 1002 | GitHub account from 2012. |
| `linus` | 1003 | GitHub account from 2019. |
| `newbie` | 1004 | GitHub account ten days old. |
| `alan` | 1005 | GitHub account from 2016. |

Signing in creates a feed account at that moment. A member is established only when the feed account is at least 3 days old and the GitHub account at least 90 days old. Until the clock passes 3 days, every mock user is a new member: no links in posts or comments, and no measurements. The eight demo members are the exception. They are seeded with accounts ten days old.

### Backdating a member

The server reads the system clock, so you cannot wait 3 days in a walkthrough. On the dev database only, change the stored time. Times are text in the form `YYYY-MM-DDTHH:MM:SS.ffffffZ`, in UTC.

```sh
sqlite3 "$FEED_DB" "UPDATE users SET created_at = strftime('%Y-%m-%dT%H:%M:%f000Z', 'now', '-4 days') WHERE login = 'ada';"
```

`%f` writes seconds with three decimals, and the added `000` makes six, which is the width the feed expects. Reload the page. Never do this on a database that holds real data, and never edit the clock-dependent rows of a deployed feed.

## Walkthrough with curl

The API has two unauthenticated `GET` endpoints.

```sh
# Health
curl -i http://127.0.0.1:8080/healthz

# Posts ranked by evidence (sort is front, new or evidence; page starts at 1)
curl -s 'http://127.0.0.1:8080/api/v1/posts?sort=evidence' | jq '.posts[] | {id, title, votes}'

# One post, with its measurements and comments (use an id from the list)
curl -s http://127.0.0.1:8080/api/v1/posts/1 | jq '.post.pack.evidence, (.comments | length)'

# A 400: sort is not one of the three values
curl -i 'http://127.0.0.1:8080/api/v1/posts?sort=hot'

# A 404: no such post
curl -i http://127.0.0.1:8080/api/v1/posts/999999
```

`jq` is optional. Pages to look at in a browser: `/`, `/evidence`, `/themes`, a pack's post, a discussion with nested comments, `/settings` (set the theme to `dark`), `/mod` and `/moderation` (as `ada`).

## Variant: seed from the repository's own registry

The fixture lists 15 invented packs. To seed from this repository's first-party packs instead, build the index offline from the registry. It then holds the 25 listed first-party packs: 22 pass packs and the 3 theme packs `high-contrast`, `large-type` and `paper`.

```sh
cargo run -p hippocamp-registry -- build --offline --out /tmp/hippocamp-registry
export FEED_REGISTRY_INDEX=/tmp/hippocamp-registry/index.json
```

`seed-demo` measures `glossary-terms` and `decision-log`, and `review-dates` only when the index lists it. Only the fixture lists `review-dates`, so this variant has two measured packs.

The feed fetches each theme pack's `theme.css` from `<FEED_RAW_URL>/<owner>/<repo>/<commit>/<path>/theme.css`. Fill a directory with those files from git, then serve it on port 9200 in place of `fixtures/feed/raw`:

```sh
raw=/tmp/hippocamp-raw
jq -r '.packs[] | select(.kind == "theme") | [.repo, .commit, .path] | @tsv' \
  /tmp/hippocamp-registry/index.json |
while IFS="$(printf '\t')" read -r repo commit path; do
  slug=${repo#https://github.com/}
  mkdir -p "$raw/$slug/$commit/$path"
  git show "$commit:$path/theme.css" > "$raw/$slug/$commit/$path/theme.css"
done
python3 -m http.server 9200 --directory "$raw"
```

The commit must exist in your local clone. A theme pack whose `theme.css` cannot be fetched or does not match the index's hash still appears on `/themes`, marked "This theme is not available.", and cannot be picked.

Start again from an empty `FEED_DB` when you change the index, so demo data and packs agree.

## Screenshots

Screenshots of the pages above, at 1280 and 360 pixels wide, are taken by hand with a headless browser. They run outside CI and outside this repository's dependencies, so install Playwright elsewhere. They are not committed.
