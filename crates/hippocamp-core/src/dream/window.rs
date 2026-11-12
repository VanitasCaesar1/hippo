//! Dreaming step 1: the window, from `last_success` to now, in day chunks.

use std::io;
use std::path::PathBuf;

use time::{Duration, OffsetDateTime};

use crate::config::Workspace;
use crate::error::{Error, Result};
use crate::records::DreamState;

/// One day of the window: turns at or after `start` and before `end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chunk {
    pub start: OffsetDateTime,
    pub end: OffsetDateTime,
}

/// `<workspace>/.hippocamp/dream-state.json`.
pub fn state_path(ws: &Workspace) -> PathBuf {
    ws.hippocamp_dir().join("dream-state.json")
}

/// `last_success` from `dream-state.json`; `None` when the file or the key
/// is absent. A file that cannot be read is an error.
pub fn last_success(ws: &Workspace) -> Result<Option<OffsetDateTime>> {
    match DreamState::load(&state_path(ws)) {
        Ok(state) => Ok(state.last_success),
        Err(Error::Io(err)) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// The window from `last_success` to `now` as day chunks in order, or the
/// 24 hours before `now` when there has been no successful run. Each chunk
/// is 24 hours long except the last, which ends at `now`. There are none
/// when `last_success` is not before `now`.
pub fn chunks(last_success: Option<OffsetDateTime>, now: OffsetDateTime) -> Vec<Chunk> {
    let mut start = last_success.unwrap_or(now - Duration::DAY);
    let mut chunks = Vec::new();
    while start < now {
        let end = (start + Duration::DAY).min(now);
        chunks.push(Chunk { start, end });
        start = end;
    }
    chunks
}

/// Records `end` as `last_success`, once every pass has finished the chunk
/// that ends there. A dry run never calls it.
pub fn advance(ws: &Workspace, end: OffsetDateTime) -> Result<()> {
    DreamState {
        last_success: Some(end),
    }
    .save(&state_path(ws))
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn the_first_run_is_the_previous_24_hours() {
        let now = datetime!(2026-11-08 03:30 UTC);
        assert_eq!(
            chunks(None, now),
            [Chunk {
                start: datetime!(2026-11-07 03:30 UTC),
                end: now,
            }]
        );
    }

    #[test]
    fn a_longer_window_is_whole_days_then_the_rest() {
        let now = datetime!(2026-11-08 03:30 UTC);
        let last = datetime!(2026-11-06 15:30 UTC);
        assert_eq!(
            chunks(Some(last), now),
            [
                Chunk {
                    start: last,
                    end: datetime!(2026-11-07 15:30 UTC),
                },
                Chunk {
                    start: datetime!(2026-11-07 15:30 UTC),
                    end: now,
                },
            ]
        );
        let two_days = chunks(Some(datetime!(2026-11-06 03:30 UTC)), now);
        assert_eq!(two_days.len(), 2);
        assert_eq!(two_days[1].end, now);
    }

    #[test]
    fn no_chunks_when_last_success_is_now_or_later() {
        let now = datetime!(2026-11-08 03:30 UTC);
        assert!(chunks(Some(now), now).is_empty());
        assert!(chunks(Some(now + Duration::HOUR), now).is_empty());
    }

    #[test]
    fn advance_writes_last_success_and_last_success_reads_it() {
        let dir = tempfile::tempdir().unwrap();
        let ws = crate::dream::tests::workspace_at(dir.path());
        assert_eq!(last_success(&ws).unwrap(), None);
        let end = datetime!(2026-11-08 03:30 UTC);
        advance(&ws, end).unwrap();
        assert_eq!(last_success(&ws).unwrap(), Some(end));
        std::fs::write(state_path(&ws), "{\"protocol\":\"HIPPOCAMP_V0\"}").unwrap();
        assert!(matches!(last_success(&ws), Err(Error::Protocol { .. })));
    }
}
