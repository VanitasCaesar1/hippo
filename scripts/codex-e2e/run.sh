#!/bin/sh
# End-to-end check of hippo's Codex hooks: a real Codex CLI, with the hooks
# `hippo hooks install --agent codex` writes, against a canned Responses API
# server (fake-responses.mjs) and a real hippo, which then indexes the same
# session from Codex's history. No model, no key and no network are used.
#
#   scripts/codex-e2e/run.sh <codex binary> <hippo binary>
#
# Everything this script writes is under a new temporary folder, which is
# its HOME, HIPPO_HOME, CODEX_HOME and XDG_* folders; it prints that folder
# at the end. It never touches your own ~/.codex, ~/.hippo or agent history.
# `--dangerously-bypass-hook-trust` stands in for the person trusting the
# hooks in Codex's review, which `codex exec` does not show.
set -eu

[ $# -eq 2 ] || { echo "usage: $0 <codex binary> <hippo binary>" >&2; exit 2; }
here=$(cd "$(dirname "$0")" && pwd)
codex_bin=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
hippo_bin=$(cd "$(dirname "$2")" && pwd)/$(basename "$2")
[ -x "$codex_bin" ] || { echo "not an executable: $codex_bin" >&2; exit 2; }
[ -x "$hippo_bin" ] || { echo "not an executable: $hippo_bin" >&2; exit 2; }

scratch=$(mktemp -d "${TMPDIR:-/tmp}/hippo-codex-e2e.XXXXXX")
scratch=$(cd "$scratch" && pwd -P)
export HOME="$scratch/home" HIPPO_HOME="$scratch/home/.hippo" CODEX_HOME="$scratch/home/.codex" TZ=UTC
export XDG_DATA_HOME="$scratch/data" XDG_CONFIG_HOME="$scratch/config" \
  XDG_CACHE_HOME="$scratch/cache" XDG_STATE_HOME="$scratch/state"
unset OPENAI_API_KEY OPENAI_BASE_URL CODEX_API_KEY
mkdir -p "$HOME/.local/bin" "$CODEX_HOME"
# The hippo under test only in the scratch ~/.local/bin, first on PATH;
# every PATH folder that holds another hippo is dropped.
ln -s "$hippo_bin" "$HOME/.local/bin/hippo"
path=
old_ifs=$IFS
IFS=:
for dir in $PATH; do
  [ -n "$dir" ] && [ ! -e "$dir/hippo" ] && path=${path:+$path:}$dir
done
IFS=$old_ifs
export PATH="$HOME/.local/bin:$path"
[ "$(command -v hippo)" = "$HOME/.local/bin/hippo" ] || { echo "another hippo is on PATH" >&2; exit 1; }

# A workspace with a notes folder: a start block and a note the prompt
# should bring.
ws="$scratch/ws"
mkdir -p "$ws/notes" "$ws/app"
git -C "$ws" init -q
printf '# Start here\n\nE2E-START: this workspace builds with pnpm and ships on Fridays.\n' >"$ws/notes/START HERE.md"
printf '# Thumbnails\n\nE2E-RELEVANT: thumbnails are made at 320 and 640 pixels.\n' >"$ws/notes/Thumbnails.md"
(cd "$ws" && hippo init --notes-dir notes --entry-note "notes/START HERE.md" >/dev/null)
hippo hooks install --agent codex >"$scratch/install.txt"
hippo hooks status --agent codex | grep -c installed | grep -qx 3 || { echo "e2e: FAIL: hooks not installed" >&2; exit 1; }

# The canned model.
export FAKE_LLM_LOG="$scratch/requests.jsonl"
node "$here/fake-responses.mjs" >"$scratch/port" &
llm=$!
trap 'kill $llm 2>/dev/null || true' EXIT
for _ in 1 2 3 4 5 6 7 8 9 10; do [ -s "$scratch/port" ] && break; sleep 0.2; done
port=$(cat "$scratch/port")

cat >"$CODEX_HOME/config.toml" <<TOML
model = "fake-model"
model_provider = "fake"
check_for_update_on_startup = false

[model_providers.fake]
name = "Fake"
base_url = "http://127.0.0.1:$port/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
TOML

# One task, in the workspace.
cd "$ws/app"
"$codex_bin" exec --skip-git-repo-check --dangerously-bypass-hook-trust --json \
  "What sizes do we make thumbnails at?" >"$scratch/run.jsonl" 2>"$scratch/run.err" \
  || { cat "$scratch/run.err" >&2; exit 1; }

node "$here/check.mjs" "$scratch"

# hippo indexes the same session from Codex's history.
hippo ingest >"$scratch/ingest.txt"
id=$(node -e 'const fs=require("fs");const d=process.argv[1];const f=fs.readdirSync(d).find(n=>n.startsWith("codex-"));console.log(f.replace(/\.json$/,""))' "$ws/.hippo/sessions")
hippo show session "$id" >"$scratch/show.txt" 2>&1 || { echo "e2e: FAIL: hippo did not index $id" >&2; cat "$scratch/ingest.txt" "$scratch/show.txt" >&2; exit 1; }
grep -q "What sizes do we make thumbnails at" "$scratch/show.txt" || { echo "e2e: FAIL: the indexed session $id has no prompt" >&2; cat "$scratch/show.txt" >&2; exit 1; }
echo "e2e: hippo indexed $id: $(cat "$scratch/ingest.txt")"
echo "scratch: $scratch"
