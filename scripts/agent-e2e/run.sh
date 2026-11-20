#!/bin/sh
# End-to-end check of hippo's guard inside real agent CLIs, each in a
# scratch home, against a stub model on 127.0.0.1 (stub-model.mjs). No
# model, no key and no account are used.
#
#   scripts/agent-e2e/run.sh <hippo binary> [agent ...]
#
# Agents: claude, codex, gemini, opencode, dsh (all by default). Each one's
# binary is found on PATH, or given as CLAUDE_BIN, CODEX_BIN, GEMINI_BIN,
# OPENCODE_BIN; dsh needs a built checkout of its repository in
# DSH_CHECKOUT. An agent that is not there is skipped, with a message.
#
# For each agent this script makes a new folder that is its HOME,
# HIPPO_HOME, CLAUDE_CONFIG_DIR, CODEX_HOME and XDG_* folders, installs the
# guard there the way a user does, and asks the agent to "set up the
# project". The stub model answers with four shell calls (a download piped
# into sh, a read of a synthetic SSH key, `ls`, and `hippo guard allow`)
# and a final message. check.mjs then reads what the stub received, the
# guard's log and the agent's output.
#
# Network: the agent's PATH holds only the hippo under test, the agent and
# node; HTTP(S)_PROXY points at the stub, which refuses and logs every
# outside host; and lsof samples the agent's process tree during the run.
# Any connection to an address that is not 127.0.0.1 fails the check.
#
# It never touches your own ~/.claude*, ~/.codex, ~/.gemini,
# ~/.config/opencode, ~/.dsh or ~/.hippo.
set -eu

[ $# -ge 1 ] || { echo "usage: $0 <hippo binary> [claude|codex|gemini|opencode|dsh ...]" >&2; exit 2; }
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
hippo_bin=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
[ -x "$hippo_bin" ] || { echo "not an executable: $hippo_bin" >&2; exit 2; }
shift
agents=${*:-claude codex gemini opencode dsh}

node_bin=$(command -v node) || { echo "agent-e2e: node is needed for the stub model" >&2; exit 2; }
real() { (cd "$(dirname "$1")" && p=$(pwd -P)/$(basename "$1"); while [ -L "$p" ]; do l=$(readlink "$p"); case $l in /*) p=$l ;; *) p=$(dirname "$p")/$l ;; esac; done; echo "$p"); }
node_bin=$(real "$node_bin")
lsof_bin=$(PATH=/usr/sbin:/usr/bin:/sbin:/bin command -v lsof) || { echo "agent-e2e: lsof is needed for the network check" >&2; exit 2; }

scratch=$(mktemp -d "${TMPDIR:-/tmp}/hippo-agent-e2e.XXXXXX")
scratch=$(cd "$scratch" && pwd -P)
echo "agent-e2e: scratch $scratch"
: >"$scratch/results"

# The descendants of a process, itself included.
tree() (
  echo "$1"
  for child in $(pgrep -P "$1" 2>/dev/null); do tree "$child"; done
)

# Samples the network sockets of $1's process tree into $2 until it exits.
watch_net() {
  while kill -0 "$1" 2>/dev/null; do
    pids=$(tree "$1" | paste -sd, -)
    [ -n "$pids" ] && "$lsof_bin" -nP -a -i -p "$pids" 2>/dev/null | awk 'NR > 1 { print $1, $(NF-1), $NF }' >>"$2"
    sleep 0.1
  done
}

# Runs "$@" in the background with a timeout, watching its network.
# Leaves the exit status in $status.
run_watched() {
  timeout=$1
  shift
  "$@" >"$dir/out.txt" 2>"$dir/err.txt" </dev/null &
  pid=$!
  watch_net "$pid" "$dir/net.txt" &
  watcher=$!
  ( sleep "$timeout"; kill "$pid" 2>/dev/null ) &
  killer=$!
  status=0
  wait "$pid" || status=$?
  kill "$killer" 2>/dev/null || true
  wait "$watcher" 2>/dev/null || true
}

# A scratch home for agent $1, the stub running for it, and the guard's
# environment. Sets dir, port, stub and PATH.
setup() {
  dir="$scratch/$1"
  mkdir -p "$dir/home/.local/bin" "$dir/bin" "$dir/ws"
  export HOME="$dir/home" HIPPO_HOME="$dir/home/.hippo" TZ=UTC
  export CLAUDE_CONFIG_DIR="$HOME/.claude" CODEX_HOME="$HOME/.codex"
  export XDG_DATA_HOME="$dir/data" XDG_CONFIG_HOME="$dir/config" \
    XDG_CACHE_HOME="$dir/cache" XDG_STATE_HOME="$dir/state"
  unset ANTHROPIC_AUTH_TOKEN ANTHROPIC_MODEL OPENAI_API_KEY OPENAI_BASE_URL CODEX_API_KEY \
    GOOGLE_API_KEY GOOGLE_CLOUD_PROJECT GOOGLE_GENAI_USE_VERTEXAI GOOGLE_GENAI_USE_GCA \
    OPENCODE_DB OPENCODE_CONFIG OPENCODE_CONFIG_DIR OPENCODE_CONFIG_CONTENT DSH_HOME DSH_AGENTS_HOME
  ln -s "$hippo_bin" "$HOME/.local/bin/hippo"
  ln -s "$node_bin" "$dir/bin/node"
  export PATH="$HOME/.local/bin:$dir/bin:/usr/bin:/bin"
  # A synthetic private key for the agent to read; never a real one.
  mkdir -p "$HOME/.ssh"
  printf 'SYNTHETIC-E2E-KEY: not a key, a marker for the check\n' >"$HOME/.ssh/id_ed25519"
  chmod 700 "$HOME/.ssh" && chmod 600 "$HOME/.ssh/id_ed25519"
  git -C "$dir/ws" init -q
  printf '# A project\n' >"$dir/ws/README.md"

  STUB_LOG="$dir/requests.jsonl" "$node_bin" "$here/stub-model.mjs" >"$dir/port" &
  stub=$!
  for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15; do [ -s "$dir/port" ] && break; sleep 0.2; done
  port=$(cat "$dir/port")
  proxy="http://127.0.0.1:$port"
  export HTTP_PROXY="$proxy" HTTPS_PROXY="$proxy" ALL_PROXY="$proxy" \
    http_proxy="$proxy" https_proxy="$proxy" all_proxy="$proxy" \
    NO_PROXY=127.0.0.1,localhost no_proxy=127.0.0.1,localhost
}

finish() {
  kill "$stub" 2>/dev/null || true
  wait "$stub" 2>/dev/null || true
  if "$node_bin" "$here/check.mjs" "$1" "$dir" "$status"; then
    echo "passed $1" >>"$scratch/results"
  else
    echo "failed $1" >>"$scratch/results"
  fi
}

skip() {
  echo "agent-e2e: $1: skipped: $2"
  echo "skipped $1" >>"$scratch/results"
}

link_agent() {
  ln -s "$(real "$1")" "$dir/bin/$2"
}

run_claude() {
  bin=${CLAUDE_BIN:-$(command -v claude || true)}
  [ -n "$bin" ] || { skip claude "no claude on PATH or in CLAUDE_BIN"; return; }
  setup claude
  link_agent "$bin" claude
  mkdir -p "$CLAUDE_CONFIG_DIR"
  hippo guard install --yes >"$dir/install.txt"
  grep -q "guard check --agent claude-code" "$CLAUDE_CONFIG_DIR/settings.json"
  export ANTHROPIC_BASE_URL="http://127.0.0.1:$port" ANTHROPIC_API_KEY=stub \
    CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 DISABLE_AUTOUPDATER=1 DISABLE_TELEMETRY=1 \
    DISABLE_ERROR_REPORTING=1
  cd "$dir/ws"
  run_watched 180 claude -p "Set up the project." --output-format stream-json --verbose \
    --allowedTools Bash --max-turns 10
  cd "$scratch"
  unset ANTHROPIC_BASE_URL ANTHROPIC_API_KEY
  finish claude
}

run_gemini() {
  bin=${GEMINI_BIN:-$(command -v gemini || true)}
  [ -n "$bin" ] || { skip gemini "no gemini on PATH or in GEMINI_BIN"; return; }
  setup gemini
  link_agent "$bin" gemini
  mkdir -p "$HOME/.gemini"
  cat >"$HOME/.gemini/settings.json" <<'JSON'
{
  "general": { "enableAutoUpdate": false, "enableAutoUpdateNotification": false },
  "privacy": { "usageStatisticsEnabled": false },
  "telemetry": { "enabled": false },
  "security": { "auth": { "selectedType": "gemini-api-key" }, "folderTrust": { "enabled": false } }
}
JSON
  hippo guard install --agent gemini --yes >"$dir/install.txt"
  grep -q "guard check --agent gemini" "$HOME/.gemini/settings.json"
  export GOOGLE_GEMINI_BASE_URL="http://127.0.0.1:$port" GEMINI_API_KEY=stub GEMINI_CLI_NO_RELAUNCH=true
  cd "$dir/ws"
  run_watched 180 gemini -p "Set up the project." --output-format stream-json --approval-mode yolo
  cd "$scratch"
  unset GOOGLE_GEMINI_BASE_URL GEMINI_API_KEY
  finish gemini
}

run_opencode() {
  bin=${OPENCODE_BIN:-$(command -v opencode || true)}
  [ -n "$bin" ] || { skip opencode "no opencode on PATH or in OPENCODE_BIN"; return; }
  setup opencode
  link_agent "$bin" opencode
  mkdir -p "$XDG_CONFIG_HOME/opencode"
  cat >"$XDG_CONFIG_HOME/opencode/opencode.json" <<JSON
{
  "\$schema": "https://opencode.ai/config.json",
  "update": "disable",
  "share": "disabled",
  "providers": {
    "stub": {
      "name": "Stub",
      "package": "@opencode/ai/providers/openai-compatible",
      "settings": { "baseURL": "http://127.0.0.1:$port/v1" },
      "models": { "stub-model": { "name": "Stub model", "limit": { "context": 64000, "output": 4000 } } }
    }
  },
  "plugins": [
    { "package": "$repo/plugins/opencode", "options": { "guard": true } }
  ]
}
JSON
  export OPENCODE_DISABLE_AUTOUPDATE=1 OPENCODE_DISABLE_MODELS_FETCH=1 \
    OPENCODE_DISABLE_CLAUDE_CODE=1 OPENCODE_DISABLE_FILEWATCHER=1
  cd "$dir/ws"
  run_watched 180 opencode --standalone run --model stub/stub-model --format json \
    --dangerously-skip-permissions "Set up the project."
  cd "$scratch"
  finish opencode
}

run_codex() {
  bin=${CODEX_BIN:-$(command -v codex || true)}
  [ -n "$bin" ] || { skip codex "no codex on PATH or in CODEX_BIN"; return; }
  setup codex
  link_agent "$bin" codex
  mkdir -p "$CODEX_HOME"
  hippo guard install --agent codex --yes >"$dir/install.txt" 2>&1 \
    || { skip codex "this hippo has no guard for Codex: $(head -1 "$dir/install.txt")"; kill "$stub"; return; }
  cat >>"$CODEX_HOME/config.toml" <<TOML
model = "stub-model"
model_provider = "stub"
check_for_update_on_startup = false

[analytics]
enabled = false

[model_providers.stub]
name = "Stub"
base_url = "http://127.0.0.1:$port/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
TOML
  cd "$dir/ws"
  # `--dangerously-bypass-hook-trust` stands in for the person trusting
  # the hook in Codex's review, which `codex exec` does not show.
  run_watched 180 codex exec --dangerously-bypass-hook-trust --skip-git-repo-check --json \
    "Set up the project."
  cd "$scratch"
  finish codex
}

run_dsh() {
  command -v pnpm >/dev/null 2>&1 || { skip dsh "pnpm is not on PATH; dsh's plugin add runs it"; return; }
  pnpm_bin=$(real "$(command -v pnpm)")
  if [ -n "${DSH_CHECKOUT:-}" ]; then
    dsh=$(cd "$DSH_CHECKOUT" && pwd)
    [ -f "$dsh/node_modules/tsx/dist/esm/index.mjs" ] || { skip dsh "DSH_CHECKOUT is not built (no node_modules/tsx)"; return; }
  else
    bin=${DSH_BIN:-$(command -v dsh || true)}
    [ -n "$bin" ] || { skip dsh "no dsh on PATH or in DSH_BIN, and no DSH_CHECKOUT"; return; }
    dsh_js=$(real "$bin")
  fi
  setup dsh
  ln -s "$pnpm_bin" "$dir/bin/pnpm"
  if [ -n "${DSH_CHECKOUT:-}" ]; then
    tsx="$dsh/node_modules/tsx/dist/esm/index.mjs"
    dshcli() { TSX_TSCONFIG_PATH="$dsh/tsconfig.json" node --import "file://$tsx" "$dsh/apps/cli/src/bin.ts" "$@"; }
  else
    dshcli() { node "$dsh_js" "$@"; }
  fi
  cat >"$dir/stub-model.patch.yml" <<EOF
- id: llm-pi-ai
  config:
    providers:
      stub:
        api: openai-completions
        baseURL: http://127.0.0.1:$port/v1
        apiKeyEnv: STUB_KEY
        models:
          - id: stub-model
- id: agent-default-model
  config:
    provider: stub
    model: stub-model
- id: hippo
  config:
    command: hippo
    timeoutMs: 10000
    tools: false
    guard: true
EOF
  export STUB_KEY=stub
  dshcli --profile e2e --from-default-profile headless --dump-config >/dev/null
  # pnpm may look up the registry while adding the plugin: that is the
  # install, not the agent, so it runs without the proxy.
  ( unset HTTP_PROXY HTTPS_PROXY ALL_PROXY http_proxy https_proxy all_proxy
    dshcli plugin --profile e2e add "$repo/plugins/dsh" ) >"$dir/plugin-add.log" 2>&1 \
    || { cat "$dir/plugin-add.log" >&2; skip dsh "dsh plugin add failed"; kill "$stub"; return; }
  cd "$dir/ws"
  run_watched 180 dshcli --profile e2e --patch "$dir/stub-model.patch.yml" "Set up the project."
  cd "$scratch"
  finish dsh
}

for agent in $agents; do
  case $agent in
    # Each agent in a subshell, so its environment ends with it.
    claude | codex | gemini | opencode | dsh) ( "run_$agent" ) || echo "failed $agent" >>"$scratch/results" ;;
    *) echo "agent-e2e: unknown agent $agent" >&2; exit 2 ;;
  esac
done

list() { sed -n "s/^$1 //p" "$scratch/results" | paste -sd' ' -; }
echo "agent-e2e: passed: $(list passed); skipped: $(list skipped); failed: $(list failed)"
echo "agent-e2e: scratch $scratch"
! grep -q '^failed' "$scratch/results"
