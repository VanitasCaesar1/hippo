//! The app's preferences: the theme in use, whether Ask writes answers, and
//! the menu-bar, notification and digest settings. This is the only module of the
//! app that writes (the app writes
//! only its preferences file), so it alone opts out of clippy's
//! `disallowed-methods` list in `app/clippy.toml`.
//!
//! The file is `{"theme": "<theme id>", "ask_answers": false}` at
//! `~/Library/Application Support/dev.hippocamp.app/preferences.json`, written
//! through a temporary file renamed over it, the same steps as `hippocamp-core`'s
//! private `util::write_atomic`, which the app cannot call.

#![allow(clippy::disallowed_methods)] // the preferences file is the one thing the app writes

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The environment variable that moves the preferences file, read once at
/// startup (tests and trial runs use it so as not to touch the real one).
pub const PATH_ENV: &str = "HIPPOCAMP_APP_PREFS";

/// The id that means "follow the system's appearance".
pub const SYSTEM: &str = "system";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prefs {
    /// A theme id: `system`, `builtin:<name>`, `user:<file>/<name>` or
    /// `pack:<pack>/<name>`.
    #[serde(default = "system")]
    pub theme: String,
    /// Ask adds a written answer, one model call, to its sources. A file
    /// without the key reads as on; a file from before reads its old keys
    /// (the pinned sessions of an earlier Sessions list) as nothing.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub ask_answers: bool,
    /// The last questions asked, newest first, at most
    /// [`RECENT_QUESTIONS`]: the Ask bar lists them on focus.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_questions: Vec<String>,
    /// The menu-bar item; on unless switched off.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub menubar: bool,
    /// Notifications by kind: an agent waiting, stopped at a
    /// limit, its limit reset with the session still to resume.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub notify_waiting: bool,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub notify_stopped: bool,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub notify_reset: bool,
    /// The local date (`2026-11-10`) the morning digest was dismissed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest_dismissed: Option<String>,
    /// When Home or the Inbox was last on screen, Unix seconds:
    /// the digest counts what is newer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inbox_seen_at: Option<i64>,
    /// The terminal Resume and Brief's starts use: `ghostty` or
    /// `terminal`; unset is Ghostty when it is installed, else Terminal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<String>,
    /// Sessions the person opened in Hippocamp (an x-ray, Open thread or
    /// Resume), by id, with when (Unix seconds), for the sidebar's Settled
    /// rule. Kept two days.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub opened_sessions: std::collections::BTreeMap<String, i64>,
    /// The sidebar's width in points, when dragged from its default
    /// (as T3 Code keeps it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar_width: Option<u32>,
    /// Sessions the person settled or un-settled by hand from the sidebar
    /// (T3 Code's Settle and Un-settle), by id: `true` settled,
    /// `false` un-settled, with when. Kept two days, as `opened_sessions`.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub settled_sessions: std::collections::BTreeMap<String, Mark>,
    /// Sessions snoozed from the sidebar (T3 Code's Snooze), by
    /// id: hidden from the shelves until `until`, Unix seconds. Dropped
    /// once `until` has passed or the session needs the person.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub snoozed_sessions: std::collections::BTreeMap<String, Snooze>,
    /// When the Incidents page was last opened, Unix seconds:
    /// the sidebar counts the incidents newer than it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incidents_seen_at: Option<i64>,
    /// Incidents marked "Not an incident", by id, with when
    /// (Unix seconds). Kept 30 days.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub dismissed_incidents: std::collections::BTreeMap<String, i64>,
}

/// A hand-made settle or un-settle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mark {
    /// `true` settled, `false` un-settled.
    pub settled: bool,
    /// When, Unix seconds.
    pub at: i64,
}

/// A snooze: hidden until `until`; `at` is when it was set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snooze {
    pub until: i64,
    pub at: i64,
}

/// `marks` with `id` settled (or un-settled) at `now`, entries older than
/// [`OPENED_FOR`] dropped.
pub fn mark_settled(
    marks: &std::collections::BTreeMap<String, Mark>,
    id: &str,
    settled: bool,
    now: i64,
) -> std::collections::BTreeMap<String, Mark> {
    let mut out: std::collections::BTreeMap<String, Mark> = marks
        .iter()
        .filter(|(_, m)| now - m.at <= OPENED_FOR)
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    out.insert(id.to_owned(), Mark { settled, at: now });
    out
}

/// `snoozes` with `id` snoozed until `until` (or woken, with `None`),
/// those already past dropped.
pub fn mark_snoozed(
    snoozes: &std::collections::BTreeMap<String, Snooze>,
    id: &str,
    until: Option<i64>,
    now: i64,
) -> std::collections::BTreeMap<String, Snooze> {
    let mut out: std::collections::BTreeMap<String, Snooze> = snoozes
        .iter()
        .filter(|(k, s)| s.until > now && k.as_str() != id)
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    if let Some(until) = until.filter(|u| *u > now) {
        out.insert(id.to_owned(), Snooze { until, at: now });
    }
    out
}

/// How long an opened session is remembered, in seconds.
pub const OPENED_FOR: i64 = 2 * 24 * 3600;

/// `opened` with `id` opened at `now`, older entries dropped.
pub fn mark_opened(
    opened: &std::collections::BTreeMap<String, i64>,
    id: &str,
    now: i64,
) -> std::collections::BTreeMap<String, i64> {
    let mut out: std::collections::BTreeMap<String, i64> = opened
        .iter()
        .filter(|(_, t)| now - **t <= OPENED_FOR)
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    out.insert(id.to_owned(), now);
    out
}

/// How many recent questions the preferences keep.
pub const RECENT_QUESTIONS: usize = 5;

/// `recent` with `question` first, once, at most [`RECENT_QUESTIONS`].
pub fn remember(recent: &[String], question: &str) -> Vec<String> {
    let mut out = vec![question.to_owned()];
    out.extend(recent.iter().filter(|q| q.as_str() != question).cloned());
    out.truncate(RECENT_QUESTIONS);
    out
}

fn system() -> String {
    SYSTEM.to_string()
}

fn yes() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

impl Default for Prefs {
    fn default() -> Prefs {
        Prefs {
            theme: system(),
            ask_answers: true,
            recent_questions: Vec::new(),
            menubar: true,
            notify_waiting: true,
            notify_stopped: true,
            notify_reset: true,
            digest_dismissed: None,
            inbox_seen_at: None,
            terminal: None,
            opened_sessions: Default::default(),
            sidebar_width: None,
            settled_sessions: Default::default(),
            snoozed_sessions: Default::default(),
            incidents_seen_at: None,
            dismissed_incidents: Default::default(),
        }
    }
}

/// Where the preferences live: `$HIPPOCAMP_APP_PREFS` when set, else
/// `~/Library/Application Support/dev.hippocamp.app/preferences.json`.
pub fn default_path() -> PathBuf {
    if let Some(p) = std::env::var_os(PATH_ENV).filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join("Library/Application Support/dev.hippocamp.app/preferences.json")
}

/// The file's contents: `Ok(None)` when it does not exist, an error naming
/// why it cannot be used otherwise.
pub fn read(path: &Path) -> Result<Option<Prefs>, String> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// The preferences, or the default (System) when the file is missing or
/// unreadable. (The app reads through [`read`], to tell a broken file, which
/// gets a notice, from a missing one.)
pub fn load(path: &Path) -> Prefs {
    read(path).ok().flatten().unwrap_or_default()
}

/// Changes the saved preferences with `change` and writes them, keeping
/// what `change` leaves alone; a missing or unreadable file starts from the
/// defaults.
pub fn update(path: &Path, change: impl FnOnce(&mut Prefs)) -> io::Result<()> {
    let mut prefs = load(path);
    change(&mut prefs);
    save(path, &prefs)
}

/// Writes the preferences, creating the directory, through
/// `.preferences.json.tmp` in the same directory, synced and renamed over the
/// file.
pub fn save(path: &Path, prefs: &Prefs) -> io::Result<()> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
    let dir = dir.unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir)?;
    let tmp = dir.join(".preferences.json.tmp");
    let mut text = serde_json::to_string_pretty(prefs).map_err(io::Error::other)?;
    text.push('\n');
    let result = (|| {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_update_keeps_what_it_does_not_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.json");
        update(&path, |p| p.ask_answers = false).unwrap();
        update(&path, |p| p.theme = "builtin:Hippocamp Dark".into()).unwrap();
        let p = load(&path);
        assert_eq!(p.theme, "builtin:Hippocamp Dark");
        assert!(!p.ask_answers);
        // A file from before reads as answers on, its pins ignored.
        std::fs::write(&path, r#"{"theme":"system","pinned":["s1"]}"#).unwrap();
        assert!(load(&path).ask_answers);
    }

    #[test]
    fn saves_and_loads_in_a_temporary_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/preferences.json");
        let prefs = Prefs {
            theme: "user:harbour.json/Harbour Dark".into(),
            ask_answers: false,
            recent_questions: vec!["who fixed the cron parser?".into()],
            menubar: false,
            notify_reset: false,
            digest_dismissed: Some("2026-11-10".into()),
            inbox_seen_at: Some(1_800_000_000),
            terminal: Some("ghostty".into()),
            ..Prefs::default()
        };
        save(&path, &prefs).unwrap();
        assert_eq!(load(&path), prefs);
        assert_eq!(read(&path).unwrap(), Some(prefs));
        assert!(
            !path.with_file_name(".preferences.json.tmp").exists(),
            "the temporary file was renamed away"
        );
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"theme\""), "{text}");
    }

    #[test]
    fn recent_questions_keep_five_newest_first_once() {
        let mut r: Vec<String> = Vec::new();
        for q in ["a", "b", "c", "a", "d", "e", "f"] {
            r = remember(&r, q);
        }
        assert_eq!(r, ["f", "e", "d", "a", "c"]);
    }

    #[test]
    fn a_missing_or_broken_file_loads_as_system() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.json");
        assert_eq!(read(&path), Ok(None));
        assert_eq!(load(&path).theme, "system");
        fs::write(&path, "{ not json").unwrap();
        assert!(read(&path).is_err());
        assert_eq!(load(&path).theme, "system");
        fs::write(&path, "{}").unwrap();
        assert_eq!(load(&path).theme, "system", "a missing key is the default");
    }

    #[test]
    fn opened_sessions_keep_two_days() {
        let mut m = std::collections::BTreeMap::new();
        m.insert("old".to_owned(), 0);
        m.insert("new".to_owned(), 100_000);
        let out = mark_opened(&m, "s1", 200_000);
        assert_eq!(out.keys().collect::<Vec<_>>(), vec!["new", "s1"]);
    }

    #[test]
    fn settle_marks_keep_two_days_and_the_newest_word_wins() {
        let mut m = std::collections::BTreeMap::new();
        m.insert(
            "old".to_owned(),
            Mark {
                settled: true,
                at: 0,
            },
        );
        let m = mark_settled(&m, "s1", true, 200_000);
        let m = mark_settled(&m, "s1", false, 200_010);
        assert_eq!(m.keys().collect::<Vec<_>>(), vec!["s1"]);
        assert!(!m["s1"].settled);
    }

    #[test]
    fn snoozes_drop_once_past_and_wake_by_hand() {
        let mut m = std::collections::BTreeMap::new();
        m.insert("past".to_owned(), Snooze { until: 50, at: 0 });
        let m = mark_snoozed(&m, "s1", Some(1_000), 100);
        assert_eq!(m.keys().collect::<Vec<_>>(), vec!["s1"]);
        assert_eq!(
            m["s1"],
            Snooze {
                until: 1_000,
                at: 100
            }
        );
        assert!(mark_snoozed(&m, "s1", None, 200).is_empty());
        // A time already past snoozes nothing.
        assert!(!mark_snoozed(&m, "s2", Some(10), 200).contains_key("s2"));
    }

    #[test]
    fn incident_marks_round_trip_and_a_file_from_before_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.json");
        update(&path, |p| {
            p.incidents_seen_at = Some(1_800_000_000);
            p.dismissed_incidents.insert("abc".into(), 1_800_000_001);
        })
        .unwrap();
        let p = load(&path);
        assert_eq!(p.incidents_seen_at, Some(1_800_000_000));
        assert_eq!(p.dismissed_incidents["abc"], 1_800_000_001);
        fs::write(&path, r#"{"theme":"system"}"#).unwrap();
        let p = load(&path);
        assert_eq!(p.incidents_seen_at, None);
        assert!(p.dismissed_incidents.is_empty());
    }

    #[test]
    fn a_failed_save_is_an_error_and_leaves_no_file() {
        let dir = tempfile::tempdir().unwrap();
        // The directory's place is taken by a file, so it cannot be created.
        let blocker = dir.path().join("blocked");
        fs::write(&blocker, "x").unwrap();
        let path = blocker.join("preferences.json");
        assert!(save(&path, &Prefs::default()).is_err());
        assert!(!path.exists());
    }
}
