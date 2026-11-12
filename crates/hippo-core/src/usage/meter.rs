//! The client's own usage meter.
//!
//! Claude Code caches each account's 5-hour and 7-day utilisation in its
//! profile's `.claude.json`, under `cachedUsageUtilization`. Only that key
//! is deserialised: every other key in the file is skipped by the parser
//! and never held. The format is undocumented, so every field is optional
//! and a value of the wrong shape reads as missing. Nothing here writes or
//! caches anything.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// A settings file larger than this is not read.
pub const MAX_BYTES: u64 = 4 * 1024 * 1024;

/// A meter fetched longer ago than this, with turns since, is stale.
pub const FRESH_FOR_SECS: i64 = 60 * 60;

/// One window's figure.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window {
    /// Percent used, 0 to 100.
    pub used_percent: f64,
    /// When the window resets, Unix seconds.
    pub resets_at: Option<i64>,
}

/// A profile's cached meter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Meter {
    /// When the client fetched it, Unix seconds.
    pub fetched_at: i64,
    pub five_hour: Option<Window>,
    pub seven_day: Option<Window>,
}

impl Meter {
    /// Whether the meter can be trusted at `now`: fetched in the last hour,
    /// or nothing used since (`last_used`, the profile's newest message).
    pub fn current(&self, now: i64, last_used: Option<i64>) -> bool {
        now - self.fetched_at <= FRESH_FOR_SECS || last_used.is_none_or(|t| t <= self.fetched_at)
    }
}

impl Window {
    /// The fraction left at `now`; a window whose reset has passed has
    /// reset (all of it left).
    pub fn left(&self, now: i64) -> f64 {
        if self.reset_passed(now) {
            1.0
        } else {
            (1.0 - self.used_percent / 100.0).clamp(0.0, 1.0)
        }
    }

    /// Whether its reset is at or before `now`.
    pub fn reset_passed(&self, now: i64) -> bool {
        self.resets_at.is_some_and(|r| r <= now)
    }
}

/// The client settings file of a Claude config dir: `~/.claude.json` for
/// `~/.claude`, else `<dir>/.claude.json`.
pub fn settings_file(dir: &Path) -> PathBuf {
    if dir.file_name().is_some_and(|n| n == ".claude") {
        if let Some(parent) = dir.parent() {
            return parent.join(".claude.json");
        }
    }
    dir.join(".claude.json")
}

/// Only the one key: serde skips every other key as it parses (unknown
/// fields are ignored without being buffered), so none is held.
#[derive(Deserialize)]
struct OnlyMeter {
    #[serde(rename = "cachedUsageUtilization", default)]
    meter: Option<Value>,
}

/// Reads the meter from the settings file at `path`; `None` when the file
/// is missing, too large or unreadable, or holds no usable meter.
pub fn read(path: &Path) -> Option<Meter> {
    let file = File::open(path).ok()?;
    if file.metadata().ok()?.len() > MAX_BYTES {
        return None;
    }
    let mut text = String::new();
    file.take(MAX_BYTES).read_to_string(&mut text).ok()?;
    parse(&text)
}

/// [`read`] on a file's text.
pub fn parse(text: &str) -> Option<Meter> {
    let only: OnlyMeter = serde_json::from_str(text).ok()?;
    let meter = only.meter?;
    let fetched_ms = meter.get("fetchedAtMs")?.as_i64()?;
    let util = meter.get("utilization")?;
    let window = |key: &str| -> Option<Window> {
        let w = util.get(key)?;
        let used = w.get("utilization")?.as_f64()?;
        if !used.is_finite() {
            return None;
        }
        let resets_at = w
            .get("resets_at")
            .and_then(Value::as_str)
            .and_then(|t| OffsetDateTime::parse(t, &Rfc3339).ok())
            .map(OffsetDateTime::unix_timestamp);
        Some(Window {
            used_percent: used.clamp(0.0, 100.0),
            resets_at,
        })
    };
    let five_hour = window("five_hour");
    let seven_day = window("seven_day");
    if five_hour.is_none() && seven_day.is_none() {
        return None;
    }
    Some(Meter {
        fetched_at: fetched_ms.div_euclid(1000),
        five_hour,
        seven_day,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn file(meter: Value) -> String {
        json!({
            "oauthAccount": {"emailAddress": "someone@example.com"},
            "projects": {"/x": {"history": ["secret"]}},
            "cachedUsageUtilization": meter,
            "numStartups": 4
        })
        .to_string()
    }

    #[test]
    fn only_the_meter_key_is_read() {
        let text = file(json!({
            "accountUuid": "u",
            "fetchedAtMs": 1_794_286_532_276_i64,
            "utilization": {
                "five_hour": {"utilization": 25, "resets_at": "2026-11-10T08:30:00.108322+00:00"},
                "seven_day": {"utilization": 80.5, "resets_at": "2026-11-10T20:00:00+00:00"},
                "seven_day_opus": {"utilization": 3, "resets_at": null}
            }
        }));
        let m = parse(&text).unwrap();
        assert_eq!(m.fetched_at, 1_794_286_532);
        let five = m.five_hour.unwrap();
        assert_eq!(five.used_percent, 25.0);
        assert_eq!(
            five.resets_at,
            Some(time::macros::datetime!(2026-11-10 08:30:00 UTC).unix_timestamp())
        );
        assert_eq!(m.seven_day.unwrap().used_percent, 80.5);
    }

    #[test]
    fn missing_or_misshapen_values_read_as_missing() {
        assert_eq!(parse("{}"), None);
        assert_eq!(parse("not json"), None);
        assert_eq!(parse(&file(Value::Null)), None);
        assert_eq!(parse(&file(json!({"utilization": {}}))), None);
        assert_eq!(
            parse(&file(json!({"fetchedAtMs": "x", "utilization": {}}))),
            None
        );
        let one = parse(&file(json!({
            "fetchedAtMs": 1000,
            "utilization": {"five_hour": {"utilization": "lots"}, "seven_day": {"utilization": 140, "resets_at": 7}}
        })))
        .unwrap();
        assert_eq!(one.five_hour, None);
        // Clamped; a reset that is not a timestamp string reads as unknown.
        assert_eq!(
            one.seven_day,
            Some(Window {
                used_percent: 100.0,
                resets_at: None
            })
        );
    }

    #[test]
    fn a_past_reset_has_reset_and_staleness_needs_turns_since() {
        let w = Window {
            used_percent: 90.0,
            resets_at: Some(1_000),
        };
        assert!((w.left(999) - 0.1).abs() < 1e-9);
        assert_eq!(w.left(1_000), 1.0);
        let m = Meter {
            fetched_at: 10_000,
            five_hour: Some(w),
            seven_day: None,
        };
        assert!(m.current(10_000 + FRESH_FOR_SECS, Some(12_000)));
        assert!(!m.current(10_001 + FRESH_FOR_SECS, Some(12_000)));
        // Nothing used since it was fetched: still current, days later.
        assert!(m.current(10_000 + 3 * 86_400, Some(9_000)));
        assert!(m.current(10_000 + 3 * 86_400, None));
    }

    #[test]
    fn the_settings_file_sits_beside_the_default_dir() {
        assert_eq!(
            settings_file(Path::new("/h/.claude")),
            Path::new("/h/.claude.json")
        );
        assert_eq!(
            settings_file(Path::new("/h/.claude-work")),
            Path::new("/h/.claude-work/.claude.json")
        );
    }

    #[test]
    fn an_oversized_or_missing_file_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read(&dir.path().join("none.json")), None);
        let big = dir.path().join("big.json");
        let f = std::fs::File::create(&big).unwrap();
        f.set_len(MAX_BYTES + 1).unwrap();
        assert_eq!(read(&big), None);
        let ok = dir.path().join("ok.json");
        std::fs::write(
            &ok,
            file(json!({"fetchedAtMs": 5000, "utilization": {"seven_day": {"utilization": 10, "resets_at": null}}})),
        )
        .unwrap();
        assert_eq!(read(&ok).unwrap().fetched_at, 5);
    }
}
