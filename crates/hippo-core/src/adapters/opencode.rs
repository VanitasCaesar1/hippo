//! OpenCode's database, as `docs/designs/opencode-adapter.md` describes it.
//!
//! OpenCode 2 keeps every session in one SQLite database,
//! `<data>/opencode.db` (or `opencode-<channel>.db`), `<data>` being
//! `$XDG_DATA_HOME/opencode` or `~/.local/share/opencode`. The database is
//! opened read-only ([`open`]) and never written. Each session is ingested
//! as a transcript of its own at `<database>#<session id>`
//! ([`session_path`]), skipped while its stamp ([`Adapter::sessions_in`])
//! is the one the index holds.
//!
//! Tables read: `session_v2` (one row per session), `session_message`
//! (its messages, in `seq` order, `data` the message as JSON),
//! `session_part` (assistant content kept apart, when OpenCode's
//! `experimental.part_rows` is on), `event_sequence` (per session, the
//! number of its newest change) and `migration` (the schema's version).
//! Turns, each stamped with its own time:
//!
//! - `user`: one `user` turn of `text`;
//! - `assistant`: one `assistant` turn per non-empty `text` item; per
//!   `tool` item, a `tool` turn for the call (`<name> <input>`, with the
//!   core tools under Claude Code's names and argument names,
//!   [`stored_name`], [`stored_input`]) and, once it finished, one for its
//!   result ([`result_text`]); reasoning is never a turn;
//! - every other message type is not a turn.
//!
//! In a fork, the messages copied from the session it was forked from are
//! skipped. A sub-agent is `opencode-<top>-agent-<id>`, `<top>` the
//! session at the top of its `parent_id` chain; any other session is
//! `opencode-<id>`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{Map, Value};
use time::OffsetDateTime;

use super::{invalid_data, push_turn, Adapter, Listed, Parsed, RawTurn, Role, Tool};
use crate::config::Config;
use crate::error::{Error, Result};

/// OpenCode's data folder when `XDG_DATA_HOME` is not set.
pub const DEFAULT_ROOT: &str = "~/.local/share/opencode";

/// The migration that named `session_v2`: the oldest schema read.
pub const OLDEST: &str = "20260804233008_loose_psylocke";

/// The newest migration this adapter was checked against (OpenCode 2.2.7).
pub const NEWEST_KNOWN: &str = "20261030054400_job";

/// How long a read waits for OpenCode's lock before it gives up.
const BUSY_TIMEOUT: Duration = Duration::from_secs(2);

/// How many parents up a sub-agent's chain is followed.
const CHAIN_DEPTH: usize = 16;

/// The columns read, by table; a database without one of them is not read.
const REQUIRED: [(&str, &[&str]); 3] = [
    (
        "session_v2",
        &[
            "id",
            "parent_id",
            "fork_session_id",
            "fork_boundary",
            "directory",
            "title",
            "time_created",
            "time_updated",
        ],
    ),
    (
        "session_message",
        &[
            "id",
            "session_id",
            "type",
            "seq",
            "time_created",
            "time_updated",
            "data",
        ],
    ),
    ("migration", &["id"]),
];

/// The OpenCode adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenCode;

impl Adapter for OpenCode {
    fn tool(&self) -> Tool {
        Tool::OpenCode
    }

    fn roots(&self, cfg: &Config) -> Vec<PathBuf> {
        super::transcript_roots(cfg)
    }

    fn file_matches(&self, path: &Path) -> bool {
        is_database(path) || split(path).is_some()
    }

    fn parse(&self, _root: &Path, path: &Path) -> Result<Parsed> {
        let (db, sid) =
            split(path).ok_or_else(|| invalid_data(path, "not an OpenCode session path"))?;
        let conn = open(&db).map_err(|e| sql_error(&db, e))?;
        check(&conn).map_err(|what| invalid_data(&db, &what))?;
        let info = info_in(&conn, &sid)
            .map_err(|e| sql_error(&db, e))?
            .ok_or_else(|| invalid_data(path, "no such OpenCode session"))?;
        let turns = turns_of(&conn, &info, path).map_err(|e| sql_error(&db, e))?;
        Ok(Parsed {
            id: hippo_id(&conn, &info),
            tool: Tool::OpenCode,
            started: info.created,
            turns,
        })
    }

    fn cwd(&self, _id: &str, path: &Path) -> Option<PathBuf> {
        info(path).and_then(|i| i.directory)
    }

    fn sessions_in(&self, path: &Path) -> Option<Result<Vec<Listed>>> {
        if !is_database(path) {
            return None;
        }
        Some(list(path))
    }
}

/// OpenCode's data folder: `$XDG_DATA_HOME/opencode` when `XDG_DATA_HOME`
/// is an absolute path, else [`DEFAULT_ROOT`].
pub fn default_root() -> String {
    match std::env::var_os("XDG_DATA_HOME").map(PathBuf::from) {
        Some(dir) if dir.is_absolute() => dir.join("opencode").to_string_lossy().into_owned(),
        _ => DEFAULT_ROOT.to_owned(),
    }
}

/// Whether `path` names an OpenCode database: `opencode.db`, or
/// `opencode-<channel>.db` other than the preview `opencode-next.db`.
pub fn is_database(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if name == "opencode.db" {
        return true;
    }
    name.strip_prefix("opencode-")
        .and_then(|rest| rest.strip_suffix(".db"))
        .is_some_and(|channel| {
            channel != "next"
                && !channel.is_empty()
                && channel
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        })
}

/// Whether `dir` is OpenCode's data folder: named `opencode`, holding a
/// database. The walks of the transcript roots do not go below it.
pub fn is_data_dir(dir: &Path) -> bool {
    dir.file_name().is_some_and(|n| n == "opencode")
        && std::fs::read_dir(dir)
            .map(|entries| entries.flatten().any(|e| is_database(&e.path())))
            .unwrap_or(false)
}

/// The path of session `sid` in the database at `db`: `<db>#<sid>`.
pub fn session_path(db: &Path, sid: &str) -> PathBuf {
    let mut s = db.as_os_str().to_owned();
    s.push("#");
    s.push(sid);
    PathBuf::from(s)
}

/// The database and session of a [`session_path`].
pub fn split(path: &Path) -> Option<(PathBuf, String)> {
    let text = path.to_str()?;
    let (db, sid) = text.rsplit_once('#')?;
    let db = PathBuf::from(db);
    (is_database(&db) && !sid.is_empty() && !sid.contains('/')).then(|| (db, sid.to_owned()))
}

/// Opens the database at `db` read-only: `mode=ro`, no `immutable`, a
/// short busy timeout. Nothing this module does writes.
pub fn open(db: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        db,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI,
    )?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    Ok(conn)
}

fn sql_error(db: &Path, err: rusqlite::Error) -> Error {
    invalid_data(db, &format!("cannot read the OpenCode database: {err}"))
}

/// Where a database's schema stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Schema {
    /// A newest migration this adapter was checked against.
    Known(String),
    /// A newer one, with every column read still there.
    Newer(String),
}

impl Schema {
    pub fn migration(&self) -> &str {
        match self {
            Schema::Known(m) | Schema::Newer(m) => m,
        }
    }
}

/// The database's newest migration, or `None` when it has none.
fn newest_migration(conn: &Connection) -> Option<String> {
    conn.query_row("SELECT max(id) FROM migration", [], |r| {
        r.get::<_, Option<String>>(0)
    })
    .ok()
    .flatten()
}

/// The columns of `table`; empty when there is no such table.
fn columns(conn: &Connection, table: &str) -> Vec<String> {
    let Ok(mut stmt) = conn.prepare("SELECT name FROM pragma_table_info(?1)") else {
        return Vec::new();
    };
    stmt.query_map([table], |r| r.get::<_, String>(0))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

/// Whether the database is one this adapter reads: its schema, or why
/// not, as the message `hippo doctor` shows.
pub fn check(conn: &Connection) -> std::result::Result<Schema, String> {
    let newest = newest_migration(conn);
    let named = newest.clone().unwrap_or_else(|| "(none)".into());
    if columns(conn, "session_v2").is_empty() || newest.as_deref().is_none_or(|m| m < OLDEST) {
        return Err(older(&named));
    }
    for (table, wanted) in REQUIRED {
        let have = columns(conn, table);
        if let Some(missing) = wanted.iter().find(|c| !have.iter().any(|h| h == *c)) {
            return Err(format!(
                "OpenCode database schema {named} has no {table}.{missing}, which hippo reads; \
                 a newer hippo is needed"
            ));
        }
    }
    let newest = newest.unwrap_or_default();
    Ok(if newest.as_str() > NEWEST_KNOWN {
        Schema::Newer(newest)
    } else {
        Schema::Known(newest)
    })
}

/// The message for a database from before OpenCode 2.
pub fn older(migration: &str) -> String {
    format!(
        "OpenCode database schema {migration} is older than hippo reads (OpenCode 2, \
         from {OLDEST}); starting a current OpenCode once upgrades it"
    )
}

/// The schema of the database at `db`, or why it is not read.
pub fn check_path(db: &Path) -> std::result::Result<Schema, String> {
    let conn = open(db).map_err(|e| format!("cannot open the OpenCode database: {e}"))?;
    check(&conn)
}

/// What the database says of a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub id: String,
    /// The session a sub-agent was started from.
    pub parent: Option<String>,
    /// For a fork, the session it was forked from and the boundary.
    pub fork_of: Option<String>,
    pub fork_boundary: Option<String>,
    pub directory: Option<PathBuf>,
    /// As written (not redacted). `None` until OpenCode names it.
    pub title: Option<String>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
    /// The start and the end of the newest run.
    pub busy: Option<OffsetDateTime>,
    pub idle: Option<OffsetDateTime>,
}

impl Info {
    /// Whether a run has started and not ended.
    pub fn running(&self) -> bool {
        self.busy.is_some_and(|b| self.idle.is_none_or(|i| b > i))
    }
}

/// Unix milliseconds as a UTC time.
fn ms(value: i64) -> Option<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(value) * 1_000_000).ok()
}

fn json_ms(field: Option<&Value>) -> Option<OffsetDateTime> {
    ms(field?.as_i64()?)
}

/// `time_busy` and `time_idle`, as SQL, or `NULL` where the schema has
/// no such column.
fn run_columns(conn: &Connection) -> (&'static str, &'static str) {
    let have = columns(conn, "session_v2");
    let has = |c: &str| have.iter().any(|h| h == c);
    (
        if has("time_busy") {
            "time_busy"
        } else {
            "NULL"
        },
        if has("time_idle") {
            "time_idle"
        } else {
            "NULL"
        },
    )
}

fn info_in(conn: &Connection, sid: &str) -> rusqlite::Result<Option<Info>> {
    let (busy, idle) = run_columns(conn);
    conn.query_row(
        &format!(
            "SELECT id, parent_id, fork_session_id, fork_boundary, directory, title, \
             time_created, time_updated, {busy}, {idle} FROM session_v2 WHERE id = ?1"
        ),
        [sid],
        |r| {
            let text = |i: usize| -> rusqlite::Result<Option<String>> {
                Ok(r.get::<_, Option<String>>(i)?.filter(|s| !s.is_empty()))
            };
            let created: i64 = r.get(6)?;
            let updated: i64 = r.get(7)?;
            Ok(Info {
                id: r.get(0)?,
                parent: text(1)?,
                fork_of: text(2)?,
                fork_boundary: text(3)?,
                directory: text(4)?.map(PathBuf::from),
                title: text(5)?,
                created: ms(created).unwrap_or(OffsetDateTime::UNIX_EPOCH),
                updated: ms(updated).unwrap_or(OffsetDateTime::UNIX_EPOCH),
                busy: r.get::<_, Option<i64>>(8)?.and_then(ms),
                idle: r.get::<_, Option<i64>>(9)?.and_then(ms),
            })
        },
    )
    .optional()
}

/// What the database says of the session at a [`session_path`]; `None`
/// when it cannot be read.
pub fn info(path: &Path) -> Option<Info> {
    let (db, sid) = split(path)?;
    let conn = open(&db).ok()?;
    check(&conn).ok()?;
    info_in(&conn, &sid).ok().flatten()
}

/// The session at the top of `info`'s `parent_id` chain: `None` for a
/// session that is not a sub-agent.
fn top_of(conn: &Connection, info: &Info) -> Option<String> {
    let mut top = info.parent.clone()?;
    for _ in 0..CHAIN_DEPTH {
        let up: Option<String> = conn
            .query_row(
                "SELECT parent_id FROM session_v2 WHERE id = ?1",
                [&top],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten()
            .flatten();
        match up {
            Some(up) if !up.is_empty() && up != top => top = up,
            _ => break,
        }
    }
    Some(top)
}

/// hippo's id for a session: `opencode-<id>`, or for a sub-agent
/// `opencode-<top>-agent-<id>`.
fn hippo_id(conn: &Connection, info: &Info) -> String {
    match top_of(conn, info) {
        Some(top) => format!(
            "opencode-{}-agent-{}",
            super::dsh::id_part(&top),
            super::dsh::id_part(&info.id)
        ),
        None => format!("opencode-{}", super::dsh::id_part(&info.id)),
    }
}

/// hippo's id for the session at a [`session_path`], and, for a
/// sub-agent, the id of the session it belongs under.
pub fn ids(path: &Path) -> Option<(String, Option<String>)> {
    let (db, sid) = split(path)?;
    let conn = open(&db).ok()?;
    check(&conn).ok()?;
    let info = info_in(&conn, &sid).ok()??;
    let parent = top_of(&conn, &info).map(|t| format!("opencode-{}", super::dsh::id_part(&t)));
    Some((hippo_id(&conn, &info), parent))
}

/// The sessions in the database at `db`, each with its stamp (its
/// `event_sequence` number, its `time_updated` and its count of messages)
/// and when it last changed (the newest of its `time_updated`,
/// `time_busy`, `time_idle` and its newest message's `time_created`).
/// Reads no message content: the counts and times come from the indexes.
pub fn list(db: &Path) -> Result<Vec<Listed>> {
    let conn = open(db).map_err(|e| sql_error(db, e))?;
    if let Err(what) = check(&conn) {
        return Err(invalid_data(db, &what));
    }
    let sequence = if columns(&conn, "event_sequence").is_empty() {
        "-1"
    } else {
        "coalesce((SELECT e.seq FROM event_sequence e WHERE e.aggregate_id = s.id), -1)"
    };
    let (busy, idle) = run_columns(&conn);
    let sql = format!(
        "SELECT s.id, s.time_updated, {sequence}, \
         (SELECT count(*) FROM session_message m WHERE m.session_id = s.id), \
         max(s.time_updated, coalesce({busy}, 0), coalesce({idle}, 0), \
         coalesce((SELECT max(m.time_created) FROM session_message m WHERE m.session_id = s.id), 0)) \
         FROM session_v2 s ORDER BY s.id"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| sql_error(db, e))?;
    let rows = stmt
        .query_map([], |r| {
            let id: String = r.get(0)?;
            let updated: i64 = r.get(1)?;
            let seq: i64 = r.get(2)?;
            let count: i64 = r.get(3)?;
            let changed: i64 = r.get(4)?;
            Ok(Listed {
                path: session_path(db, &id),
                stamp: format!("opencode:{seq}:{updated}:{count}"),
                modified: ms(changed),
            })
        })
        .map_err(|e| sql_error(db, e))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| sql_error(db, e))
}

/// One message, as read.
struct Message {
    kind: String,
    seq: i64,
    created: Option<OffsetDateTime>,
    data: Value,
}

/// The session's own messages in `seq` order: in a fork, those copied
/// from the session it was forked from are left out. Assistant messages
/// have their `session_part` rows put first in their `content`. A
/// message whose `data` is not JSON is skipped with a warning.
fn messages(conn: &Connection, info: &Info, path: &Path) -> rusqlite::Result<Vec<Message>> {
    let parts = parts_of(conn, &info.id)?;
    let own_after = fork_start(conn, info)?;
    let mut stmt = conn.prepare(
        "SELECT id, type, seq, time_created, data FROM session_message \
         WHERE session_id = ?1 ORDER BY seq",
    )?;
    let mut rows = stmt.query([&info.id])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let seq: i64 = r.get(2)?;
        let created: i64 = r.get(3)?;
        match own_after {
            ForkStart::AfterSeq(b) if seq <= b => continue,
            ForkStart::Since(t) if ms(created).is_some_and(|c| c < t) => continue,
            _ => {}
        }
        let text: String = r.get(4)?;
        let Ok(mut data) = serde_json::from_str::<Value>(&text) else {
            tracing::warn!(
                "{}: skipping message seq {seq}, whose data is not JSON",
                path.display()
            );
            continue;
        };
        let id: String = r.get(0)?;
        if let (Some(extra), Some(obj)) = (parts.get(&id), data.as_object_mut()) {
            let inline = obj
                .get("content")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let mut content = extra.clone();
            content.extend(inline);
            obj.insert("content".into(), Value::Array(content));
        }
        out.push(Message {
            kind: r.get(1)?,
            seq,
            created: json_ms(data.pointer("/time/created")).or_else(|| ms(created)),
            data,
        });
    }
    Ok(out)
}

/// `session_part` rows of a session by message, in `ordinal` order, each
/// a content item with its `type`.
fn parts_of(conn: &Connection, sid: &str) -> rusqlite::Result<HashMap<String, Vec<Value>>> {
    let mut out: HashMap<String, Vec<Value>> = HashMap::new();
    let have = columns(conn, "session_part");
    if !["message_id", "session_id", "ordinal", "type", "data"]
        .iter()
        .all(|c| have.iter().any(|h| h == c))
    {
        return Ok(out);
    }
    let mut stmt = conn.prepare(
        "SELECT message_id, type, data FROM session_part WHERE session_id = ?1 \
         ORDER BY message_id, ordinal",
    )?;
    let mut rows = stmt.query([sid])?;
    while let Some(r) = rows.next()? {
        let id: String = r.get(0)?;
        let kind: String = r.get(1)?;
        let text: String = r.get(2)?;
        let Ok(mut item) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if let Some(obj) = item.as_object_mut() {
            obj.entry("type").or_insert(Value::String(kind));
        }
        out.entry(id).or_default().push(item);
    }
    Ok(out)
}

/// Where a fork's own messages start.
enum ForkStart {
    /// Not a fork: every message is its own.
    All,
    /// After this `seq`: the copy ends at the boundary.
    AfterSeq(i64),
    /// From this time: the boundary is not in the database any more, and
    /// the copied messages are older than the fork.
    Since(OffsetDateTime),
}

fn fork_start(conn: &Connection, info: &Info) -> rusqlite::Result<ForkStart> {
    if info.fork_of.is_none() {
        return Ok(ForkStart::All);
    }
    let boundary: Option<Value> = info
        .fork_boundary
        .as_deref()
        .and_then(|b| serde_json::from_str(b).ok());
    let message = boundary
        .as_ref()
        .and_then(|b| b.get("messageID"))
        .and_then(Value::as_str);
    let through = boundary
        .as_ref()
        .and_then(|b| b.get("type"))
        .and_then(Value::as_str)
        != Some("before");
    if let Some(message) = message {
        let seq: Option<i64> = conn
            .query_row(
                "SELECT seq FROM session_message WHERE id = ?1",
                [message],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(seq) = seq {
            return Ok(ForkStart::AfterSeq(if through { seq } else { seq - 1 }));
        }
    }
    Ok(ForkStart::Since(info.created))
}

fn turns_of(conn: &Connection, info: &Info, path: &Path) -> rusqlite::Result<Vec<RawTurn>> {
    let mut turns = Vec::new();
    for m in messages(conn, info, path)? {
        let Some(ts) = m.created else {
            tracing::warn!(
                "{}: skipping message seq {}, which has no usable time",
                path.display(),
                m.seq
            );
            continue;
        };
        match m.kind.as_str() {
            "user" => {
                if let Some(text) = m.data.get("text").and_then(Value::as_str) {
                    if !text.trim().is_empty() {
                        push_turn(&mut turns, Role::User, ts, None, text.to_owned());
                    }
                }
            }
            "assistant" => {
                let Some(content) = m.data.get("content").and_then(Value::as_array) else {
                    continue;
                };
                for item in content {
                    add_item(&mut turns, item, ts);
                }
            }
            _ => {}
        }
    }
    Ok(turns)
}

/// The turns of one assistant content item.
fn add_item(turns: &mut Vec<RawTurn>, item: &Value, ts: OffsetDateTime) {
    match item.get("type").and_then(Value::as_str) {
        Some("text") => {
            if let Some(text) = item.get("text").and_then(Value::as_str) {
                if !text.trim().is_empty() {
                    push_turn(turns, Role::Assistant, ts, None, text.to_owned());
                }
            }
        }
        Some("tool") => {
            let Some(call) = Call::of(item) else { return };
            let at = json_ms(item.pointer("/time/created")).unwrap_or(ts);
            push_turn(turns, Role::Tool, at, Some(call.name.clone()), call.text());
            if let Some(result) = call.result {
                let done = json_ms(item.pointer("/time/completed"))
                    .or_else(|| json_ms(item.pointer("/time/ran")))
                    .unwrap_or(at);
                push_turn(turns, Role::Tool, done, Some(call.name), result);
            }
        }
        _ => {}
    }
}

/// A tool item, as stored.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    /// The call's id.
    pub id: String,
    /// The stored name ([`stored_name`]).
    pub name: String,
    /// The stored arguments ([`stored_input`]).
    pub input: Value,
    /// `streaming`, `running`, `completed` or `error`.
    pub status: String,
    /// The result's text, once the call finished ([`result_text`]).
    pub result: Option<String>,
}

impl Call {
    /// The call in a tool item; `None` when it has no name.
    pub fn of(item: &Value) -> Option<Call> {
        let raw = item.get("name").and_then(Value::as_str)?;
        let name = stored_name(raw).to_owned();
        let state = item.get("state").cloned().unwrap_or(Value::Null);
        let status = state
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let input = match state.get("input") {
            Some(Value::Object(map)) => stored_input(raw, map.clone()),
            // A call still streaming has its input as raw text.
            Some(Value::String(s)) => serde_json::from_str::<Value>(s)
                .ok()
                .and_then(|v| v.as_object().cloned())
                .map(|map| stored_input(raw, map))
                .unwrap_or(Value::Null),
            _ => Value::Null,
        };
        let result = match status.as_str() {
            "completed" => Some(result_text(
                &name,
                &content_text(state.get("content")),
                false,
            )),
            "error" => {
                let message = state
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .or_else(|| state.get("error").and_then(Value::as_str))
                    .unwrap_or("");
                let content = content_text(state.get("content"));
                let text = [message, content.as_str()]
                    .into_iter()
                    .filter(|t| !t.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                Some(result_text(&name, &text, true))
            }
            _ => None,
        };
        Some(Call {
            id: item
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            name,
            input,
            status,
            result,
        })
    }

    /// The call's turn text: `<name> <input as JSON>`, or the name alone.
    pub fn text(&self) -> String {
        match &self.input {
            Value::Object(map) if !map.is_empty() => format!("{} {}", self.name, self.input),
            _ => self.name.clone(),
        }
    }
}

/// The text items of a result's `content`, joined by newlines.
fn content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter(|i| i.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|i| i.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The name an OpenCode tool is stored under: Claude Code's name for the
/// core tools that do the same thing, else OpenCode's own. The names
/// before OpenCode 2 (`bash`, `task`) map as their successors do.
pub fn stored_name(name: &str) -> &str {
    match name {
        "shell" | "bash" => "Bash",
        "read" => "Read",
        "write" => "Write",
        "edit" => "Edit",
        "patch" | "apply_patch" => "apply_patch",
        "glob" => "Glob",
        "grep" => "Grep",
        "webfetch" => "WebFetch",
        "websearch" => "WebSearch",
        "todowrite" => "TodoWrite",
        "question" => "AskUserQuestion",
        "skill" => "Skill",
        "subagent" | "task" => "Agent",
        other => other,
    }
}

/// A call's arguments with Claude Code's names for the core tools' keys
/// ([`stored_name`]); every other key, and every other tool's, as written.
pub fn stored_input(name: &str, mut input: Map<String, Value>) -> Value {
    let rename = |input: &mut Map<String, Value>, from: &str, to: &str| {
        if !input.contains_key(to) {
            if let Some(v) = input.remove(from) {
                input.insert(to.to_owned(), v);
            }
        }
    };
    match stored_name(name) {
        "Read" | "Write" | "Edit" => {
            rename(&mut input, "filePath", "file_path");
            rename(&mut input, "path", "file_path");
            rename(&mut input, "oldString", "old_string");
            rename(&mut input, "newString", "new_string");
            rename(&mut input, "replaceAll", "replace_all");
        }
        "apply_patch" => rename(&mut input, "patchText", "input"),
        "Skill" => {
            rename(&mut input, "id", "skill");
            rename(&mut input, "name", "skill");
        }
        "Agent" => rename(&mut input, "agent", "subagent_type"),
        "AskUserQuestion" => {
            if let Some(Value::Array(questions)) = input.get_mut("questions") {
                for q in questions.iter_mut().filter_map(Value::as_object_mut) {
                    rename(q, "multiple", "multiSelect");
                }
            }
        }
        _ => {}
    }
    Value::Object(input)
}

/// A result's text as hippo's readers of failures know it: a `Bash`
/// result whose last line is `Exit code: N` (a signal name may follow)
/// starts `Exit code N` on a line of its own instead; a failed call is
/// wrapped in `<tool_use_error>`. Anything else as written.
pub fn result_text(stored: &str, text: &str, is_error: bool) -> String {
    let text = match (stored, exit_marker(text)) {
        ("Bash", Some((code, body))) => format!("Exit code {code}\n{body}"),
        _ => text.to_owned(),
    };
    if is_error {
        format!("<tool_use_error>{text}</tool_use_error>")
    } else {
        text
    }
}

/// `(N, the text before it)` when the last line is `Exit code: N`.
fn exit_marker(text: &str) -> Option<(i64, &str)> {
    let trimmed = text.trim_end();
    let (body, last) = match trimmed.rfind('\n') {
        Some(i) => (&trimmed[..i], &trimmed[i + 1..]),
        None => ("", trimmed),
    };
    let rest = last.strip_prefix("Exit code: ")?;
    let digits = rest.split_whitespace().next()?;
    Some((digits.parse().ok()?, body))
}

/// The model and the effort of the session's newest assistant message:
/// its `model.id` and `model.variant`, as written (not redacted).
pub fn model(path: &Path) -> (Option<String>, Option<String>) {
    let Some((db, sid)) = split(path) else {
        return (None, None);
    };
    let Ok(conn) = open(&db) else {
        return (None, None);
    };
    let data: Option<String> = conn
        .query_row(
            "SELECT data FROM session_message WHERE session_id = ?1 AND type = 'assistant' \
             ORDER BY seq DESC LIMIT 1",
            [&sid],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten();
    let Some(v) = data.and_then(|d| serde_json::from_str::<Value>(&d).ok()) else {
        return (None, None);
    };
    let text = |at: &str| {
        v.pointer(at)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    (text("/model/id"), text("/model/variant"))
}

/// Whether `title` is one OpenCode gives a session before it names it.
pub fn is_placeholder_title(title: &str) -> bool {
    ["New session - ", "Child session - "]
        .iter()
        .any(|p| title.starts_with(p))
}

/// The sessions of the OpenCode databases under `roots` changed at or
/// after `since` (Unix seconds), as [`list`] dates them.
pub fn sessions_since(roots: &[PathBuf], since: i64) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for db in databases(roots) {
        let Ok(listed) = list(&db) else { continue };
        out.extend(
            listed
                .into_iter()
                .filter(|l| l.modified.is_some_and(|m| m.unix_timestamp() >= since))
                .map(|l| l.path),
        );
    }
    out.sort();
    out.dedup();
    out
}

/// The OpenCode databases under `roots`: in a root, or in an OpenCode
/// data folder below it.
pub fn databases(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for root in roots {
        let entries = walkdir::WalkDir::new(root)
            .into_iter()
            .filter_entry(super::descend);
        for e in entries.flatten() {
            if e.file_type().is_file() && is_database(e.path()) {
                out.push(e.into_path());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// The time of the newest message of the session at a [`session_path`].
pub fn last_turn_at(path: &Path) -> Option<OffsetDateTime> {
    let (db, sid) = split(path)?;
    let conn = open(&db).ok()?;
    let t: Option<i64> = conn
        .query_row(
            "SELECT max(time_updated) FROM session_message WHERE session_id = ?1 \
             AND type IN ('user', 'assistant')",
            [&sid],
            |r| r.get(0),
        )
        .ok()?;
    t.and_then(ms)
}

/// The session's newest `max` user and assistant messages as Claude Code
/// transcript records, one JSON line each, oldest first: the shape Fleet's
/// tail reader knows. A person's message is a `user` record of text; an
/// assistant step an `assistant` record of its text and `tool_use` blocks
/// (stored names and arguments), with `stop_reason` `end_turn` once it
/// finished without a call; each finished call's result a `user` record
/// of one `tool_result`. Nothing is redacted: these lines are read and
/// dropped, never stored.
pub fn as_claude_lines(path: &Path, max: usize) -> Vec<String> {
    use serde_json::json;
    let Some((db, sid)) = split(path) else {
        return Vec::new();
    };
    let Ok(conn) = open(&db) else {
        return Vec::new();
    };
    let Ok(Some(info)) = info_in(&conn, &sid) else {
        return Vec::new();
    };
    let Ok(all) = messages(&conn, &info, path) else {
        return Vec::new();
    };
    let own: Vec<&Message> = all
        .iter()
        .filter(|m| matches!(m.kind.as_str(), "user" | "assistant"))
        .collect();
    let rfc = |t: OffsetDateTime| {
        t.format(&time::format_description::well_known::Rfc3339)
            .ok()
    };
    let mut out = Vec::new();
    for m in &own[own.len().saturating_sub(max)..] {
        let Some(ts) = m.created.and_then(rfc) else {
            continue;
        };
        if m.kind == "user" {
            let text = m.data.get("text").and_then(Value::as_str).unwrap_or("");
            out.push(
                json!({"type": "user", "timestamp": ts,
                       "message": {"role": "user", "content": [{"type": "text", "text": text}]}})
                .to_string(),
            );
            continue;
        }
        let mut content = Vec::new();
        let mut results = Vec::new();
        let mut calls = false;
        for item in m
            .data
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            match item.get("type").and_then(Value::as_str) {
                Some("text") => content.push(json!({"type": "text", "text": item.get("text")})),
                Some("tool") => {
                    let Some(call) = Call::of(item) else { continue };
                    calls = true;
                    content.push(json!({"type": "tool_use", "id": call.id,
                                        "name": call.name, "input": call.input}));
                    if let Some(result) = &call.result {
                        let at = json_ms(item.pointer("/time/completed"))
                            .and_then(rfc)
                            .unwrap_or_else(|| ts.clone());
                        results.push(json!({"type": "user", "timestamp": at,
                            "message": {"role": "user", "content": [{"type": "tool_result",
                            "tool_use_id": call.id, "content": result}]}}));
                    }
                }
                _ => {}
            }
        }
        let finished = m
            .data
            .pointer("/time/completed")
            .is_some_and(|v| !v.is_null());
        let stop = (finished && !calls).then_some("end_turn");
        let n = |k: &str| m.data.pointer(k).and_then(Value::as_u64).unwrap_or(0);
        out.push(
            json!({"type": "assistant", "timestamp": ts,
                   "message": {"role": "assistant", "content": content, "stop_reason": stop,
                               "usage": {"input_tokens": n("/tokens/input"),
                                         "cache_read_input_tokens": n("/tokens/cache/read"),
                                         "cache_creation_input_tokens": n("/tokens/cache/write")}}})
            .to_string(),
        );
        out.extend(results.into_iter().map(|r| r.to_string()));
    }
    out
}

#[cfg(test)]
pub(crate) mod tests;
