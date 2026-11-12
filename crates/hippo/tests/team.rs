//! One workspace shared by two people through git. Two scratch homes, a bare
//! remote, the workspace cloned twice; each person has their own synthetic
//! Claude Code sessions (one in the workspace, one in another project),
//! runs a session start and end with a handoff that links the same note,
//! dreams with a stand-in model that appends to that note, commits with
//! `git add -A`, and pulls the other's commits with `git pull --rebase`.
//!
//! It checks that nothing per person is ever staged, that the pull has no
//! conflict, that neither dream moves the other's window, that another
//! project's session leaves no record, and that `doctor`'s `receipts` and
//! `git` rows stay quiet after the merge. Skipped when `git` is missing.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};

const NOTE: &str = "notes/Shared.md";

/// The stand-in model: as the reviewer it confirms; otherwise it appends
/// `$TEAM_TEXT` to the shared note, citing turn 0 of `$TEAM_SESSION`.
const MODEL: &str = r#"#!/bin/sh
cat > /dev/null
if [ -n "$TEAM_REVIEW" ]; then
  printf '{"result":"{\\"verdict\\":\\"confirm\\",\\"notes\\":\\"the session says so\\"}"}\n'
  exit 0
fi
printf '{"result":"[{\\"note\\":\\"notes/Shared.md\\",\\"kind\\":\\"append\\",\\"old\\":\\"\\",\\"new\\":\\"\\\\n- %s\\\\n\\",\\"evidence\\":[{\\"session\\":\\"%s\\",\\"ordinal\\":0}],\\"reason\\":\\"the session learned it\\"}]"}\n' "$TEAM_TEXT" "$TEAM_SESSION"
"#;

fn git_found() -> bool {
    Command::new("git")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

struct Person {
    name: &'static str,
    /// Their home folder, `$HOME` and `$HIPPO_HOME` below it.
    dir: PathBuf,
    ws: PathBuf,
    session: String,
}

impl Person {
    fn new(base: &Path, name: &'static str) -> Person {
        let dir = base.join(name);
        fs::create_dir_all(dir.join("hippo")).unwrap();
        Person {
            name,
            ws: dir.join("ws"),
            session: format!("{name}-session"),
            dir,
        }
    }

    fn home(&self) -> PathBuf {
        self.dir.join("hippo")
    }

    fn hippo(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_hippo"))
            .args(args)
            .current_dir(&self.ws)
            .env("HOME", &self.dir)
            .env("HIPPO_HOME", self.home())
            .env("TEAM_SESSION", &self.session)
            .env(
                "TEAM_TEXT",
                format!("{} learned that the cache key needs the locale", self.name),
            )
            .env_remove("RUST_LOG")
            .env_remove("CLAUDE_CONFIG_DIR")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.hippo(args);
        assert!(
            out.status.success(),
            "{}: hippo {args:?}: {}{}",
            self.name,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn git(&self, args: &[&str]) -> String {
        git_in(&self.ws, &self.dir, self.name, args)
    }

    /// Writes the person's two transcripts: one run in the workspace, one
    /// in another project.
    fn transcripts(&self, at: OffsetDateTime) {
        let root = self.dir.join("transcripts").join("projects").join("p");
        fs::create_dir_all(&root).unwrap();
        let ts = |m: i64| (at + Duration::minutes(m)).format(&Rfc3339).unwrap();
        let line = |id: &str, cwd: &Path, role: &str, text: &str, m: i64| {
            let content = serde_json::json!(text);
            let message = if role == "user" {
                format!(r#"{{"role":"user","content":{content}}}"#)
            } else {
                format!(
                    r#"{{"id":"m{m}","type":"message","role":"assistant","model":"synthetic-model","content":[{{"type":"text","text":{content}}}]}}"#
                )
            };
            format!(
                r#"{{"cwd":{cwd},"sessionId":"{id}","type":"{role}","message":{message},"uuid":"00000000-0000-4000-8000-{m:012}","timestamp":"{ts}"}}"#,
                cwd = serde_json::json!(cwd.display().to_string()),
                ts = ts(m),
            ) + "\n"
        };
        let here = line(
            &self.session,
            &self.ws,
            "user",
            &format!("{} learned that the cache key needs the locale", self.name),
            0,
        ) + &line(
            &self.session,
            &self.ws,
            "assistant",
            "Noted in the shared note.",
            1,
        );
        fs::write(root.join(format!("{}.jsonl", self.session)), here).unwrap();
        let other = format!("{}-other", self.name);
        let elsewhere = self.dir.join("other-project");
        let text = line(&other, &elsewhere, "user", "rename the billing export", 2)
            + &line(&other, &elsewhere, "assistant", "Renamed.", 3);
        fs::write(root.join(format!("{other}.jsonl")), text).unwrap();
    }

    fn configure(&self, model: &Path) {
        let model = model.display().to_string();
        let config = format!(
            "workspace = {ws:?}\n\
             transcript_roots = [{roots:?}]\n\
             model_command = [{model:?}, \"--system-prompt\", \"{{system_prompt}}\"]\n\
             reviewer_command_override = [\"env\", \"TEAM_REVIEW=1\", {model:?}, \"--system-prompt\", \"{{system_prompt}}\"]\n",
            ws = fs::canonicalize(&self.ws).unwrap().display().to_string(),
            roots = self.dir.join("transcripts").join("projects").display().to_string(),
        );
        fs::write(self.home().join("config.toml"), config).unwrap();
    }

    fn last_success(&self) -> Option<String> {
        let path = self.ws.join(".hippo/dream-state.json");
        let text = fs::read_to_string(path).ok()?;
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        v["last_success"].as_str().map(str::to_owned)
    }

    /// `doctor`'s row `name`: `(status, detail)`.
    fn doctor_row(&self, name: &str) -> (String, String) {
        let out = self.hippo(&["doctor"]);
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let line = text
            .lines()
            .find(|l| l.starts_with(&format!("{name} | ")))
            .unwrap_or_else(|| panic!("no {name} row in\n{text}"));
        let mut parts = line.splitn(3, " | ");
        parts.next();
        (
            parts.next().unwrap_or("").to_owned(),
            parts.next().unwrap_or("").to_owned(),
        )
    }

    /// Staged or untracked paths, as `git status --porcelain` lists them.
    fn status(&self) -> Vec<String> {
        self.git(&["status", "--porcelain", "--untracked-files=all"])
            .lines()
            .map(|l| l[3..].to_owned())
            .collect()
    }

    fn assert_nothing_per_person(&self, when: &str) {
        let listed: Vec<String> = self
            .git(&["ls-files"])
            .lines()
            .chain(self.status().iter().map(String::as_str))
            .map(str::to_owned)
            .collect();
        let per_person = [
            ".hippo/sessions/",
            ".hippo/reports/",
            ".hippo/snapshots/",
            ".hippo/dream-state.json",
            "notes/Session log.md",
        ];
        for path in &listed {
            assert!(
                !per_person.iter().any(|p| path.starts_with(p)),
                "{} {when}: git sees {path}",
                self.name
            );
        }
    }
}

fn git_in(dir: &Path, home: &Path, name: &str, args: &[&str]) -> String {
    let out = Command::new("git")
        .args([
            "-c",
            &format!("user.name={name}"),
            "-c",
            &format!("user.email={name}@example.invalid"),
            "-c",
            "init.defaultBranch=main",
            "-c",
            "core.autocrlf=false",
        ])
        .args(args)
        .current_dir(dir)
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{name}: git {args:?}: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn two_people_share_one_workspace_through_git() {
    if !git_found() {
        eprintln!("skipped: git is not on PATH");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(base.path()).unwrap();
    let model = base.join("model.sh");
    fs::write(&model, MODEL).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&model, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let remote = base.join("remote.git");
    git_in(&base, &base, "setup", &["init", "--bare", "remote.git"]);
    let remote = remote.display().to_string();

    // Ann starts the workspace and pushes it.
    let ann = Person::new(&base, "ann");
    git_in(&ann.dir, &ann.dir, "ann", &["clone", "-q", &remote, "ws"]);
    fs::create_dir_all(ann.ws.join("notes")).unwrap();
    fs::write(ann.ws.join(NOTE), "# Shared\n\nWhat the team knows.\n").unwrap();
    let out = ann.ok(&[
        "init",
        "--notes-dir",
        "notes",
        "--entry-note",
        "notes/START HERE.md",
    ]);
    assert!(out.contains(".gitattributes"), "{out}");
    let attributes = fs::read_to_string(ann.ws.join(".gitattributes")).unwrap();
    assert_eq!(attributes, "/notes/**/*.md merge=union\n");
    ann.git(&["add", "-A"]);
    ann.git(&["commit", "-q", "-m", "a workspace"]);
    ann.git(&["push", "-q", "origin", "HEAD:main"]);

    // Ben clones it; `init` with no flags keeps the workspace's files.
    let ben = Person::new(&base, "ben");
    git_in(&ben.dir, &ben.dir, "ben", &["clone", "-q", &remote, "ws"]);
    let out = ben.ok(&["init"]);
    assert!(out.contains("kept"), "{out}");
    assert_eq!(
        ben.status(),
        Vec::<String>::new(),
        "init changed nothing tracked"
    );

    let at = OffsetDateTime::now_utc() - Duration::hours(3);
    for (p, offset) in [(&ann, 0), (&ben, 30)] {
        p.configure(&model);
        p.transcripts(at + Duration::minutes(offset));
        // In an existing setup, init only checks the rules.
        let out = p.ok(&["init"]);
        assert!(out.contains("are in place"), "{out}");
        p.ok(&[
            "session",
            "start",
            "--tool",
            "claude-code",
            "--session",
            &p.session,
        ]);
        let handoff = p.dir.join("handoff.md");
        fs::write(&handoff, "The cache key needs the locale: [[Shared]]\n").unwrap();
        let handoff = handoff.display().to_string();
        p.ok(&[
            "session",
            "end",
            "--session",
            &p.session,
            "--note",
            NOTE,
            "--handoff",
            &handoff,
        ]);
        let out = p.ok(&["ingest"]);
        assert!(out.starts_with("indexed 2 files"), "{out}");
        // Another project's session is indexed and leaves no record.
        let records = p.ws.join(".hippo/sessions");
        assert!(records.join(format!("{}.json", p.session)).is_file());
        assert!(!records.join(format!("{}-other.json", p.name)).exists());
        p.ok(&["log"]);
        let out = p.ok(&["dream", "--pass", "verify"]);
        assert!(out.contains("applied"), "{}: {out}", p.name);
        assert!(p.last_success().is_some(), "{} dreamed", p.name);
        p.assert_nothing_per_person("after dreaming");
    }
    let ann_window = ann.last_success();
    let ben_window = ben.last_success();
    assert_ne!(ann_window, ben_window, "two runs, two windows");

    // Each commits everything git sees; Ann pushes first.
    for p in [&ann, &ben] {
        p.git(&["add", "-A"]);
        p.assert_nothing_per_person("after git add -A");
        p.git(&["commit", "-q", "-m", &format!("{}'s notes", p.name)]);
    }
    ann.git(&["push", "-q", "origin", "HEAD:main"]);
    ben.git(&["pull", "-q", "--rebase", "origin", "main"]);
    ben.git(&["push", "-q", "origin", "HEAD:main"]);
    ann.git(&["pull", "-q", "--rebase", "origin", "main"]);

    for p in [&ann, &ben] {
        let note = fs::read_to_string(p.ws.join(NOTE)).unwrap();
        for who in ["ann", "ben"] {
            let line = format!("- {who} learned that the cache key needs the locale");
            assert_eq!(note.matches(&line).count(), 1, "{}: {note}", p.name);
        }
        assert!(!note.contains("<<<<<<<"), "{note}");
        p.assert_nothing_per_person("after pulling");
        // Neither pull moved the other's dreaming window.
        let own = if p.name == "ann" {
            &ann_window
        } else {
            &ben_window
        };
        assert_eq!(&p.last_success(), own, "{}'s window", p.name);
        // The other's session records never arrive.
        let other = if p.name == "ann" { "ben" } else { "ann" };
        assert!(!p
            .ws
            .join(format!(".hippo/sessions/{other}-session.json"))
            .exists());
        let (status, detail) = p.doctor_row("receipts");
        assert_eq!(status, "ok", "{}: receipts {detail}", p.name);
        let (status, detail) = p.doctor_row("git");
        assert_eq!(status, "ok", "{}: git {detail}", p.name);
        assert_eq!(detail, "per-person files ignored");
    }
}

#[test]
fn doctor_names_tracked_per_person_files_and_the_fix() {
    if !git_found() {
        eprintln!("skipped: git is not on PATH");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(base.path()).unwrap();
    let p = Person::new(&base, "cy");
    fs::create_dir_all(p.ws.join("notes")).unwrap();
    git_in(&p.ws, &p.dir, "cy", &["init", "-q"]);
    p.ok(&[
        "init",
        "--notes-dir",
        "notes",
        "--entry-note",
        "notes/START HERE.md",
    ]);
    // A workspace from before: its rules are gone and a record is tracked.
    fs::write(p.ws.join(".hippo/.gitignore"), "snapshots/\n").unwrap();
    fs::remove_file(p.ws.join(".gitignore")).unwrap();
    let record = p.ws.join(".hippo/sessions/s1.json");
    fs::create_dir_all(record.parent().unwrap()).unwrap();
    fs::write(&record, "{}\n").unwrap();
    p.git(&["add", "-A"]);
    p.git(&["commit", "-q", "-m", "everything"]);

    let (status, detail) = p.doctor_row("git");
    assert_eq!(status, "warn", "{detail}");
    assert!(detail.contains(".hippo/sessions/s1.json"), "{detail}");
    assert!(
        detail.contains("git rm -r --cached -- .hippo/sessions")
            && detail.contains("git commit -m"),
        "{detail}"
    );

    p.git(&["rm", "-q", "-r", "--cached", "--", ".hippo/sessions"]);
    p.git(&["commit", "-q", "-m", "stop tracking"]);
    let (status, detail) = p.doctor_row("git");
    assert_eq!(status, "warn", "{detail}");
    assert!(detail.contains("hippo init"), "{detail}");

    // `init` with no flags adds only the missing lines.
    let out = p.ok(&["init"]);
    assert!(out.contains("updated"), "{out}");
    assert_eq!(
        fs::read_to_string(p.ws.join(".hippo/.gitignore")).unwrap(),
        "snapshots/\nsessions/\nreports/\ndream-state.json\n"
    );
    assert_eq!(
        fs::read_to_string(p.ws.join(".gitignore")).unwrap(),
        "/notes/Session log.md\n"
    );
    let (status, detail) = p.doctor_row("git");
    assert_eq!(
        (status.as_str(), detail.as_str()),
        ("ok", "per-person files ignored")
    );
}

#[test]
fn a_closed_pipe_ends_quietly() {
    // `hippo --help | head -1` must not panic.
    let base = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_hippo"))
        .arg("--help")
        .env("HIPPO_HOME", base.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Close the read end before the child writes.
    drop(child.stdout.take());
    let out = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert!(!stderr.contains("Broken pipe"), "{stderr}");
}
