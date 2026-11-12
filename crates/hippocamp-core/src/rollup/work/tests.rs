//! Tests of where a session worked and what it did to pull requests. Every
//! transcript and repository here is synthetic; the shapes copy real mess:
//! a working directory that is not where the work was, worktrees that are
//! gone, pull requests named but never opened, merges that failed.

use std::path::Path;

use super::*;

const T: &str = "2026-11-08T08:00:00Z";

fn turn(role: &str, tool: Option<&str>, text: &str) -> StoredTurn {
    StoredTurn {
        ordinal: 0,
        role: role.to_owned(),
        ts: T.to_owned(),
        tool_name: tool.map(str::to_owned),
        text: text.to_owned(),
    }
}

fn bash(command: &str) -> StoredTurn {
    let input = serde_json::json!({ "command": command, "description": "run" });
    turn("tool", Some("Bash"), &format!("Bash {input}"))
}

fn out(text: &str) -> StoredTurn {
    turn("tool", Some("Bash"), text)
}

fn edit(path: &Path) -> StoredTurn {
    let input = serde_json::json!({ "file_path": path, "old_string": "a", "new_string": "b" });
    turn("tool", Some("Edit"), &format!("Edit {input}"))
}

fn say(text: &str) -> StoredTurn {
    turn("assistant", None, text)
}

/// A checkout at `dir` whose origin is `remote`.
fn checkout(dir: &Path, remote: &str) {
    fs::create_dir_all(dir.join(".git")).unwrap();
    fs::write(
        dir.join(".git/config"),
        format!(
            "[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = https://example.org/x/y.git\n[remote \"origin\"]\n\turl = {remote}\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n"
        ),
    )
    .unwrap();
}

/// A worktree at `dir` of the checkout at `main`.
fn worktree(dir: &Path, main: &Path, name: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::create_dir_all(main.join(".git/worktrees").join(name)).unwrap();
    fs::write(
        dir.join(".git"),
        format!(
            "gitdir: {}\n",
            main.join(".git/worktrees").join(name).display()
        ),
    )
    .unwrap();
}

fn repos() -> Repos {
    Repos {
        roots: HashMap::new(),
        home: None,
    }
}

#[test]
fn commands_split_into_simple_commands() {
    let c = simple_commands(
        "cd /w/a && FOO=1 timeout 60 git -C '../b c' status; gh pr create --title \"x && y\" | tail -1\necho $(git rev-parse HEAD)",
    );
    let words: Vec<Vec<&str>> = c
        .iter()
        .map(|w| w.iter().map(String::as_str).collect())
        .collect();
    assert_eq!(
        words,
        [
            vec!["cd", "/w/a"],
            vec!["FOO=1", "timeout", "60", "git", "-C", "../b c", "status"],
            vec!["gh", "pr", "create", "--title", "x && y"],
            vec!["tail", "-1"],
            vec!["echo"],
            vec!["git", "rev-parse", "HEAD"],
        ]
    );
    assert_eq!(program(&c[1])[0], "git");
    // A here-document is text: its apostrophe does not open a quote.
    let c = simple_commands(
        "cd /w/a && git commit -q -F - <<'EOF'\nfix: the job's environment\n\nIt's \"quoted\" | piped\nEOF\ngit push -q && gh pr create --title \"t\" --body-file - <<EOF2\nbody's\nEOF2\necho done",
    );
    let firsts: Vec<&str> = c.iter().map(|w| w[0].as_str()).collect();
    assert_eq!(firsts, ["cd", "git", "git", "gh", "echo"]);
    // A command inside `$(…)`, and after a shell keyword.
    let c = simple_commands(
        "url=$(gh pr create --body \"$(cat x)\") && if gh pr checks 3 | grep -q fail; then echo red; else gh pr merge 3 --squash; fi",
    );
    let programs: Vec<&str> = c
        .iter()
        .map(|w| program(w).first().map_or("", String::as_str))
        .collect();
    assert_eq!(programs, ["", "gh", "gh", "grep", "echo", "gh", "fi"]);
}

#[test]
fn remotes_give_owner_and_name() {
    let p = |u: &str| remote_parts(u);
    let want = Some(("acme".to_owned(), "widgets".to_owned()));
    assert_eq!(p("git@github.com:acme/widgets.git"), want);
    assert_eq!(p("https://github.com/acme/widgets"), want);
    assert_eq!(p("ssh://git@github.com/acme/widgets.git"), want);
    assert_eq!(
        origin_url("[remote \"origin\"]\n\turl = git@h:o/n.git\n").as_deref(),
        Some("git@h:o/n.git")
    );
}

#[test]
fn the_project_is_where_it_worked_not_where_it_started() {
    let tmp = tempfile::tempdir().unwrap();
    let umbrella = tmp.path().join("umbrella");
    let widgets = tmp.path().join("code/widgets");
    let wt = tmp.path().join("code/widgets-wt/fix-listing");
    fs::create_dir_all(&umbrella).unwrap();
    checkout(&widgets, "git@github.com:acme/widgets.git");
    worktree(&wt, &widgets, "fix-listing");
    let turns = [
        turn("user", None, "Fix the listing page"),
        // It reads the umbrella's notes, then works in a worktree of widgets.
        bash(&format!("cd {} && git status --short", widgets.display())),
        out(&format!("Shell cwd was reset to {}", umbrella.display())),
        edit(&wt.join("src/listing.rs")),
        turn("tool", Some("Edit"), "The file has been updated."),
        edit(&wt.join("src/page.rs")),
        bash(&format!("git -C {} diff --stat", wt.display())),
        out(" 2 files changed"),
        edit(&umbrella.join("notes/log.md")),
    ];
    let w = read(&turns, Some(&umbrella), &mut repos());
    assert_eq!(w.project, Some(("widgets".to_owned(), 4)));

    // Mostly notes in the working directory, which is no repository, and
    // one fix in widgets: the working directory's.
    let notes = [
        edit(&umbrella.join("notes/a.md")),
        edit(&umbrella.join("notes/b.md")),
        edit(&wt.join("src/listing.rs")),
        edit(Path::new("/elsewhere/scratch/x.md")),
    ];
    let w = read(&notes, Some(&umbrella), &mut repos());
    assert_eq!(w.project, Some(("umbrella".to_owned(), 2)));
}

#[test]
fn a_gone_worktree_is_its_checkouts() {
    let tmp = tempfile::tempdir().unwrap();
    let widgets = tmp.path().join("widgets");
    checkout(&widgets, "https://github.com/acme/widgets.git");
    let gone = tmp.path().join("wt/gone");
    let claude_wt = widgets.join(".claude/worktrees/quick-fix");
    let turns = [
        bash(&format!(
            "cd {} && git worktree add -b fix/gone ../wt/gone origin/main",
            widgets.display()
        )),
        out("Preparing worktree"),
        edit(&gone.join("src/a.rs")),
        edit(&gone.join("src/b.rs")),
        edit(&claude_wt.join("README.md")),
    ];
    let w = read(&turns, Some(Path::new("/elsewhere/umbrella")), &mut repos());
    assert_eq!(w.project, Some(("widgets".to_owned(), 4)));
    // A Codex worktree names its repository in its path.
    let codex = [edit(Path::new(
        "/u/.codex/worktrees/a1b2/parser/src/lib.rs",
    ))];
    let w = read(&codex, None, &mut repos());
    assert_eq!(w.project, Some(("parser".to_owned(), 1)));
}

#[test]
fn with_no_work_the_project_is_the_working_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let widgets = tmp.path().join("widgets");
    checkout(&widgets, "git@github.com:acme/widgets-app.git");
    let read_only = [turn("user", None, "what does this do?"), say("It renders.")];
    let w = read(&read_only, Some(&widgets.join("src")), &mut repos());
    assert_eq!(w.project, Some(("widgets-app".to_owned(), 0)));
    let w = read(
        &read_only,
        Some(Path::new("/nowhere/umbrella")),
        &mut repos(),
    );
    assert_eq!(w.project, Some(("umbrella".to_owned(), 0)));
    assert_eq!(read(&read_only, None, &mut repos()).project, None);
}

#[test]
fn named_pull_requests_are_not_counted() {
    let tmp = tempfile::tempdir().unwrap();
    let widgets = tmp.path().join("widgets");
    checkout(&widgets, "git@github.com:acme/widgets.git");
    let cd = format!("cd {}", widgets.display());
    let turns = [
        turn("user", None, "Review https://github.com/acme/widgets/pull/40 and land the listing"),
        bash(&format!("{cd} && gh pr list --state all --limit 30")),
        out("44\tHardening\thttps://github.com/acme/widgets/pull/44\n46\tListing\thttps://github.com/acme/widgets/pull/46"),
        bash(&format!("{cd} && gh pr view 44 --json state")),
        out("{\"state\":\"MERGED\"}"),
        say("PR #46 (the listing) is open. Its CI and #44's hardening are running; see https://github.com/acme/widgets/pull/46 and https://github.com/acme/widgets/pull/44, both merged upstream."),
        turn("tool", Some("Agent"), "Agent {\"prompt\":\"merge https://github.com/acme/widgets/pull/12 when green\"}"),
    ];
    let w = read(&turns, None, &mut repos());
    assert!(w.pulls.is_empty(), "{:?}", w.pulls);
}

#[test]
fn opened_is_what_gh_pr_create_printed() {
    let tmp = tempfile::tempdir().unwrap();
    let widgets = tmp.path().join("widgets");
    checkout(&widgets, "git@github.com:acme/widgets.git");
    let cd = format!("cd {}", widgets.display());
    let turns = [
        bash(&format!("{cd} && gh pr create --base main --title \"Listing\" --body-file /tmp/b.md 2>&1 | tail -3")),
        out("Warning: 1 uncommitted change\nhttps://github.com/acme/widgets/pull/46"),
        bash(&format!("{cd} && gh pr create --title again")),
        out("a pull request for branch \"fix\" into branch \"main\" already exists:\nhttps://github.com/acme/widgets/pull/46"),
        bash(&format!("{cd} && gh pr create --title other")),
        out("Exit code 1\npull request create failed: GraphQL: No commits between main and fix"),
        turn("tool", Some("mcp__github__create_pull_request"), "mcp__github__create_pull_request {\"owner\":\"acme\",\"repo\":\"gears\",\"title\":\"x\"}"),
        turn("tool", Some("mcp__github__create_pull_request"), "{\"number\":3,\"html_url\":\"https://github.com/acme/gears/pull/3\"}"),
    ];
    let w = read(&turns, None, &mut repos());
    assert_eq!(
        w.pulls,
        [
            (
                "https://github.com/acme/widgets/pull/46".to_owned(),
                PullDone::Opened,
                T.to_owned()
            ),
            (
                "https://github.com/acme/gears/pull/3".to_owned(),
                PullDone::Opened,
                T.to_owned()
            ),
        ]
    );
}

#[test]
fn merged_is_a_merge_that_did_not_fail() {
    let tmp = tempfile::tempdir().unwrap();
    let widgets = tmp.path().join("widgets");
    checkout(&widgets, "git@github.com:acme/widgets.git");
    let cd = format!("cd {}", widgets.display());
    let url = |n: u64| format!("https://github.com/acme/widgets/pull/{n}");
    let turns = [
        // Output piped away: nothing indexed for the result, a success.
        bash(&format!("{cd} && gh pr merge 45 --squash --delete-branch 2>&1 | tail -1; git checkout -q main")),
        say("Merged."),
        bash(&format!("{cd} && gh pr merge 46 --squash")),
        out("GraphQL: Base branch was modified. Review and try the merge again. (mergePullRequest)"),
        bash(&format!("{cd} && gh pr merge 47 --squash --auto")),
        out("✓ Pull request acme/widgets#47 will be automatically merged via squash when all requirements are met"),
        bash(&format!("{cd} && gh pr merge 48 --squash && git pull -q && git log --oneline -1")),
        out("f793642 Listing filters (#48)"),
        bash(&format!("gh pr merge {} --squash; gh pr checkout 2 && cargo test", url(49))),
        out("Exit code 101\nerror: test failed, to rerun pass `-p widgets`"),
        bash("gh pr merge 50 -R acme/gears --squash"),
        out("✓ Squashed and merged pull request acme/gears#50 (Gears)"),
        bash("gh pr merge 51 --squash"),
        out("Exit code 1\nX Pull request acme/widgets#51 is not mergeable: the merge commit cannot be cleanly created."),
        bash("./scripts/land.py merge https://github.com/acme/widgets/pull/52 --tested yes"),
        out("checks green; merged https://github.com/acme/widgets/pull/52"),
        bash("./scripts/land.py merge https://github.com/acme/widgets/pull/53"),
        out("waiting for review"),
    ];
    let w = read(&turns, Some(&widgets), &mut repos());
    let merged: Vec<&str> = w
        .pulls
        .iter()
        .filter(|p| p.1 == PullDone::Merged)
        .map(|p| p.0.as_str())
        .collect();
    assert_eq!(
        merged,
        [
            url(45).as_str(),
            url(48).as_str(),
            url(49).as_str(),
            "https://github.com/acme/gears/pull/50",
            url(52).as_str(),
        ]
    );
}

#[test]
fn a_number_without_a_remote_takes_the_owner_the_session_saw() {
    let tmp = tempfile::tempdir().unwrap();
    let widgets = tmp.path().join("widgets");
    // A checkout with no origin.
    fs::create_dir_all(widgets.join(".git")).unwrap();
    let turns = [
        say("CI is green on https://github.com/acme/widgets/pull/7."),
        bash(&format!(
            "cd {} && gh pr merge 7 --squash",
            widgets.display()
        )),
        out("✓ Merged pull request #7"),
    ];
    let w = read(&turns, None, &mut repos());
    assert_eq!(w.pulls.len(), 1);
    assert_eq!(w.pulls[0].0, "https://github.com/acme/widgets/pull/7");
}

#[test]
fn parallel_calls_pair_with_their_results_in_order() {
    let tmp = tempfile::tempdir().unwrap();
    let widgets = tmp.path().join("widgets");
    checkout(&widgets, "git@github.com:acme/widgets.git");
    let cd = format!("cd {}", widgets.display());
    let turns = [
        bash(&format!("{cd} && gh pr create --title a")),
        bash(&format!("{cd} && gh pr create --title b")),
        out("https://github.com/acme/widgets/pull/8"),
        out("https://github.com/acme/widgets/pull/9"),
    ];
    let w = read(&turns, None, &mut repos());
    let urls: Vec<&str> = w.pulls.iter().map(|p| p.0.as_str()).collect();
    assert_eq!(
        urls,
        [
            "https://github.com/acme/widgets/pull/8",
            "https://github.com/acme/widgets/pull/9"
        ]
    );
}

#[test]
fn codex_commands_are_read_too() {
    let tmp = tempfile::tempdir().unwrap();
    let widgets = tmp.path().join("widgets");
    checkout(&widgets, "git@github.com:acme/widgets.git");
    let input = serde_json::json!({ "command": ["bash", "-lc", "gh pr create --fill"], "workdir": widgets });
    let turns = [
        turn("tool", Some("shell"), &format!("shell {input}")),
        turn(
            "tool",
            Some("shell"),
            "https://github.com/acme/widgets/pull/11",
        ),
    ];
    let w = read(&turns, Some(&widgets), &mut repos());
    assert_eq!(w.pulls.len(), 1);
    assert_eq!(w.project, Some(("widgets".to_owned(), 2)));
}

#[test]
fn a_pull_request_keeps_the_title_its_command_gave_it() {
    let turns = [
        bash("gh pr create --base main --head fix/parser --title \"Parser: no unwrap on input\" --body x"),
        out("https://github.com/acme/widgets/pull/12"),
        bash("gh pr create -t 'Second one' --body y"),
        out("https://github.com/acme/widgets/pull/13"),
        bash("gh pr create --fill"),
        out("https://github.com/acme/widgets/pull/14"),
    ];
    let w = read(&turns, None, &mut repos());
    assert_eq!(w.pulls.len(), 3, "{:?}", w.pulls);
    assert_eq!(
        w.pull_titles,
        [
            (
                "https://github.com/acme/widgets/pull/12".to_owned(),
                "Parser: no unwrap on input".to_owned()
            ),
            (
                "https://github.com/acme/widgets/pull/13".to_owned(),
                "Second one".to_owned()
            ),
        ]
    );
}
