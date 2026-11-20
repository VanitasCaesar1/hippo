#!/bin/sh
# Installs hippo's Codex plugin from this checkout into a scratch Codex, the
# way a user does from GitHub, and checks that Codex loads its skill:
# `codex plugin marketplace add`, `codex plugin add hippo@hippo`, then
# skills/list through `codex app-server`. No model is called.
#
#   scripts/codex-e2e/plugin.sh <codex binary>
#
# Everything is written under a new temporary folder, its HOME and
# CODEX_HOME, which is deleted at the end. Your own ~/.codex is not read.
set -eu

[ $# -eq 1 ] || { echo "usage: $0 <codex binary>" >&2; exit 2; }
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
codex_bin=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
[ -x "$codex_bin" ] || { echo "not an executable: $codex_bin" >&2; exit 2; }

scratch=$(mktemp -d "${TMPDIR:-/tmp}/hippo-codex-plugin.XXXXXX")
scratch=$(cd "$scratch" && pwd -P)
trap 'rm -rf "$scratch"' EXIT INT TERM
export HOME="$scratch" CODEX_HOME="$scratch/.codex"
unset OPENAI_API_KEY OPENAI_BASE_URL CODEX_API_KEY
mkdir -p "$CODEX_HOME" "$scratch/project"
version=$(sed -n 's/^ *"version": *"\(.*\)",*$/\1/p' "$root/plugins/codex/.codex-plugin/plugin.json")

fail() { echo "plugin: FAIL: $*" >&2; exit 1; }

"$codex_bin" plugin marketplace add "$root" >"$scratch/add.txt" 2>&1 || { cat "$scratch/add.txt" >&2; fail "marketplace add"; }
grep -q "Added marketplace \`hippo\`" "$scratch/add.txt" || { cat "$scratch/add.txt" >&2; fail "the marketplace is not named hippo"; }
"$codex_bin" plugin add hippo@hippo >"$scratch/plugin.txt" 2>&1 || { cat "$scratch/plugin.txt" >&2; fail "plugin add"; }
"$codex_bin" plugin list >"$scratch/list.txt" 2>&1 || { cat "$scratch/list.txt" >&2; fail "plugin list"; }
grep -Eq "^hippo@hippo +installed, enabled +$version " "$scratch/list.txt" || { cat "$scratch/list.txt" >&2; fail "hippo@hippo $version is not installed and enabled"; }
cache="$CODEX_HOME/plugins/cache/hippo/hippo/$version"
[ -f "$cache/skills/hippo/SKILL.md" ] || fail "no skill in $cache"
[ ! -e "$cache/hooks" ] || fail "the plugin carries hooks"

(cd "$scratch/project" && node "$here/skills-list.mjs" "$codex_bin" "$scratch/project") >"$scratch/skills.txt" 2>&1 \
  || { cat "$scratch/skills.txt" >&2; fail "skills/list"; }
grep -q "^hippo:hippo $cache/skills/hippo/SKILL.md$" "$scratch/skills.txt" || { cat "$scratch/skills.txt" >&2; fail "Codex does not list hippo:hippo"; }
grep -qx 'errors \[\]' "$scratch/skills.txt" || { cat "$scratch/skills.txt" >&2; fail "Codex reported skill errors"; }
echo "plugin: hippo@hippo $version installed from $root; Codex lists hippo:hippo with no errors"
