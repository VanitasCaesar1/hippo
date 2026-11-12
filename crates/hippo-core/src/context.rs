//! The context compiler: the budgeted block an agent sees when a session
//! starts.
//!
//! Six parts, in this order: 1. warning lines, 2. the entry note's opening
//! section, 3. the required notes, 4. the session line, 5. relevant notes
//! (only with a task) and 6. linter counts. Parts 1 to 4 are required: when
//! they exceed the budget, compiling fails with [`Error::OverBudget`].
//! Parts 5 and 6 are optional: each is kept only if it fits whole, and is
//! otherwise listed in [`Compiled::omitted`]. Parts are separated by one
//! blank line, and an empty part is left out.

pub mod rank;

use std::cmp::Ordering;
use std::fmt;
use std::path::{Path, PathBuf};

use time::format_description::well_known::Rfc3339;
use time::{Date, Duration, OffsetDateTime};

use crate::config::Workspace;
use crate::dream::report;
use crate::error::{Error, Result};
use crate::index::Index;
use crate::notes::{normalise, LintFinding, Note, Notes};
use crate::records::{DreamState, Session};

/// A part of the context block, in output order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Part {
    Warning,
    Entry,
    Required,
    Session,
    Relevant,
    Linter,
}

impl Part {
    /// The part's name in `omitted:` lines.
    pub fn as_str(self) -> &'static str {
        match self {
            Part::Warning => "warning",
            Part::Entry => "entry",
            Part::Required => "required",
            Part::Session => "session",
            Part::Relevant => "relevant",
            Part::Linter => "linter",
        }
    }
}

impl fmt::Display for Part {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which parts to compile: `--parts required|optional|all`. `hippo hook
/// session-start` compiles the required parts and `hippo hook prompt` the
/// optional ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PartSelection {
    /// Parts 1 to 4.
    Required,
    /// Parts 5 and 6.
    Optional,
    /// All six.
    #[default]
    All,
}

/// A compiled context block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compiled {
    /// The parts kept, in order, without trailing newlines.
    pub parts: Vec<(Part, String)>,
    /// The size of [`Compiled::text`] in bytes.
    pub bytes: usize,
    /// Optional parts left out because they did not fit whole, with their
    /// own size in bytes. These do not count against the budget.
    pub omitted: Vec<(Part, usize)>,
    /// The notes the kept `relevant` part lists, in its order, as paths
    /// relative to the workspace. Empty when that part was not compiled or
    /// was omitted for the budget.
    pub relevant: Vec<PathBuf>,
    /// Why each note in [`Compiled::relevant`] was picked, in its order
    /// ([`rank::Pick::reason`]).
    pub why: Vec<String>,
    /// Whether each note in [`Compiled::relevant`] is a weak pick, in its
    /// order: it does not name the repository and matched
    /// no word of the task or scored under half the top pick's score.
    pub weak: Vec<bool>,
    /// The notes the `required:` part names, in its order, as normalised
    /// paths relative to the workspace. Empty when that part was not
    /// compiled.
    pub required: Vec<PathBuf>,
}

impl Compiled {
    /// The block: the parts separated by one blank line, ending with a
    /// newline; empty when no part was kept.
    pub fn text(&self) -> String {
        render(&self.parts)
    }

    /// One `omitted: <part> (<bytes>)` line per dropped part, for stderr.
    pub fn omitted_lines(&self) -> Vec<String> {
        self.omitted
            .iter()
            .map(|(part, bytes)| format!("omitted: {part} ({bytes})"))
            .collect()
    }
}

/// Compiles the context block for `ws`. `session` adds the session line;
/// `task` enables the relevant notes, and `repo`, the name of the
/// repository the session works in ([`rank::repo_name`]), lifts the notes
/// that name it. Notes are scanned now; the index is not read.
pub fn compile(
    ws: &Workspace,
    _index: Option<&Index>,
    task: Option<&str>,
    repo: Option<&str>,
    session: Option<&Session>,
    budget: usize,
    parts: PartSelection,
) -> Result<Compiled> {
    compile_at(
        ws,
        task.map(|text| (text, repo)),
        session,
        budget,
        parts,
        OffsetDateTime::now_utc(),
    )
}

fn compile_at(
    ws: &Workspace,
    task: Option<(&str, Option<&str>)>,
    session: Option<&Session>,
    budget: usize,
    selection: PartSelection,
    now: OffsetDateTime,
) -> Result<Compiled> {
    let mut parts: Vec<(Part, String)> = Vec::new();
    let mut required_paths = Vec::new();
    if selection != PartSelection::Optional {
        parts.push((Part::Warning, warnings(ws, now)?.join("\n")));
        parts.push((Part::Entry, entry_section(ws)?));
        let required: Vec<String> = ws
            .required_notes
            .iter()
            .map(|path| format!("required: {}", path.display()))
            .collect();
        parts.push((Part::Required, required.join("\n")));
        required_paths = ws
            .required_notes
            .iter()
            .map(|path| normalise(path))
            .collect();
        if let Some(session) = session {
            parts.push((Part::Session, session_line(session)));
        }
        parts.retain(|(_, text)| !text.is_empty());
        let required_bytes = render(&parts).len();
        if required_bytes > budget {
            return Err(Error::OverBudget {
                required_bytes,
                budget,
            });
        }
    }

    let mut omitted = Vec::new();
    let mut listed = Vec::new();
    let mut why = Vec::new();
    let mut weak = Vec::new();
    if selection != PartSelection::Required {
        let notes = Notes::scan(&ws.notes_root())?;
        let mut optional = Vec::new();
        if let Some((task, repo)) = task {
            let (text, paths, reasons, weaks) = relevant(ws, &notes, task, repo);
            listed = paths;
            why = reasons;
            weak = weaks;
            optional.push((Part::Relevant, text));
        }
        optional.push((Part::Linter, linter_counts(&notes.lint())));
        for (part, text) in optional {
            if text.is_empty() {
                continue;
            }
            parts.push((part, text));
            if render(&parts).len() > budget {
                let (part, text) = parts.pop().expect("just pushed");
                omitted.push((part, text.len()));
            }
        }
    }

    if !parts.iter().any(|(part, _)| *part == Part::Relevant) {
        listed.clear();
        why.clear();
        weak.clear();
    }

    Ok(Compiled {
        bytes: render(&parts).len(),
        parts,
        omitted,
        relevant: listed,
        why,
        weak,
        required: required_paths,
    })
}

/// The parts separated by one blank line, with a final newline.
fn render(parts: &[(Part, String)]) -> String {
    if parts.is_empty() {
        return String::new();
    }
    let texts: Vec<&str> = parts.iter().map(|(_, text)| text.as_str()).collect();
    let mut out = texts.join("\n\n");
    out.push('\n');
    out
}

/// Part 1, failure first. Only dreaming reports (`*-dream.md`) count, and
/// before the first one exists there is no warning. A `dream-state.json`
/// that cannot be read counts as absent.
pub fn warnings(ws: &Workspace, now: OffsetDateTime) -> Result<Vec<String>> {
    let Some(newest) = report::newest(ws)? else {
        return Ok(Vec::new());
    };

    let mut lines = Vec::new();
    if newest.failed {
        lines.push("warning: last dreaming run failed".to_owned());
    }
    let state = DreamState::load(&ws.hippo_dir().join("dream-state.json"));
    match state.ok().and_then(|state| state.last_success) {
        None => lines.push("warning: no dreaming run recorded".to_owned()),
        Some(at) if now - at > Duration::hours(36) => {
            lines.push(format!("warning: last dreaming success {}", rfc3339(at)));
        }
        Some(_) => {}
    }
    Ok(lines)
}

/// Part 2: the entry note's body before its first `## ` heading, without
/// leading blank lines or trailing whitespace.
fn entry_section(ws: &Workspace) -> Result<String> {
    let note = Note::read(&ws.root.join(&ws.entry_note))?;
    let body = String::from_utf8_lossy(&note.body);
    let mut section = String::new();
    for line in body.split_inclusive('\n') {
        if line.starts_with("## ") {
            break;
        }
        section.push_str(line);
    }
    Ok(section
        .trim_start_matches(['\n', '\r'])
        .trim_end()
        .to_owned())
}

/// Part 4: `session: <id> <tool> <started>`.
fn session_line(session: &Session) -> String {
    format!(
        "session: {} {} {}",
        session.id,
        session.tool.as_str(),
        rfc3339(session.started)
    )
}

/// Part 5: `relevant:` and up to five `- <title>: <first body line>` lines,
/// the notes [`rank::rank`] picks for the task; empty when it picks none.
/// The entry note, the required notes and the log notes are never picked:
/// the block names them already, or they say nothing about a task.
/// Also returns the picked notes' paths (the notes directory joined to each
/// note's path below it) and why each was picked.
fn relevant(
    ws: &Workspace,
    notes: &Notes,
    task: &str,
    repo: Option<&str>,
) -> (String, Vec<PathBuf>, Vec<String>, Vec<bool>) {
    let picks = rank::rank(
        notes,
        task,
        repo,
        &skipped(ws, notes),
        &rank::Params::default(),
    );
    if picks.is_empty() {
        return (String::new(), Vec::new(), Vec::new(), Vec::new());
    }
    let top = picks.iter().map(|p| p.score).fold(0.0_f64, f64::max);
    let mut text = String::from("relevant:");
    let mut paths = Vec::new();
    let mut why = Vec::new();
    let mut weak = Vec::new();
    for pick in picks {
        let note = &notes.notes[&pick.path];
        paths.push(normalise(&ws.notes_dir.join(&pick.path)));
        why.push(pick.reason(repo));
        weak.push(pick.is_weak(top));
        let line = format!(
            "- {}: {}",
            note.title(),
            note.first_body_line().unwrap_or_default()
        );
        text.push('\n');
        text.push_str(line.trim_end());
    }
    (text, paths, why, weak)
}

/// The notes the relevant part never picks, relative to the notes
/// directory: the entry note, the required notes and the log notes,
/// `log_path` among them.
pub fn skipped(ws: &Workspace, notes: &Notes) -> Vec<PathBuf> {
    let notes_dir = normalise(&ws.notes_dir);
    let mut out: Vec<PathBuf> = std::iter::once(&ws.entry_note)
        .chain(&ws.required_notes)
        .filter_map(|p| {
            normalise(p)
                .strip_prefix(&notes_dir)
                .ok()
                .map(Path::to_path_buf)
        })
        .collect();
    out.extend(crate::log_notes::below_notes_dir(ws, notes));
    out
}

/// Most recent `updated` first; notes without one last.
fn newest_first(a: Option<Date>, b: Option<Date>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => b.cmp(&a),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// Part 6: `linter: overdue=<n> dangling=<m>`.
fn linter_counts(findings: &[LintFinding]) -> String {
    let overdue = findings
        .iter()
        .filter(|f| matches!(f, LintFinding::Overdue { .. }))
        .count();
    let dangling = findings
        .iter()
        .filter(|f| matches!(f, LintFinding::DanglingLink { .. }))
        .count();
    format!("linter: overdue={overdue} dangling={dangling}")
}

fn rfc3339(at: OffsetDateTime) -> String {
    at.format(&Rfc3339)
        .expect("RFC 3339 formats every in-range date")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::Tool;
    use std::fs;
    use time::macros::datetime;

    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures");

    const ENTRY: &str = "---\ntype: policy\n---\n# Start\n\nRead the protocol first.\n\
                         Then the open questions.\n\n## Later\n\nNot in the entry section.\n";

    /// A workspace in a temporary directory whose notes are `notes` (path,
    /// text) under `brain/`, the first being the entry note.
    fn workspace(notes: &[(&str, &str)]) -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();
        for (path, text) in notes {
            let path = dir.path().join("brain").join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        let ws = Workspace {
            root: dir.path().to_path_buf(),
            notes_dir: "brain".into(),
            entry_note: Path::new("brain").join(notes[0].0),
            required_notes: Vec::new(),
            budget_bytes: crate::DEFAULT_CONTEXT_BUDGET,
            log_path: "brain/Session log.md".into(),
            sample_tasks: Vec::new(),
            passes_off: Vec::new(),
            lessons: true,
            snapshot_keep_days: None,
            log_notes: None,
        };
        (dir, ws)
    }

    /// The fixture notes, read in place.
    fn fixture_workspace() -> Workspace {
        Workspace {
            root: PathBuf::from(FIXTURES),
            notes_dir: "notes".into(),
            entry_note: "notes/START HERE.md".into(),
            required_notes: vec!["notes/Topic A.md".into(), "notes/Sub/Topic B.md".into()],
            budget_bytes: crate::DEFAULT_CONTEXT_BUDGET,
            log_path: "notes/Session log.md".into(),
            sample_tasks: Vec::new(),
            passes_off: Vec::new(),
            lessons: true,
            snapshot_keep_days: None,
            log_notes: None,
        }
    }

    fn session() -> Session {
        Session {
            id: "ab12cd34".into(),
            tool: Tool::ClaudeCode,
            task: String::new(),
            task_seen: false,
            context_bytes_at_start: 0,
            started: datetime!(2026-11-08 03:00:00 UTC),
            ended: None,
            notes_touched: Vec::new(),
            handoff: None,
            handoff_derived: false,
            no_change_reason: None,
            context_start: None,
            context_prompt: None,
            derived: false,
        }
    }

    const NOW: OffsetDateTime = datetime!(2026-11-08 12:00:00 UTC);

    fn compile_now(
        ws: &Workspace,
        task: Option<&str>,
        session: Option<&Session>,
        budget: usize,
        selection: PartSelection,
    ) -> Result<Compiled> {
        compile_at(ws, task.map(|t| (t, None)), session, budget, selection, NOW)
    }

    fn part(compiled: &Compiled, wanted: Part) -> Option<&str> {
        compiled
            .parts
            .iter()
            .find(|(part, _)| *part == wanted)
            .map(|(_, text)| text.as_str())
    }

    fn write_hippo(ws: &Workspace, path: &str, text: &str) {
        let path = ws.hippo_dir().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn each_part_has_its_format() {
        let ws = fixture_workspace();
        let task = "Find the dup notes";
        let compiled = compile_now(
            &ws,
            Some(task),
            Some(&session()),
            25_600,
            PartSelection::All,
        )
        .unwrap();
        let order: Vec<Part> = compiled.parts.iter().map(|(part, _)| *part).collect();
        // No reports exist, so there is no warning part.
        assert_eq!(
            order,
            [
                Part::Entry,
                Part::Required,
                Part::Session,
                Part::Relevant,
                Part::Linter
            ]
        );
        assert_eq!(
            part(&compiled, Part::Entry),
            Some("# Start here\n\nA synthetic entry note for the parser and linter tests.")
        );
        assert_eq!(
            part(&compiled, Part::Required),
            Some("required: notes/Topic A.md\nrequired: notes/Sub/Topic B.md")
        );
        assert_eq!(
            part(&compiled, Part::Session),
            Some("session: ab12cd34 claude-code 2026-11-08T03:00:00Z")
        );
        // "dup" (and "notes", in most bodies, too common to count): the two
        // Dup notes match in title and path and tie, the more recently
        // updated first. The required notes and the entry note are never
        // picked; "find" and "the" are stop words.
        assert_eq!(
            part(&compiled, Part::Relevant),
            Some(
                "relevant:\n\
                 - Dup in Sub: The other synthetic note named Dup.md. A path link such as \
                 [[Sub/Dup]] still resolves.\n\
                 - Dup in Other: One of two synthetic notes named Dup.md, so a bare link to \
                 that name is ambiguous."
            )
        );
        assert_eq!(
            part(&compiled, Part::Linter),
            Some("linter: overdue=1 dangling=2")
        );
        let text = compiled.text();
        assert_eq!(compiled.bytes, text.len());
        assert!(text.contains("tests.\n\nrequired: notes/Topic A.md"));
        assert!(text.ends_with("dangling=2\n"));
        assert!(compiled.omitted.is_empty());
    }

    #[test]
    fn relevant_never_offers_log_notes() {
        let mut journal = String::from("# Work log\n\n");
        for day in 1..=12 {
            journal.push_str(&format!(
                "## 2026-10-{day:02} webhook retry work\n\nWebhook retry.\n\n"
            ));
        }
        let (_dir, ws) = workspace(&[
            ("START HERE.md", ENTRY),
            ("Webhook retry.md", "# Webhook retry\n\nBackoff rules.\n"),
            ("Ops/Work log.md", &journal),
        ]);
        let compiled = compile_now(
            &ws,
            Some("webhook retry backoff"),
            None,
            25_600,
            PartSelection::All,
        )
        .unwrap();
        assert_eq!(compiled.relevant, [Path::new("brain/Webhook retry.md")]);
        // Named as a log note by setting, a topic note is skipped too; the
        // journal, no longer matched by a rule, comes back.
        let ws = Workspace {
            log_notes: Some(vec!["webhook*".into()]),
            ..ws
        };
        let compiled = compile_now(
            &ws,
            Some("webhook retry backoff"),
            None,
            25_600,
            PartSelection::All,
        )
        .unwrap();
        assert_eq!(compiled.relevant, [Path::new("brain/Ops/Work log.md")]);
    }

    #[test]
    fn relevant_paths_match_the_rendered_lines() {
        let ws = fixture_workspace();
        let compiled = compile_now(&ws, Some("dup"), None, 25_600, PartSelection::All).unwrap();
        // Paths are workspace-relative, in the order of the rendered lines.
        assert_eq!(
            compiled.relevant,
            [
                Path::new("notes/Sub/Dup.md"),
                Path::new("notes/Other/Dup.md")
            ]
        );
        // Why each was picked, in the same order (Brief shows it).
        assert_eq!(compiled.why, ["matches dup", "matches dup"]);
        let notes = Notes::scan(&ws.notes_root()).unwrap();
        let titles: Vec<String> = compiled
            .relevant
            .iter()
            .map(|path| {
                let key = path.strip_prefix(&ws.notes_dir).unwrap();
                format!("- {}:", notes.notes[key].title())
            })
            .collect();
        let rendered: Vec<&str> = part(&compiled, Part::Relevant)
            .unwrap()
            .lines()
            .skip(1)
            .collect();
        assert_eq!(titles.len(), rendered.len());
        for (title, line) in titles.iter().zip(rendered) {
            assert!(line.starts_with(title), "{line} vs {title}");
        }

        // Not compiled without a task, nor when the part is omitted.
        let none = compile_now(&ws, None, None, 25_600, PartSelection::All).unwrap();
        assert!(none.relevant.is_empty());
        assert!(none.why.is_empty());
        let required = compile_now(&ws, None, None, 25_600, PartSelection::Required).unwrap();
        let tight = compile_now(
            &ws,
            Some("dup"),
            None,
            required.bytes + 1,
            PartSelection::All,
        )
        .unwrap();
        assert!(tight
            .omitted
            .iter()
            .any(|(part, _)| *part == Part::Relevant));
        assert!(tight.relevant.is_empty());
        assert!(tight.why.is_empty());
    }

    #[test]
    fn relevant_keeps_five_ties_by_date_then_path() {
        let mut notes = vec![("Entry.md".to_owned(), "# Entry\n\nStart.\n".to_owned())];
        // Notes without the word, so that it is not in every note.
        for i in 0..20 {
            notes.push((format!("x{i}.md"), format!("# Other {i}\n")));
        }
        // Seven notes match "webhook" once; dates decide, then paths.
        for (name, updated) in [
            ("g.md", None),
            ("f.md", Some("2026-01-01")),
            ("e.md", Some("2026-05-01")),
            ("d.md", Some("2026-05-01")),
            ("c.md", Some("2026-09-01")),
            ("b.md", None),
            ("a.md", Some("2025-01-01")),
        ] {
            let frontmatter =
                updated.map_or(String::new(), |d| format!("---\nupdated: {d}\n---\n"));
            let text = format!("{frontmatter}# Webhook {name}\n\nAbout {name}.\n");
            notes.push((name.to_owned(), text));
        }
        let refs: Vec<(&str, &str)> = notes
            .iter()
            .map(|(p, t)| (p.as_str(), t.as_str()))
            .collect();
        let (_dir, ws) = workspace(&refs);
        let compiled =
            compile_now(&ws, Some("webhook"), None, 25_600, PartSelection::Optional).unwrap();
        assert_eq!(
            part(&compiled, Part::Relevant),
            Some(
                "relevant:\n- Webhook c.md: About c.md.\n- Webhook d.md: About d.md.\n\
                 - Webhook e.md: About e.md.\n- Webhook f.md: About f.md.\n\
                 - Webhook a.md: About a.md."
            )
        );
    }

    #[test]
    fn warnings_follow_reports_and_dream_state() {
        let (_dir, ws) = workspace(&[("Entry.md", ENTRY)]);
        let warning = |ws: &Workspace| {
            let compiled = compile_now(ws, None, None, 25_600, PartSelection::Required).unwrap();
            part(&compiled, Part::Warning).map(str::to_owned)
        };
        // Before any dreaming report exists: no warning, even without state.
        assert_eq!(warning(&ws), None);
        write_hippo(&ws, "reports/2026-11-01-eval.md", "an eval report\n");
        assert_eq!(warning(&ws), None);

        // A report exists but no state, or state without last_success.
        write_hippo(&ws, "reports/2026-11-06-dream.md", "status: ok\n");
        assert_eq!(
            warning(&ws).as_deref(),
            Some("warning: no dreaming run recorded")
        );
        write_hippo(&ws, "dream-state.json", "{\"protocol\":\"HIPPO_V1\"}\n");
        assert_eq!(
            warning(&ws).as_deref(),
            Some("warning: no dreaming run recorded")
        );

        // last_success 36 hours old or less is fine; older is stale.
        let state = |at: &str| format!("{{\"protocol\":\"HIPPO_V1\",\"last_success\":\"{at}\"}}\n");
        write_hippo(&ws, "dream-state.json", &state("2026-11-07T00:00:00Z"));
        assert_eq!(warning(&ws), None);
        write_hippo(&ws, "dream-state.json", &state("2026-11-06T23:59:59Z"));
        assert_eq!(
            warning(&ws).as_deref(),
            Some("warning: last dreaming success 2026-11-06T23:59:59Z")
        );

        // The newest dream report failed: that line comes first.
        write_hippo(
            &ws,
            "reports/2026-11-08-dream.md",
            "status: failed\nerror: model\n",
        );
        assert_eq!(
            warning(&ws).as_deref(),
            Some(
                "warning: last dreaming run failed\n\
                 warning: last dreaming success 2026-11-06T23:59:59Z"
            )
        );
        write_hippo(&ws, "dream-state.json", &state("2026-11-08T03:35:00Z"));
        assert_eq!(
            warning(&ws).as_deref(),
            Some("warning: last dreaming run failed")
        );
    }

    #[test]
    fn an_optional_part_that_does_not_fit_is_omitted() {
        let (_dir, ws) = workspace(&[
            ("Entry.md", ENTRY),
            ("Webhooks.md", "# Webhooks\n\nRetries back off.\n"),
        ]);
        let all = compile_now(&ws, Some("webhooks"), None, 25_600, PartSelection::All).unwrap();
        let relevant = part(&all, Part::Relevant).unwrap().len();
        let linter = part(&all, Part::Linter).unwrap().len();

        // A budget that fits the required parts and the linter line but not
        // the relevant notes as well: relevant is omitted, linter still fits.
        let required = compile_now(&ws, None, None, 25_600, PartSelection::Required).unwrap();
        let budget = required.bytes + 2 + linter + 1;
        let tight = compile_now(&ws, Some("webhooks"), None, budget, PartSelection::All).unwrap();
        assert_eq!(tight.omitted, [(Part::Relevant, relevant)]);
        assert_eq!(
            tight.omitted_lines(),
            [format!("omitted: relevant ({relevant})")]
        );
        assert!(part(&tight, Part::Linter).is_some());
        assert!(tight.bytes <= budget);
    }

    #[test]
    fn required_parts_over_budget_fail() {
        let (_dir, ws) = workspace(&[("Entry.md", ENTRY)]);
        let required = compile_now(&ws, None, None, 25_600, PartSelection::Required).unwrap();
        let entry = part(&required, Part::Entry).unwrap();
        assert_eq!(
            entry,
            "# Start\n\nRead the protocol first.\nThen the open questions."
        );
        // The entry section alone is over a budget one byte short of it.
        let budget = required.bytes - 1;
        match compile_now(&ws, None, None, budget, PartSelection::All) {
            Err(Error::OverBudget {
                required_bytes,
                budget: b,
            }) => {
                assert_eq!(required_bytes, required.bytes);
                assert_eq!(b, budget);
            }
            other => panic!("expected OverBudget, got {other:?}"),
        }
        // The optional parts alone never fail.
        let (_dir, ws) = workspace(&[("Entry.md", ENTRY), ("Launch.md", "# Launch\n")]);
        let optional = compile_now(&ws, Some("launch"), None, 10, PartSelection::Optional).unwrap();
        assert!(optional.parts.is_empty());
        assert_eq!(optional.omitted.len(), 2);
    }

    #[test]
    fn required_lists_the_normalised_required_paths() {
        let (_dir, mut ws) =
            workspace(&[("Entry.md", ENTRY), ("A.md", "# A\n"), ("B.md", "# B\n")]);
        ws.notes_dir = "./brain".into();
        ws.required_notes = vec!["./brain/B.md".into(), "brain/A.md".into()];
        let compiled = compile_now(&ws, None, None, 25_600, PartSelection::All).unwrap();
        assert_eq!(
            compiled.required,
            [PathBuf::from("brain/B.md"), PathBuf::from("brain/A.md")]
        );
        let optional = compile_now(&ws, None, None, 25_600, PartSelection::Optional).unwrap();
        assert!(optional.required.is_empty());
    }

    #[test]
    fn relevant_paths_are_normalised_for_a_dot_notes_directory() {
        let (dir, mut ws) = workspace(&[("Entry.md", ENTRY), ("Webhook.md", "# Webhook\n")]);
        // Move the notes up one level so that the notes directory is `.`.
        let root = dir.path();
        fs::rename(root.join("brain/Entry.md"), root.join("Entry.md")).unwrap();
        fs::rename(root.join("brain/Webhook.md"), root.join("Webhook.md")).unwrap();
        ws.notes_dir = ".".into();
        ws.entry_note = "Entry.md".into();
        let compiled =
            compile_now(&ws, Some("webhook"), None, 25_600, PartSelection::Optional).unwrap();
        assert_eq!(compiled.relevant, [PathBuf::from("Webhook.md")]);
    }

    #[test]
    fn warnings_are_public_and_empty_without_a_report() {
        let (_dir, ws) = workspace(&[("Entry.md", ENTRY)]);
        assert!(warnings(&ws, OffsetDateTime::now_utc()).unwrap().is_empty());
    }
}
