#!/bin/sh
# The no-terminal first run, end to end, on a machine with nothing: the
# install script downloads hippo from a release served on 127.0.0.1, then
# `hippo setup --yes` runs for each agent the way the plugins run it, then
# the real agent CLI starts a session against a stub model and the check
# reads what the model was sent. "Start a new session and it's ready" holds
# when the first request carries the entry note.
#
#   scripts/setup-e2e/run.sh <hippo binary> [codex] [claude] [app]
#   HIPPO_APP_DIST=<folder> …  app   # a folder holding Hippo-<version>-macos.tar.gz
#                                    # and its .sha256 (app/scripts/release-app.sh)
#
# `app` installs with `install.sh --app` instead, checks the bundle carries
# no quarantine attribute, then opens it in the background on the scratch
# home (open -g -n, nothing comes to the front), captures its window by id
# into the scratch folder and quits it by its PID.
#
# No model, no key and no account are used. Everything is under a new
# temporary folder, which is HOME, HIPPO_HOME, CODEX_HOME,
# CLAUDE_CONFIG_DIR and the XDG folders; it is printed at the end. PATH
# holds only the scratch ~/.local/bin, the agent, node and /usr/bin:/bin.
# Codex runs with --dangerously-bypass-hook-trust, standing in for the
# person trusting hippo's hooks in Codex's review. Claude Code loads this
# checkout's plugin with --plugin-dir.
set -eu

[ $# -ge 1 ] || { echo "usage: $0 <hippo binary> [codex] [claude] [app]" >&2; exit 2; }
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
hippo_bin=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
shift
agents=${*:-codex claude}
[ -x "$hippo_bin" ] || { echo "not an executable: $hippo_bin" >&2; exit 2; }
codex_bin=${CODEX_BIN:-$(command -v codex || true)}
claude_bin=${CLAUDE_BIN:-$(command -v claude || true)}
node_dir=$(dirname "$(command -v node)")

scratch=$(mktemp -d "${TMPDIR:-/tmp}/hippo-setup-e2e.XXXXXX")
scratch=$(cd "$scratch" && pwd -P)
fail() { echo "setup-e2e: FAIL: $*" >&2; exit 1; }

export HOME="$scratch/home" HIPPO_HOME="$scratch/home/.hippo" CODEX_HOME="$scratch/home/.codex" \
  CLAUDE_CONFIG_DIR="$scratch/home/.claude" TZ=UTC
export XDG_DATA_HOME="$scratch/data" XDG_CONFIG_HOME="$scratch/config" \
  XDG_CACHE_HOME="$scratch/cache" XDG_STATE_HOME="$scratch/state"
unset OPENAI_API_KEY OPENAI_BASE_URL CODEX_API_KEY ANTHROPIC_API_KEY ANTHROPIC_BASE_URL
# Anything an agent sends past the stubs goes to a closed port.
export HTTPS_PROXY=http://127.0.0.1:9 HTTP_PROXY=http://127.0.0.1:9 ALL_PROXY=http://127.0.0.1:9 NO_PROXY=127.0.0.1,localhost
mkdir -p "$HOME/.local/bin" "$CODEX_HOME" "$CLAUDE_CONFIG_DIR"
export PATH="$HOME/.local/bin:$node_dir:/usr/bin:/bin"

# The release: one tarball for this machine and its checksum, as
# scripts/release.sh lays them out.
version=$("$hippo_bin" --version | awk '{print $2}')
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) target=aarch64-apple-darwin ;;
  Darwin-x86_64) target=x86_64-apple-darwin ;;
  Linux-x86_64) target=x86_64-unknown-linux-musl ;;
  Linux-aarch64) target=aarch64-unknown-linux-musl ;;
  *) fail "no target for $(uname -s) $(uname -m)" ;;
esac
dist="$scratch/dist"
mkdir -p "$dist/hippo-$version-$target"
cp "$hippo_bin" "$dist/hippo-$version-$target/hippo"
cp "$root/LICENSE" "$root/README.md" "$dist/hippo-$version-$target/"
(cd "$dist" && tar -czf "hippo-$version-$target.tar.gz" "hippo-$version-$target" && rm -r "hippo-$version-$target")
(cd "$dist" && shasum -a 256 "hippo-$version-$target.tar.gz" >"hippo-$version-$target.tar.gz.sha256")
case " $agents " in
  *" app "*)
    [ -n "${HIPPO_APP_DIST:-}" ] || fail "app needs HIPPO_APP_DIST"
    cp "$HIPPO_APP_DIST/Hippo-$version-macos.tar.gz" "$HIPPO_APP_DIST/Hippo-$version-macos.tar.gz.sha256" "$dist/"
    ;;
esac
SERVE_LOG="$scratch/serve.log" node "$here/serve.mjs" "$dist" >"$scratch/serve.port" &
serve=$!
pids=$serve
trap 'kill $pids 2>/dev/null || true' EXIT INT TERM
for _ in 1 2 3 4 5 6 7 8 9 10; do [ -s "$scratch/serve.port" ] && break; sleep 0.2; done
base="http://127.0.0.1:$(cat "$scratch/serve.port")"

# 1. Nothing installed; the install script, as the plugins run it.
[ -z "$(command -v hippo || true)" ] || fail "a hippo is on PATH before the install"
flags=
case " $agents " in *" app "*) flags=--app ;; esac
# shellcheck disable=SC2086
HIPPO_BASE_URL=$base HIPPO_VERSION=$version sh "$root/install.sh" $flags >"$scratch/install.txt" 2>&1 \
  || { cat "$scratch/install.txt" >&2; fail "install.sh"; }
[ "$(command -v hippo)" = "$HOME/.local/bin/hippo" ] || fail "install.sh put no hippo in ~/.local/bin"
grep -q "Checked hippo-$version-$target.tar.gz against its published sha256" "$scratch/install.txt" || fail "install.sh did not check the checksum"
grep -q "\"sha256\": \"$(shasum -a 256 "$HOME/.local/bin/hippo" | cut -d ' ' -f 1)\"" "$HIPPO_HOME/install.json" \
  || fail "install.json does not record the installed binary's checksum"
echo "setup-e2e: install.sh installed hippo $version from $base and recorded its sha256"

# The project: Git, and a notes folder with an entry note to look for.
ws="$scratch/ws"
mkdir -p "$ws/notes" "$ws/src"
git -C "$ws" init -q
printf '# Start here\n\nE2E-SETUP-START: tern builds with pnpm and ships on Fridays.\n' >"$ws/notes/START HERE.md"

# first_request <log> <api>: the first logged request body of that API.
first_request() {
  node -e '
    const fs = require("fs")
    const [file, api] = process.argv.slice(1)
    const rows = fs.readFileSync(file, "utf8").trim().split("\n").filter(Boolean).map(JSON.parse)
    // Claude Code may send a short request of its own first; the session is the one with tools.
    const row = rows.find(r => api === "responses" ? true : r.api === api && r.tools > 0)
    if (!row) process.exit(1)
    console.log(JSON.stringify(api === "responses" ? row : row.body))
  ' "$1" "$2"
}

run_codex() {
  [ -n "$codex_bin" ] || { echo "setup-e2e: skip codex (no codex on PATH or in CODEX_BIN)"; return; }
  (cd "$ws/src" && hippo setup --agent codex --yes) >"$scratch/setup-codex.txt" 2>&1 \
    || { cat "$scratch/setup-codex.txt" >&2; fail "hippo setup --agent codex --yes"; }
  grep -q "Ready. Start a new Codex session" "$scratch/setup-codex.txt" || fail "setup did not say ready"
  [ ! -d "$HOME/Library/LaunchAgents" ] || fail "setup scheduled something"
  FAKE_LLM_LOG="$scratch/codex-requests.jsonl" node "$root/scripts/codex-e2e/fake-responses.mjs" >"$scratch/codex.port" &
  pids="$pids $!"
  for _ in 1 2 3 4 5 6 7 8 9 10; do [ -s "$scratch/codex.port" ] && break; sleep 0.2; done
  cat >"$CODEX_HOME/config.toml" <<TOML
model = "fake-model"
model_provider = "fake"
check_for_update_on_startup = false

[model_providers.fake]
name = "Fake"
base_url = "http://127.0.0.1:$(cat "$scratch/codex.port")/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
TOML
  (cd "$ws/src" && "$codex_bin" exec --skip-git-repo-check --dangerously-bypass-hook-trust --json \
    "What does tern build with?" </dev/null) >"$scratch/codex-run.jsonl" 2>"$scratch/codex-run.err" \
    || { cat "$scratch/codex-run.err" >&2; fail "codex exec"; }
  first_request "$scratch/codex-requests.jsonl" responses >"$scratch/codex-first.json" || fail "the stub got no Codex request"
  grep -q "E2E-SETUP-START" "$scratch/codex-first.json" || fail "Codex's first request does not carry the entry note"
  echo "setup-e2e: codex: after hippo setup, a new session's first request carried the entry note"
}

run_claude() {
  [ -n "$claude_bin" ] || { echo "setup-e2e: skip claude (no claude on PATH or in CLAUDE_BIN)"; return; }
  (cd "$ws/src" && hippo setup --yes) >"$scratch/setup-claude.txt" 2>&1 \
    || { cat "$scratch/setup-claude.txt" >&2; fail "hippo setup --yes"; }
  grep -q "Ready. Start a new Claude Code session" "$scratch/setup-claude.txt" || fail "setup did not say ready"
  STUB_SCRIPT=none STUB_LOG="$scratch/claude-requests.jsonl" node "$root/scripts/agent-e2e/stub-model.mjs" >"$scratch/claude.port" &
  pids="$pids $!"
  for _ in 1 2 3 4 5 6 7 8 9 10; do [ -s "$scratch/claude.port" ] && break; sleep 0.2; done
  (cd "$ws/src" && ANTHROPIC_BASE_URL="http://127.0.0.1:$(cat "$scratch/claude.port")" ANTHROPIC_API_KEY=stub \
    CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 DISABLE_AUTOUPDATER=1 DISABLE_TELEMETRY=1 DISABLE_ERROR_REPORTING=1 \
    "$claude_bin" -p "What does tern build with?" --plugin-dir "$root/plugin" --max-turns 1 </dev/null) \
    >"$scratch/claude-run.txt" 2>&1 || { cat "$scratch/claude-run.txt" >&2; fail "claude -p"; }
  first_request "$scratch/claude-requests.jsonl" anthropic >"$scratch/claude-first.json" || fail "the stub got no Claude Code request"
  grep -q "E2E-SETUP-START" "$scratch/claude-first.json" || fail "Claude Code's first request does not carry the entry note"
  echo "setup-e2e: claude: after hippo setup, a new session's first request carried the entry note"
}

run_app() {
  bundle="$HOME/Applications/Hippo.app"
  [ -x "$bundle/Contents/MacOS/hippo-app" ] || { cat "$scratch/install.txt" >&2; fail "install.sh --app put no Hippo.app in ~/Applications"; }
  grep -q "Checked Hippo-$version-macos.tar.gz against its published sha256" "$scratch/install.txt" || fail "the app's checksum was not checked"
  if /usr/bin/xattr -p com.apple.quarantine "$bundle" >/dev/null 2>&1; then fail "the bundle carries a quarantine attribute"; fi
  hippo app status >"$scratch/app-status.txt" || { cat "$scratch/app-status.txt" >&2; fail "hippo app status"; }
  codesign --verify --strict "$bundle" || fail "the bundle's ad-hoc signature does not verify"
  # Open it in the background on the scratch home, find its window by PID,
  # capture it, quit it.
  before=$(pgrep -f "$bundle/Contents/MacOS/hippo-app" || true)
  [ -z "$before" ] || fail "a Hippo from the scratch folder is already running"
  open -g -n "$bundle" --env HOME="$HOME" --env HIPPO_HOME="$HIPPO_HOME" --env PATH="$HOME/.local/bin:/usr/bin:/bin" \
    --env TZ=UTC --env HIPPO_APP_DEV_SIZE=1200x800 --env HIPPO_APP_DEV_VIEW=morning --env HIPPO_APP_PREFS="$scratch/app-prefs.json"
  pid=
  for _ in $(seq 1 40); do pid=$(pgrep -f "$bundle/Contents/MacOS/hippo-app" | head -n 1 || true); [ -n "$pid" ] && break; sleep 0.25; done
  [ -n "$pid" ] || fail "Hippo.app did not start"
  pids="$pids $pid"
  wid=
  for _ in $(seq 1 40); do
    wid=$(swift -e 'import CoreGraphics; let p = Int32(CommandLine.arguments[1])!; for w in (CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as! [[String: Any]]) where (w[kCGWindowOwnerPID as String] as? Int32) == p && (w[kCGWindowLayer as String] as? Int) == 0 { if let b = w[kCGWindowBounds as String] as? [String: Any], (b["Height"] as? Double ?? 0) > 100 { print(w[kCGWindowNumber as String]!); break } }' "$pid" 2>/dev/null || true)
    [ -n "$wid" ] && break
    sleep 0.25
  done
  sleep 3
  if [ -n "$wid" ]; then /usr/sbin/screencapture -o -x -l "$wid" "$scratch/app.png"; fi
  kill "$pid"
  [ -n "$wid" ] || fail "Hippo.app opened no window"
  echo "setup-e2e: app: install.sh --app installed Hippo.app $version (checksum checked, no quarantine, ad-hoc signature verifies); it opened a window (captured to $scratch/app.png)"
}

for agent in $agents; do
  case $agent in
    codex) run_codex ;;
    claude) run_claude ;;
    app) run_app ;;
    *) fail "unknown agent $agent" ;;
  esac
done
outside=$(grep -v '^GET /hippo-\|^GET /Hippo-' "$scratch/serve.log" || true)
[ -z "$outside" ] || fail "unexpected requests to the release server: $outside"
echo "scratch: $scratch"
