#!/bin/sh
# A synthetic model for crates/hippocamp/tests/dream_stub_model.rs, standing in
# for model_command. It reads the prompt on stdin, ignores its arguments,
# and prints a canned reply the way `claude -p --output-format json` does:
# a JSON object whose `result` is the reply text.
#
#   HIPPOCAMP_STUB_REVIEW set   a reviewer verdict, HIPPOCAMP_STUB_VERDICT (confirm
#                           or reject)
#   otherwise               the proposal of HIPPOCAMP_STUB_SCENARIO: replace,
#                           outside, dup, unparsable, eval or demo
#
# Every proposal but `eval` targets notes/Topic A.md and quotes, as `new`,
# the text of turn stub-1 1 of fixtures/claude-code/synthetic.jsonl.
#
# The `eval` scenario is for `hippocamp eval --with-model`. It reads the prompt:
# when the prompt lists notes/Backups.md it replies with one correction of
# that note's planted error, quoting turn 7 of session eval-01 in
# fixtures/eval/transcripts; for any other note it replies `[]`.
#
# The `demo` scenario is for app/scripts/demo-home.sh. It reads the prompt
# and proposes one change per eval note, each quoting its session's turn;
# Sharing links' change quotes nothing, so it fails validation. As the
# reviewer it rejects the Auth sessions and Search index changes and
# confirms the rest, each with notes a person can read.
reply() {
  printf '{"result":"%s"}\n' "$(printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g')"
}
change() {
  # note kind old new session ordinal reason
  printf '[{"note":"notes/%s.md","kind":"%s","old":"%s","new":"%s","evidence":[{"session":"%s","ordinal":%s}],"reason":"%s"}]' "$@"
}
if [ "$HIPPOCAMP_STUB_SCENARIO" = demo ]; then
  prompt=$(cat)
  if [ -n "$HIPPOCAMP_STUB_REVIEW" ]; then
    case "$prompt" in
      *'24 hours'*)
        reply '{"verdict":"reject","notes":"The turn is the assistant saying it changed a config value mid-session; nothing in the evidence shows the 24 hour lifetime was deployed, and the note describes production. Keep 7 days until a release confirms it."}' ;;
      *'six hours behind'*)
        reply '{"verdict":"reject","notes":"This is one afternoon'"'"'s incident, not a lasting fact about the search index; it would be wrong tomorrow."}' ;;
      *)
        reply '{"verdict":"confirm","notes":"The cited turn reports the change as made in this session, and the note still states the old value."}' ;;
    esac
    exit 0
  fi
  case "$prompt" in
    *'{"note":"notes/Backups.md"'*)
      reply "$(change Backups replace 'Nightly backups are kept for 14 days.' 'Backups are now kept for 30 days after the retention change.' eval-01 7 'eval-01 raised retention from 14 to 30 days after the storage grant; the note still says 14.')" ;;
    *'{"note":"notes/Thumbnail pipeline.md"'*)
      reply "$(change 'Thumbnail pipeline' replace '- Thumbnails are generated at 256 and 1024 pixels on the long edge.' '- The thumbnail pipeline now writes 320 and 1280 pixels on the long edge.' eval-02 6 'eval-02 changed the sizes to 320 and 1280 for denser screens.')" ;;
    *'{"note":"notes/Upload API.md"'*)
      reply "$(change 'Upload API' replace '- The upload limit is 50 MB per file.' '- The upload API limit is 200 MB per file now.' eval-03 7 'eval-03 raised the per-file limit to 200 MB with ops agreeing.')" ;;
    *'{"note":"notes/Database migrations.md"'*)
      reply "$(change 'Database migrations' replace '- Migrations run automatically on deploy.' '- Schema changes no longer run on deploy, so run them by hand before the release.' eval-04 8 'eval-04 removed the migration step from deploy after a hung deploy.')" ;;
    *'{"note":"notes/Auth sessions.md"'*)
      reply "$(change 'Auth sessions' replace '- Login sessions last 7 days.' '- The session lifetime is now 24 hours after the security review.' eval-05 7 'eval-05 shortened sessions to one day after the security review.')" ;;
    *'{"note":"notes/Release process.md"'*)
      reply "$(change 'Release process' replace '- Releases are cut every second Tuesday.' '- The release schedule is now every Thursday.' eval-06 6 'eval-06 moved releases to every Thursday.')" ;;
    *'{"note":"notes/Sharing links.md"'*)
      reply "$(change 'Sharing links' replace '- Share links expire after 30 days by default.' '- Share links now expire after 7 days.' eval-07 3 'A forum request asked for 7 days.')" ;;
    *'{"note":"notes/Search index.md"'*)
      reply "$(change 'Search index' append '' '\n- The index is six hours behind. A full rebuild should fix it.\n' eval-08 3 'eval-08 found the index lagging.')" ;;
    *) reply '[]' ;;
  esac
  exit 0
fi
# The `lessons` scenario is for app/scripts/demo-home.sh's lessons pass. As
# the drafter it reads the prompt: the correction batch gets a staging rule
# for its first item, a toolbar rule for its second (a new note) and no
# lesson for the rest; the failure batch gets a zsh glob rule and a BSD
# sed rule, by the error each item names. As the
# reviewer it confirms, with notes a person can read.
if [ "$HIPPOCAMP_STUB_SCENARIO" = lessons ]; then
  prompt=$(cat)
  if [ -n "$HIPPOCAMP_STUB_REVIEW" ]; then
    reply '{"verdict":"confirm","notes":"The person corrected the same thing in more than one session, right after the agent did it, and the rule says what to do instead."}'
    exit 0
  fi
  case "$prompt" in
    *'A developer asks whether any of their coding agents'*)
      reply 'Yes. In two sessions the person told the agent to run previews against the staging database, not production [1] [2].' ;;
    *'"body"'*)
      reply '{"title":"Previews use the staging database","body":"Agents run previews of the Tern web app against the staging database. Production was corrected twice."}' ;;
    *'A developer asked their coding agents'*)
      reply '{"title":"Previews against staging only","rule":"Run previews of the Tern web app against the staging database, never production.","note":"notes/START HERE.md","section":"Reading order"}' ;;
    *'A coding agent searched a developer'*)
      reply '{"title":"The release signing key is in On-call","rule":"The release signing key and how to use it are in the On-call note, under Keys; read it before searching the notes for the key.","note":"notes/On-call.md","section":null}' ;;
    *'"kind":"failure"'*)
      # One rule per failure cluster, by the error its line names; both go
      # to the same section, so the app groups them.
      items=''
      lines=$(printf '%s\n' "$prompt" | grep '"kind":"failure"')
      while IFS= read -r line; do
        n=$(printf '%s' "$line" | grep -o '"n":[0-9]*' | head -n 1 | cut -d: -f2)
        [ -n "$n" ] || continue
        case "$line" in
          *'invalid command code'*)
            item='{"n":'$n',"lesson":true,"title":"Give BSD sed -i an empty suffix","rule":"On macOS, sed -i needs a suffix argument: write sed -i '"'"''"'"' s/a/b/ file, or the next word is taken as the suffix.","note":"notes/START HERE.md","section":"Also read"}' ;;
          *)
            item='{"n":'$n',"lesson":true,"title":"Quote glob patterns in zsh","rule":"The shell is zsh, where an unmatched glob aborts the command. Quote globs passed to tools, such as --include='"'"'*.ts'"'"'.","note":"notes/START HERE.md","section":"Also read"}' ;;
        esac
        items="${items:+$items,}$item"
      done <<EOF_LINES
$lines
EOF_LINES
      reply "[$items]" ;;
    *)
      reply '[{"n":1,"lesson":true,"title":"Use the staging database for previews, never production","rule":"Run previews and manual checks against the staging database (npm run preview -- --db staging), never production.","note":"notes/START HERE.md","section":"Reading order"},{"n":2,"lesson":true,"title":"The export button belongs in the toolbar, not the footer","rule":"In the Tern web app, the export button sits in the toolbar; never move it to the footer.","note":null,"section":null},{"n":3,"lesson":false},{"n":4,"lesson":false}]' ;;
  esac
  exit 0
fi
if [ "$HIPPOCAMP_STUB_SCENARIO" = eval ] && [ -z "$HIPPOCAMP_STUB_REVIEW" ]; then
  prompt=$(cat)
  case "$prompt" in
    *'{"note":"notes/Backups.md"'*)
      printf '{"result":"[{\\"note\\":\\"notes/Backups.md\\",\\"kind\\":\\"replace\\",\\"old\\":\\"Nightly backups are kept for 14 days.\\",\\"new\\":\\"Backups are now kept for 30 days after the retention change.\\",\\"evidence\\":[{\\"session\\":\\"eval-01\\",\\"ordinal\\":7}],\\"reason\\":\\"stub\\"}]"}\n'
      ;;
    *) printf '{"result":"[]"}\n' ;;
  esac
  exit 0
fi
cat > /dev/null
if [ -n "$HIPPOCAMP_STUB_REVIEW" ]; then
  printf '{"result":"{\\"verdict\\":\\"%s\\",\\"notes\\":\\"stub\\"}"}\n' "$HIPPOCAMP_STUB_VERDICT"
  exit 0
fi
new="I'll ask a sub-agent where the sender sleeps between retries."
case "$HIPPOCAMP_STUB_SCENARIO" in
  replace) note='notes/Topic A.md'; old='other notes link to' ;;
  outside) note='../etc/passwd'; old='root' ;;
  dup) note='notes/Topic A.md'; old='notes' ;;
  unparsable)
    printf '{"result":"not json"}\n'
    exit 0
    ;;
  *)
    echo "unknown HIPPOCAMP_STUB_SCENARIO: $HIPPOCAMP_STUB_SCENARIO" >&2
    exit 1
    ;;
esac
printf '{"result":"[{\\"note\\":\\"%s\\",\\"kind\\":\\"replace\\",\\"old\\":\\"%s\\",\\"new\\":\\"%s\\",\\"evidence\\":[{\\"session\\":\\"stub-1\\",\\"ordinal\\":1}],\\"reason\\":\\"stub\\"}]"}\n' "$note" "$old" "$new"
