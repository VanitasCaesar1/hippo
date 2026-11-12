//! First open: what the onboarding screens say.
//!
//! A new user sees, in order: the welcome (the one sentence), what hippo
//! found on the Mac ([`found`], or [`empty`] when nothing), the first read
//! ([`reading`] from `hippo ingest --progress`, with [`so_far`] from the
//! rollup), the first home, and step two, a notes folder ([`inspect_folder`]
//! and [`step_two_commands`]). Every value here is read-only; the app runs the
//! commands it shows.

use std::path::{Path, PathBuf};

use hippo_core::detect::{Detected, Dir};
use hippo_core::ingest::Progress;
use hippo_core::rollup::Rollup;
use time::{Date, OffsetDateTime, UtcOffset};

use crate::edition::thousands;

/// The headline of the welcome screen and the first half of the sentence.
pub const HEADLINE: &str = "One brain for all your coding agents.";
/// The line under it.
pub const LINE: &str =
    "hippo reads everything they did, keeps your notes true, and briefs every new session.";
/// The three jobs, as the welcome screen lists them.
pub const JOBS: [(&str, &str); 3] = [
    (
        "Reads",
        "Every Claude Code, Codex, DeepSeek Harness and OpenCode session on this Mac, from every profile, in one place you can search.",
    ),
    (
        "Keeps true",
        "Each night it checks your notes against what the agents actually did and fixes what is wrong. A fresh reviewer signs every fix; any fix can be undone.",
    ),
    (
        "Briefs",
        "Each new session starts with the notes its task needs, within a hard budget.",
    ),
];
/// The steps along the foot of the onboarding pages.
pub const STEPS: [&str; 4] = [
    "1 Find history",
    "2 Read it",
    "3 Your week",
    "4 Give your agents a memory (optional)",
];

/// How fast a first read goes, for the estimate: about 18 MB of transcript
/// a second on an Apple-silicon Mac.
const BYTES_PER_SECOND: u64 = 18_000_000;

/// One row of the table of what was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundRow {
    pub source: String,
    /// A profile under its tool, drawn indented.
    pub indent: bool,
    pub place: String,
    pub sessions: String,
    pub disk: String,
    pub latest: String,
    /// A folder with nothing to read: drawn dim.
    pub empty: bool,
    /// The one folder the row stands for, `~` kept, when it is one with
    /// something to read: Connect's switch for it adds or removes it from
    /// `exclude_roots`.
    pub dir: Option<String>,
}

/// The Found screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// `Claude Code on 5 profiles, and Codex: 5,247 sessions since 7 October.`
    pub headline: String,
    pub rows: Vec<FoundRow>,
    /// Tools present with no adapter: their names and folders.
    pub unsupported: Option<(String, String)>,
    pub total: FoundRow,
    pub files: u64,
    /// `About 20 minutes for this much. You can use the app while it reads.`
    pub estimate: String,
}

/// `1.2 GB`, `340 MB`, `12 KB`, in powers of 1,000.
pub fn bytes(n: u64) -> String {
    const UNITS: [(&str, u64); 3] = [("GB", 1_000_000_000), ("MB", 1_000_000), ("KB", 1_000)];
    for (unit, size) in UNITS {
        if n >= size {
            return format!("{:.1} {unit}", n as f64 / size as f64);
        }
    }
    format!("{n} B")
}

/// When something last happened, as a person says it: `just now`, `24
/// minutes ago`, `02:41` today, `Sun 13:15` this week, `Tue 3 Nov` before.
pub fn when(t: OffsetDateTime, now: OffsetDateTime, offset: UtcOffset) -> String {
    let age = now - t;
    if age < time::Duration::minutes(1) {
        return "just now".into();
    }
    if age < time::Duration::hours(1) {
        let m = age.whole_minutes();
        return format!("{m} minute{} ago", if m == 1 { "" } else { "s" });
    }
    let local = t.to_offset(offset);
    let today = now.to_offset(offset).date();
    let hm = format!("{:02}:{:02}", local.hour(), local.minute());
    if local.date() == today {
        hm
    } else if today - local.date() < time::Duration::days(6) {
        format!("{} {hm}", weekday(local.date()))
    } else {
        short_date(local.date())
    }
}

fn weekday(d: Date) -> &'static str {
    use time::Weekday::*;
    match d.weekday() {
        Monday => "Mon",
        Tuesday => "Tue",
        Wednesday => "Wed",
        Thursday => "Thu",
        Friday => "Fri",
        Saturday => "Sat",
        Sunday => "Sun",
    }
}

fn month(d: Date) -> &'static str {
    [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ][usize::from(u8::from(d.month())) - 1]
}

fn long_month(d: Date) -> String {
    d.month().to_string()
}

/// `Tue 3 Nov`.
pub fn short_date(d: Date) -> String {
    format!("{} {} {}", weekday(d), d.day(), month(d))
}

/// A Claude Code profile directory as people name it: `~/.claude-work`
/// for `~/.claude-work/projects`.
fn profile_place(dir: &Dir) -> String {
    dir.display
        .strip_suffix("/projects")
        .unwrap_or(&dir.display)
        .to_owned()
}

fn row_of(
    source: String,
    indent: bool,
    place: String,
    dirs: &[&Dir],
    now: OffsetDateTime,
    offset: UtcOffset,
) -> FoundRow {
    let files: u64 = dirs.iter().map(|d| d.files).sum();
    let size: u64 = dirs.iter().map(|d| d.bytes).sum();
    let newest = dirs.iter().filter_map(|d| d.newest).max();
    FoundRow {
        source,
        indent,
        place,
        sessions: thousands(files),
        disk: if files == 0 {
            "—".into()
        } else {
            bytes(size)
        },
        latest: match newest {
            Some(t) if files > 0 => when(t, now, offset),
            _ => "empty, skipped".into(),
        },
        empty: files == 0,
        dir: (dirs.len() == 1 && files > 0).then(|| dirs[0].display.clone()),
    }
}

/// The Found screen for what [`hippo_core::detect::detect`] returned.
pub fn found(d: &Detected, now: OffsetDateTime, offset: UtcOffset) -> Found {
    let mut rows = Vec::new();
    let mut claude_profiles = 0;
    let mut codex_files = 0;
    let mut dsh_files = 0;
    let mut opencode_files = 0;
    let mut muse_files = 0;
    for root in &d.roots {
        let dirs: Vec<&Dir> = root.dirs.iter().collect();
        if dirs.is_empty() {
            continue;
        }
        let tool = dirs[0].tool;
        let name = match tool {
            "codex" => "Codex",
            "dsh" => "dsh",
            "opencode" => "OpenCode",
            "muse" => "Muse Code",
            _ => "Claude Code",
        };
        if tool == "codex" {
            codex_files += dirs.iter().map(|d| d.files).sum::<u64>();
        } else if tool == "dsh" {
            dsh_files += dirs.iter().map(|d| d.files).sum::<u64>();
        } else if tool == "opencode" {
            opencode_files += dirs.iter().map(|d| d.files).sum::<u64>();
        } else if tool == "muse" {
            muse_files += dirs.iter().map(|d| d.files).sum::<u64>();
        } else {
            claude_profiles += dirs.iter().filter(|d| d.files > 0).count();
        }
        rows.push(row_of(
            name.into(),
            false,
            root.pattern.clone(),
            &dirs,
            now,
            offset,
        ));
        if tool == "claude-code" && dirs.len() > 1 {
            let mut sorted = dirs.clone();
            sorted.sort_by(|a, b| b.files.cmp(&a.files).then(a.path.cmp(&b.path)));
            for dir in sorted {
                let profile = dir.profile.clone().unwrap_or_else(|| profile_place(dir));
                rows.push(row_of(
                    profile,
                    true,
                    profile_place(dir),
                    &[dir],
                    now,
                    offset,
                ));
            }
        }
    }
    let mut names: Vec<&str> = Vec::new();
    let mut places: Vec<&str> = Vec::new();
    for u in d.unsupported.iter().filter(|u| u.present) {
        // One folder per tool: the first that is there.
        if !names.contains(&u.name) {
            names.push(u.name);
            places.push(u.path);
        }
    }
    let unsupported = (!names.is_empty()).then(|| (names.join(", "), places.join("  ")));
    let all: Vec<&Dir> = d.dirs().collect();
    let files = d.files();
    let span = d.span();
    let mut total = row_of("Everything".into(), false, String::new(), &all, now, offset);
    total.dir = None;
    if let Some((oldest, newest)) = span {
        let (o, n) = (
            oldest.to_offset(offset).date(),
            newest.to_offset(offset).date(),
        );
        let end = if n == now.to_offset(offset).date() {
            "today".to_owned()
        } else {
            format!("{} {}", n.day(), month(n))
        };
        total.latest = format!("{} {} → {end}", o.day(), month(o));
    }
    let others: Vec<&str> = [
        ("Codex", codex_files),
        ("dsh", dsh_files),
        ("OpenCode", opencode_files),
        ("Muse Code", muse_files),
    ]
    .into_iter()
    .filter(|(_, files)| *files > 0)
    .map(|(name, _)| name)
    .collect();
    let tools = match (claude_profiles, others.as_slice()) {
        (0, []) => "Your agents".to_owned(),
        (0, others) => crate::text::and_list(others),
        (1, others) => crate::text::and_list(&[&["Claude Code"], others].concat()),
        (n, []) => format!("Claude Code on {n} profiles"),
        (n, [one]) => format!("Claude Code on {n} profiles, and {one}"),
        (n, others) => format!(
            "Claude Code on {n} profiles, {}",
            crate::text::and_list(others)
        ),
    };
    let since = span
        .map(|(o, _)| {
            let o = o.to_offset(offset).date();
            format!(" since {} {}", o.day(), long_month(o))
        })
        .unwrap_or_default();
    let headline = format!(
        "{tools}: {} session{}{since}.",
        thousands(files),
        if files == 1 { "" } else { "s" }
    );
    Found {
        headline,
        rows,
        unsupported,
        total,
        files,
        estimate: estimate(d.bytes()),
    }
}

/// How long a first read of `bytes` takes, as a sentence.
pub fn estimate(bytes: u64) -> String {
    let minutes = bytes / BYTES_PER_SECOND / 60;
    let time = match minutes {
        0 => "Under a minute".to_owned(),
        1 => "About a minute".to_owned(),
        m => format!("About {m} minutes"),
    };
    format!("{time} for this much. You can use the app while it reads.")
}

/// A row of the Empty screen's "where it looked".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Looked {
    pub source: String,
    pub place: String,
    pub state: String,
}

/// Where hippo looked when it found nothing.
pub fn empty(d: &Detected) -> Vec<Looked> {
    let mut rows: Vec<Looked> = d
        .roots
        .iter()
        .map(|root| {
            let source = if root.pattern.contains(".codex") {
                "Codex"
            } else if root.pattern.contains(".dsh") {
                "dsh"
            } else if root.pattern.contains("opencode") {
                "OpenCode"
            } else if root.pattern.contains("muse") {
                "Muse Code"
            } else {
                "Claude Code"
            };
            Looked {
                source: source.into(),
                place: root.pattern.clone(),
                state: match root.dirs.len() {
                    0 => "no folder".into(),
                    1 => "an empty folder".into(),
                    n => format!("{n} empty folders"),
                },
            }
        })
        .collect();
    let mut names: Vec<&str> = Vec::new();
    for u in &d.unsupported {
        if !names.contains(&u.name) {
            names.push(u.name);
        }
    }
    let present = d.unsupported.iter().any(|u| u.present);
    rows.push(Looked {
        source: names.join(", "),
        place: d
            .unsupported
            .iter()
            .map(|u| u.path)
            .collect::<Vec<_>>()
            .join(" · "),
        state: if present {
            "installed, not read yet".into()
        } else {
            "not supported yet".into()
        },
    });
    rows
}

/// The Reading screen, after one [`Progress`] line.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    /// `Reading your agents' history. About 8 minutes left.`
    pub headline: String,
    /// 0 to 1.
    pub fraction: f32,
    /// `3,244 of 5,247 sessions`.
    pub count: String,
    /// `now at Sat 31 Oct, in ~/.claude-work`.
    pub place: String,
    /// `13.1 of 21.6 GB · 12 min elapsed`.
    pub size: String,
    /// The status bar's `ingest 62% · 3,244 of 5,247`.
    pub status: String,
    pub done: bool,
}

/// The Reading screen for `p`, `elapsed` after the read started.
pub fn reading(
    p: &Progress,
    elapsed: time::Duration,
    offset: UtcOffset,
    user_home: Option<&Path>,
) -> Reading {
    let fraction = if p.bytes_total > 0 {
        p.bytes_done as f32 / p.bytes_total as f32
    } else if p.files_total > 0 {
        p.files_done as f32 / p.files_total as f32
    } else {
        1.0
    };
    let done = p.files_done >= p.files_total;
    let secs = elapsed.whole_seconds().max(0) as u64;
    let left = if done {
        None
    } else if secs >= 3 && p.bytes_done > 0 {
        let rate = p.bytes_done as f64 / secs as f64;
        Some(((p.bytes_total - p.bytes_done) as f64 / rate / 60.0).round() as u64)
    } else {
        None
    };
    let headline = match (done, left) {
        (true, _) => "Read your agents' history.".to_owned(),
        (false, Some(0)) => "Reading your agents' history. Under a minute left.".to_owned(),
        (false, Some(1)) => "Reading your agents' history. About a minute left.".to_owned(),
        (false, Some(m)) => format!("Reading your agents' history. About {m} minutes left."),
        (false, None) => "Reading your agents' history.".to_owned(),
    };
    let profile = profile_dir(&p.path, user_home);
    let day = p.modified.map(|t| short_date(t.to_offset(offset).date()));
    let place = match (day, profile) {
        (Some(d), Some(dir)) => format!("now at {d}, in {dir}"),
        (Some(d), None) => format!("now at {d}"),
        (None, Some(dir)) => format!("now in {dir}"),
        (None, None) => String::new(),
    };
    let minutes = secs / 60;
    let elapsed_text = if minutes == 0 {
        format!("{secs} s elapsed")
    } else {
        format!("{minutes} min elapsed")
    };
    Reading {
        headline,
        fraction: fraction.clamp(0.0, 1.0),
        count: format!(
            "{} of {} sessions",
            thousands(p.files_done),
            thousands(p.files_total)
        ),
        place,
        size: format!(
            "{} of {} · {elapsed_text}",
            bytes(p.bytes_done),
            bytes(p.bytes_total)
        ),
        status: format!(
            "ingest {}% · {} of {}",
            (fraction * 100.0).floor() as u32,
            thousands(p.files_done),
            thousands(p.files_total)
        ),
        done,
    }
}

/// The agent folder a transcript is under, `~` for the home directory:
/// `~/.claude-work` or `~/.codex`.
fn profile_dir(path: &Path, user_home: Option<&Path>) -> Option<String> {
    let mut dir = PathBuf::new();
    for c in path.components() {
        dir.push(c);
        let name = c.as_os_str().to_string_lossy();
        if name.starts_with(".claude") || name == ".codex" {
            return Some(match user_home.and_then(|h| dir.strip_prefix(h).ok()) {
                Some(rest) => format!("~/{}", rest.display()),
                None => dir.display().to_string(),
            });
        }
    }
    None
}

/// What the read has found so far, from the rollup.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SoFar {
    /// `(number, label)`, eight of them, in reading order.
    pub stats: Vec<(String, String)>,
    /// Top-level sessions a person started.
    pub threads: usize,
}

/// The eight numbers of the Reading screen. `unread` is the files the index
/// could not parse.
pub fn so_far(
    rollup: &Rollup,
    unread: usize,
    offset: UtcOffset,
) -> hippo_core::error::Result<SoFar> {
    let summary = rollup.summary()?;
    let days = rollup.days(offset)?;
    let (agents, turns, edits, pulls) = days.iter().fold((0, 0, 0, 0), |(a, t, e, p), d| {
        (a + d.agents, t + d.turns, e + d.edits, p + d.pulls)
    });
    let dated: Vec<Date> = days.iter().filter_map(|d| d.date).collect();
    let span = match dated.iter().min() {
        Some(a) => format!("threads since {} {}", a.day(), month(*a)),
        None => "threads".to_owned(),
    };
    let stats = vec![
        (thousands(summary.sessions as u64), span),
        (thousands(agents as u64), "sub-agents they ran".into()),
        (thousands(turns), "turns indexed".into()),
        (thousands(edits), "file edits".into()),
        (thousands(pulls as u64), "pull requests named".into()),
        (thousands(dated.len() as u64), "days with activity".into()),
        (
            thousands(summary.profiles.len() as u64),
            "profiles seen".into(),
        ),
        (thousands(unread as u64), "files it could not read".into()),
    ];
    Ok(SoFar {
        stats,
        threads: summary.sessions,
    })
}

/// A folder picked as the notes folder, and what is in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotesFolder {
    /// The folder itself.
    pub path: PathBuf,
    /// Its parent: the workspace `hippo init` names.
    pub workspace: PathBuf,
    /// Its name: `--notes-dir`.
    pub notes_dir: String,
    /// The entry note relative to the workspace: `--entry-note`.
    pub entry_note: Option<String>,
    /// Markdown notes under it, and the folders they sit in.
    pub notes: usize,
    pub folders: usize,
    pub exists: bool,
}

/// Entry note names, in the order they are preferred.
const ENTRY_NAMES: [&str; 5] = [
    "START HERE.md",
    "README.md",
    "index.md",
    "Home.md",
    "Index.md",
];

/// Reads what a folder holds: its Markdown notes (hidden entries and
/// `.hippo/` skipped) and the note that should open the context block.
/// Reads directory entries only.
pub fn inspect_folder(path: &Path) -> NotesFolder {
    let workspace = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let notes_dir = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut notes = 0;
    let mut folders = std::collections::BTreeSet::new();
    let mut top: Vec<String> = Vec::new();
    let walk = walkdir::WalkDir::new(path)
        .max_depth(8)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.'));
    for entry in walk.flatten() {
        if entry.file_type().is_file() && entry.path().extension().is_some_and(|e| e == "md") {
            notes += 1;
            if let Some(parent) = entry.path().parent() {
                folders.insert(parent.to_path_buf());
            }
            if entry.depth() == 1 {
                top.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
    }
    top.sort();
    let entry = ENTRY_NAMES
        .iter()
        .find(|n| top.iter().any(|t| t == *n))
        .map(|n| (*n).to_owned())
        .or_else(|| top.first().cloned());
    NotesFolder {
        exists: path.is_dir(),
        path: path.to_path_buf(),
        entry_note: entry.map(|e| format!("{notes_dir}/{e}")),
        workspace,
        notes_dir,
        notes,
        folders: folders.len(),
    }
}

/// Folder names that hold notes, compared without case (APFS ignores
/// case, so `Notes` and `notes` are one folder).
const NOTES_NAMES: [&str; 4] = ["brain", "notes", "wiki", "memory"];

/// The folders directly in `dir` named like a notes folder, by their
/// names on disk, and `docs/notes`.
fn notes_children(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| {
                    let name = e.file_name().to_string_lossy().to_lowercase();
                    NOTES_NAMES.contains(&name.as_str())
                })
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    let docs = dir.join("docs").join("notes");
    if docs.is_dir() {
        out.push(docs);
    }
    out
}

/// A notes folder to suggest for step two: the folder named like a notes
/// folder (`brain`, `notes`, `wiki`, …) with Markdown notes at its top,
/// found in or above the directories the agents worked in (up to three
/// levels up), the one most sessions were under first. Reads directory
/// entries only.
pub fn suggest_folder(cwds: &[PathBuf]) -> Option<PathBuf> {
    let mut hits: Vec<(PathBuf, usize)> = Vec::new();
    let mut checked: std::collections::HashMap<PathBuf, bool> = std::collections::HashMap::new();
    for cwd in cwds {
        let mut seen_for_this = Vec::new();
        for dir in cwd.ancestors().take(4) {
            for candidate in notes_children(dir) {
                let ok = *checked
                    .entry(candidate.clone())
                    .or_insert_with(|| has_top_notes(&candidate));
                if ok && !seen_for_this.contains(&candidate) {
                    seen_for_this.push(candidate.clone());
                    match hits.iter_mut().find(|(p, _)| *p == candidate) {
                        Some((_, n)) => *n += 1,
                        None => hits.push((candidate, 1)),
                    }
                }
            }
        }
    }
    hits.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    hits.into_iter().next().map(|(p, _)| p)
}

fn has_top_notes(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|entries| {
        entries
            .flatten()
            .any(|e| e.path().extension().is_some_and(|x| x == "md"))
    })
}

/// The second step's commands for `folder`, as `hippo` arguments, in the order
/// they run: `init`, then `schedule install` when dreaming is on, then `hooks
/// install` when the briefing is on. `None` without an entry note.
pub fn step_two_commands(
    folder: &NotesFolder,
    dreaming: bool,
    briefing: bool,
) -> Option<Vec<Vec<String>>> {
    let entry = folder.entry_note.clone()?;
    let ws = folder.workspace.to_string_lossy().into_owned();
    let mut out = vec![vec![
        "init".to_owned(),
        "--workspace".to_owned(),
        ws.clone(),
        "--notes-dir".to_owned(),
        folder.notes_dir.clone(),
        "--entry-note".to_owned(),
        entry,
    ]];
    if dreaming {
        out.push(vec!["schedule".into(), "install".into()]);
    }
    if briefing {
        out.push(vec![
            "hooks".into(),
            "install".into(),
            "--workspace".into(),
            ws,
        ]);
    }
    Some(out)
}

/// `hippo` and its arguments as a shell would print them, quoting an
/// argument with a space or a quote.
pub fn shell_line(args: &[String]) -> String {
    let mut out = String::from("hippo");
    for a in args {
        out.push(' ');
        if a.is_empty()
            || a.chars()
                .any(|c| c.is_whitespace() || "\"'$`\\".contains(c))
        {
            out.push('"');
            for c in a.chars() {
                if "\"$`\\".contains(c) {
                    out.push('\\');
                }
                out.push(c);
            }
            out.push('"');
        } else {
            out.push_str(a);
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // temporary folders
mod tests {
    use super::*;
    use hippo_core::detect::{Root, Unsupported};
    use time::macros::datetime;

    fn dir(
        display: &str,
        profile: Option<&str>,
        tool: &'static str,
        files: u64,
        newest: Option<OffsetDateTime>,
    ) -> Dir {
        Dir {
            path: PathBuf::from(display.replace('~', "/u")),
            display: display.into(),
            tool,
            profile: profile.map(str::to_owned),
            files,
            bytes: files * 1_000_000,
            newest,
            oldest: newest.map(|t| t - time::Duration::days(30)),
            excluded: false,
        }
    }

    #[test]
    fn found_names_profiles_counts_and_the_span() {
        let now = datetime!(2026-11-09 02:50 UTC);
        let d = Detected {
            roots: vec![
                Root {
                    pattern: "~/.claude*/projects".into(),
                    dirs: vec![
                        dir(
                            "~/.claude/projects",
                            Some("default"),
                            "claude-code",
                            10,
                            Some(now - time::Duration::minutes(24)),
                        ),
                        dir(
                            "~/.claude-empty/projects",
                            Some("empty"),
                            "claude-code",
                            0,
                            None,
                        ),
                        dir(
                            "~/.claude-work/projects",
                            Some("work"),
                            "claude-code",
                            20,
                            Some(datetime!(2026-11-08 15:07 UTC)),
                        ),
                    ],
                },
                Root {
                    pattern: "~/.codex/sessions".into(),
                    dirs: vec![dir(
                        "~/.codex/sessions",
                        None,
                        "codex",
                        3,
                        Some(datetime!(2026-11-01 10:00 UTC)),
                    )],
                },
            ],
            unsupported: vec![
                Unsupported {
                    name: "Cursor",
                    path: "~/.cursor",
                    present: true,
                },
                Unsupported {
                    name: "Gemini",
                    path: "~/.gemini",
                    present: false,
                },
            ],
        };
        let f = found(&d, now, UtcOffset::UTC);
        assert_eq!(
            f.headline,
            "Claude Code on 2 profiles, and Codex: 33 sessions since 2 October."
        );
        let names: Vec<&str> = f.rows.iter().map(|r| r.source.as_str()).collect();
        assert_eq!(names, ["Claude Code", "work", "default", "empty", "Codex"]);
        assert_eq!(f.rows[1].place, "~/.claude-work");
        assert_eq!(f.rows[1].latest, "Sun 15:07");
        assert_eq!(f.rows[2].latest, "24 minutes ago");
        assert_eq!(f.rows[3].latest, "empty, skipped");
        assert!(f.rows[3].empty);
        assert_eq!(f.rows[4].latest, "Sun 1 Nov");
        assert_eq!(f.unsupported, Some(("Cursor".into(), "~/.cursor".into())));
        assert_eq!(f.total.sessions, "33");
        assert_eq!(f.total.latest, "2 Oct → today");
    }

    #[test]
    fn reading_says_how_far_and_how_long() {
        let p = Progress {
            files_done: 3244,
            files_total: 5247,
            bytes_done: 13_100_000_000,
            bytes_total: 21_600_000_000,
            indexed: 3244,
            skipped: 0,
            path: "/u/.claude-work/projects/-w/x.jsonl".into(),
            modified: Some(datetime!(2026-10-31 12:00 UTC)),
            rollup: false,
        };
        let r = reading(
            &p,
            time::Duration::minutes(12),
            UtcOffset::UTC,
            Some(Path::new("/u")),
        );
        assert_eq!(r.count, "3,244 of 5,247 sessions");
        assert_eq!(r.place, "now at Sat 31 Oct, in ~/.claude-work");
        assert_eq!(r.size, "13.1 GB of 21.6 GB · 12 min elapsed");
        assert_eq!(r.status, "ingest 60% · 3,244 of 5,247");
        assert_eq!(
            r.headline,
            "Reading your agents' history. About 8 minutes left."
        );
    }

    #[test]
    fn a_notes_folder_gives_its_entry_note_and_the_commands() {
        let dir = tempfile::tempdir().unwrap();
        let brain = dir.path().join("brain");
        std::fs::create_dir_all(brain.join("Ops")).unwrap();
        std::fs::create_dir_all(brain.join(".obsidian")).unwrap();
        for f in ["START HERE.md", "a.md", "Ops/b.md", ".obsidian/c.md"] {
            std::fs::write(brain.join(f), "x").unwrap();
        }
        let f = inspect_folder(&brain);
        assert_eq!((f.notes, f.folders), (3, 2));
        assert_eq!(f.entry_note.as_deref(), Some("brain/START HERE.md"));
        let cmds = step_two_commands(&f, true, false).unwrap();
        assert_eq!(cmds.len(), 2);
        assert_eq!(
            shell_line(&cmds[0][3..]),
            "hippo --notes-dir brain --entry-note \"brain/START HERE.md\""
        );
        let work = dir.path().join("app/src");
        std::fs::create_dir_all(&work).unwrap();
        assert_eq!(suggest_folder(&[work.clone(), work]), Some(brain.clone()));
        let empty = inspect_folder(&dir.path().join("none"));
        assert!(!empty.exists);
        assert_eq!(step_two_commands(&empty, true, true), None);
    }
}
