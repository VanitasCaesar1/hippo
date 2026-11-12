//! The transcript index: SQLite full-text search over redacted turns, kept
//! in `<home>/index.sqlite`.
//!
//! The file is an observation, not a record: it can be deleted and rebuilt
//! from the transcripts at any time (`hippocamp ingest --full`, which is
//! [`Index::rebuild`]). `sessions` has one row per transcript file, and
//! `turns` is an FTS5 table over the turns' `text`, the other fields of a
//! turn stored beside it unindexed. The file is in WAL mode with
//! `user_version` 2. `turn_keys` maps a session's turns, by ordinal, to
//! their rowids in `turns`, which FTS5 cannot look up by `session_id`
//! itself. Writers hold the ingest lock; readers open the file
//! with [`Index::open`]`(home, false)` and get [`Error::MissingIndex`] when
//! there is none.
//!
//! A transcript that could not be parsed is recorded as well, so that it is
//! not parsed again until it changes: [`Index::record_failure`] stores the
//! hash of its bytes and the error in the `error` column, which is null for
//! every other row. A transcript has one row; a successful
//! [`Index::replace_session`] removes any other.
//!
//! **Invariant 2.** No raw transcript text reaches the index. The only
//! method that writes turn text, [`Index::replace_session`], takes
//! [`Turn`]s, and a `Turn`'s text is a [`Redacted`], which only
//! [`redact`] can make.
//!
//! Timestamps (`ts`, `first_turn`, `last_turn`) are stored in one
//! fixed-width RFC 3339 form, UTC with six fractional digits and `Z`
//! (`2026-11-08T03:00:00.000000Z`), so text order is time order even
//! within a second. Hits and stored turns give those strings back.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OpenFlags, TransactionBehavior};
use time::{OffsetDateTime, UtcOffset};

use crate::adapters::{RawTurn, Role};
use crate::error::{Error, Result};
use crate::records::Tool;
use crate::redact::{redact, Redacted};

/// The index's file name inside `$HIPPOCAMP_HOME`.
pub(crate) const FILE_NAME: &str = "index.sqlite";

/// The most hits one [`Index::recall`] returns (Search: maximum 200).
pub const MAX_LIMIT: usize = 200;

/// `PRAGMA user_version` of an index this code reads and writes.
const FORMAT_VERSION: i64 = 2;

/// The format before `turn_keys`. Readers still open it; the next writer
/// upgrades it in place.
const FORMAT_1: i64 = 1;

/// The id of the row of a transcript that never parsed is this and the
/// transcript's path. A session id has no `:`, so it cannot be one.
const ERROR_ID_PREFIX: &str = "error:";

/// The schema, run once when an index is created.
const DDL: &str = "\
CREATE TABLE sessions(id TEXT PRIMARY KEY, tool TEXT NOT NULL, transcript_path TEXT NOT NULL, first_turn TEXT, last_turn TEXT, file_hash TEXT NOT NULL, error TEXT);
CREATE INDEX sessions_path ON sessions(transcript_path);
CREATE VIRTUAL TABLE turns USING fts5(session_id UNINDEXED, ordinal UNINDEXED, role UNINDEXED, ts UNINDEXED, tool_name UNINDEXED, text, tokenize='unicode61 remove_diacritics 2');
CREATE TABLE turn_keys(session_id TEXT NOT NULL, ordinal INTEGER NOT NULL, turn INTEGER NOT NULL, PRIMARY KEY(session_id, ordinal)) WITHOUT ROWID;
PRAGMA user_version=2;";

/// The upgrade from format 1: the keys of the turns already there.
const UPGRADE_1_TO_2: &str = "\
CREATE TABLE turn_keys(session_id TEXT NOT NULL, ordinal INTEGER NOT NULL, turn INTEGER NOT NULL, PRIMARY KEY(session_id, ordinal)) WITHOUT ROWID;
INSERT INTO turn_keys SELECT session_id, ordinal, rowid FROM turns;
PRAGMA user_version=2;";

/// How many of a session's first turns [`Index::openings`] looks at.
pub const OPENING_WINDOW: i64 = 12;

/// The most sessions one format-1 scan of [`Index::turns`] looks for.
const SCAN_CHUNK: usize = 500;

/// The recall query, as the design gives it. Column 5 of `turns` is `text`.
const RECALL_SQL: &str = "SELECT session_id, ordinal, role, ts, tool_name, snippet(turns, 5, '', '', '…', 24), bm25(turns) FROM turns WHERE turns MATCH ?1 ORDER BY bm25(turns), ts DESC LIMIT ?2";

/// One indexable turn. Its text is [`Redacted`], so a turn cannot be built
/// from raw transcript text (Invariant 2):
///
/// ```compile_fail,E0423
/// use hippocamp_core::redact::Redacted;
/// // `Redacted` has a private field; only `redact` makes one.
/// let _ = Redacted(String::from("raw transcript text"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    /// Counts emitted turns from 0 within the session.
    pub ordinal: u32,
    pub role: Role,
    /// The containing record's timestamp.
    pub ts: OffsetDateTime,
    /// Set for `tool` turns.
    pub tool_name: Option<String>,
    pub text: Redacted,
}

impl Turn {
    /// The indexable form of a parsed turn: its text, and its tool name
    /// too, go through [`redact`].
    pub fn from_raw(raw: RawTurn) -> Turn {
        Turn {
            ordinal: raw.ordinal,
            role: raw.role,
            ts: raw.ts,
            tool_name: raw.tool_name.map(|name| redact(&name).as_str().to_owned()),
            text: redact(&raw.text),
        }
    }
}

/// Where a session came from: the columns of its `sessions` row that the
/// turns do not give.
#[derive(Debug, Clone, Copy)]
pub struct SessionSource<'a> {
    /// The session id per Conventions.
    pub id: &'a str,
    pub tool: Tool,
    /// The transcript file the turns were parsed from.
    pub transcript_path: &'a Path,
    /// Lowercase hex SHA-256 of that file's bytes.
    pub file_hash: &'a str,
}

/// One search result: a turn that matched, as the index stores it.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    /// The session id.
    pub session: String,
    /// The session's tool, `claude-code` or `codex`.
    pub tool: String,
    pub ordinal: u32,
    /// `user`, `assistant` or `tool`.
    pub role: String,
    /// RFC 3339 in UTC.
    pub ts: String,
    /// Set for `tool` turns.
    pub tool_name: Option<String>,
    /// Up to 24 tokens around the match, `…` where the text goes on. It can
    /// span several lines; the one-line output must flatten it.
    pub snippet: String,
    /// The bm25 rank: lower is a better match, and it is negative.
    pub score: f64,
}

/// One turn read back from the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredTurn {
    pub ordinal: u32,
    /// `user`, `assistant` or `tool`.
    pub role: String,
    /// RFC 3339 in UTC.
    pub ts: String,
    pub tool_name: Option<String>,
    /// The redacted text.
    pub text: String,
}

/// A turn read back from the index with its session's id: what dreaming
/// gathers as input and cites as evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTurn {
    pub session: String,
    pub ordinal: u32,
    /// `user`, `assistant` or `tool`.
    pub role: String,
    /// RFC 3339 in UTC.
    pub ts: String,
    pub tool_name: Option<String>,
    /// The redacted text.
    pub text: String,
}

/// The most characters of a turn's text a [`TurnMark`] keeps from its
/// start, and from its end.
pub const MARK_HEAD: usize = 2_000;
pub const MARK_TAIL: usize = 400;

/// One turn as [`Index::turn_marks`] reads it: its place and kind, the
/// start and end of its text, and for a tool call the file, description
/// and command its input names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnMark {
    pub ordinal: u32,
    /// `user`, `assistant` or `tool`.
    pub role: String,
    /// RFC 3339 in UTC.
    pub ts: String,
    pub tool_name: Option<String>,
    /// The text's first [`MARK_HEAD`] characters.
    pub head: String,
    /// Its last [`MARK_TAIL`] characters, when the head does not hold all
    /// of it.
    pub tail: Option<String>,
    /// A call's `file_path`, `notebook_path` or `path`.
    pub file: Option<String>,
    /// A call's `description`, else its `title`.
    pub about: Option<String>,
    /// A call's `command` (an array as its JSON text), cut to
    /// [`MARK_HEAD`] characters.
    pub command: Option<String>,
    /// It is a tool call whose input is JSON.
    pub call: bool,
}

/// A transcript whose latest parse failed, as [`Index::record_failure`]
/// recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedFile {
    pub transcript_path: String,
    /// Lowercase hex SHA-256 of the bytes that did not parse.
    pub file_hash: String,
    /// The redacted error.
    pub error: String,
}

/// An open transcript index.
#[derive(Debug)]
pub struct Index {
    conn: Connection,
    path: PathBuf,
    format: i64,
}

impl Index {
    /// Opens `<home>/index.sqlite`. When there is none, or only an empty,
    /// never-initialised database file, `create` false gives
    /// [`Error::MissingIndex`] and `create` true makes one: the DDL, WAL
    /// mode and `user_version` 2, with the directory if need be.
    ///
    /// A writer (`create` true) upgrades a format-1 index in place, in one
    /// transaction: readers keep the old snapshot until it commits. A reader
    /// accepts formats 1 and 2 and changes nothing. An index of another
    /// format fails with a message naming `hippocamp ingest --full`, which
    /// writes a new one.
    ///
    /// `create` true is for writers. It also puts an existing index back in
    /// WAL mode (a [`rebuild`](Index::rebuild) leaves a rollback-journal
    /// file) and sets `synchronous` to `NORMAL`: with WAL that can lose the
    /// last commits to a power cut but not corrupt the file, and the index
    /// can be rebuilt.
    pub fn open(home: &Path, create: bool) -> Result<Index> {
        let path = home.join(FILE_NAME);
        let present = match fs::metadata(&path) {
            Ok(_) => true,
            Err(e) if e.kind() == io::ErrorKind::NotFound => false,
            Err(e) => return Err(Error::io_at(&path, e)),
        };
        if !present && !create {
            return Err(Error::MissingIndex);
        }
        if !present {
            fs::create_dir_all(home).map_err(|e| Error::io_at(home, e))?;
        }
        Index::open_at(path, create)
    }

    fn open_at(path: PathBuf, create: bool) -> Result<Index> {
        let mut flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        if create {
            flags |= OpenFlags::SQLITE_OPEN_CREATE;
        }
        let mut conn = Connection::open_with_flags(&path, flags)?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        let empty: bool = conn.query_row(
            "SELECT NOT EXISTS(SELECT 1 FROM sqlite_master)",
            [],
            |row| row.get(0),
        )?;
        let mut format = version;
        if version == FORMAT_VERSION {
            if create {
                tune_for_writing(&conn)?;
            }
        } else if version == FORMAT_1 {
            if create {
                tune_for_writing(&conn)?;
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                tx.execute_batch(UPGRADE_1_TO_2)?;
                tx.commit()?;
                format = FORMAT_VERSION;
            }
        } else if version == 0 && empty {
            if !create {
                return Err(Error::MissingIndex);
            }
            tune_for_writing(&conn)?;
            let tx = conn.transaction()?;
            tx.execute_batch(DDL)?;
            tx.commit()?;
            format = FORMAT_VERSION;
        } else {
            let message = format!(
                "{}: index format {version}, expected 1 or 2; run hippocamp ingest --full",
                path.display()
            );
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                message,
            )));
        }
        Ok(Index { conn, path, format })
    }

    /// The index's format: 1 for one a reader opened before a writer
    /// upgraded it, otherwise 2.
    pub fn format(&self) -> i64 {
        self.format
    }

    /// Builds a new index in `<home>/index.sqlite.tmp`, lets `fill` write
    /// the sessions into it, and swaps it in over `index.sqlite`; the
    /// result is `fill`'s. Until the swap the old index is untouched, and
    /// when `fill` fails it stays.
    ///
    /// The new file is checkpointed, switched to `journal_mode=DELETE` and
    /// closed, any stale `index.sqlite-wal` and `-shm` are removed, and it
    /// is renamed over `index.sqlite`, so nothing but that one file is left
    /// behind. The next [`Index::open`] with `create` true restores WAL.
    /// Callers hold the ingest lock.
    pub fn rebuild<T>(home: &Path, fill: impl FnOnce(&mut Index) -> Result<T>) -> Result<T> {
        fs::create_dir_all(home).map_err(|e| Error::io_at(home, e))?;
        let target = home.join(FILE_NAME);
        let temp = home.join(format!("{FILE_NAME}.tmp"));
        remove_with_sidecars(&temp)?;
        let built = Index::open_at(temp.clone(), true).and_then(|mut index| {
            let value = fill(&mut index)?;
            index.finish()?;
            Ok(value)
        });
        let value = match built {
            Ok(value) => value,
            Err(err) => {
                // Best effort: the error that matters is the one being returned.
                let _ = remove_with_sidecars(&temp);
                return Err(err);
            }
        };
        // Never inherit a stale write-ahead log: its frames would replay
        // into the new file.
        remove_if_exists(&with_suffix(&target, "-wal"))?;
        remove_if_exists(&with_suffix(&target, "-shm"))?;
        remove_if_exists(&with_suffix(&temp, "-wal"))?;
        remove_if_exists(&with_suffix(&temp, "-shm"))?;
        fs::rename(&temp, &target).map_err(|e| Error::io_at(&target, e))?;
        Ok(value)
    }

    /// Checkpoints the write-ahead log into the file, leaves WAL mode and
    /// closes the connection, so the file stands alone.
    fn finish(self) -> Result<()> {
        let Index { conn, path, .. } = self;
        conn.flush_prepared_statement_cache();
        let busy: i64 = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?;
        if busy != 0 {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::WouldBlock,
                format!(
                    "{}: the write-ahead log could not be checkpointed",
                    path.display()
                ),
            )));
        }
        let mode: String =
            conn.pragma_update_and_check(None, "journal_mode", "DELETE", |row| row.get(0))?;
        if !mode.eq_ignore_ascii_case("delete") {
            return Err(Error::Io(io::Error::other(format!(
                "{}: journal mode is {mode}, not delete",
                path.display()
            ))));
        }
        conn.close().map_err(|(_, err)| Error::Sqlite(err))
    }

    /// Replaces everything the index holds for `session`: in one
    /// transaction it removes the other rows of the same transcript (its
    /// failure, or the session it held under another id, with that
    /// session's turns), deletes the session's turns, inserts `turns` and
    /// upserts the `sessions` row, which has no error. `first_turn` and
    /// `last_turn` are the timestamps of the first and the last of `turns`
    /// (in ordinal order), or null when there are none.
    ///
    /// A turn's `ts` is stored in UTC. The deletes go through `turn_keys`,
    /// so none of them scans the turns.
    pub fn replace_session(&mut self, session: &SessionSource<'_>, turns: &[Turn]) -> Result<()> {
        let first_turn = turns.first().map(|turn| format_ts(turn.ts)).transpose()?;
        let last_turn = turns.last().map(|turn| format_ts(turn.ts)).transpose()?;
        let path = session.transcript_path.to_string_lossy();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let others: Vec<(String, bool)> = {
            let mut stmt = tx.prepare_cached(
                "SELECT id, first_turn IS NOT NULL FROM sessions \
                 WHERE transcript_path = ?1 AND id != ?2",
            )?;
            let rows = stmt.query_map(params![path, session.id], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for (id, has_turns) in &others {
            // A row with no first turn has no turns to delete.
            if *has_turns {
                delete_turns(&tx, id)?;
            }
            tx.execute("DELETE FROM sessions WHERE id = ?1", [id])?;
        }
        if session_row_exists(&tx, session.id)? {
            delete_turns(&tx, session.id)?;
        }
        {
            let mut insert = tx.prepare_cached(
                "INSERT INTO turns(session_id, ordinal, role, ts, tool_name, text) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            let mut key = tx.prepare_cached(
                "INSERT INTO turn_keys(session_id, ordinal, turn) VALUES (?1, ?2, ?3)",
            )?;
            for turn in turns {
                insert.execute(params![
                    session.id,
                    turn.ordinal,
                    turn.role.as_str(),
                    format_ts(turn.ts)?,
                    turn.tool_name,
                    turn.text.as_str(),
                ])?;
                key.execute(params![session.id, turn.ordinal, tx.last_insert_rowid()])?;
            }
        }
        tx.execute(
            "INSERT OR REPLACE INTO sessions(id, tool, transcript_path, first_turn, last_turn, file_hash, error) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)",
            params![
                session.id,
                session.tool.as_str(),
                path,
                first_turn,
                last_turn,
                session.file_hash,
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Records that the transcript at `transcript_path`, whose bytes hash
    /// to `file_hash`, could not be parsed, so that [`Index::file_hashes`]
    /// holds the hash and ingest leaves the file alone until it changes.
    /// The error is [`Redacted`], like everything else the index stores: a
    /// parser's message can quote the transcript.
    ///
    /// A transcript that has a row keeps it, turns and all (they are from
    /// the last parse that worked), with the new hash and the error. One
    /// that has none gets the row `error:<transcript path>`: `tool`, the
    /// path, the hash and the error, no first or last turn and no turns. A
    /// later [`Index::replace_session`] for the transcript removes either
    /// kind of error.
    pub fn record_failure(
        &mut self,
        tool: Tool,
        transcript_path: &Path,
        file_hash: &str,
        error: &Redacted,
    ) -> Result<()> {
        let path = transcript_path.to_string_lossy();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let marked = tx.execute(
            "UPDATE sessions SET file_hash = ?2, error = ?3 WHERE transcript_path = ?1",
            params![path, file_hash, error.as_str()],
        )?;
        if marked == 0 {
            tx.execute(
                "INSERT OR REPLACE INTO sessions(id, tool, transcript_path, first_turn, last_turn, file_hash, error) \
                 VALUES (?1, ?2, ?3, NULL, NULL, ?4, ?5)",
                params![
                    format!("{ERROR_ID_PREFIX}{path}"),
                    tool.as_str(),
                    path,
                    file_hash,
                    error.as_str(),
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Removes the failure row of a transcript that has only a failure (it
    /// never parsed), so a file no adapter reads any more stops counting as
    /// a parse failure. A session's own row is left alone. Returns whether
    /// a row was removed.
    pub fn forget_failure(&mut self, transcript_path: &str) -> Result<bool> {
        let removed = self.conn.execute(
            "DELETE FROM sessions WHERE transcript_path = ?1 AND error IS NOT NULL AND id = ?2",
            params![
                transcript_path,
                format!("{ERROR_ID_PREFIX}{transcript_path}")
            ],
        )?;
        Ok(removed > 0)
    }

    /// The transcripts whose latest parse failed, by path.
    pub fn failed_files(&self) -> Result<Vec<FailedFile>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT transcript_path, file_hash, error FROM sessions \
             WHERE error IS NOT NULL ORDER BY transcript_path",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(FailedFile {
                transcript_path: row.get(0)?,
                file_hash: row.get(1)?,
                error: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The file hash of every transcript the index holds a row for,
    /// parsed or failed, by the transcript path as stored: what ingest
    /// compares a file with to skip it.
    pub fn file_hashes(&self) -> Result<HashMap<String, String>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT transcript_path, file_hash FROM sessions")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Every parsed session's file hash, tool and transcript path, by id:
    /// what the rollup compares with to find the sessions to compute again
    /// (`crate::rollup::refresh`). Transcripts that failed are left out.
    pub fn session_hashes(&self) -> Result<HashMap<String, (String, String, PathBuf)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, file_hash, tool, transcript_path FROM sessions WHERE error IS NULL",
        )?;
        let rows = stmt.query_map([], |row| {
            let path: String = row.get(3)?;
            Ok((row.get(0)?, (row.get(1)?, row.get(2)?, PathBuf::from(path))))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Searches the turns, best match first and, among equal matches, the
    /// newest first: at most `limit` hits, never more than [`MAX_LIMIT`].
    ///
    /// Unless `raw`, `query` is one quoted phrase (its inner `"` doubled),
    /// so no query is invalid. A `raw` query is FTS5 syntax, and one that
    /// is not valid fails with the index's own error ([`Error::Sqlite`]).
    pub fn recall(&self, query: &str, limit: usize, raw: bool) -> Result<Vec<Hit>> {
        let limit = limit.min(MAX_LIMIT) as i64;
        let phrase;
        let query = if raw {
            query
        } else {
            phrase = quote_phrase(query);
            &phrase
        };
        // One snapshot for both statements, so every hit's session is there.
        let snapshot = self.conn.unchecked_transaction()?;
        let mut hits = {
            let mut stmt = snapshot.prepare_cached(RECALL_SQL)?;
            let rows = stmt.query_map(params![query, limit], |row| {
                Ok(Hit {
                    session: row.get(0)?,
                    tool: String::new(),
                    ordinal: row.get(1)?,
                    role: row.get(2)?,
                    ts: row.get(3)?,
                    tool_name: row.get(4)?,
                    snippet: row.get(5)?,
                    score: row.get(6)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<Hit>>>()?
        };
        let mut tool_of = snapshot.prepare_cached("SELECT tool FROM sessions WHERE id = ?1")?;
        for hit in &mut hits {
            hit.tool = tool_of.query_row([&hit.session], |row| row.get(0))?;
        }
        Ok(hits)
    }

    /// Whether the index holds turn `ordinal` of `session`.
    pub fn exists(&self, session: &str, ordinal: u32) -> Result<bool> {
        if !session_row_exists(&self.conn, session)? {
            return Ok(false);
        }
        let sql = if self.format >= 2 {
            "SELECT EXISTS(SELECT 1 FROM turn_keys WHERE session_id = ?1 AND ordinal = ?2)"
        } else {
            "SELECT EXISTS(SELECT 1 FROM turns WHERE session_id = ?1 AND ordinal = ?2)"
        };
        let mut stmt = self.conn.prepare_cached(sql)?;
        Ok(stmt.query_row(params![session, ordinal], |row| row.get(0))?)
    }

    /// Every turn of `session`, by ordinal; none for a session the index
    /// does not hold.
    pub fn turns_for_session(&self, id: &str) -> Result<Vec<StoredTurn>> {
        if !session_row_exists(&self.conn, id)? {
            return Ok(Vec::new());
        }
        let sql = if self.format >= 2 {
            "SELECT k.ordinal, t.role, t.ts, t.tool_name, t.text FROM turn_keys k JOIN turns t ON t.rowid = k.turn WHERE k.session_id = ?1 ORDER BY k.ordinal"
        } else {
            "SELECT ordinal, role, ts, tool_name, text FROM turns WHERE session_id = ?1 ORDER BY ordinal"
        };
        let mut stmt = self.conn.prepare_cached(sql)?;
        let rows = stmt.query_map([id], |row| {
            Ok(StoredTurn {
                ordinal: row.get(0)?,
                role: row.get(1)?,
                ts: row.get(2)?,
                tool_name: row.get(3)?,
                text: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The ids of `parent`'s sub-agent sessions (`<parent>-agent-…`), sorted.
    pub fn sub_agents(&self, parent: &str) -> Result<Vec<String>> {
        let prefix = format!("{parent}-agent-");
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id FROM sessions WHERE substr(id, 1, ?2) = ?1 ORDER BY id")?;
        let rows = stmt.query_map(params![prefix, prefix.len() as i64], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The turns of `session` that can name a note, by ordinal: user and
    /// assistant turns, and tool calls (a tool turn whose text starts with
    /// its tool's name and a space), whose text holds `.md` or `[[`. Tool
    /// results are left out.
    pub fn note_mentions(&self, id: &str) -> Result<Vec<StoredTurn>> {
        if !session_row_exists(&self.conn, id)? {
            return Ok(Vec::new());
        }
        let filter = "(instr(t.text, '.md') > 0 OR instr(t.text, '[[') > 0) \
             AND (t.role <> 'tool' OR (t.tool_name IS NOT NULL \
             AND substr(t.text, 1, length(t.tool_name) + 1) = t.tool_name || ' '))";
        let sql = if self.format >= 2 {
            format!("SELECT k.ordinal, t.role, t.ts, t.tool_name, t.text FROM turn_keys k JOIN turns t ON t.rowid = k.turn WHERE k.session_id = ?1 AND {filter} ORDER BY k.ordinal")
        } else {
            format!("SELECT t.ordinal, t.role, t.ts, t.tool_name, t.text FROM turns t WHERE t.session_id = ?1 AND {filter} ORDER BY CAST(t.ordinal AS INTEGER)")
        };
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map([id], |row| {
            Ok(StoredTurn {
                ordinal: row.get(0)?,
                role: row.get(1)?,
                ts: row.get(2)?,
                tool_name: row.get(3)?,
                text: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The tool calls of session `id` whose input holds `needle` (a tool
    /// turn whose text starts with its tool's name and a space), by
    /// ordinal. Tool results are left out.
    pub fn calls_holding(&self, id: &str, needle: &str) -> Result<Vec<StoredTurn>> {
        if !session_row_exists(&self.conn, id)? {
            return Ok(Vec::new());
        }
        let filter = "instr(t.text, ?2) > 0 AND t.role = 'tool' AND t.tool_name IS NOT NULL \
             AND substr(t.text, 1, length(t.tool_name) + 1) = t.tool_name || ' '";
        let sql = if self.format >= 2 {
            format!("SELECT k.ordinal, t.role, t.ts, t.tool_name, t.text FROM turn_keys k JOIN turns t ON t.rowid = k.turn WHERE k.session_id = ?1 AND {filter} ORDER BY k.ordinal")
        } else {
            format!("SELECT t.ordinal, t.role, t.ts, t.tool_name, t.text FROM turns t WHERE t.session_id = ?1 AND {filter} ORDER BY CAST(t.ordinal AS INTEGER)")
        };
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(rusqlite::params![id, needle], |row| {
            Ok(StoredTurn {
                ordinal: row.get(0)?,
                role: row.get(1)?,
                ts: row.get(2)?,
                tool_name: row.get(3)?,
                text: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The number of turns of every indexed session, in one scan of
    /// `turn_keys` (of the `turns` table on a format-1 index).
    pub fn turn_counts(&self) -> Result<HashMap<String, u64>> {
        let sql = if self.format >= 2 {
            "SELECT session_id, COUNT(*) FROM turn_keys GROUP BY session_id"
        } else {
            "SELECT session_id, COUNT(*) FROM turns GROUP BY session_id"
        };
        let mut stmt = self.conn.prepare_cached(sql)?;
        let rows = stmt.query_map([], |row| {
            let count: i64 = row.get(1)?;
            Ok((row.get(0)?, u64::try_from(count).unwrap_or(0)))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// How many sessions the index holds.
    pub fn session_count(&self) -> Result<u64> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    /// Every indexed session's tool (as the index writes it) and transcript
    /// path, by id.
    pub fn sources(&self) -> Result<HashMap<String, (String, PathBuf)>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id, tool, transcript_path FROM sessions")?;
        let rows = stmt.query_map([], |row| {
            let path: String = row.get(2)?;
            Ok((row.get(0)?, (row.get(1)?, PathBuf::from(path))))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// One session's tool and transcript path, when it is indexed.
    pub fn source(&self, id: &str) -> Result<Option<(String, PathBuf)>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT tool, transcript_path FROM sessions WHERE id = ?1")?;
        let mut rows = stmt.query_map([id], |row| {
            let path: String = row.get(1)?;
            Ok((row.get(0)?, PathBuf::from(path)))
        })?;
        Ok(rows.next().transpose()?)
    }

    /// Each session's opening user turns: those among its first
    /// [`OPENING_WINDOW`] turns, at most `n` of them, in order, each cut to
    /// `max_chars` characters. One query; sessions without a user turn in
    /// that window are left out. On a format-1 index it scans every turn.
    pub fn openings(&self, n: usize, max_chars: usize) -> Result<HashMap<String, Vec<String>>> {
        let sql = if self.format >= 2 {
            "SELECT k.session_id, substr(t.text, 1, ?2) FROM turn_keys k JOIN turns t ON t.rowid = k.turn WHERE k.ordinal < ?1 AND t.role = 'user' ORDER BY k.session_id, k.ordinal"
        } else {
            "SELECT session_id, substr(text, 1, ?2) FROM turns WHERE CAST(ordinal AS INTEGER) < ?1 AND role = 'user' ORDER BY session_id, CAST(ordinal AS INTEGER)"
        };
        let mut stmt = self.conn.prepare_cached(sql)?;
        let max = i64::try_from(max_chars).unwrap_or(i64::MAX);
        let rows = stmt.query_map(params![OPENING_WINDOW, max], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out: HashMap<String, Vec<String>> = HashMap::new();
        for row in rows {
            let (id, text) = row?;
            let turns = out.entry(id).or_default();
            if turns.len() < n {
                turns.push(text);
            }
        }
        Ok(out)
    }

    /// [`Index::openings`] for the sessions `ids` only, by their keys; on a
    /// format-1 index, one scan for all of them.
    pub fn openings_for(
        &self,
        ids: &[&str],
        n: usize,
        max_chars: usize,
    ) -> Result<HashMap<String, Vec<String>>> {
        if self.format < 2 {
            let mut all = self.openings(n, max_chars)?;
            all.retain(|id, _| ids.contains(&id.as_str()));
            return Ok(all);
        }
        let mut stmt = self.conn.prepare_cached(
            "SELECT substr(t.text, 1, ?3) FROM turn_keys k JOIN turns t ON t.rowid = k.turn WHERE k.session_id = ?1 AND k.ordinal < ?2 AND t.role = 'user' ORDER BY k.ordinal LIMIT ?4",
        )?;
        let max = i64::try_from(max_chars).unwrap_or(i64::MAX);
        let limit = i64::try_from(n).unwrap_or(i64::MAX);
        let mut out = HashMap::new();
        for id in ids {
            let rows = stmt.query_map(params![id, OPENING_WINDOW, max, limit], |row| row.get(0))?;
            let texts: Vec<String> = rows.collect::<rusqlite::Result<_>>()?;
            if !texts.is_empty() {
                out.insert((*id).to_string(), texts);
            }
        }
        Ok(out)
    }

    /// The ids of the sessions with a turn at or after `start` and before
    /// `end`, sorted. It scans the turns, as every lookup by an unindexed
    /// column does.
    pub fn sessions_between(
        &self,
        start: OffsetDateTime,
        end: OffsetDateTime,
    ) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT DISTINCT session_id FROM turns WHERE ts >= ?1 AND ts < ?2 ORDER BY session_id",
        )?;
        let rows = stmt.query_map(params![format_ts(start)?, format_ts(end)?], |row| {
            row.get(0)
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The ids of the sessions whose last turn is at or after `since`,
    /// sorted: one read of the `sessions` table, no turn scan. A session
    /// that ran before `since` and after it is included; one that ended
    /// before is not.
    pub fn sessions_active_since(&self, since: OffsetDateTime) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id FROM sessions WHERE error IS NULL AND last_turn >= ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map([format_ts(since)?], |row| row.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The `user` turns at or after `since` whose text is at most
    /// `max_chars` characters, by session and ordinal: the lessons pass's
    /// candidates for a person's turn. One scan of the turns.
    pub fn user_turns_since(
        &self,
        since: OffsetDateTime,
        max_chars: usize,
    ) -> Result<Vec<SessionTurn>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT session_id, CAST(ordinal AS INTEGER), role, ts, tool_name, text FROM turns WHERE role = 'user' AND ts >= ?1 AND length(text) <= ?2 ORDER BY session_id, CAST(ordinal AS INTEGER)",
        )?;
        let max = i64::try_from(max_chars).unwrap_or(i64::MAX);
        let rows = stmt.query_map(params![format_ts(since)?, max], session_turn_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The tool results at or after `since` that report a failure, the
    /// first `max_chars` characters of each: text starting `Exit code `
    /// (Claude Code), `Exit code: ` (Codex) or `<tool_use_error>`. Whether
    /// the exit code is 0 is left to the caller. One scan of the turns.
    pub fn failed_results_since(
        &self,
        since: OffsetDateTime,
        max_chars: usize,
    ) -> Result<Vec<SessionTurn>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT session_id, CAST(ordinal AS INTEGER), role, ts, tool_name, substr(text, 1, ?2) FROM turns WHERE role = 'tool' AND ts >= ?1 AND (substr(text, 1, 10) = 'Exit code ' OR substr(text, 1, 11) = 'Exit code: ' OR substr(text, 1, 16) = '<tool_use_error>') ORDER BY session_id, CAST(ordinal AS INTEGER)",
        )?;
        let max = i64::try_from(max_chars).unwrap_or(i64::MAX);
        let rows = stmt.query_map(params![format_ts(since)?, max], session_turn_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Up to `n` turns of `session` before `ordinal`, nearest first, each
    /// cut to its first `max_chars` characters.
    pub fn turns_before(
        &self,
        session: &str,
        ordinal: u32,
        n: u32,
        max_chars: usize,
    ) -> Result<Vec<SessionTurn>> {
        let sql = if self.format >= 2 {
            "SELECT k.session_id, k.ordinal, t.role, t.ts, t.tool_name, substr(t.text, 1, ?4) FROM turn_keys k JOIN turns t ON t.rowid = k.turn WHERE k.session_id = ?1 AND k.ordinal < ?2 AND k.ordinal >= ?3 ORDER BY k.ordinal DESC"
        } else {
            "SELECT session_id, CAST(ordinal AS INTEGER), role, ts, tool_name, substr(text, 1, ?4) FROM turns WHERE session_id = ?1 AND CAST(ordinal AS INTEGER) < ?2 AND CAST(ordinal AS INTEGER) >= ?3 ORDER BY CAST(ordinal AS INTEGER) DESC"
        };
        let mut stmt = self.conn.prepare_cached(sql)?;
        let max = i64::try_from(max_chars).unwrap_or(i64::MAX);
        let rows = stmt.query_map(
            params![session, ordinal, ordinal.saturating_sub(n), max],
            session_turn_row,
        )?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The turns of `session` from `start` up to (not including) `end`, by
    /// ordinal, each as a [`TurnMark`]: what a day's lanes and a
    /// correction's step read. A tool call's
    /// file, description and command are read from its input in the
    /// query, so a large call is not read whole.
    pub fn turn_marks(
        &self,
        session: &str,
        start: OffsetDateTime,
        end: OffsetDateTime,
    ) -> Result<Vec<TurnMark>> {
        let source = if self.format >= 2 {
            "SELECT k.ordinal AS ordinal, t.role AS role, t.ts AS ts, t.tool_name AS tool_name, t.text AS text FROM turn_keys k JOIN turns t ON t.rowid = k.turn WHERE k.session_id = ?1"
        } else {
            "SELECT CAST(ordinal AS INTEGER) AS ordinal, role, ts, tool_name, text FROM turns WHERE session_id = ?1"
        };
        // A call's input is the text after `<tool> `, when it is JSON.
        let sql = format!(
            "SELECT ordinal, role, ts, tool_name, head, tail, \
               coalesce(json_extract(input, '$.file_path'), json_extract(input, '$.notebook_path'), json_extract(input, '$.path')), \
               coalesce(json_extract(input, '$.description'), json_extract(input, '$.title')), \
               substr(json_extract(input, '$.command'), 1, ?4), \
               input IS NOT NULL \
             FROM (SELECT ordinal, role, ts, tool_name, \
                     substr(text, 1, ?4) AS head, \
                     CASE WHEN length(text) > ?4 THEN substr(text, -?5) END AS tail, \
                     CASE WHEN role = 'tool' AND tool_name IS NOT NULL \
                       AND substr(text, 1, length(tool_name) + 2) = tool_name || ' {{' \
                       AND json_valid(substr(text, length(tool_name) + 2)) \
                     THEN substr(text, length(tool_name) + 2) END AS input \
                   FROM ({source}) WHERE ts >= ?2 AND ts < ?3) ORDER BY ordinal"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(
            params![
                session,
                format_ts(start)?,
                format_ts(end)?,
                MARK_HEAD as i64,
                MARK_TAIL as i64
            ],
            |row| {
                let text_field = |i: usize| -> rusqlite::Result<Option<String>> {
                    // `json_extract` of an array is its JSON text; of a
                    // number, a number.
                    match row.get_ref(i)? {
                        rusqlite::types::ValueRef::Text(t) => {
                            Ok(Some(String::from_utf8_lossy(t).into_owned()))
                        }
                        _ => Ok(None),
                    }
                };
                Ok(TurnMark {
                    ordinal: row.get(0)?,
                    role: row.get(1)?,
                    ts: row.get(2)?,
                    tool_name: row.get(3)?,
                    head: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    tail: row.get(5)?,
                    file: text_field(6)?,
                    about: text_field(7)?,
                    command: text_field(8)?,
                    call: row.get::<_, Option<bool>>(9)?.unwrap_or(false),
                })
            },
        )?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// How many turns hold a word that starts with `prefix` (letters and
    /// digits only), counted with the full-text index and stopped at
    /// `cap`: whether a word is rare in this home. A prefix with other characters counts 0.
    pub fn turns_holding(&self, prefix: &str, cap: usize) -> Result<usize> {
        if prefix.is_empty() || !prefix.chars().all(char::is_alphanumeric) {
            return Ok(0);
        }
        let mut stmt = self.conn.prepare_cached(
            "SELECT count(*) FROM (SELECT 1 FROM turns WHERE turns MATCH ?1 LIMIT ?2)",
        )?;
        let n: i64 = stmt.query_row(
            params![
                format!("\"{prefix}\"*"),
                i64::try_from(cap).unwrap_or(i64::MAX)
            ],
            |r| r.get(0),
        )?;
        Ok(usize::try_from(n).unwrap_or(0))
    }

    /// How many turns the index holds.
    pub fn turn_total(&self) -> Result<u64> {
        let sql = if self.format >= 2 {
            "SELECT count(*) FROM turn_keys"
        } else {
            "SELECT count(*) FROM turns"
        };
        let n: i64 = self.conn.query_row(sql, [], |r| r.get(0))?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    /// The ordinal of every indexed session's last turn, in one scan of
    /// `turn_keys` (of the `turns` table on a format-1 index).
    pub fn last_ordinals(&self) -> Result<HashMap<String, u32>> {
        let sql = if self.format >= 2 {
            "SELECT session_id, MAX(ordinal) FROM turn_keys GROUP BY session_id"
        } else {
            "SELECT session_id, MAX(ordinal) FROM turns GROUP BY session_id"
        };
        let mut stmt = self.conn.prepare_cached(sql)?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The ordinal of `session`'s last turn; `None` when it has none.
    pub fn last_ordinal(&self, session: &str) -> Result<Option<u32>> {
        let sql = if self.format >= 2 {
            "SELECT MAX(ordinal) FROM turn_keys WHERE session_id = ?1"
        } else {
            "SELECT MAX(CAST(ordinal AS INTEGER)) FROM turns WHERE session_id = ?1"
        };
        let mut stmt = self.conn.prepare_cached(sql)?;
        Ok(stmt.query_row([session], |row| row.get(0))?)
    }

    /// Turn `ordinal` of `session`; `None` when the index does not hold it.
    pub fn turn(&self, session: &str, ordinal: u32) -> Result<Option<SessionTurn>> {
        if !session_row_exists(&self.conn, session)? {
            return Ok(None);
        }
        let sql = if self.format >= 2 {
            "SELECT t.role, t.ts, t.tool_name, t.text FROM turn_keys k JOIN turns t ON t.rowid = k.turn WHERE k.session_id = ?1 AND k.ordinal = ?2"
        } else {
            "SELECT role, ts, tool_name, text FROM turns WHERE session_id = ?1 AND ordinal = ?2"
        };
        let mut stmt = self.conn.prepare_cached(sql)?;
        let mut rows = stmt.query(params![session, ordinal])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        Ok(Some(SessionTurn {
            session: session.to_owned(),
            ordinal,
            role: row.get(0)?,
            ts: row.get(1)?,
            tool_name: row.get(2)?,
            text: row.get(3)?,
        }))
    }

    /// The turns the `(session, ordinal)` pairs name, in the pairs' order;
    /// a pair the index does not hold is left out. On format 2 each pair is
    /// one indexed lookup. On format 1 the turns are found by one scan of
    /// `turns` for each chunk of 500 distinct sessions, and a session
    /// without a row is skipped, as in [`Index::turn`].
    pub fn turns(&self, pairs: &[(String, u32)]) -> Result<Vec<SessionTurn>> {
        if self.format >= 2 {
            let mut found = Vec::new();
            for (session, ordinal) in pairs {
                found.extend(self.turn(session, *ordinal)?);
            }
            return Ok(found);
        }
        let mut sessions: Vec<&str> = Vec::new();
        for (session, _) in pairs {
            if !sessions.contains(&session.as_str()) {
                sessions.push(session);
            }
        }
        let wanted: std::collections::HashSet<(&str, u32)> =
            pairs.iter().map(|(s, o)| (s.as_str(), *o)).collect();
        let mut held: HashMap<(String, u32), SessionTurn> = HashMap::new();
        for chunk in sessions.chunks(SCAN_CHUNK) {
            let marks = vec!["?"; chunk.len()].join(", ");
            let mut stmt = self.conn.prepare(&format!(
                "SELECT session_id, ordinal, role, ts, tool_name, text FROM turns WHERE session_id IN ({marks})"
            ))?;
            let mut rows = stmt.query(rusqlite::params_from_iter(chunk.iter()))?;
            while let Some(row) = rows.next()? {
                let turn = SessionTurn {
                    session: row.get(0)?,
                    ordinal: row.get(1)?,
                    role: row.get(2)?,
                    ts: row.get(3)?,
                    tool_name: row.get(4)?,
                    text: row.get(5)?,
                };
                if wanted.contains(&(turn.session.as_str(), turn.ordinal)) {
                    held.insert((turn.session.clone(), turn.ordinal), turn);
                }
            }
        }
        Ok(pairs
            .iter()
            .filter_map(|(session, ordinal)| held.get(&(session.clone(), *ordinal)).cloned())
            .collect())
    }

    /// The best `limit` turns that match `query`, FTS5 syntax as written,
    /// best match first and, among equal matches, the newest first. Ranks
    /// from the full-text index alone, then reads only those turns' text,
    /// so a common word does not read every matching turn from disk.
    pub fn search_top(&self, query: &str, limit: usize) -> Result<Vec<SessionTurn>> {
        let ids: Vec<i64> = {
            let mut stmt = self.conn.prepare_cached(
                "SELECT rowid FROM turns WHERE turns MATCH ?1 ORDER BY bm25(turns) LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![query, limit as i64], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut stmt = self.conn.prepare_cached(
            "SELECT session_id, ordinal, role, ts, tool_name, text FROM turns WHERE rowid = ?1",
        )?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let turn = stmt.query_row([id], |row| {
                Ok(SessionTurn {
                    session: row.get(0)?,
                    ordinal: row.get(1)?,
                    role: row.get(2)?,
                    ts: row.get(3)?,
                    tool_name: row.get(4)?,
                    text: row.get(5)?,
                })
            })?;
            out.push(turn);
        }
        Ok(out)
    }

    /// The turns that match `query`, FTS5 syntax as written, best match
    /// first and, among equal matches, the newest first. `keep` sees each
    /// match in that order, and the search stops at the first it refuses.
    pub fn search_turns(
        &self,
        query: &str,
        mut keep: impl FnMut(&SessionTurn) -> bool,
    ) -> Result<Vec<SessionTurn>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT session_id, ordinal, role, ts, tool_name, text FROM turns \
             WHERE turns MATCH ?1 ORDER BY bm25(turns), ts DESC",
        )?;
        let mut rows = stmt.query([query])?;
        let mut turns = Vec::new();
        while let Some(row) = rows.next()? {
            let turn = SessionTurn {
                session: row.get(0)?,
                ordinal: row.get(1)?,
                role: row.get(2)?,
                ts: row.get(3)?,
                tool_name: row.get(4)?,
                text: row.get(5)?,
            };
            if !keep(&turn) {
                break;
            }
            turns.push(turn);
        }
        Ok(turns)
    }
}

/// WAL mode, so readers and the one writer do not block each other, and
/// `synchronous=NORMAL`. WAL is best effort: a file system that cannot
/// give it leaves the journal mode as it was.
fn tune_for_writing(conn: &Connection) -> Result<()> {
    match conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get::<_, String>(0)) {
        Ok(mode) if mode.eq_ignore_ascii_case("wal") => {}
        Ok(mode) => tracing::warn!("the index stays in journal mode {mode}, not wal"),
        Err(err) => tracing::warn!("the index could not enter WAL mode: {err}"),
    }
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    Ok(())
}

/// Deletes `session`'s turns, found through `turn_keys`, and its keys.
fn delete_turns(conn: &Connection, session: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM turns WHERE rowid IN (SELECT turn FROM turn_keys WHERE session_id = ?1)",
        [session],
    )?;
    conn.execute("DELETE FROM turn_keys WHERE session_id = ?1", [session])?;
    Ok(())
}

fn session_row_exists(conn: &Connection, id: &str) -> Result<bool> {
    let mut stmt = conn.prepare_cached("SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?1)")?;
    Ok(stmt.query_row([id], |row| row.get(0))?)
}

/// `query` as one FTS5 phrase: wrapped in `"`, inner `"` doubled.
fn quote_phrase(query: &str) -> String {
    format!("\"{}\"", query.replace('"', "\"\""))
}

/// The one form the index stores a timestamp in: RFC 3339 in UTC with six
/// fractional digits and `Z`, fixed width so that text order is time order.
fn format_ts(ts: OffsetDateTime) -> Result<String> {
    let fixed = time::macros::format_description!(
        "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:6]Z"
    );
    ts.checked_to_offset(UtcOffset::UTC)
        .ok_or_else(|| format!("{ts:?} is out of range in UTC"))
        .and_then(|utc| utc.format(&fixed).map_err(|e| e.to_string()))
        .map_err(|message| {
            Error::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("timestamp cannot be indexed: {message}"),
            ))
        })
}

/// `path` with `suffix` appended to its file name: `index.sqlite-wal`.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::io_at(path, e)),
    }
}

/// Removes `path` and the write-ahead log and shared memory beside it.
fn remove_with_sidecars(path: &Path) -> Result<()> {
    remove_if_exists(path)?;
    remove_if_exists(&with_suffix(path, "-wal"))?;
    remove_if_exists(&with_suffix(path, "-shm"))
}

/// A `SessionTurn` from a row of `session_id, ordinal, role, ts,
/// tool_name, text`.
fn session_turn_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionTurn> {
    Ok(SessionTurn {
        session: row.get(0)?,
        ordinal: row.get(1)?,
        role: row.get(2)?,
        ts: row.get(3)?,
        tool_name: row.get(4)?,
        text: row.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::format_description::well_known::Rfc3339;

    fn at(ts: &str) -> OffsetDateTime {
        OffsetDateTime::parse(ts, &Rfc3339).unwrap()
    }

    #[test]
    fn turn_counts_cover_every_indexed_session() {
        let home = tempfile::tempdir().unwrap();
        let mut index = Index::open(home.path(), true).unwrap();
        assert!(index.turn_counts().unwrap().is_empty());
        let turns = [
            turn(0, Role::User, "2026-11-08T03:00:00Z", None, "one"),
            turn(1, Role::Assistant, "2026-11-08T03:00:01Z", None, "two"),
        ];
        index
            .replace_session(&source("s1", Tool::ClaudeCode, "/t/s1.jsonl", "h1"), &turns)
            .unwrap();
        index
            .replace_session(&source("s2", Tool::Codex, "/t/s2.jsonl", "h2"), &turns[..1])
            .unwrap();
        let counts = index.turn_counts().unwrap();
        assert_eq!(counts.len(), 2);
        assert_eq!((counts["s1"], counts["s2"]), (2, 1));
    }

    #[test]
    fn openings_are_the_first_user_turns_cut_short_and_sources_name_transcripts() {
        let home = tempfile::tempdir().unwrap();
        let mut index = Index::open(home.path(), true).unwrap();
        let mut turns = vec![
            turn(0, Role::User, "2026-11-08T03:00:00Z", None, "first ask"),
            turn(1, Role::Assistant, "2026-11-08T03:00:01Z", None, "reply"),
            turn(
                2,
                Role::Tool,
                "2026-11-08T03:00:02Z",
                Some("Bash"),
                "Bash {}",
            ),
            turn(
                3,
                Role::User,
                "2026-11-08T03:00:03Z",
                None,
                "second ask, longer",
            ),
            turn(4, Role::User, "2026-11-08T03:00:04Z", None, "third"),
        ];
        // A user turn past the window is not an opening.
        turns.push(turn(12, Role::User, "2026-11-08T03:00:12Z", None, "late"));
        index
            .replace_session(&source("s1", Tool::ClaudeCode, "/t/s1.jsonl", "h1"), &turns)
            .unwrap();
        index
            .replace_session(
                &source("s2", Tool::Codex, "/t/s2.jsonl", "h2"),
                &turns[1..3],
            )
            .unwrap();
        let openings = index.openings(2, 10).unwrap();
        assert_eq!(openings.len(), 1, "s2 has no user turn");
        assert_eq!(openings["s1"], vec!["first ask", "second ask"]);
        assert_eq!(index.openings(9, 100).unwrap()["s1"].len(), 3);
        let some = index.openings_for(&["s1", "s2", "nope"], 2, 10).unwrap();
        assert_eq!(some.len(), 1);
        assert_eq!(some["s1"], vec!["first ask", "second ask"]);
        let sources = index.sources().unwrap();
        assert_eq!(
            sources["s2"],
            ("codex".to_string(), PathBuf::from("/t/s2.jsonl"))
        );
        assert_eq!(sources.len(), 2);
    }

    #[test]
    fn a_new_index_looks_sessions_up_by_transcript_path() {
        let home = tempfile::tempdir().unwrap();
        let index = Index::open(home.path(), true).unwrap();
        let sql: String = index
            .conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = 'sessions_path'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            sql,
            "CREATE INDEX sessions_path ON sessions(transcript_path)"
        );
    }

    #[test]
    fn turn_marks_read_a_time_span_and_a_call_s_file_description_and_command() {
        let (_home, mut index) = open_new();
        let turns = vec![
            turn(
                0,
                Role::User,
                "2026-11-10T09:00:00Z",
                None,
                "crop the covers",
            ),
            turn(
                1,
                Role::Tool,
                "2026-11-10T09:01:00Z",
                Some("Edit"),
                r#"Edit {"file_path":"/w/src/covers.rs","old_string":"a","new_string":"b"}"#,
            ),
            turn(
                2,
                Role::Tool,
                "2026-11-10T09:02:00Z",
                Some("Bash"),
                r#"Bash {"command":"cargo test covers","description":"Run the cover tests"}"#,
            ),
            turn(
                3,
                Role::Tool,
                "2026-11-10T09:03:00Z",
                Some("Bash"),
                "test result: ok",
            ),
            turn(4, Role::Assistant, "2026-11-10T10:00:00Z", None, "Done."),
        ];
        index
            .replace_session(&source("s1", Tool::ClaudeCode, "/t/s1.jsonl", "h1"), &turns)
            .unwrap();
        let marks = index
            .turn_marks("s1", at("2026-11-10T09:01:00Z"), at("2026-11-10T10:00:00Z"))
            .unwrap();
        assert_eq!(
            marks.iter().map(|m| m.ordinal).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert!(marks[0].call);
        assert_eq!(marks[0].file.as_deref(), Some("/w/src/covers.rs"));
        assert_eq!(marks[1].about.as_deref(), Some("Run the cover tests"));
        assert_eq!(marks[1].command.as_deref(), Some("cargo test covers"));
        assert!(!marks[2].call);
        assert_eq!(marks[2].file, None);
        assert_eq!(index.turn_total().unwrap(), 5);
        assert_eq!(index.turns_holding("cover", 100).unwrap(), 3);
        assert_eq!(index.turns_holding("cover", 2).unwrap(), 2);
        assert_eq!(index.turns_holding("co\"ver", 100).unwrap(), 0);
    }

    #[test]
    fn timestamps_are_stored_fixed_width_in_utc_so_text_order_is_time_order() {
        assert_eq!(
            format_ts(at("2026-11-08T03:00:00Z")).unwrap(),
            "2026-11-08T03:00:00.000000Z"
        );
        assert_eq!(
            format_ts(at("2026-11-08T04:00:00.5+01:00")).unwrap(),
            "2026-11-08T03:00:00.500000Z"
        );
        let whole = format_ts(at("2026-11-08T03:00:00Z")).unwrap();
        let later = format_ts(at("2026-11-08T03:00:00.25Z")).unwrap();
        let next = format_ts(at("2026-11-08T03:00:01Z")).unwrap();
        assert!(whole < later && later < next);
    }

    fn turn(ordinal: u32, role: Role, ts: &str, tool_name: Option<&str>, text: &str) -> Turn {
        Turn {
            ordinal,
            role,
            ts: at(ts),
            tool_name: tool_name.map(str::to_owned),
            text: redact(text),
        }
    }

    fn source<'a>(id: &'a str, tool: Tool, path: &'a str, hash: &'a str) -> SessionSource<'a> {
        SessionSource {
            id,
            tool,
            transcript_path: Path::new(path),
            file_hash: hash,
        }
    }

    /// `(session, ordinal)` of each hit, in order.
    fn ids_of(hits: &[Hit]) -> Vec<(&str, u32)> {
        hits.iter()
            .map(|h| (h.session.as_str(), h.ordinal))
            .collect()
    }

    fn open_new() -> (tempfile::TempDir, Index) {
        let home = tempfile::tempdir().unwrap();
        let index = Index::open(home.path(), true).unwrap();
        (home, index)
    }

    fn file_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    fn journal_mode(index: &Index) -> String {
        index
            .conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap()
    }

    fn user_version(index: &Index) -> i64 {
        index
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn recall_ranks_by_bm25_then_newest_and_snippets_the_match() {
        let (_home, mut index) = open_new();
        let filler = "lorem ipsum dolor sit amet ".repeat(8);
        index
            .replace_session(
                &source("s1", Tool::ClaudeCode, "/t/s1.jsonl", "h1"),
                &[
                    turn(
                        0,
                        Role::User,
                        "2026-11-08T03:00:00Z",
                        None,
                        "invoice webhook retry",
                    ),
                    turn(
                        1,
                        Role::Assistant,
                        "2026-11-08T03:00:05Z",
                        None,
                        &format!("{filler}invoice webhook retry {filler}"),
                    ),
                ],
            )
            .unwrap();
        index
            .replace_session(
                &source("s2", Tool::Codex, "/t/s2.jsonl", "h2"),
                &[
                    turn(
                        0,
                        Role::User,
                        "2026-11-08T04:00:00Z",
                        None,
                        "invoice webhook retry",
                    ),
                    turn(
                        1,
                        Role::Tool,
                        "2026-11-08T04:00:01Z",
                        Some("Bash"),
                        "unrelated output about kubernetes",
                    ),
                    turn(
                        2,
                        Role::Assistant,
                        "2026-11-08T04:00:09Z",
                        None,
                        "the invoice webhook retry backs off for thirty seconds",
                    ),
                ],
            )
            .unwrap();

        let hits = index.recall("invoice webhook retry", 20, false).unwrap();
        // s2/0 and s1/0 are the same text, so they tie and the newer is
        // first; then the medium turn, then the long one.
        assert_eq!(ids_of(&hits), [("s2", 0), ("s1", 0), ("s2", 2), ("s1", 1)]);
        assert!(hits.windows(2).all(|pair| pair[0].score <= pair[1].score));
        assert!(hits[0].score < 0.0);
        assert_eq!(hits[0].score, hits[1].score);
        assert!(hits[1].score < hits[2].score && hits[2].score < hits[3].score);
        assert_eq!(hits[0].tool, "codex");
        assert_eq!(hits[1].tool, "claude-code");
        assert_eq!(hits[0].role, "user");
        assert_eq!(hits[0].ts, "2026-11-08T04:00:00.000000Z");
        assert_eq!(hits[0].tool_name, None);
        // A short turn is its own snippet; a long one is cut around the
        // match with an ellipsis at each end.
        assert_eq!(hits[0].snippet, "invoice webhook retry");
        let long = &hits[3].snippet;
        assert!(long.contains("invoice webhook retry"), "{long}");
        assert!(long.starts_with('…') && long.ends_with('…'), "{long}");
        assert!(long.split_whitespace().count() <= 24 + 2, "{long}");

        // A tool turn comes back with its name; a missing word, with nothing.
        let hits = index.recall("kubernetes", 20, false).unwrap();
        assert_eq!(ids_of(&hits), [("s2", 1)]);
        assert_eq!(hits[0].role, "tool");
        assert_eq!(hits[0].tool_name.as_deref(), Some("Bash"));
        assert_eq!(hits[0].tool, "codex");
        assert!(index.recall("terraform", 20, false).unwrap().is_empty());
    }

    #[test]
    fn recall_folds_case_and_diacritics() {
        let (_home, mut index) = open_new();
        index
            .replace_session(
                &source("s1", Tool::Codex, "/t/s1.jsonl", "h1"),
                &[turn(
                    0,
                    Role::User,
                    "2026-11-08T03:00:00Z",
                    None,
                    "A quiet café",
                )],
            )
            .unwrap();
        for query in ["cafe", "CAFÉ", "Quiet Cafe"] {
            assert_eq!(index.recall(query, 20, false).unwrap().len(), 1, "{query}");
        }
    }

    #[test]
    fn non_raw_queries_are_one_quoted_phrase() {
        assert_eq!(quote_phrase("a b"), "\"a b\"");
        assert_eq!(quote_phrase("say \"hi\""), "\"say \"\"hi\"\"\"");

        let (_home, mut index) = open_new();
        index
            .replace_session(
                &source("s1", Tool::ClaudeCode, "/t/s1.jsonl", "h1"),
                &[
                    turn(
                        0,
                        Role::User,
                        "2026-11-08T03:00:00Z",
                        None,
                        "they say \"hello\" loudly",
                    ),
                    turn(
                        1,
                        Role::User,
                        "2026-11-08T03:00:01Z",
                        None,
                        "run the foo-bar flag",
                    ),
                    turn(
                        2,
                        Role::User,
                        "2026-11-08T03:00:02Z",
                        None,
                        "invoice webhook only",
                    ),
                    turn(
                        3,
                        Role::User,
                        "2026-11-08T03:00:03Z",
                        None,
                        "invoice and webhook",
                    ),
                ],
            )
            .unwrap();
        // Quotes in the query are part of the phrase, not of the syntax.
        let hits = index.recall("say \"hello\"", 20, false).unwrap();
        assert_eq!(ids_of(&hits), [("s1", 0)]);
        // `-` would be an operator in a raw query.
        let hits = index.recall("foo-bar", 20, false).unwrap();
        assert_eq!(ids_of(&hits), [("s1", 1)]);
        // `AND` is a word in a phrase, so the words must be adjacent...
        let hits = index.recall("invoice AND webhook", 20, false).unwrap();
        assert_eq!(ids_of(&hits), [("s1", 3)]);
        // ...and an operator in a raw query.
        let hits = index.recall("invoice AND webhook", 20, true).unwrap();
        assert_eq!(ids_of(&hits), [("s1", 3), ("s1", 2)]);
        // Whatever the text, a phrase is never a syntax error.
        for query in [
            "", "\"", "\"\"", "AND", "(", "NEAR(", "*", "???", "a:b", "x -y",
        ] {
            index
                .recall(query, 20, false)
                .unwrap_or_else(|e| panic!("{query:?}: {e}"));
        }
    }

    #[test]
    fn raw_queries_with_invalid_syntax_fail_with_the_index_error() {
        let (_home, index) = open_new();
        for (query, message) in [
            ("AND", "fts5: syntax error near \"AND\""),
            ("a OR", "fts5: syntax error"),
            ("(a", "fts5: syntax error"),
            ("\"unterminated", "unterminated string"),
        ] {
            match index.recall(query, 20, true) {
                Err(Error::Sqlite(err)) => {
                    assert!(err.to_string().contains(message), "{query:?}: {err}");
                }
                other => panic!("{query:?}: expected an index error, got {other:?}"),
            }
        }
    }

    #[test]
    fn recall_limits_to_the_maximum() {
        let (_home, mut index) = open_new();
        let turns: Vec<Turn> = (0..250)
            .map(|n| {
                turn(
                    n,
                    Role::User,
                    "2026-11-08T03:00:00Z",
                    None,
                    "needle in a haystack",
                )
            })
            .collect();
        index
            .replace_session(&source("s1", Tool::Codex, "/t/s1.jsonl", "h1"), &turns)
            .unwrap();
        assert_eq!(index.recall("needle", 5, false).unwrap().len(), 5);
        assert_eq!(index.recall("needle", 0, false).unwrap().len(), 0);
        assert_eq!(index.recall("needle", 200, false).unwrap().len(), 200);
        assert_eq!(
            index.recall("needle", 100_000, false).unwrap().len(),
            MAX_LIMIT
        );
    }

    #[test]
    fn exists_and_turns_for_session_return_what_was_stored() {
        let (_home, mut index) = open_new();
        index
            .replace_session(
                &source("s1", Tool::ClaudeCode, "/t/s1.jsonl", "h1"),
                // Inserted out of order; a +09:30 timestamp is stored in UTC.
                &[
                    turn(
                        2,
                        Role::Assistant,
                        "2026-11-08T12:30:10+09:30",
                        None,
                        "third",
                    ),
                    turn(0, Role::User, "2026-11-08T03:00:00Z", None, "first"),
                    turn(
                        1,
                        Role::Tool,
                        "2026-11-08T03:00:01.25Z",
                        Some("Bash"),
                        "second",
                    ),
                ],
            )
            .unwrap();
        index
            .replace_session(
                &source("s2", Tool::Codex, "/t/s2.jsonl", "h2"),
                &[turn(0, Role::User, "2026-11-08T04:00:00Z", None, "other")],
            )
            .unwrap();

        for ordinal in 0..3 {
            assert!(index.exists("s1", ordinal).unwrap(), "s1/{ordinal}");
        }
        assert!(!index.exists("s1", 3).unwrap());
        assert!(!index.exists("s2", 1).unwrap());
        assert!(!index.exists("nope", 0).unwrap());

        let stored =
            |ordinal, role: &str, ts: &str, tool_name: Option<&str>, text: &str| StoredTurn {
                ordinal,
                role: role.into(),
                ts: ts.into(),
                tool_name: tool_name.map(str::to_owned),
                text: text.into(),
            };
        assert_eq!(
            index.turns_for_session("s1").unwrap(),
            [
                stored(0, "user", "2026-11-08T03:00:00.000000Z", None, "first"),
                stored(
                    1,
                    "tool",
                    "2026-11-08T03:00:01.250000Z",
                    Some("Bash"),
                    "second"
                ),
                stored(2, "assistant", "2026-11-08T03:00:10.000000Z", None, "third"),
            ]
        );
        assert_eq!(index.turns_for_session("s2").unwrap().len(), 1);
        assert!(index.turns_for_session("nope").unwrap().is_empty());
    }

    #[test]
    fn the_sessions_row_holds_the_tool_path_hash_and_first_and_last_turn_in_utc() {
        let (_home, mut index) = open_new();
        index
            .replace_session(
                &source("s1", Tool::ClaudeCode, "/t/s1.jsonl", "h1"),
                &[
                    turn(0, Role::User, "2026-11-08T12:30:00+09:30", None, "first"),
                    turn(
                        1,
                        Role::Assistant,
                        "2026-11-08T03:00:01.25Z",
                        None,
                        "middle",
                    ),
                    turn(
                        2,
                        Role::Assistant,
                        "2026-11-08T13:00:00+09:30",
                        None,
                        "last",
                    ),
                ],
            )
            .unwrap();
        let row: [String; 5] = index
            .conn
            .query_row(
                "SELECT tool, transcript_path, first_turn, last_turn, file_hash FROM sessions WHERE id = 's1'",
                [],
                |r| Ok([r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?]),
            )
            .unwrap();
        assert_eq!(
            row,
            [
                "claude-code",
                "/t/s1.jsonl",
                "2026-11-08T03:00:00.000000Z",
                "2026-11-08T03:30:00.000000Z",
                "h1"
            ]
        );
    }

    #[test]
    fn replace_session_replaces_the_turns_and_the_row() {
        let (_home, mut index) = open_new();
        let s1 = source("s1", Tool::ClaudeCode, "/t/s1.jsonl", "old-hash");
        let s2 = source("s2", Tool::Codex, "/t/s2.jsonl", "h2");
        index
            .replace_session(
                &s1,
                &[
                    turn(0, Role::User, "2026-11-08T03:00:00Z", None, "alpha one"),
                    turn(1, Role::User, "2026-11-08T03:00:01Z", None, "alpha two"),
                ],
            )
            .unwrap();
        index
            .replace_session(
                &s2,
                &[turn(
                    0,
                    Role::User,
                    "2026-11-08T03:00:00Z",
                    None,
                    "alpha three",
                )],
            )
            .unwrap();
        assert_eq!(index.recall("alpha", 20, false).unwrap().len(), 3);

        // The transcript changed: fewer turns, a new hash and path.
        let changed = source("s1", Tool::ClaudeCode, "/t/moved/s1.jsonl", "new-hash");
        index
            .replace_session(
                &changed,
                &[turn(
                    0,
                    Role::User,
                    "2026-11-08T05:00:00Z",
                    None,
                    "beta only",
                )],
            )
            .unwrap();
        assert_eq!(
            ids_of(&index.recall("alpha", 20, false).unwrap()),
            [("s2", 0)]
        );
        assert_eq!(
            ids_of(&index.recall("beta", 20, false).unwrap()),
            [("s1", 0)]
        );
        assert!(!index.exists("s1", 1).unwrap());
        let hashes = index.file_hashes().unwrap();
        assert_eq!(hashes.len(), 2);
        assert_eq!(hashes["/t/moved/s1.jsonl"], "new-hash");
        assert_eq!(hashes["/t/s2.jsonl"], "h2");
        let sessions: i64 = index
            .conn
            .query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sessions, 2);
    }

    #[test]
    fn a_session_without_turns_has_null_first_and_last_turn() {
        let (_home, mut index) = open_new();
        index
            .replace_session(&source("empty", Tool::Codex, "/t/empty.jsonl", "h0"), &[])
            .unwrap();
        let (first, last): (Option<String>, Option<String>) = index
            .conn
            .query_row(
                "SELECT first_turn, last_turn FROM sessions WHERE id = 'empty'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((first, last), (None, None));
        assert!(index.turns_for_session("empty").unwrap().is_empty());
        assert_eq!(index.file_hashes().unwrap()["/t/empty.jsonl"], "h0");
    }

    #[test]
    fn turn_text_is_redacted_before_it_is_indexed() {
        let raw = RawTurn {
            ordinal: 0,
            role: Role::Tool,
            ts: at("2026-11-08T03:00:00Z"),
            tool_name: Some("Bash".into()),
            text: "aws AKIAFAKEFAKEFAKE0000".into(),
        };
        let from_raw = Turn::from_raw(raw);
        assert_eq!(from_raw.text.as_str(), "aws [redacted:aws]");
        assert_eq!(from_raw.tool_name.as_deref(), Some("Bash"));

        let (_home, mut index) = open_new();
        index
            .replace_session(
                &source("s1", Tool::ClaudeCode, "/t/s1.jsonl", "h1"),
                &[from_raw],
            )
            .unwrap();
        assert!(index
            .recall("AKIAFAKEFAKEFAKE0000", 20, false)
            .unwrap()
            .is_empty());
        assert_eq!(index.recall("redacted aws", 20, false).unwrap().len(), 1);
    }

    /// Every column of every `sessions` row, by id.
    type SessionRow = (
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        String,
        Option<String>,
    );

    fn session_rows(index: &Index) -> Vec<SessionRow> {
        index
            .conn
            .prepare(
                "SELECT id, tool, transcript_path, first_turn, last_turn, file_hash, error \
                 FROM sessions ORDER BY id",
            )
            .unwrap()
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    fn row(
        id: &str,
        tool: &str,
        path: &str,
        turns: Option<(&str, &str)>,
        hash: &str,
        error: Option<&str>,
    ) -> SessionRow {
        (
            id.into(),
            tool.into(),
            path.into(),
            turns.map(|(first, _)| first.into()),
            turns.map(|(_, last)| last.into()),
            hash.into(),
            error.map(Into::into),
        )
    }

    #[test]
    fn a_transcript_that_never_parsed_gets_an_error_row_and_a_held_hash() {
        let (_home, mut index) = open_new();
        let error = redact("expected value at line 1 column 1");
        index
            .record_failure(Tool::Codex, Path::new("/t/bad.jsonl"), "h-bad", &error)
            .unwrap();

        // The hash is held, so ingest skips the file until it changes.
        assert_eq!(index.file_hashes().unwrap()["/t/bad.jsonl"], "h-bad");
        // The row: id from the path, its tool, no turns, the hash and the error.
        assert_eq!(
            session_rows(&index),
            [row(
                "error:/t/bad.jsonl",
                "codex",
                "/t/bad.jsonl",
                None,
                "h-bad",
                Some("expected value at line 1 column 1")
            )]
        );
        assert_eq!(
            index.failed_files().unwrap(),
            [FailedFile {
                transcript_path: "/t/bad.jsonl".into(),
                file_hash: "h-bad".into(),
                error: "expected value at line 1 column 1".into(),
            }]
        );
        assert!(!index.exists("error:/t/bad.jsonl", 0).unwrap());
        assert!(index
            .turns_for_session("error:/t/bad.jsonl")
            .unwrap()
            .is_empty());

        // Failing again, on other bytes, replaces the row rather than adding one.
        index
            .record_failure(
                Tool::Codex,
                Path::new("/t/bad.jsonl"),
                "h-bad-2",
                &redact("still bad"),
            )
            .unwrap();
        assert_eq!(index.file_hashes().unwrap()["/t/bad.jsonl"], "h-bad-2");
        assert_eq!(
            session_rows(&index),
            [row(
                "error:/t/bad.jsonl",
                "codex",
                "/t/bad.jsonl",
                None,
                "h-bad-2",
                Some("still bad")
            )]
        );
    }

    #[test]
    fn a_failure_is_stored_redacted() {
        let (_home, mut index) = open_new();
        let message = "invalid type: string \"aws AKIAFAKEFAKEFAKE0000\", expected a map";
        index
            .record_failure(
                Tool::ClaudeCode,
                Path::new("/t/leaky.jsonl"),
                "h1",
                &redact(message),
            )
            .unwrap();
        let failed = index.failed_files().unwrap();
        assert_eq!(
            failed[0].error,
            "invalid type: string \"aws [redacted:aws]\", expected a map"
        );
    }

    #[test]
    fn a_failure_keeps_the_rows_of_the_last_good_parse() {
        let (_home, mut index) = open_new();
        let turns = [turn(
            0,
            Role::User,
            "2026-11-08T03:00:00Z",
            None,
            "alpha words",
        )];
        index
            .replace_session(&source("s1", Tool::ClaudeCode, "/t/s1.jsonl", "h1"), &turns)
            .unwrap();

        // The file changed and no longer parses: its row stays, with the new
        // hash and the error, and so do its turns.
        index
            .record_failure(
                Tool::ClaudeCode,
                Path::new("/t/s1.jsonl"),
                "h2",
                &redact("truncated"),
            )
            .unwrap();
        assert_eq!(
            session_rows(&index),
            [row(
                "s1",
                "claude-code",
                "/t/s1.jsonl",
                Some(("2026-11-08T03:00:00.000000Z", "2026-11-08T03:00:00.000000Z")),
                "h2",
                Some("truncated")
            )]
        );
        assert_eq!(index.file_hashes().unwrap()["/t/s1.jsonl"], "h2");
        assert_eq!(index.recall("alpha", 20, false).unwrap().len(), 1);
        assert_eq!(index.failed_files().unwrap().len(), 1);

        // Fixed: the same session again, and the error is gone.
        index
            .replace_session(&source("s1", Tool::ClaudeCode, "/t/s1.jsonl", "h3"), &turns)
            .unwrap();
        assert!(index.failed_files().unwrap().is_empty());
        assert_eq!(session_rows(&index).len(), 1);
        assert_eq!(session_rows(&index)[0].6, None);
        assert_eq!(index.file_hashes().unwrap()["/t/s1.jsonl"], "h3");
    }

    #[test]
    fn a_good_parse_removes_the_other_rows_of_the_transcript() {
        let (_home, mut index) = open_new();
        // A transcript that failed, then parses: its error row goes.
        index
            .record_failure(
                Tool::Codex,
                Path::new("/t/fixed.jsonl"),
                "h-bad",
                &redact("bad"),
            )
            .unwrap();
        index
            .replace_session(
                &source("fixed", Tool::Codex, "/t/fixed.jsonl", "h-good"),
                &[turn(
                    0,
                    Role::User,
                    "2026-11-08T03:00:00Z",
                    None,
                    "fixed words",
                )],
            )
            .unwrap();
        assert!(index.failed_files().unwrap().is_empty());
        let ids: Vec<String> = session_rows(&index).into_iter().map(|r| r.0).collect();
        assert_eq!(ids, ["fixed"]);
        assert_eq!(index.file_hashes().unwrap().len(), 1);

        // A transcript whose session id changed: the old session, with its
        // turns, goes. Another transcript's rows stay.
        index
            .replace_session(
                &source("other", Tool::Codex, "/t/other.jsonl", "h-other"),
                &[turn(
                    0,
                    Role::User,
                    "2026-11-08T03:00:00Z",
                    None,
                    "other words",
                )],
            )
            .unwrap();
        index
            .replace_session(
                &source("renamed", Tool::Codex, "/t/fixed.jsonl", "h-new"),
                &[turn(
                    0,
                    Role::User,
                    "2026-11-08T04:00:00Z",
                    None,
                    "renamed words",
                )],
            )
            .unwrap();
        let ids: Vec<String> = session_rows(&index).into_iter().map(|r| r.0).collect();
        assert_eq!(ids, ["other", "renamed"]);
        assert!(index.recall("fixed", 20, false).unwrap().is_empty());
        assert!(!index.exists("fixed", 0).unwrap());
        assert_eq!(
            ids_of(&index.recall("renamed", 20, false).unwrap()),
            [("renamed", 0)]
        );
        assert_eq!(
            ids_of(&index.recall("other", 20, false).unwrap()),
            [("other", 0)]
        );
        let hashes = index.file_hashes().unwrap();
        assert_eq!(hashes.len(), 2);
        assert_eq!(hashes["/t/fixed.jsonl"], "h-new");

        // A failure and a changed session id together: gone too.
        index
            .record_failure(
                Tool::Codex,
                Path::new("/t/fixed.jsonl"),
                "h-bad-again",
                &redact("bad"),
            )
            .unwrap();
        index
            .replace_session(
                &source("third", Tool::Codex, "/t/fixed.jsonl", "h-third"),
                &[],
            )
            .unwrap();
        let ids: Vec<String> = session_rows(&index).into_iter().map(|r| r.0).collect();
        assert_eq!(ids, ["other", "third"]);
        assert!(index.recall("renamed", 20, false).unwrap().is_empty());
        assert!(index.failed_files().unwrap().is_empty());
    }

    #[test]
    fn open_without_an_index_is_missing_unless_asked_to_create() {
        let home = tempfile::tempdir().unwrap();
        let result = Index::open(home.path(), false);
        assert!(matches!(result, Err(Error::MissingIndex)), "{result:?}");
        assert!(
            file_names(home.path()).is_empty(),
            "open(false) must not create anything"
        );
        let missing_home = home.path().join("not").join("there");
        let result = Index::open(&missing_home, false);
        assert!(matches!(result, Err(Error::MissingIndex)), "{result:?}");
        assert!(!missing_home.exists());

        // An empty file is a database that was never initialised.
        fs::write(home.path().join(FILE_NAME), b"").unwrap();
        let result = Index::open(home.path(), false);
        assert!(matches!(result, Err(Error::MissingIndex)), "{result:?}");
        Index::open(home.path(), true).unwrap();
        Index::open(home.path(), false).unwrap();

        // create makes the missing directories too.
        Index::open(&missing_home, true).unwrap();
        assert!(missing_home.join(FILE_NAME).is_file());
    }

    #[test]
    fn creating_writes_the_ddl_in_wal_mode_and_reopening_keeps_the_data() {
        let home = tempfile::tempdir().unwrap();
        {
            let mut index = Index::open(home.path(), true).unwrap();
            assert_eq!(journal_mode(&index), "wal");
            assert_eq!(user_version(&index), 2);
            assert_eq!(index.format(), 2);
            let schema: Vec<String> = index
                .conn
                .prepare("SELECT sql FROM sqlite_master WHERE type = 'table' AND name IN ('sessions', 'turns') ORDER BY name")
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            assert_eq!(schema.len(), 2);
            assert_eq!(
                schema[0],
                "CREATE TABLE sessions(id TEXT PRIMARY KEY, tool TEXT NOT NULL, transcript_path TEXT NOT NULL, first_turn TEXT, last_turn TEXT, file_hash TEXT NOT NULL, error TEXT)"
            );
            assert!(schema[1].contains("USING fts5(session_id UNINDEXED, ordinal UNINDEXED, role UNINDEXED, ts UNINDEXED, tool_name UNINDEXED, text, tokenize='unicode61 remove_diacritics 2')"));
            index
                .replace_session(
                    &source("s1", Tool::Codex, "/t/s1.jsonl", "h1"),
                    &[turn(
                        0,
                        Role::User,
                        "2026-11-08T03:00:00Z",
                        None,
                        "persisted words",
                    )],
                )
                .unwrap();
        }
        let index = Index::open(home.path(), false).unwrap();
        assert_eq!(index.recall("persisted words", 20, false).unwrap().len(), 1);
        // Closing the last connection removes the write-ahead log.
        drop(index);
        assert_eq!(file_names(home.path()), [FILE_NAME]);
    }

    #[test]
    fn another_format_fails_naming_ingest_full() {
        let home = tempfile::tempdir().unwrap();
        drop(Index::open(home.path(), true).unwrap());
        let raw = Connection::open(home.path().join(FILE_NAME)).unwrap();
        raw.pragma_update(None, "user_version", 3).unwrap();
        drop(raw);
        for create in [false, true] {
            match Index::open(home.path(), create) {
                Err(Error::Io(err)) => {
                    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
                    let message = err.to_string();
                    assert!(
                        message.ends_with(
                            "index format 3, expected 1 or 2; run hippocamp ingest --full"
                        ),
                        "{message}"
                    );
                    assert!(message.contains(FILE_NAME), "{message}");
                }
                other => panic!("create={create}: expected a format error, got {other:?}"),
            }
        }
        // A database that is not ours at all is the same refusal, not a rewrite.
        let foreign = tempfile::tempdir().unwrap();
        let conn = Connection::open(foreign.path().join(FILE_NAME)).unwrap();
        conn.execute_batch("CREATE TABLE other(x)").unwrap();
        drop(conn);
        let result = Index::open(foreign.path(), true);
        assert!(
            matches!(&result, Err(Error::Io(e)) if e.to_string().contains("index format 0")),
            "{result:?}"
        );
    }

    #[test]
    fn rebuild_swaps_in_a_new_index_and_leaves_no_sidecars() {
        let home = tempfile::tempdir().unwrap();
        {
            let mut old = Index::open(home.path(), true).unwrap();
            old.replace_session(
                &source("old", Tool::Codex, "/t/old.jsonl", "h-old"),
                &[turn(0, Role::User, "2026-11-08T03:00:00Z", None, "alpha")],
            )
            .unwrap();
        }
        // What a crashed run leaves: a log beside the old index, and a
        // half-built temporary.
        for stale in [
            "index.sqlite-wal",
            "index.sqlite-shm",
            "index.sqlite.tmp",
            "index.sqlite.tmp-wal",
        ] {
            fs::write(home.path().join(stale), b"stale bytes").unwrap();
        }

        let written = Index::rebuild(home.path(), |index| {
            index.replace_session(
                &source("new", Tool::ClaudeCode, "/t/new.jsonl", "h-new"),
                &[turn(0, Role::User, "2026-11-08T03:00:00Z", None, "beta")],
            )?;
            Ok(7)
        })
        .unwrap();
        assert_eq!(written, 7);
        assert_eq!(file_names(home.path()), [FILE_NAME]);

        let index = Index::open(home.path(), false).unwrap();
        assert_eq!(journal_mode(&index), "delete");
        assert_eq!(
            ids_of(&index.recall("beta", 20, false).unwrap()),
            [("new", 0)]
        );
        assert!(index.recall("alpha", 20, false).unwrap().is_empty());
        assert_eq!(index.file_hashes().unwrap().len(), 1);
        drop(index);
        assert_eq!(file_names(home.path()), [FILE_NAME]);

        // A writer puts the rebuilt file back in WAL mode.
        let index = Index::open(home.path(), true).unwrap();
        assert_eq!(journal_mode(&index), "wal");
        assert_eq!(index.recall("beta", 20, false).unwrap().len(), 1);
    }

    #[test]
    fn rebuild_without_an_old_index_creates_one() {
        let home = tempfile::tempdir().unwrap();
        let nested = home.path().join("fresh").join("home");
        Index::rebuild(&nested, |_| Ok(())).unwrap();
        assert_eq!(file_names(&nested), [FILE_NAME]);
        let index = Index::open(&nested, false).unwrap();
        assert!(index.recall("anything", 20, false).unwrap().is_empty());
    }

    #[test]
    fn a_failed_rebuild_keeps_the_old_index_and_cleans_up() {
        let home = tempfile::tempdir().unwrap();
        {
            let mut old = Index::open(home.path(), true).unwrap();
            old.replace_session(
                &source("old", Tool::Codex, "/t/old.jsonl", "h-old"),
                &[turn(0, Role::User, "2026-11-08T03:00:00Z", None, "alpha")],
            )
            .unwrap();
        }
        let result: Result<()> = Index::rebuild(home.path(), |index| {
            index.replace_session(
                &source("new", Tool::Codex, "/t/new.jsonl", "h-new"),
                &[turn(0, Role::User, "2026-11-08T03:00:00Z", None, "beta")],
            )?;
            Err(Error::MissingIndex)
        });
        assert!(matches!(result, Err(Error::MissingIndex)), "{result:?}");
        assert_eq!(file_names(home.path()), [FILE_NAME]);
        let index = Index::open(home.path(), false).unwrap();
        assert_eq!(
            ids_of(&index.recall("alpha", 20, false).unwrap()),
            [("old", 0)]
        );
        assert!(index.recall("beta", 20, false).unwrap().is_empty());
    }

    /// Two sessions: `a` with turns on the 7th and 8th, `b` on the 9th.
    fn three_days() -> (tempfile::TempDir, Index) {
        let (home, mut index) = open_new();
        let a = [
            turn(
                0,
                Role::User,
                "2026-11-07T10:00:00Z",
                None,
                "webhook retry plan",
            ),
            turn(
                1,
                Role::Assistant,
                "2026-11-08T09:00:00Z",
                None,
                "the retry backs off",
            ),
        ];
        index
            .replace_session(&source("a", Tool::ClaudeCode, "/t/a.jsonl", "h1"), &a)
            .unwrap();
        let b = [
            turn(0, Role::User, "2026-11-09T00:00:00Z", None, "unrelated"),
            turn(
                1,
                Role::Tool,
                "2026-11-09T00:00:01Z",
                Some("Bash"),
                "webhook webhook",
            ),
            turn(2, Role::Assistant, "2026-11-09T00:00:02Z", None, "done"),
        ];
        index
            .replace_session(&source("b", Tool::Codex, "/t/rollout-b.jsonl", "h2"), &b)
            .unwrap();
        (home, index)
    }

    #[test]
    fn sessions_between_takes_turns_from_start_up_to_end() {
        let (_home, index) = three_days();
        let between = |start, end| index.sessions_between(at(start), at(end)).unwrap();
        assert_eq!(
            between("2026-11-08T00:00:00Z", "2026-11-09T00:00:00Z"),
            ["a"]
        );
        assert_eq!(
            between("2026-11-09T00:00:00Z", "2026-11-10T00:00:00Z"),
            ["b"]
        );
        assert_eq!(
            between("2026-11-07T00:00:00Z", "2026-11-10T00:00:00Z"),
            ["a", "b"]
        );
        assert!(between("2026-11-10T00:00:00Z", "2026-11-11T00:00:00Z").is_empty());
    }

    #[test]
    fn last_ordinals_and_single_turns() {
        let (_home, index) = three_days();
        let last = index.last_ordinals().unwrap();
        assert_eq!(last.get("a"), Some(&1));
        assert_eq!(last.get("b"), Some(&2));
        let tool = index.turn("b", 1).unwrap().unwrap();
        assert_eq!(
            (
                tool.session.as_str(),
                tool.role.as_str(),
                tool.tool_name.as_deref()
            ),
            ("b", "tool", Some("Bash"))
        );
        assert_eq!(tool.text, "webhook webhook");
        assert!(index.turn("b", 3).unwrap().is_none());
        assert!(index.turn("missing", 0).unwrap().is_none());
    }

    #[test]
    fn search_turns_ranks_whole_turns_and_stops_where_keep_refuses() {
        let (_home, index) = three_days();
        let all = index.search_turns("\"webhook\"", |_| true).unwrap();
        let ids: Vec<(&str, u32)> = all
            .iter()
            .map(|t| (t.session.as_str(), t.ordinal))
            .collect();
        assert_eq!(ids, [("b", 1), ("a", 0)]);
        assert_eq!(all[1].text, "webhook retry plan");
        let mut seen = 0;
        let first = index
            .search_turns("\"webhook\" OR \"retry\"", |_| {
                seen += 1;
                seen <= 1
            })
            .unwrap();
        assert_eq!(first.len(), 1);
        assert!(index
            .search_turns("\"absent\"", |_| true)
            .unwrap()
            .is_empty());
    }

    /// The format-1 schema, as it was written before `turn_keys`.
    const DDL_1: &str = "\
CREATE TABLE sessions(id TEXT PRIMARY KEY, tool TEXT NOT NULL, transcript_path TEXT NOT NULL, first_turn TEXT, last_turn TEXT, file_hash TEXT NOT NULL, error TEXT);
CREATE INDEX sessions_path ON sessions(transcript_path);
CREATE VIRTUAL TABLE turns USING fts5(session_id UNINDEXED, ordinal UNINDEXED, role UNINDEXED, ts UNINDEXED, tool_name UNINDEXED, text, tokenize='unicode61 remove_diacritics 2');
PRAGMA user_version=1;";

    /// A format-1 index in a new home: sessions `a` (two turns) and `b`
    /// (three), written with the old DDL.
    fn format_1_home() -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        let conn = Connection::open(home.path().join(FILE_NAME)).unwrap();
        conn.execute_batch(DDL_1).unwrap();
        for (id, path, turns) in [("a", "/t/a.jsonl", 2), ("b", "/t/b.jsonl", 3)] {
            conn.execute(
                "INSERT INTO sessions(id, tool, transcript_path, first_turn, last_turn, file_hash) VALUES (?1, 'codex', ?2, '2026-11-08T00:00:00.000000Z', '2026-11-08T00:00:00.000000Z', 'h')",
                params![id, path],
            )
            .unwrap();
            for ordinal in 0..turns {
                conn.execute(
                    "INSERT INTO turns(session_id, ordinal, role, ts, tool_name, text) VALUES (?1, ?2, 'user', '2026-11-08T00:00:00.000000Z', NULL, ?3)",
                    params![id, ordinal, format!("{id} turn {ordinal}")],
                )
                .unwrap();
            }
        }
        home
    }

    fn keys(index: &Index) -> Vec<(String, u32, i64)> {
        index
            .conn
            .prepare("SELECT session_id, ordinal, turn FROM turn_keys ORDER BY session_id, ordinal")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    fn rowids(index: &Index) -> Vec<(String, u32, i64)> {
        index
            .conn
            .prepare("SELECT session_id, ordinal, rowid FROM turns ORDER BY session_id, ordinal")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn a_reader_opens_format_1_unchanged_and_a_writer_upgrades_it_keeping_every_turn() {
        let home = format_1_home();
        let reader = Index::open(home.path(), false).unwrap();
        assert_eq!(reader.format(), 1);
        assert_eq!(user_version(&reader), 1);
        assert_eq!(reader.turns_for_session("b").unwrap().len(), 3);
        drop(reader);
        let has_keys = |home: &Path| -> bool {
            Connection::open(home.join(FILE_NAME))
                .unwrap()
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = 'turn_keys')",
                    [],
                    |r| r.get(0),
                )
                .unwrap()
        };
        assert!(!has_keys(home.path()), "a reader must not upgrade");

        let writer = Index::open(home.path(), true).unwrap();
        assert_eq!(writer.format(), 2);
        assert_eq!(user_version(&writer), 2);
        assert_eq!(keys(&writer), rowids(&writer));
        assert_eq!(keys(&writer).len(), 5);
        assert_eq!(writer.turns_for_session("a").unwrap().len(), 2);
        assert_eq!(writer.turn("b", 2).unwrap().unwrap().text, "b turn 2");
        assert_eq!(writer.recall("turn", 20, false).unwrap().len(), 5);
        drop(writer);
        assert_eq!(Index::open(home.path(), false).unwrap().format(), 2);
    }

    #[test]
    fn replacing_and_moving_a_session_keep_turn_keys_equal_to_the_turns() {
        let (_home, mut index) = open_new();
        let two = [
            turn(0, Role::User, "2026-11-08T03:00:00Z", None, "one"),
            turn(1, Role::User, "2026-11-08T03:00:01Z", None, "two"),
        ];
        index
            .replace_session(&source("s1", Tool::Codex, "/t/x.jsonl", "h1"), &two)
            .unwrap();
        index
            .replace_session(&source("s2", Tool::Codex, "/t/y.jsonl", "h2"), &two)
            .unwrap();
        assert_eq!(keys(&index), rowids(&index));
        assert_eq!(keys(&index).len(), 4);
        // Re-ingest with fewer turns.
        index
            .replace_session(&source("s1", Tool::Codex, "/t/x.jsonl", "h3"), &two[..1])
            .unwrap();
        assert_eq!(keys(&index), rowids(&index));
        assert_eq!(keys(&index).len(), 3);
        // The same transcript under another id removes the first id's turns.
        index
            .replace_session(&source("s3", Tool::Codex, "/t/x.jsonl", "h4"), &two)
            .unwrap();
        assert_eq!(keys(&index), rowids(&index));
        assert_eq!(keys(&index).len(), 4);
        assert!(index.turns_for_session("s1").unwrap().is_empty());
    }

    #[test]
    fn every_read_gives_the_same_rows_on_formats_1_and_2() {
        let old_home = format_1_home();
        let old = Index::open(old_home.path(), false).unwrap();
        let new_home = format_1_home();
        let new = Index::open(new_home.path(), true).unwrap();
        assert_eq!((old.format(), new.format()), (1, 2));
        assert_eq!(
            old.turns_for_session("b").unwrap(),
            new.turns_for_session("b").unwrap()
        );
        assert_eq!(old.turn("a", 1).unwrap(), new.turn("a", 1).unwrap());
        assert_eq!(old.turn("a", 2).unwrap(), new.turn("a", 2).unwrap());
        assert_eq!(old.exists("b", 2).unwrap(), new.exists("b", 2).unwrap());
        assert_eq!(old.exists("b", 3).unwrap(), new.exists("b", 3).unwrap());
        assert_eq!(old.turn_counts().unwrap(), new.turn_counts().unwrap());
        assert_eq!(old.last_ordinals().unwrap(), new.last_ordinals().unwrap());
        let pairs: Vec<(String, u32)> = vec![
            ("b".into(), 2),
            ("a".into(), 0),
            ("a".into(), 9),
            ("zz".into(), 0),
            ("b".into(), 2),
        ];
        assert_eq!(old.turns(&pairs).unwrap(), new.turns(&pairs).unwrap());
    }

    #[test]
    fn turns_keeps_the_pairs_order_and_leaves_out_a_missing_pair() {
        let (_home, index) = three_days();
        let pairs: Vec<(String, u32)> = vec![
            ("b".into(), 2),
            ("a".into(), 0),
            ("nope".into(), 0),
            ("a".into(), 7),
            ("b".into(), 0),
        ];
        let got: Vec<(String, u32)> = index
            .turns(&pairs)
            .unwrap()
            .into_iter()
            .map(|t| (t.session, t.ordinal))
            .collect();
        assert_eq!(
            got,
            [
                ("b".to_string(), 2),
                ("a".to_string(), 0),
                ("b".to_string(), 0)
            ]
        );
        assert!(index.turns(&[]).unwrap().is_empty());
    }

    #[test]
    fn a_format_1_scan_works_across_chunks_of_sessions() {
        let home = tempfile::tempdir().unwrap();
        let conn = Connection::open(home.path().join(FILE_NAME)).unwrap();
        conn.execute_batch(DDL_1).unwrap();
        let count = SCAN_CHUNK * 2 + 7;
        let mut pairs = Vec::new();
        for i in 0..count {
            let id = format!("s{i}");
            conn.execute(
                "INSERT INTO sessions(id, tool, transcript_path, file_hash) VALUES (?1, 'codex', ?2, 'h')",
                params![id, format!("/t/{i}")],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO turns(session_id, ordinal, role, ts, tool_name, text) VALUES (?1, 0, 'user', 't', NULL, 'x')",
                [&id],
            )
            .unwrap();
            pairs.push((id, 0));
        }
        drop(conn);
        pairs.reverse();
        let index = Index::open(home.path(), false).unwrap();
        assert_eq!(index.format(), 1);
        let got = index.turns(&pairs).unwrap();
        assert_eq!(got.len(), count);
        assert_eq!(got[0].session, format!("s{}", count - 1));
        assert_eq!(got[count - 1].session, "s0");
    }

    #[test]
    fn turns_for_session_on_format_2_goes_through_turn_keys() {
        let (_home, index) = three_days();
        let plan: Vec<String> = index
            .conn
            .prepare("EXPLAIN QUERY PLAN SELECT k.ordinal, t.role, t.ts, t.tool_name, t.text FROM turn_keys k JOIN turns t ON t.rowid = k.turn WHERE k.session_id = ?1 ORDER BY k.ordinal")
            .unwrap()
            .query_map(["a"], |r| r.get(3))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let plan = plan.join("\n");
        // `turn_keys` is searched by its key; FTS5 reports its rowid lookup
        // as a scan of the virtual table with index 0 (the `=` constraint).
        assert!(
            plan.contains("SEARCH k USING PRIMARY KEY (session_id=?)"),
            "{plan}"
        );
        assert!(!plan.contains("SCAN k"), "{plan}");
    }
    #[test]
    fn sessions_active_since_reads_last_turns() {
        let (_home, mut index) = open_new();
        index
            .replace_session(
                &source("old", Tool::ClaudeCode, "/t/old.jsonl", "h1"),
                &[turn(
                    0,
                    Role::User,
                    "2026-11-01T03:00:00Z",
                    None,
                    "old work",
                )],
            )
            .unwrap();
        index
            .replace_session(
                &source("long", Tool::ClaudeCode, "/t/long.jsonl", "h2"),
                &[
                    turn(0, Role::User, "2026-11-01T03:00:00Z", None, "start"),
                    turn(
                        1,
                        Role::Assistant,
                        "2026-11-08T03:00:00Z",
                        None,
                        "still going",
                    ),
                ],
            )
            .unwrap();
        assert_eq!(
            index
                .sessions_active_since(at("2026-11-07T00:00:00Z"))
                .unwrap(),
            ["long"]
        );
        assert_eq!(
            index
                .sessions_active_since(at("2026-10-01T00:00:00Z"))
                .unwrap(),
            ["long", "old"]
        );
    }
}
