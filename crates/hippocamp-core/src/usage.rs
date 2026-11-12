//! Token usage per profile and the headroom estimate built on it.
//!
//! A cache in `<home>/usage.sqlite`, like the rollup an observation that
//! can be deleted and rebuilt at any time. [`refresh`] runs at the end of
//! `hippocamp ingest`. It reads the transcripts under `transcript_roots`
//! modified in the last [`LOOKBACK`], each only past the offset it reached
//! last time, and keeps numbers only:
//!
//! - every Claude Code assistant message's usage, once per message id, as
//!   [weighted tokens](weighted);
//! - the client's limit messages (`You've hit your 5-hour limit · resets
//!   2:40pm (…)`), with the reset they name;
//! - Codex's own rate-limit figure, when a rollout's `token_count` event
//!   carries one.
//!
//! No text is kept. [`Snapshot::read`] adds what was written since the last
//! refresh, read-only, so a reader is current between ingests, and
//! [`headroom`] turns a snapshot into an estimate per profile. The estimate
//! is learned only from the profile's own past limit hits; without one it
//! says so ([`Estimate::NoEstimate`]) rather than borrowing a number.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::SystemTime;

use regex::Regex;
use rusqlite::{params, Connection, OpenFlags};
use serde_json::Value;
use time::format_description::well_known::Rfc3339;
use time::{Date, Duration, Month, OffsetDateTime, Time, UtcOffset};

use crate::config::Config;
use crate::error::{Error, Result};

pub mod meter;

/// The cache's file name inside `$HIPPOCAMP_HOME`.
pub const FILE_NAME: &str = "usage.sqlite";

/// `PRAGMA user_version` of a cache this code reads and writes.
const FORMAT: i64 = 1;

/// How far back usage and limit hits are kept and learned from.
pub const LOOKBACK: Duration = Duration::days(8);

/// The length of Claude's usage window.
pub const WINDOW: Duration = Duration::hours(5);

/// Without a cache entry, a reader reads a whole transcript only when it
/// was written this recently: enough for the current window.
const UNCACHED_TAIL: Duration = Duration::hours(6);

const DDL: &str = "\
CREATE TABLE files(path TEXT PRIMARY KEY, profile TEXT NOT NULL, offset INTEGER NOT NULL, mtime INTEGER NOT NULL);
CREATE TABLE messages(profile TEXT NOT NULL, id TEXT NOT NULL, ts INTEGER NOT NULL, units REAL NOT NULL, PRIMARY KEY(profile, id)) WITHOUT ROWID;
CREATE INDEX messages_ts ON messages(profile, ts);
CREATE TABLE limits(profile TEXT NOT NULL, ts INTEGER NOT NULL, kind TEXT NOT NULL, reset INTEGER, PRIMARY KEY(profile, ts, kind)) WITHOUT ROWID;
CREATE TABLE meters(profile TEXT NOT NULL, ts INTEGER NOT NULL, used_percent REAL NOT NULL, window_minutes INTEGER, resets_at INTEGER, PRIMARY KEY(profile, ts)) WITHOUT ROWID;
CREATE TABLE profiles(profile TEXT PRIMARY KEY, tool TEXT NOT NULL, dir TEXT);
PRAGMA user_version=1;";

// ---------------------------------------------------------------------------
// What a transcript line holds

/// One assistant message's usage.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub profile: String,
    /// The API's message id (else the request id): a message written
    /// several times, once per content block, counts once.
    pub id: String,
    /// Unix seconds.
    pub ts: i64,
    /// [`weighted`] tokens.
    pub units: f64,
}

/// Which limit a limit message names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LimitKind {
    /// The 5-hour window (`5-hour limit`, `session limit`, and any other
    /// limit that is not weekly).
    Window,
    /// The weekly limit.
    Weekly,
}

impl LimitKind {
    pub fn as_str(self) -> &'static str {
        match self {
            LimitKind::Window => "window",
            LimitKind::Weekly => "weekly",
        }
    }

    fn parse(s: &str) -> LimitKind {
        if s == "weekly" {
            LimitKind::Weekly
        } else {
            LimitKind::Window
        }
    }
}

/// A limit message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitHit {
    pub profile: String,
    pub ts: i64,
    pub kind: LimitKind,
    /// When the message says the limit resets, unix seconds.
    pub reset: Option<i64>,
}

/// Codex's own figure, from a rollout's `token_count` event.
#[derive(Debug, Clone, PartialEq)]
pub struct Meter {
    pub profile: String,
    pub ts: i64,
    pub used_percent: f64,
    pub window_minutes: Option<i64>,
    pub resets_at: Option<i64>,
}

/// Input, cache writes, cache reads and output, weighted by their relative
/// prices: `input + 1.25 × cache_creation + 0.1 × cache_read + 5 ×
/// output`. Only consistency matters: a limit is learned in these units.
pub fn weighted(input: f64, cache_creation: f64, cache_read: f64, output: f64) -> f64 {
    input + 1.25 * cache_creation + 0.1 * cache_read + 5.0 * output
}

static LIMIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bhit your ((?:[\w-]+ ){0,3})limit\b|usage limit reached")
        .expect("valid regex")
});
static RESET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\bresets?\s+(?:at\s+)?(?:(?P<mon>jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)[a-z]*\.?\s+(?P<day>[0-9]{1,2}),?\s+(?:at\s+)?)?(?P<h>[0-9]{1,2})(?::(?P<m>[0-9]{2}))?\s*(?P<ap>[ap])\.?m\b",
    )
    .expect("valid regex")
});

/// The reset a limit message names, as an instant: its clock time in
/// `offset` (the machine's), on the named day, else on the first day that
/// puts it after the message at `ts`.
pub fn parse_reset(text: &str, ts: OffsetDateTime, offset: UtcOffset) -> Option<OffsetDateTime> {
    let c = RESET.captures(text)?;
    let mut hour: u8 = c["h"].parse().ok()?;
    let minute: u8 = c.name("m").map_or(Some(0), |m| m.as_str().parse().ok())?;
    if hour == 0 || hour > 12 || minute > 59 {
        return None;
    }
    let pm = c["ap"].eq_ignore_ascii_case("p");
    hour = match (hour, pm) {
        (12, false) => 0,
        (12, true) => 12,
        (h, true) => h + 12,
        (h, false) => h,
    };
    let clock = Time::from_hms(hour, minute, 0).ok()?;
    let local = ts.to_offset(offset);
    if let (Some(mon), Some(day)) = (c.name("mon"), c.name("day")) {
        let month = month_of(mon.as_str())?;
        let day: u8 = day.as_str().parse().ok()?;
        for year in [local.year(), local.year() + 1] {
            let date = Date::from_calendar_date(year, month, day).ok()?;
            let at = date.with_time(clock).assume_offset(offset);
            if at > ts {
                return Some(at);
            }
        }
        return None;
    }
    let mut at = local.date().with_time(clock).assume_offset(offset);
    if at <= ts {
        at += Duration::days(1);
    }
    Some(at)
}

fn month_of(s: &str) -> Option<Month> {
    let m = match s.get(..3)?.to_ascii_lowercase().as_str() {
        "jan" => Month::January,
        "feb" => Month::February,
        "mar" => Month::March,
        "apr" => Month::April,
        "may" => Month::May,
        "jun" => Month::June,
        "jul" => Month::July,
        "aug" => Month::August,
        "sep" => Month::September,
        "oct" => Month::October,
        "nov" => Month::November,
        "dec" => Month::December,
        _ => return None,
    };
    Some(m)
}

/// What one transcript line adds.
#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    Message(Message),
    Limit(LimitHit),
    Meter(Meter),
}

/// Reads one line of a transcript of `profile`. Lines that carry no usage,
/// limit or meter are `None`; a quick text check skips most of them unparsed.
pub fn parse_line(line: &str, profile: &str, codex: bool, offset: UtcOffset) -> Option<Line> {
    if codex {
        if !line.contains("\"rate_limits\"") || !line.contains("\"used_percent\"") {
            return None;
        }
        let v: Value = serde_json::from_str(line).ok()?;
        let ts = parse_ts(v.get("timestamp")?.as_str()?)?;
        let limits = v.pointer("/payload/rate_limits")?;
        let primary = limits.get("primary").filter(|p| p.is_object())?;
        let used = primary.get("used_percent")?.as_f64()?;
        let resets_at = primary
            .get("resets_at")
            .and_then(Value::as_i64)
            .or_else(|| {
                primary
                    .get("resets_in_seconds")
                    .and_then(Value::as_i64)
                    .map(|s| ts.unix_timestamp() + s)
            });
        return Some(Line::Meter(Meter {
            profile: profile.to_owned(),
            ts: ts.unix_timestamp(),
            used_percent: used,
            window_minutes: primary.get("window_minutes").and_then(Value::as_i64),
            resets_at,
        }));
    }
    if !line.contains("\"assistant\"") {
        return None;
    }
    let synthetic = line.contains("<synthetic>");
    if !synthetic && !line.contains("\"usage\"") {
        return None;
    }
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "assistant" {
        return None;
    }
    let ts = parse_ts(v.get("timestamp")?.as_str()?)?;
    let message = v.get("message")?;
    if message.get("model").and_then(Value::as_str) == Some("<synthetic>") {
        let text = text_of(message.get("content")?);
        let c = LIMIT.captures(&text)?;
        let words = c.get(1).map_or("", |m| m.as_str()).to_ascii_lowercase();
        let kind = if words.contains("weekly") {
            LimitKind::Weekly
        } else {
            LimitKind::Window
        };
        return Some(Line::Limit(LimitHit {
            profile: profile.to_owned(),
            ts: ts.unix_timestamp(),
            kind,
            reset: parse_reset(&text, ts, offset).map(OffsetDateTime::unix_timestamp),
        }));
    }
    let usage = message.get("usage")?;
    let n = |k: &str| usage.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let id = message
        .get("id")
        .and_then(Value::as_str)
        .or_else(|| v.get("requestId").and_then(Value::as_str))?;
    Some(Line::Message(Message {
        profile: profile.to_owned(),
        id: id.to_owned(),
        ts: ts.unix_timestamp(),
        units: weighted(
            n("input_tokens"),
            n("cache_creation_input_tokens"),
            n("cache_read_input_tokens"),
            n("output_tokens"),
        ),
    }))
}

fn text_of(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

fn parse_ts(s: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(s, &Rfc3339).ok()
}

/// The machine's UTC offset at `t`, from the C library's time zone rules
/// (`localtime_r`, which is safe with other threads running); UTC when it
/// cannot be read.
pub fn local_offset_at(t: OffsetDateTime) -> UtcOffset {
    #[cfg(unix)]
    {
        let secs = t.unix_timestamp() as libc::time_t;
        // SAFETY: `tm` is plain data that `localtime_r` fills; both
        // pointers are valid for the call.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        let ok = unsafe { !libc::localtime_r(&secs, &mut tm).is_null() };
        if ok {
            if let Ok(off) = UtcOffset::from_whole_seconds(tm.tm_gmtoff as i32) {
                return off;
            }
        }
    }
    let _ = t;
    UtcOffset::UTC
}

// ---------------------------------------------------------------------------
// Profiles and files

/// A transcript file and the profile it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transcript {
    pub path: PathBuf,
    pub profile: String,
    pub codex: bool,
    /// The profile's config dir (`~/.claude-work`), for Claude Code.
    pub dir: Option<PathBuf>,
    /// Unix seconds.
    pub mtime: i64,
    pub size: u64,
}

/// The Claude config dir a transcript is under: the nearest ancestor named
/// `.claude` or `.claude-<name>`.
pub fn config_dir_of(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|a| {
            a.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n == ".claude" || n.starts_with(".claude-"))
        })
        .map(Path::to_path_buf)
}

fn mtime_of(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

/// Every transcript under `roots` modified at or after `since` (unix
/// seconds), newest first.
pub fn transcripts(roots: &[PathBuf], since: i64) -> Vec<Transcript> {
    let mut out = Vec::new();
    for root in roots {
        for entry in walkdir::WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            let mtime = mtime_of(&meta);
            if mtime < since {
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let codex = name.starts_with("rollout-");
            let tool = if codex { "codex" } else { "claude-code" };
            let Some(profile) = crate::rollup::profile_of(tool, path) else {
                continue;
            };
            out.push(Transcript {
                path: path.to_path_buf(),
                profile,
                codex,
                dir: (!codex).then(|| config_dir_of(path)).flatten(),
                mtime,
                size: meta.len(),
            });
        }
    }
    out.sort_by(|a, b| b.mtime.cmp(&a.mtime).then_with(|| a.path.cmp(&b.path)));
    out
}

/// Reads `t` from byte `from` to its last complete line: what the lines
/// add, and the offset after the last complete line. A file shorter than
/// `from` (rewritten) is read from the start.
pub fn read_from(t: &Transcript, from: u64, offset: UtcOffset) -> io::Result<(Vec<Line>, u64)> {
    let mut file = File::open(&t.path)?;
    let len = file.metadata()?.len();
    let mut pos = if from > len { 0 } else { from };
    file.seek(SeekFrom::Start(pos))?;
    let mut reader = BufReader::with_capacity(1 << 20, file);
    let mut buf = Vec::new();
    let mut out = Vec::new();
    loop {
        buf.clear();
        let n = reader.read_until(b'\n', &mut buf)?;
        if n == 0 || buf.last() != Some(&b'\n') {
            break;
        }
        pos += n as u64;
        if let Some(l) = std::str::from_utf8(&buf)
            .ok()
            .and_then(|line| parse_line(line, &t.profile, t.codex, offset))
        {
            out.push(l);
        }
    }
    Ok((out, pos))
}

// ---------------------------------------------------------------------------
// The cache

fn open_for_writing(home: &Path) -> Result<Connection> {
    let path = home.join(FILE_NAME);
    let open = || -> Result<Option<Connection>> {
        let conn = Connection::open(&path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let empty: bool =
            conn.query_row("SELECT NOT EXISTS(SELECT 1 FROM sqlite_master)", [], |r| {
                r.get(0)
            })?;
        if version == FORMAT {
            Ok(Some(conn))
        } else if version == 0 && empty {
            conn.execute_batch(DDL)?;
            Ok(Some(conn))
        } else {
            Ok(None)
        }
    };
    match open() {
        Ok(Some(conn)) => Ok(conn),
        Ok(None) | Err(Error::Sqlite(_)) => {
            remove(home)?;
            open()?.ok_or_else(|| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: cannot create the usage cache", path.display()),
                ))
            })
        }
        Err(e) => Err(e),
    }
}

/// Deletes `<home>/usage.sqlite` and its WAL files, if present.
pub fn remove(home: &Path) -> Result<()> {
    for suffix in ["", "-wal", "-shm"] {
        let path = home.join(format!("{FILE_NAME}{suffix}"));
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::io_at(&path, e)),
        }
    }
    Ok(())
}

/// What [`refresh`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Refreshed {
    /// Files read (in part or whole).
    pub read: u64,
    /// Messages added.
    pub messages: u64,
}

fn store(tx: &rusqlite::Transaction<'_>, lines: &[Line]) -> Result<u64> {
    let mut added = 0;
    let mut msg = tx.prepare_cached(
        "INSERT OR IGNORE INTO messages(profile, id, ts, units) VALUES (?1, ?2, ?3, ?4)",
    )?;
    let mut lim = tx.prepare_cached(
        "INSERT OR IGNORE INTO limits(profile, ts, kind, reset) VALUES (?1, ?2, ?3, ?4)",
    )?;
    let mut met = tx.prepare_cached(
        "INSERT OR REPLACE INTO meters(profile, ts, used_percent, window_minutes, resets_at) VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for l in lines {
        match l {
            Line::Message(m) => {
                added += msg.execute(params![m.profile, m.id, m.ts, m.units])? as u64
            }
            Line::Limit(h) => {
                lim.execute(params![h.profile, h.ts, h.kind.as_str(), h.reset])?;
            }
            Line::Meter(m) => {
                met.execute(params![
                    m.profile,
                    m.ts,
                    m.used_percent,
                    m.window_minutes,
                    m.resets_at
                ])?;
            }
        }
    }
    Ok(added)
}

/// Brings `<home>/usage.sqlite` up to date with the transcripts under the
/// configuration's roots modified in the last [`LOOKBACK`], and forgets
/// what is older. `offset` is the machine's UTC offset, which limit
/// messages' clock times are read in.
pub fn refresh(cfg: &Config, now: OffsetDateTime, offset: UtcOffset) -> Result<Refreshed> {
    let roots = crate::adapters::transcript_roots(cfg);
    refresh_roots(&cfg.home, &roots, now, offset)
}

/// [`refresh`] over `roots`.
pub fn refresh_roots(
    home: &Path,
    roots: &[PathBuf],
    now: OffsetDateTime,
    offset: UtcOffset,
) -> Result<Refreshed> {
    let mut conn = open_for_writing(home)?;
    let since = (now - LOOKBACK).unix_timestamp();
    let known: HashMap<String, (u64, i64)> = {
        let mut stmt = conn.prepare("SELECT path, offset, mtime FROM files")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                (r.get::<_, i64>(1)? as u64, r.get(2)?),
            ))
        })?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let files = transcripts(roots, since);
    let mut done = Refreshed::default();
    let mut seen = HashSet::new();
    let mut profiles: BTreeMap<String, (bool, Option<PathBuf>)> = BTreeMap::new();
    for chunk in files.chunks(50) {
        let tx = conn.transaction()?;
        for t in chunk {
            let key = t.path.to_string_lossy().into_owned();
            seen.insert(key.clone());
            profiles
                .entry(t.profile.clone())
                .or_insert((t.codex, t.dir.clone()));
            let from = match known.get(&key) {
                Some((off, mtime)) if *mtime == t.mtime && *off == t.size => continue,
                Some((off, _)) => *off,
                None => 0,
            };
            let (lines, end) = match read_from(t, from, offset) {
                Ok(r) => r,
                Err(err) => {
                    tracing::debug!("usage: cannot read {}: {err}", t.path.display());
                    continue;
                }
            };
            done.read += 1;
            done.messages += store(&tx, &lines)?;
            tx.execute(
                "INSERT OR REPLACE INTO files(path, profile, offset, mtime) VALUES (?1, ?2, ?3, ?4)",
                params![key, t.profile, end as i64, t.mtime],
            )?;
        }
        tx.commit()?;
    }
    let tx = conn.transaction()?;
    for (profile, (codex, dir)) in &profiles {
        tx.execute(
            "INSERT OR REPLACE INTO profiles(profile, tool, dir) VALUES (?1, ?2, ?3)",
            params![
                profile,
                if *codex { "codex" } else { "claude-code" },
                dir.as_ref().map(|d| d.to_string_lossy().into_owned())
            ],
        )?;
    }
    for key in known.keys().filter(|k| !seen.contains(*k)) {
        tx.execute("DELETE FROM files WHERE path = ?1", [key])?;
    }
    tx.execute("DELETE FROM messages WHERE ts < ?1", [since])?;
    tx.execute("DELETE FROM limits WHERE ts < ?1", [since])?;
    tx.execute("DELETE FROM meters WHERE ts < ?1", [since])?;
    tx.commit()?;
    Ok(done)
}

// ---------------------------------------------------------------------------
// Reading

/// A profile hippocamp has seen transcripts of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileInfo {
    pub name: String,
    pub codex: bool,
    /// The Claude config dir.
    pub dir: Option<PathBuf>,
}

/// Everything [`headroom`] needs, the cache plus what was written since.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub profiles: Vec<ProfileInfo>,
    /// By profile, in time order, once per message id.
    pub messages: BTreeMap<String, Vec<(i64, f64)>>,
    pub limits: Vec<LimitHit>,
    pub meters: Vec<Meter>,
    /// Whether a cache was there to read; without one only the last hours
    /// are known and no limit can be learned.
    pub cached: bool,
    /// Each Claude profile's cached client meter, read when
    /// the snapshot is read; never stored.
    pub claude_meters: BTreeMap<String, meter::Meter>,
}

impl Snapshot {
    /// Reads the cache read-only (if any), then the bytes written since to
    /// the transcripts under `roots`. Changes nothing.
    pub fn read(
        home: &Path,
        roots: &[PathBuf],
        now: OffsetDateTime,
        offset: UtcOffset,
    ) -> Result<Snapshot> {
        let since = (now - LOOKBACK).unix_timestamp();
        let mut snap = Snapshot::default();
        let mut ids: HashSet<(String, String)> = HashSet::new();
        let mut msgs: Vec<Message> = Vec::new();
        let mut known: HashMap<String, (u64, i64)> = HashMap::new();
        let mut profiles: BTreeMap<String, ProfileInfo> = BTreeMap::new();
        let path = home.join(FILE_NAME);
        if path.exists() {
            let conn = Connection::open_with_flags(
                &path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
            if version == FORMAT {
                snap.cached = true;
                read_cache(
                    &conn,
                    since,
                    &mut snap,
                    &mut msgs,
                    &mut known,
                    &mut profiles,
                )?;
            }
        }
        for m in &msgs {
            ids.insert((m.profile.clone(), m.id.clone()));
        }
        // What was written since: tails of known files, recent new files.
        let recent = (now - UNCACHED_TAIL).unix_timestamp();
        for t in transcripts(roots, since) {
            profiles
                .entry(t.profile.clone())
                .or_insert_with(|| ProfileInfo {
                    name: t.profile.clone(),
                    codex: t.codex,
                    dir: t.dir.clone(),
                });
            let key = t.path.to_string_lossy().into_owned();
            let from = match known.get(&key) {
                Some((off, mtime)) if *mtime == t.mtime && *off == t.size => continue,
                Some((off, _)) => *off,
                None if t.mtime >= recent => 0,
                None => continue,
            };
            let Ok((lines, _)) = read_from(&t, from, offset) else {
                continue;
            };
            for l in lines {
                match l {
                    Line::Message(m) => {
                        if ids.insert((m.profile.clone(), m.id.clone())) {
                            msgs.push(m);
                        }
                    }
                    Line::Limit(h) => {
                        if !snap.limits.contains(&h) {
                            snap.limits.push(h);
                        }
                    }
                    Line::Meter(m) => snap.meters.push(m),
                }
            }
        }
        for m in msgs.into_iter().filter(|m| m.ts >= since) {
            snap.messages
                .entry(m.profile)
                .or_default()
                .push((m.ts, m.units));
        }
        for v in snap.messages.values_mut() {
            v.sort_by_key(|m| m.0);
        }
        snap.limits.sort_by_key(|h| h.ts);
        snap.meters.sort_by_key(|m| m.ts);
        snap.profiles = profiles.into_values().collect();
        snap.read_meters();
        Ok(snap)
    }
}

impl Snapshot {
    /// Reads each Claude profile's client meter from its
    /// settings file; a profile without one keeps the estimate.
    pub fn read_meters(&mut self) {
        self.claude_meters = self
            .profiles
            .iter()
            .filter(|p| !p.codex)
            .filter_map(|p| {
                let dir = p.dir.as_ref()?;
                meter::read(&meter::settings_file(dir)).map(|m| (p.name.clone(), m))
            })
            .collect();
    }
}

type Known = HashMap<String, (u64, i64)>;

fn read_cache(
    conn: &Connection,
    since: i64,
    snap: &mut Snapshot,
    msgs: &mut Vec<Message>,
    known: &mut Known,
    profiles: &mut BTreeMap<String, ProfileInfo>,
) -> Result<()> {
    let mut stmt = conn.prepare("SELECT path, offset, mtime FROM files")?;
    for r in stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)? as u64,
            r.get::<_, i64>(2)?,
        ))
    })? {
        let (p, o, m) = r?;
        known.insert(p, (o, m));
    }
    let mut stmt = conn.prepare("SELECT profile, id, ts, units FROM messages WHERE ts >= ?1")?;
    for r in stmt.query_map([since], |r| {
        Ok(Message {
            profile: r.get(0)?,
            id: r.get(1)?,
            ts: r.get(2)?,
            units: r.get(3)?,
        })
    })? {
        msgs.push(r?);
    }
    let mut stmt = conn.prepare("SELECT profile, ts, kind, reset FROM limits WHERE ts >= ?1")?;
    for r in stmt.query_map([since], |r| {
        Ok(LimitHit {
            profile: r.get(0)?,
            ts: r.get(1)?,
            kind: LimitKind::parse(&r.get::<_, String>(2)?),
            reset: r.get(3)?,
        })
    })? {
        snap.limits.push(r?);
    }
    let mut stmt = conn.prepare(
        "SELECT profile, ts, used_percent, window_minutes, resets_at FROM meters WHERE ts >= ?1",
    )?;
    for r in stmt.query_map([since], |r| {
        Ok(Meter {
            profile: r.get(0)?,
            ts: r.get(1)?,
            used_percent: r.get(2)?,
            window_minutes: r.get(3)?,
            resets_at: r.get(4)?,
        })
    })? {
        snap.meters.push(r?);
    }
    let mut stmt = conn.prepare("SELECT profile, tool, dir FROM profiles")?;
    for r in stmt.query_map([], |r| {
        Ok(ProfileInfo {
            name: r.get(0)?,
            codex: r.get::<_, String>(1)? == "codex",
            dir: r.get::<_, Option<String>>(2)?.map(PathBuf::from),
        })
    })? {
        let p = r?;
        profiles.insert(p.name.clone(), p);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Headroom

/// The current window of a profile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowState {
    /// No message in the last [`WINDOW`]: the whole allowance is there.
    None,
    /// A window is open: when it started and ends, and the weighted tokens
    /// used in it so far. `anchored` when a limit message gave its end.
    Open {
        start: i64,
        end: i64,
        used: f64,
        anchored: bool,
    },
    /// A limit message's reset is still ahead.
    Limited { reset: i64 },
}

/// How much of the window is left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Estimate {
    /// A fraction from 0 to 1.
    Left(f64),
    /// A window is open but no limit was learned for this profile.
    NoEstimate,
}

/// Where a window's figure comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The client's own meter.
    Meter,
    /// The estimate from the profile's transcripts.
    Estimate,
}

/// What is left of one window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Left {
    /// A fraction from 0 to 1.
    pub frac: f64,
    pub source: Source,
    /// When the window resets, if it is open and the reset is known.
    pub resets_at: Option<i64>,
    /// The meter named a reset that has passed and was not refreshed since.
    pub reset_passed: Option<i64>,
}

/// One profile's headroom.
#[derive(Debug, Clone, PartialEq)]
pub struct Headroom {
    pub profile: String,
    pub codex: bool,
    pub dir: Option<PathBuf>,
    pub window: WindowState,
    pub estimate: Estimate,
    /// The learned limit, in weighted tokens, and from how many windows
    /// that ended in a limit hit.
    pub limit: Option<f64>,
    pub limit_from: usize,
    /// A weekly limit whose reset is ahead.
    pub weekly_until: Option<i64>,
    /// The newest message.
    pub last_seen: Option<i64>,
    /// For Codex: its own figure was used.
    pub from_meter: bool,
    /// The client's cached meter, when the profile's settings file has one.
    pub meter: Option<meter::Meter>,
    /// The meter's fetch time when it is stale.
    pub meter_stale: Option<i64>,
    /// The 5-hour window: the meter's figure, or the estimate.
    pub five: Option<Left>,
    /// The 7-day window: the meter's figure only.
    pub seven: Option<Left>,
}

impl Headroom {
    /// The fraction left, when one is known.
    pub fn left(&self) -> Option<f64> {
        match self.estimate {
            Estimate::Left(f) => Some(f),
            Estimate::NoEstimate => None,
        }
    }

    /// When the current limit or window resets, if known.
    pub fn resets(&self) -> Option<i64> {
        if let Some(w) = self.weekly_until {
            return Some(w);
        }
        match self.window {
            WindowState::Limited { reset } => Some(reset),
            WindowState::Open { end, .. } => Some(end),
            WindowState::None => None,
        }
    }

    /// Whether a person could use this profile now: no limit hit ahead,
    /// and neither window used up.
    pub fn usable(&self) -> bool {
        self.weekly_until.is_none()
            && !matches!(self.window, WindowState::Limited { .. })
            && [self.five, self.seven]
                .iter()
                .flatten()
                .all(|l| l.frac > 0.0)
    }

    /// Whether any figure is the client's meter.
    pub fn metered(&self) -> bool {
        [self.five, self.seven]
            .iter()
            .flatten()
            .any(|l| l.source == Source::Meter)
    }
}

/// The weighted tokens used from `start` to `end` inclusive.
fn used_between(msgs: &[(i64, f64)], start: i64, end: i64) -> f64 {
    let lo = msgs.partition_point(|m| m.0 < start);
    let hi = msgs.partition_point(|m| m.0 <= end);
    msgs[lo..hi.max(lo)].iter().map(|m| m.1).sum()
}

/// The limit learned from `hits` (one profile's 5-hour window hits): per
/// distinct reset, the weighted tokens from the window's start to the
/// first hit; the median of those, and how many there were.
pub fn learn_limit(msgs: &[(i64, f64)], hits: &[&LimitHit]) -> (Option<f64>, usize) {
    let mut first: BTreeMap<i64, i64> = BTreeMap::new();
    for h in hits {
        if let Some(r) = h.reset {
            first
                .entry(r)
                .and_modify(|t| *t = (*t).min(h.ts))
                .or_insert(h.ts);
        }
    }
    let mut used: Vec<f64> = first
        .iter()
        .map(|(reset, at)| used_between(msgs, reset - WINDOW.whole_seconds(), *at))
        .filter(|u| *u > 0.0)
        .collect();
    if used.is_empty() {
        return (None, 0);
    }
    used.sort_by(|a, b| a.total_cmp(b));
    let n = used.len();
    let median = if n % 2 == 1 {
        used[n / 2]
    } else {
        (used[n / 2 - 1] + used[n / 2]) / 2.0
    };
    (Some(median), n)
}

/// The current window from `msgs` (in time order) and the profile's
/// 5-hour window hits, at `now`.
pub fn current_window(msgs: &[(i64, f64)], hits: &[&LimitHit], now: i64) -> WindowState {
    let w = WINDOW.whole_seconds();
    if let Some(reset) = hits
        .iter()
        .filter_map(|h| h.reset.filter(|r| *r > now && h.ts <= now))
        .max()
    {
        return WindowState::Limited { reset };
    }
    // Forward through the messages: a window starts at the first message
    // after the previous one ended, or at a known reset minus 5 hours.
    let resets: Vec<i64> = {
        let mut r: Vec<i64> = hits.iter().filter_map(|h| h.reset).collect();
        r.sort_unstable();
        r.dedup();
        r
    };
    let mut cur: Option<(i64, i64, bool)> = None;
    for &(ts, _) in msgs.iter().filter(|m| m.0 <= now) {
        if cur.is_none_or(|(_, end, _)| ts >= end) {
            // A reset whose window holds this message anchors it.
            let anchored = resets.iter().find(|r| **r - w <= ts && ts < **r);
            cur = Some(match anchored {
                Some(r) => (r - w, *r, true),
                // The client's resets fall on 10-minute marks: a window
                // starts on the mark at or before its first message.
                None => (ts - ts.rem_euclid(600), ts - ts.rem_euclid(600) + w, false),
            });
        }
    }
    match cur {
        Some((start, end, anchored)) if now < end => WindowState::Open {
            start,
            end,
            used: used_between(msgs, start, now),
            anchored,
        },
        _ => WindowState::None,
    }
}

/// Every profile's headroom at `now`, from `snap`. Profiles seen in the
/// last [`LOOKBACK`] only, by name.
pub fn headroom(snap: &Snapshot, now: OffsetDateTime) -> Vec<Headroom> {
    let now_s = now.unix_timestamp();
    let empty = Vec::new();
    snap.profiles
        .iter()
        .map(|p| {
            let msgs = snap.messages.get(&p.name).unwrap_or(&empty);
            let window_hits: Vec<&LimitHit> = snap
                .limits
                .iter()
                .filter(|h| h.profile == p.name && h.kind == LimitKind::Window)
                .collect();
            let weekly_until = snap
                .limits
                .iter()
                .filter(|h| h.profile == p.name && h.kind == LimitKind::Weekly)
                .filter_map(|h| h.reset.filter(|r| *r > now_s))
                .max();
            let last_seen = msgs.last().map(|m| m.0);
            if p.codex {
                let meter = snap
                    .meters
                    .iter()
                    .rev()
                    .find(|m| m.profile == p.name && m.resets_at.is_some_and(|r| r > now_s));
                return Headroom {
                    profile: p.name.clone(),
                    codex: true,
                    dir: None,
                    window: match meter {
                        Some(m) => WindowState::Open {
                            start: m.ts,
                            end: m.resets_at.unwrap_or(m.ts),
                            used: m.used_percent,
                            anchored: true,
                        },
                        None => WindowState::None,
                    },
                    estimate: match meter {
                        Some(m) => Estimate::Left((1.0 - m.used_percent / 100.0).clamp(0.0, 1.0)),
                        None => Estimate::NoEstimate,
                    },
                    limit: None,
                    limit_from: 0,
                    weekly_until: None,
                    last_seen: snap
                        .meters
                        .iter()
                        .rev()
                        .find(|m| m.profile == p.name)
                        .map(|m| m.ts),
                    from_meter: meter.is_some(),
                    meter: None,
                    meter_stale: None,
                    five: None,
                    seven: None,
                };
            }
            let (limit, limit_from) = learn_limit(msgs, &window_hits);
            let window = current_window(msgs, &window_hits, now_s);
            let estimate = match (window, limit) {
                (WindowState::Limited { .. }, _) => Estimate::Left(0.0),
                (WindowState::None, _) => Estimate::Left(1.0),
                (WindowState::Open { used, .. }, Some(l)) if l > 0.0 => {
                    Estimate::Left((1.0 - used / l).clamp(0.0, 1.0))
                }
                (WindowState::Open { .. }, _) => Estimate::NoEstimate,
            };
            let client = snap.claude_meters.get(&p.name).copied();
            let (five, seven, meter_stale) = windows(client, window, estimate, last_seen, now_s);
            // The figure every reader uses: the tighter window's, when the
            // meter gave one; a limit message wins.
            let estimate = match (window, five, seven) {
                (WindowState::Limited { .. }, _, _) => Estimate::Left(0.0),
                (_, f, s) if f.is_some_and(|l| l.source == Source::Meter) || s.is_some() => {
                    let tight = [f, s].iter().flatten().map(|l| l.frac).fold(1.0, f64::min);
                    Estimate::Left(tight)
                }
                _ => estimate,
            };
            Headroom {
                profile: p.name.clone(),
                codex: false,
                dir: p.dir.clone(),
                window,
                estimate: if weekly_until.is_some() {
                    Estimate::Left(0.0)
                } else {
                    estimate
                },
                limit,
                limit_from,
                weekly_until,
                last_seen,
                from_meter: false,
                meter: client,
                meter_stale,
                five,
                seven,
            }
        })
        .collect()
}

/// A Claude profile's two windows at `now`: the 5-hour
/// window from a current meter, else the transcript estimate; the 7-day
/// window from the meter at any age. Also the meter's fetch time when it
/// is stale.
fn windows(
    client: Option<meter::Meter>,
    window: WindowState,
    estimate: Estimate,
    last_seen: Option<i64>,
    now: i64,
) -> (Option<Left>, Option<Left>, Option<i64>) {
    let from_meter = |w: meter::Window| Left {
        frac: w.left(now),
        source: Source::Meter,
        resets_at: w.resets_at.filter(|r| *r > now),
        reset_passed: w.resets_at.filter(|r| *r <= now),
    };
    let estimated = match estimate {
        Estimate::Left(frac) => Some(Left {
            frac,
            source: Source::Estimate,
            resets_at: match window {
                WindowState::Open { end, .. } => Some(end),
                WindowState::Limited { reset } => Some(reset),
                WindowState::None => None,
            },
            reset_passed: None,
        }),
        Estimate::NoEstimate => None,
    };
    let Some(m) = client else {
        return (estimated, None, None);
    };
    let current = m.current(now, last_seen);
    let five = match m.five_hour {
        Some(w) if current => Some(from_meter(w)),
        _ => estimated,
    };
    let seven = m.seven_day.map(from_meter);
    (five, seven, (!current).then_some(m.fetched_at))
}

/// The Claude profile to use next: the most headroom among those with an
/// estimate, usable now; ties go to the one seen most recently. With the
/// meter, a profile's headroom is its tighter window's.
pub fn use_next(rooms: &[Headroom]) -> Option<&Headroom> {
    use_next_except(rooms, &[])
}

/// [`use_next`] leaving out the profiles in `skip`, such as those whose
/// sign-in failed recently.
pub fn use_next_except<'a>(rooms: &'a [Headroom], skip: &[String]) -> Option<&'a Headroom> {
    rooms
        .iter()
        .filter(|h| !h.codex && h.usable() && !skip.contains(&h.profile))
        .filter_map(|h| h.left().map(|l| (h, l)))
        .max_by(|(a, la), (b, lb)| {
            la.total_cmp(lb)
                .then_with(|| a.last_seen.unwrap_or(0).cmp(&b.last_seen.unwrap_or(0)))
        })
        .map(|(h, _)| h)
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests;
