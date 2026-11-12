//! Consistent copies of the database, while the server keeps serving.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use regex::Regex;
use time::format_description::FormatItem;
use time::macros::format_description;
use time::{Duration, OffsetDateTime, PrimitiveDateTime};

use crate::clock;
use crate::db;

const NAME_TIME: &[FormatItem<'static>] =
    format_description!("[year][month][day]T[hour][minute][second]Z");

/// What a backup produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupInfo {
    pub path: PathBuf,
    pub bytes: u64,
    pub version: i64,
}

/// Writes `<dir>/feed-<YYYYMMDDTHHMMSSZ>.sqlite` with `VACUUM INTO`, checks
/// it, records the time in the live database, and deletes copies older than
/// `keep_days` days (never the newest). Any failure is an error.
pub fn backup(
    db_path: &Path,
    dir: &Path,
    keep_days: u32,
    now: OffsetDateTime,
) -> anyhow::Result<BackupInfo> {
    create_dir_private(dir)?;

    let stamp = now
        .to_offset(time::UtcOffset::UTC)
        .format(NAME_TIME)
        .context("cannot format the backup's time")?;
    let target = dir.join(format!("feed-{stamp}.sqlite"));
    if target.exists() {
        bail!("{} already exists; not overwriting it", target.display());
    }

    // The copy: a consistent, compacted snapshot taken on its own connection.
    let live = db::open(db_path)
        .with_context(|| format!("cannot open the database {}", db_path.display()))?;
    let target_text = target
        .to_str()
        .context("the backup path is not valid UTF-8")?;
    live.execute("VACUUM INTO ?1", [target_text])
        .context("VACUUM INTO failed")?;

    // The copy must pass the integrity check; its schema version is reported.
    let version = {
        let copy = rusqlite::Connection::open_with_flags(
            &target,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .context("cannot open the copy")?;
        let verdict: String = copy.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        if verdict != "ok" {
            bail!("the copy failed its integrity check: {verdict}");
        }
        db::user_version(&copy)?
    };
    let bytes = fs::metadata(&target)?.len();

    // The live database learns of the backup; the copy keeps the old value.
    live.execute(
        "INSERT INTO meta (key, value) VALUES ('last_backup', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [clock::storage(now)],
    )?;

    prune(dir, keep_days, now)?;
    Ok(BackupInfo {
        path: target,
        bytes,
        version,
    })
}

/// Deletes the files in `dir` named like a backup whose time is more than
/// `keep_days` days before `now`, except the newest such file. Other files are
/// left alone.
fn prune(dir: &Path, keep_days: u32, now: OffsetDateTime) -> anyhow::Result<()> {
    let pattern = Regex::new(r"^feed-(\d{8}T\d{6}Z)\.sqlite$")?;
    let mut copies: Vec<(OffsetDateTime, PathBuf)> = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(caps) = pattern.captures(name) else {
            continue;
        };
        let Ok(taken) = PrimitiveDateTime::parse(&caps[1], NAME_TIME) else {
            continue;
        };
        copies.push((taken.assume_utc(), entry.path()));
    }
    let newest = copies.iter().map(|(t, _)| *t).max();
    let cutoff = now - Duration::days(i64::from(keep_days));
    for (taken, path) in copies {
        if taken < cutoff && Some(taken) != newest {
            fs::remove_file(&path)
                .with_context(|| format!("cannot delete the old copy {}", path.display()))?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn create_dir_private(dir: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    if dir.is_dir() {
        return Ok(());
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .with_context(|| format!("cannot create {}", dir.display()))
}

#[cfg(not(unix))]
fn create_dir_private(dir: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn setup() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("feed.sqlite");
        db::migrate(&path).expect("migrate");
        (dir, path)
    }

    #[test]
    fn a_backup_is_named_by_time_and_checked() {
        let (dir, path) = setup();
        let out = dir.path().join("backups");
        let now = datetime!(2026-11-08 02:30:05 UTC);
        let info = backup(&path, &out, 14, now).expect("backup");
        assert_eq!(info.path, out.join("feed-20261108T023005Z.sqlite"));
        assert_eq!(info.version, db::latest_version());
        assert!(info.bytes > 0);
        let live = db::open(&path).expect("open");
        let last: String = live
            .query_row(
                "SELECT value FROM meta WHERE key = 'last_backup'",
                [],
                |r| r.get(0),
            )
            .expect("last_backup");
        assert_eq!(last, "2026-11-08T02:30:05.000000Z");
        // The copy keeps the previous value: none.
        let copy = db::open(&info.path).expect("copy");
        let none: Option<String> = copy
            .query_row(
                "SELECT value FROM meta WHERE key = 'last_backup'",
                [],
                |r| r.get(0),
            )
            .ok();
        assert_eq!(none, None);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&out).expect("meta").permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
    }

    #[test]
    fn a_backup_never_overwrites() {
        let (dir, path) = setup();
        let out = dir.path().join("b");
        let now = datetime!(2026-11-08 02:30:05 UTC);
        backup(&path, &out, 14, now).expect("first");
        let e = backup(&path, &out, 14, now)
            .expect_err("second")
            .to_string();
        assert!(e.contains("already exists"), "{e}");
    }

    #[test]
    fn pruning_keeps_the_newest_and_other_files() {
        let (dir, path) = setup();
        let out = dir.path().join("b");
        fs::create_dir_all(&out).expect("dir");
        for name in [
            "feed-20260101T000000Z.sqlite",
            "feed-20261020T000000Z.sqlite",
            "feed-20261026T000000Z.sqlite",
            "notes.txt",
            "feed-2026.sqlite",
            "other-20260101T000000Z.sqlite",
        ] {
            fs::write(out.join(name), b"x").expect("write");
        }
        let now = datetime!(2026-11-08 02:30:05 UTC);
        backup(&path, &out, 14, now).expect("backup");
        let mut left: Vec<String> = fs::read_dir(&out)
            .expect("read")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            vec![
                "feed-2026.sqlite",
                "feed-20261026T000000Z.sqlite",
                "feed-20261108T023005Z.sqlite",
                "notes.txt",
                "other-20260101T000000Z.sqlite",
            ]
        );
    }

    #[test]
    fn the_newest_copy_survives_whatever_its_age() {
        let (dir, path) = setup();
        let out = dir.path().join("b");
        backup(&path, &out, 14, datetime!(2026-01-01 00:00:00 UTC)).expect("old");
        // Pruning from a much later clock, with a new copy: the new one is
        // the newest, so the old one goes.
        backup(&path, &out, 14, datetime!(2026-11-08 00:00:00 UTC)).expect("new");
        let names: Vec<String> = fs::read_dir(&out)
            .expect("read")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["feed-20261108T000000Z.sqlite".to_string()]);
        // With keep_days 0 the newest still stays.
        prune(&out, 0, datetime!(2030-01-01 00:00:00 UTC)).expect("prune");
        assert_eq!(fs::read_dir(&out).expect("read").count(), 1);
    }
}
