#!/bin/sh
# The first two minutes, end to end, on synthetic data only. It builds a scratch home with synthetic agent
# transcripts, runs the path a new user takes, and fails on any dead end:
# a command that exits 2 with a bare list, a bare file error before setup,
# a doctor that fails a healthy new setup, a model error with no way out,
# an empty result that says nothing, a session prefix one command refuses.
#
#   scripts/first-run-check.sh [path/to/hippo]
#   CAPTURE_DIR=<dir> scripts/first-run-check.sh   # also keep each screen
#
# Every run uses a scratch HOME and HIPPO_HOME under a temporary directory
# and a PATH that holds only the hippo under test and the system tools, so
# no model CLI is found and nothing is sent anywhere. Nothing outside the
# temporary directory is read or written (it never runs `hippo schedule`
# or `hippo hooks`, which would touch launchd or an agent's settings).
#
# The synthetic history comes from `app/scripts/demo-home.sh` on macOS
# (sessions from today, sub-agents, corrections, several profiles), and
# from `fixtures/` elsewhere.

set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
bin=${1:-$root/target/debug/hippo}
case $bin in /*) ;; *) bin=$(pwd)/$bin ;; esac
[ -x "$bin" ] || {
  echo "first-run-check: no hippo binary at $bin (cargo build -p hippo, or pass its path)" >&2
  exit 2
}

tmp=$(mktemp -d "${TMPDIR:-/tmp}/hippo-first-run.XXXXXX")
trap 'rm -rf "$tmp"' EXIT INT TERM
mkdir -p "$tmp/bin"
ln -s "$bin" "$tmp/bin/hippo"
captures=${CAPTURE_DIR:-}
[ -z "$captures" ] || mkdir -p "$captures"

failures=0
fail() {
  echo "FAIL: $*" >&2
  failures=$((failures + 1))
}

# run <home> <name> <expected exit> <args…>: runs hippo, keeps its output
# in $out, and checks the exit code.
run() {
  home=$1 name=$2 want=$3
  shift 3
  set +e
  out=$(cd "$home" && env -i HOME="$home" HIPPO_HOME="$home/.hippo" \
    PATH="$tmp/bin:/usr/bin:/bin" LANG=C.UTF-8 hippo "$@" </dev/null 2>&1)
  code=$?
  set -e
  if [ -n "$captures" ]; then
    printf '$ hippo %s\n%s\n[exit %s]\n' "$*" "$out" "$code" >"$captures/$name.txt"
  fi
  [ "$code" = "$want" ] || fail "hippo $* exited $code, not $want:
$out"
}

# has <text>…: every text is in $out.
has() {
  for text in "$@"; do
    case $out in *"$text"*) ;; *) fail "missing \"$text\" in:
$out" ;; esac
  done
}

# lacks <text>: the text is not in $out.
lacks() {
  case $out in *"$1"*) fail "unexpected \"$1\" in:
$out" ;; esac
}

# ---------------------------------------------------------------------------
# A home with no agent history.

empty=$tmp/empty
mkdir -p "$empty"
run "$empty" 01-no-history 0
has "No agent history found" "--add-root"

# ---------------------------------------------------------------------------
# A home with synthetic history.

home=$tmp/home
mkdir -p "$home/.claude/projects/-work-tern" "$home/.codex/sessions"
if [ "$(uname)" = Darwin ]; then
  demo=$tmp/demo
  mkdir -p "$demo" "$tmp/demo-user"
  HOME=$tmp/demo-user HIPPO_BIN=$bin sh "$root/app/scripts/demo-home.sh" "$demo" >/dev/null
  cp -R "$demo/profiles/.claude/." "$home/.claude/"
  cp -R "$demo/profiles/.claude-work" "$home/.claude-work"
  cp -R "$demo/transcripts/claude-code/." "$home/.claude/projects/-work-tern/"
  mkdir -p "$home/.claude/projects/-work-demo"
  cp -R "$demo/transcripts/demo-proj/." "$home/.claude/projects/-work-demo/"
  cp -R "$demo/transcripts/codex/." "$home/.codex/sessions/"
else
  cp "$root"/fixtures/eval/transcripts/claude-code/*.jsonl "$home/.claude/projects/-work-tern/"
  cp "$root"/fixtures/eval/transcripts/codex/*.jsonl "$home/.codex/sessions/"
fi

# DeepSeek Harness (dsh) logs: one project of the synthetic fixtures, a
# compressed fork among them.
mkdir -p "$home/.dsh/sessions"
cp -R "$root/fixtures/dsh/--home-synthetic-shop--" "$home/.dsh/sessions/"

# Before setup: one command, and every other command says how to start.
run "$home" 02-hippo-before-setup 0
has "Looking for agent history" "nothing is sent anywhere" "hippo start --yes"
has "~/.dsh/sessions 5 files"
[ ! -e "$home/.hippo/config.toml" ] || fail "hippo alone wrote config.toml without a yes"

run "$home" 03-help 0 --help
has "Start here:" "Every day:" "Run by hippo itself"

for cmd in recall ingest fleet incidents doctor; do
  case $cmd in
    recall) args="recall retry" ;;
    *) args=$cmd ;;
  esac
  # shellcheck disable=SC2086
  run "$home" "04-before-setup-$cmd" 1 $args
  has "hippo is not set up yet" "run hippo"
done

# The one-command start, timed.
started=$(date +%s)
run "$home" 05-start 0 start --yes
elapsed=$(($(date +%s) - started))
has "wrote " "Try next" "hippo ask --sources-only" "Every command: hippo --help"
lacks "nverified"
[ "$elapsed" -le 10 ] || fail "hippo start --yes took ${elapsed}s, over 10"
echo "hippo start --yes: ${elapsed}s"

# Set up: hippo alone is the overview.
run "$home" 06-hippo-after-setup 0
has "Busiest sessions" "Try next"
lacks "nverified"
id=$(printf '%s\n' "$out" | sed -n 's/^  hippo xray \([^ ]*\) .*/\1/p' | head -n 1)
[ -n "$id" ] || fail "the overview names no session to x-ray"

# A healthy new setup passes doctor and sends nothing.
run "$home" 07-doctor 0 doctor
lacks "| problem |"
has "dsh logs | ok"

# The card: numbers and a date, a PNG.
run "$home" 07-card 0 card --out "$tmp/card.png"
has "My agents did " "of work" "hippo · one brain for all your coding agents" "(1200 x 675)"
lacks "tern"
lacks "Tern"
[ "$(head -c 8 "$tmp/card.png" | od -An -tx1 | tr -d ' \n')" = 89504e470d0a1a0a ] || fail "hippo card wrote no PNG"
[ "$(head -c 24 "$tmp/card.png" | tail -c 8 | od -An -tx1 | tr -d ' \n')" = 000004b0000002a3 ] || fail "the card is not 1200 x 675"

# The same session prefix works everywhere.
for cmd in "show session" xray resume; do
  # shellcheck disable=SC2086
  run "$home" "08-prefix-$(echo "$cmd" | tr ' ' -)" 0 $cmd "$id"
done
run "$home" 08-prefix-unknown 1 show session zzzz-no-such
has "no indexed session zzzz-no-such"

# Empty results name the next step.
run "$home" 09-recall-empty 0 recall "zebra quantum harmonica"
has "no turn matches" "hippo ask --sources-only"

# No model CLI: the sources still print, and the error names the way out.
run "$home" 10-ask-no-model 0 ask "export queue retry"
has "is not on PATH" "install Claude Code" "hippo config set model_command"
run "$home" 11-config-model-command 0 config set model_command "mymodel --json"
has "model_command = [\"mymodel\", \"--json\"]"

# A notes folder that does not exist yet.
ws=$home/tern
mkdir -p "$ws"
run "$home" 12-init-notes 0 init --workspace "$ws" --notes-dir notes --entry-note "notes/START HERE.md"
has "(it did not exist)" "a starter entry note" "hippo ingest --full"
[ -f "$ws/notes/START HERE.md" ] || fail "init wrote no starter entry note"
run "$home" 13-doctor-with-notes 0 doctor
lacks "| problem |"
# Sessions of other folders get no record and that is no gap.
has "session records | ok"
run "$home" 14-ingest-full 0 ingest --full
run "$home" 15-context 0 context
has "Start here"

# The first dream on these notes: nothing to work on, and it says why.
run "$home" 16-dream-dry-run 0 dream --dry-run
has "nothing proposed"

if [ "$failures" -gt 0 ]; then
  echo "first-run-check: $failures failed" >&2
  exit 1
fi
echo "first-run-check: ok"
