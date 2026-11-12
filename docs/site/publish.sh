#!/bin/sh
# Publishes the built site (docs/site/out, from docs/site/build.sh) to the
# gh-pages branch of the remote, for https://vanitascaesar1.github.io/hippocamp/.
# It commits the folder as the whole branch, adds
# .nojekyll so GitHub serves it as is, and pushes that branch only. Nothing
# else in the repository changes.
#
#   docs/site/build.sh && docs/site/publish.sh [remote]     # remote: origin
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
out="$root/docs/site/out"
remote=${1:-origin}
cd "$root"

[ -f "$out/index.html" ] && [ -f "$out/docs/index.html" ] && [ -f "$out/packs.html" ] || {
  echo "publish: no built site in $out; run docs/site/build.sh first" >&2
  exit 1
}
[ -z "$(git status --porcelain --untracked-files=no)" ] || {
  echo "publish: the working tree has changes; commit or stash them so the site matches a commit" >&2
  exit 1
}
commit=$(git rev-parse --short HEAD)

work=$(mktemp -d "${TMPDIR:-/tmp}/hippocamp-pages.XXXXXX")
trap 'git worktree remove --force "$work" 2>/dev/null; rm -rf "$work"' EXIT INT TERM
git worktree add --detach "$work" >/dev/null
(
  cd "$work"
  git checkout --orphan gh-pages-publish >/dev/null 2>&1
  git rm -rfq . >/dev/null 2>&1 || true
  cp -R "$out"/. .
  touch .nojekyll
  git add -A
  git commit -qm "site: built from $commit"
  git push --force "$remote" HEAD:gh-pages
  git checkout --detach >/dev/null 2>&1
  git branch -D gh-pages-publish >/dev/null
)
echo "published $out to $remote gh-pages (from $commit)"
