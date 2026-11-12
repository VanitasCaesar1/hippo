#!/bin/sh
# Re-pins every entry in registry/pending/ to one repository and commit, and
# moves the entries into registry/packs/, where `hippocamp-registry check`
# and `build` read them. Each still needs a review before it is listed
# (registry/pending/README.md says how).
#
#   scripts/registry-repin.sh <owner>/<repo> <commit> [<repo_id> <owner_id>]
#
# Without the two ids it asks GitHub for them with `gh api`. The commit must
# be a full 40-character hash that the repository holds.
set -eu
[ $# -eq 2 ] || [ $# -eq 4 ] || {
  echo "usage: scripts/registry-repin.sh <owner>/<repo> <commit> [<repo_id> <owner_id>]" >&2
  exit 2
}
repo=$1 commit=$2
case $repo in */*) ;; *) echo "registry-repin: give the repository as <owner>/<repo>" >&2; exit 2 ;; esac
printf '%s' "$commit" | grep -Eq '^[0-9a-f]{40}$' || {
  echo "registry-repin: the commit must be a full 40-character hash" >&2
  exit 2
}
if [ $# -eq 4 ]; then
  repo_id=$3 owner_id=$4
else
  ids=$(gh api "repos/$repo" --jq '.id, .owner.id')
  repo_id=$(printf '%s\n' "$ids" | sed -n 1p)
  owner_id=$(printf '%s\n' "$ids" | sed -n 2p)
fi
for n in "$repo_id" "$owner_id"; do
  printf '%s' "$n" | grep -Eq '^[0-9]+$' || { echo "registry-repin: not an id: $n" >&2; exit 2; }
done

root=$(cd "$(dirname "$0")/.." && pwd)
pending=$root/registry/pending
packs=$root/registry/packs
moved=0
for f in "$pending"/*.toml; do
  [ -e "$f" ] || continue
  name=$(basename "$f")
  [ ! -e "$packs/$name" ] || { echo "registry-repin: $packs/$name exists already" >&2; exit 1; }
  tmp=$(mktemp "$packs/.$name.XXXXXX")
  sed -e "s|^repo = .*|repo = \"https://github.com/$repo\"|" \
    -e "s|^repo_id = .*|repo_id = $repo_id|" \
    -e "s|^owner_id = .*|owner_id = $owner_id|" \
    -e "s|^commit = .*|commit = \"$commit\"|" "$f" >"$tmp"
  mv "$tmp" "$packs/$name"
  rm "$f"
  moved=$((moved + 1))
done
echo "re-pinned $moved entries to $repo at $commit (repository $repo_id, owner $owner_id) in registry/packs/"
echo "next: review each entry (registry/pending/README.md), then cargo run -p hippocamp-registry -- check"
