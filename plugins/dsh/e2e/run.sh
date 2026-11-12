#!/bin/sh
# End-to-end check of dsh-plugin-hippo: a real dsh, run from a checkout of
# its repository, with the plugin installed by `dsh plugin add`, against a
# canned OpenAI-compatible server (e2e/fake-llm.mjs) and a real hippo.
# No model, no key and no network are used.
#
#   plugins/dsh/e2e/run.sh <dsh checkout> <hippo binary>
#
# The dsh checkout must be built as its README says (`pnpm install`,
# `pnpm run build`) plus `pnpm run build:native-system`, and `pnpm` must be
# on PATH. Everything this script writes is under a new temporary folder,
# which is its HOME, HIPPO_HOME and DSH_HOME; it prints that folder at the
# end. It never touches your own ~/.dsh, ~/.hippo or agent history.
set -eu

[ $# -eq 2 ] || { echo "usage: $0 <dsh checkout> <hippo binary>" >&2; exit 2; }
plugin=$(cd "$(dirname "$0")/.." && pwd)
repo=$(cd "$plugin/../.." && pwd)
dsh=$(cd "$1" && pwd)
hippo_bin=$(cd "$(dirname "$2")" && pwd)/$(basename "$2")
[ -x "$hippo_bin" ] || { echo "not an executable: $hippo_bin" >&2; exit 2; }

scratch=$(mktemp -d "${TMPDIR:-/tmp}/dsh-plugin-hippo-e2e.XXXXXX")
export HOME="$scratch/home" HIPPO_HOME="$scratch/home/.hippo" TZ=UTC
unset DSH_HOME DSH_AGENTS_HOME DSH_PERMISSION_MODE
mkdir -p "$HOME/.local/bin"
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

cat >"$scratch/fake-model.patch.yml" <<EOF
- id: llm-pi-ai
  config:
    providers:
      fake:
        api: openai-completions
        baseURL: http://127.0.0.1:$port/v1
        apiKeyEnv: FAKE_LLM_KEY
        models:
          - id: fake-model
- id: agent-default-model
  config:
    provider: fake
    model: fake-model
EOF
export FAKE_LLM_KEY=not-a-real-key

# dsh from source, as `pnpm dsh` runs it, but from the workspace's folder:
# tsx needs dsh's tsconfig to resolve dsh's packages to their sources.
tsx="$dsh/node_modules/tsx/dist/esm/index.mjs"
dshcli() { TSX_TSCONFIG_PATH="$dsh/tsconfig.json" node --import "file://$tsx" "$dsh/apps/cli/src/bin.ts" "$@"; }

# A profile of its own, from dsh's headless template, with the plugin
# installed the way a user installs it. `dsh plugin` runs pnpm, which must
# be on PATH.
dshcli --profile e2e --from-default-profile headless --dump-config >/dev/null
dshcli plugin --profile e2e add "$plugin" >"$scratch/plugin-add.log" 2>&1 || { cat "$scratch/plugin-add.log" >&2; exit 1; }
dshcli --profile e2e --dump-config >"$scratch/config.yml"
grep -q 'dsh-plugin-hippo' "$scratch/config.yml"

# One headless task, in the workspace.
cd "$ws/app"
dshcli --profile e2e --patch "$scratch/fake-model.patch.yml" --json "What did agents already do about thumbnail sizes?" \
  >"$scratch/run.jsonl" 2>"$scratch/run.err" || { cat "$scratch/run.err" >&2; exit 1; }

node "$plugin/e2e/check.mjs" "$scratch"
echo "scratch: $scratch"
