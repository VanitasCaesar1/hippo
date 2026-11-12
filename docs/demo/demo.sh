#!/bin/sh
# A two-to-three-minute demo of hippocamp on synthetic data. See demo.md.
#
#   HIPPOCAMP_BIN    the hippocamp binary (default: hippocamp on PATH)
#   DEMO_PAUSE   whole seconds, the unit of the waits: 3 after each comment line,
#                7 after each output (default 1, about two minutes in all; 0 for none)
#
# Works in /tmp/hippocamp-demo, removed at the start and at the end. Offline, no
# account, no real model: dreaming and the pack review use the stub model.
set -eu

HIPPOCAMP_BIN=${HIPPOCAMP_BIN:-hippocamp}
DEMO_PAUSE=${DEMO_PAUSE:-1}
case $HIPPOCAMP_BIN in */*) HIPPOCAMP_BIN=$(cd "$(dirname "$HIPPOCAMP_BIN")" && pwd)/$(basename "$HIPPOCAMP_BIN") ;; esac
if ! command -v "$HIPPOCAMP_BIN" >/dev/null 2>&1; then
  echo "demo.sh: no hippocamp binary: set HIPPOCAMP_BIN" >&2
  exit 2
fi
REPO=$(cd "$(dirname "$0")/../.." && pwd)

D=/tmp/hippocamp-demo
rm -rf "$D"
trap 'rm -rf "$D"' EXIT

# Everything the demo needs is copied here, so no other path shows.
mkdir -p "$D/home" "$D/work"
cp -R "$REPO/fixtures/eval/notes" "$D/work/notes"
cp -R "$REPO/fixtures/eval/sessions" "$D/sessions"
cp -R "$REPO/fixtures/eval/transcripts" "$D/transcripts"
cp "$REPO/fixtures/stub-model/model.sh" "$D/model.sh"
cp -R "$REPO/packs/glossary-terms" "$D/glossary-terms"
chmod +x "$D/model.sh"

# Dreaming reads the window since the last run, so move the synthetic
# sessions to yesterday and open the window at midnight before them.
YESTERDAY=$(date -u -v-1d +%Y-%m-%d 2>/dev/null || date -u -d yesterday +%Y-%m-%d)
for f in "$D"/transcripts/*/*.jsonl "$D"/sessions/*.json; do
  sed "s/2026-11-0[0-9]/$YESTERDAY/g" "$f" >"$f.new" && mv "$f.new" "$f"
done

export HOME="$D/home"
export HIPPOCAMP_HOME="$D/home/.hippocamp-home"
export HIPPOCAMP_STUB_SCENARIO=eval
export HIPPOCAMP_STUB_VERDICT=confirm
export RUST_LOG=
cd "$D/work"

# DEMO_PAUSE is the unit: 3 units after a comment, 7 after an output.
wait_units() { [ "$DEMO_PAUSE" = 0 ] || sleep $(($1 * DEMO_PAUSE)); }
pause() { wait_units 7; }
say() { printf '\n# %s\n' "$1"; wait_units 3; }

# run [-n LINES] ARGS...: echo the command, run hippocamp, show the first LINES
# lines of output, and stop the demo if hippocamp fails.
run() {
  lines=1000
  if [ "$1" = -n ]; then lines=$2; shift 2; fi
  printf '$ hippocamp'
  for a in "$@"; do
    case $a in *' '*) printf ' "%s"' "$a" ;; *) printf ' %s' "$a" ;; esac
  done
  printf '\n'
  if "$HIPPOCAMP_BIN" "$@" >"$D/out" 2>&1; then rc=0; else rc=$?; fi
  head -n "$lines" "$D/out"
  if [ "$(wc -l <"$D/out")" -gt "$lines" ]; then echo "..."; fi
  if [ "$rc" -ne 0 ]; then
    echo "demo.sh: hippocamp $* exited $rc" >&2
    exit 1
  fi
  pause
}

say "A workspace with 16 notes about an imaginary photo library, and eight agent sessions."
run init --workspace "$D/work" --notes-dir notes --entry-note "notes/START HERE.md"
cat >"$HIPPOCAMP_HOME/config.toml" <<CFG
workspace = "$D/work"
transcript_roots = ["$D/transcripts"]
model_command = ["$D/model.sh", "--system-prompt", "{system_prompt}"]
reviewer_command_override = ["env", "HIPPOCAMP_STUB_REVIEW=1", "HIPPOCAMP_STUB_VERDICT=confirm", "$D/model.sh", "--system-prompt", "{system_prompt}"]
CFG
mkdir -p "$D/work/.hippocamp/sessions"
for s in 1 2 3 4 5 6 7 8; do
  cp "$D/sessions/eval-0$s.json" "$D/work/.hippocamp/sessions/eval-0$s.json"
done

say "Ingest: every turn of every session becomes searchable. Secrets are redacted on the way in."
run ingest

say "Recall: which session settled how the CI cache is keyed?"
run -n 12 recall "keyed on the hash of package-lock.json"

say "Context: the block an agent gets at session start, under a hard byte budget."
run -n 14 context --task "how long do we keep the nightly backups and how do i restore one"

say "Packs are checked, reviewed in a fresh context, then copied into the workspace."
run packs check "$D/glossary-terms"
run add "$D/glossary-terms"
run packs list

say "Dreaming: the note says backups are kept 14 days. A later session says 30."
printf '$ grep "kept for" notes/Backups.md\n'
grep "kept for" notes/Backups.md
pause
printf '{"protocol":"HIPPOCAMP_V1","last_success":"%sT00:00:00Z"}\n' "$YESTERDAY" >.hippocamp/dream-state.json
run -n 40 dream

say "The change has a receipt. The note now:"
printf '$ grep "kept for" notes/Backups.md\n'
grep "kept for" notes/Backups.md
pause
RECEIPT=$(grep -l '"verdict": "confirm"' .hippocamp/receipts/*.json | head -n 1 | sed 's#.*/##; s#\.json$##')
say "Revert it from the snapshot:"
run revert "$RECEIPT"
printf '$ grep "kept for" notes/Backups.md\n'
grep "kept for" notes/Backups.md
pause

say "Doctor: the setup checks in one table. No schedule is installed in the demo, so launchd is flagged."
# No schedule is installed in the demo, so the launchd row reports a problem
# and doctor exits 1. That row alone is expected; any other problem fails.
printf '$ hippocamp doctor\n'
if "$HIPPOCAMP_BIN" doctor >"$D/out" 2>&1; then rc=0; else rc=$?; fi
cat "$D/out"
if [ "$rc" -gt 1 ] || grep '| problem |' "$D/out" | grep -v '^launchd |' | grep -q .; then
  echo "demo.sh: hippocamp doctor reported a problem besides launchd" >&2
  exit 1
fi
pause

echo
echo "# Done. Nothing left this machine."
