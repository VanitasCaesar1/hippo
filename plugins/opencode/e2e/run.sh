#!/bin/sh
# End-to-end check of opencode-plugin-hippo: a real OpenCode 2, with the
# plugin loaded from opencode.json, against a canned OpenAI-compatible
# server (e2e/fake-llm.mjs) and a real hippo, which then reads the session
# back from OpenCode's database. No model, no key and no network are used.
#
#   plugins/opencode/e2e/run.sh <opencode binary> <hippo binary> [plugin spec]
#
# The plugin spec defaults to this folder; pass a package spec such as
# "github:VanitasCaesar1/hippo#v0.4.0::path:plugins/opencode" to check an
# install from GitHub (that one needs the network). Everything this script
# writes is under a new temporary folder, which is its HOME, HIPPO_HOME
# and XDG_* folders; it prints that folder at the end. It never touches
# your own ~/.config/opencode, ~/.local/share/opencode, ~/.hippo or agent
# history.
set -eu

[ $# -ge 2 ] || { echo "usage: $0 <opencode binary> <hippo binary> [plugin spec]" >&2; exit 2; }
plugin=$(cd "$(dirname "$0")/.." && pwd)
repo=$(cd "$plugin/../.." && pwd)
opencode_bin=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
hippo_bin=$(cd "$(dirname "$2")" && pwd)/$(basename "$2")
spec=${3:-$plugin}
[ -x "$opencode_bin" ] || { echo "not an executable: $opencode_bin" >&2; exit 2; }
[ -x "$hippo_bin" ] || { echo "not an executable: $hippo_bin" >&2; exit 2; }

scratch=$(mktemp -d "${TMPDIR:-/tmp}/opencode-plugin-hippo-e2e.XXXXXX")
export HOME="$scratch/home" HIPPO_HOME="$scratch/home/.hippo" TZ=UTC
export XDG_DATA_HOME="$scratch/data" XDG_CONFIG_HOME="$scratch/config" \
  XDG_CACHE_HOME="$scratch/cache" XDG_STATE_HOME="$scratch/state"
export OPENCODE_DISABLE_AUTOUPDATE=1 OPENCODE_DISABLE_MODELS_FETCH=1 \
  OPENCODE_DISABLE_CLAUDE_CODE=1 OPENCODE_DISABLE_FILEWATCHER=1
unset OPENCODE_DB OPENCODE_CONFIG OPENCODE_CONFIG_DIR OPENCODE_CONFIG_CONTENT
mkdir -p "$HOME/.local/bin" "$XDG_CONFIG_HOME/opencode"
# The hippo under test only in the scratch ~/.local/bin, where the plugin
# looks after PATH; every PATH folder that holds another hippo is dropped.
ln -s "$hippo_bin" "$HOME/.local/bin/hippo"
path=
old_ifs=$IFS
IFS=:
for dir in $PATH; do
  [ -n "$dir" ] && [ ! -e "$dir/hippo" ] && path=${path:+$path:}$dir
done
IFS=$old_ifs
export PATH="$path"
! command -v hippo >/dev/null 2>&1 || { echo "a hippo is still on PATH" >&2; exit 1; }
hippo() { "$hippo_bin" "$@"; }

# A workspace with a notes folder, and synthetic history to search.
ws="$scratch/ws"
mkdir -p "$ws/notes" "$ws/app"
git -C "$ws" init -q
printf '# Start here\n\nE2E-MARKER: this workspace builds with pnpm and ships on Fridays.\n' >"$ws/notes/START HERE.md"
(cd "$ws" && hippo init --notes-dir notes --entry-note "notes/START HERE.md" >/dev/null)
mkdir -p "$HOME/.claude/projects/-work-tern"
cp "$repo"/fixtures/eval/transcripts/claude-code/*.jsonl "$HOME/.claude/projects/-work-tern/"
hippo ingest >/dev/null

# The canned model.
export FAKE_LLM_LOG="$scratch/requests.jsonl"
node "$plugin/e2e/fake-llm.mjs" >"$scratch/port" &
llm=$!
trap 'kill $llm 2>/dev/null || true' EXIT
for _ in 1 2 3 4 5 6 7 8 9 10; do [ -s "$scratch/port" ] && break; sleep 0.2; done
port=$(cat "$scratch/port")

# OpenCode's own configuration: the canned model as a provider, and the
# plugin, as a user adds it.
cat >"$XDG_CONFIG_HOME/opencode/opencode.json" <<JSON
{
  "\$schema": "https://opencode.ai/config.json",
  "update": "disable",
  "share": "disabled",
  "providers": {
    "fake": {
      "name": "Fake",
      "package": "@opencode/ai/providers/openai-compatible",
      "settings": { "baseURL": "http://127.0.0.1:$port/v1" },
      "models": { "fake-model": { "name": "Fake model", "limit": { "context": 64000, "output": 4000 } } }
    }
  },
  "plugins": [
    { "package": "$spec", "options": { "tools": true } }
  ]
}
JSON

# One task, in the workspace, in OpenCode's standalone mode (no background
# service), allowed to run tools without asking.
cd "$ws/app"
"$opencode_bin" --standalone run --model fake/fake-model --format json --dangerously-skip-permissions \
  "What did agents already do about thumbnail sizes?" >"$scratch/run.jsonl" 2>"$scratch/run.err" \
  || { cat "$scratch/run.err" >&2; exit 1; }

node "$plugin/e2e/check.mjs" "$scratch"

# hippo reads the session back from OpenCode's database.
hippo ingest >"$scratch/ingest.txt"
hippo recall hippo_ask >"$scratch/recall.txt" || true
grep -q "opencode-ses_" "$scratch/recall.txt" || { echo "e2e: FAIL: hippo did not index the OpenCode session" >&2; cat "$scratch/ingest.txt" "$scratch/recall.txt" >&2; exit 1; }
hippo doctor | grep "OpenCode database"
echo "e2e: hippo indexed the OpenCode session: $(cat "$scratch/ingest.txt")"
echo "scratch: $scratch"
