//! The one clock every time-dependent decision reads, and the time formats.
//!
//! The database stores times as fixed-width text so that text order is time
//! order. JSON outputs use RFC 3339 to the second. Page ages are written for
//! people.

use std::sync::Mutex;

use time::format_description::FormatItem;
use time::macros::format_description;
use time::{Duration, OffsetDateTime, PrimitiveDateTime};

/// A source of the current time.
pub trait Clock: Send + Sync {
    fn now(&self) -> OffsetDateTime;
}

/// The system clock, used in production.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

/// A clock that only moves when told to, used in tests.
#[derive(Debug)]
pub struct FixedClock {
    now: Mutex<OffsetDateTime>,
}

impl FixedClock {
    pub fn new(start: OffsetDateTime) -> Self {
        Self {
            now: Mutex::new(start),
        }
    }

    pub fn set(&self, to: OffsetDateTime) {
        *self.lock() = to;
    }

    pub fn advance(&self, by: Duration) {
        let mut now = self.lock();
        *now += by;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, OffsetDateTime> {
        // A poisoned lock only means another test thread panicked; the stored
        // time is still valid.
        self.now.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Clock for FixedClock {
    fn now(&self) -> OffsetDateTime {
        *self.lock()
    }
}

const STORAGE: &[FormatItem<'static>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:6]Z");
const RFC3339_SECONDS: &[FormatItem<'static>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z");
const DAY_MONTH_YEAR: &[FormatItem<'static>] =
    format_description!("[day padding:none] [month repr:short] [year]");

/// The database's text form of a time: `YYYY-MM-DDTHH:MM:SS.ffffffZ`, in UTC.
pub fn storage(t: OffsetDateTime) -> String {
    format_utc(t, STORAGE)
}

/// Reads a time written by [`storage`].
pub fn parse_storage(text: &str) -> Option<OffsetDateTime> {
    PrimitiveDateTime::parse(text, STORAGE)
        .ok()
        .map(PrimitiveDateTime::assume_utc)
}

/// RFC 3339 to the second, in UTC, as the JSON outputs write times.
pub fn rfc3339(t: OffsetDateTime) -> String {
    format_utc(t, RFC3339_SECONDS)
}

/// A date as a page writes it: "8 Nov 2026", in UTC.
pub fn date_text(t: OffsetDateTime) -> String {
    format_utc(t, DAY_MONTH_YEAR)
}

/// The RFC 3339 form of a stored time, or `None` if the text is not one.
pub fn storage_to_rfc3339(text: &str) -> Option<String> {
    parse_storage(text).map(rfc3339)
}

fn format_utc(t: OffsetDateTime, format: &[FormatItem<'static>]) -> String {
    t.to_offset(time::UtcOffset::UTC)
        .format(format)
        .unwrap_or_default()
}

/// A time as a page writes it: "just now" under a minute, then minutes, hours
/// (under 48), days (under 30), and a date such as "8 Nov 2026" after that.
/// Units are whole numbers rounded down, singular at one.
pub fn age_text(then: OffsetDateTime, now: OffsetDateTime) -> String {
    let secs = (now - then).whole_seconds();
    if secs < 60 {
        return "just now".to_string();
    }
    let minutes = secs / 60;
    let hours = minutes / 60;
    let days = hours / 24;
    if hours < 1 {
        unit(minutes, "minute")
    } else if hours < 48 {
        unit(hours, "hour")
    } else if days < 30 {
        unit(days, "day")
    } else {
        format_utc(then, DAY_MONTH_YEAR)
    }
}

fn unit(n: i64, name: &str) -> String {
    if n == 1 {
        format!("1 {name} ago")
    } else {
        format!("{n} {name}s ago")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn storage_round_trips_and_sorts() {
        let t = datetime!(2026-11-08 10:00:00.5 UTC);
        let text = storage(t);
        assert_eq!(text, "2026-11-08T10:00:00.500000Z");
        assert_eq!(parse_storage(&text), Some(t));
        assert!(storage(t) < storage(t + Duration::seconds(1)));
        let forever = parse_storage("9999-12-31T23:59:59.999999Z");
        assert!(forever.is_some());
    }

    #[test]
    fn rfc3339_is_to_the_second() {
        let t = datetime!(2026-11-08 10:00:00.9 UTC);
        assert_eq!(rfc3339(t), "2026-11-08T10:00:00Z");
        assert_eq!(
            storage_to_rfc3339("2026-11-08T02:30:05.123456Z").as_deref(),
            Some("2026-11-08T02:30:05Z")
        );
        assert_eq!(storage_to_rfc3339("not a time"), None);
    }

    #[test]
    fn age_text_at_each_threshold() {
        let now = datetime!(2026-11-08 10:00:00 UTC);
        let ago = |d: Duration| age_text(now - d, now);
        assert_eq!(ago(Duration::seconds(59)), "just now");
        assert_eq!(ago(Duration::minutes(1)), "1 minute ago");
        assert_eq!(ago(Duration::minutes(59)), "59 minutes ago");
        assert_eq!(ago(Duration::hours(1)), "1 hour ago");
        assert_eq!(ago(Duration::hours(47)), "47 hours ago");
        assert_eq!(ago(Duration::hours(48)), "2 days ago");
        assert_eq!(ago(Duration::days(29)), "29 days ago");
        assert_eq!(ago(Duration::days(30)), "9 Oct 2026");
        // A time slightly in the future (clock skew) reads as now.
        assert_eq!(age_text(now + Duration::seconds(5), now), "just now");
    }

    #[test]
    fn fixed_clock_moves_only_when_told() {
        let clock = FixedClock::new(datetime!(2026-11-08 10:00:00 UTC));
        assert_eq!(clock.now(), datetime!(2026-11-08 10:00:00 UTC));
        clock.advance(Duration::hours(2));
        assert_eq!(clock.now(), datetime!(2026-11-08 12:00:00 UTC));
        clock.set(datetime!(2027-01-01 00:00:00 UTC));
        assert_eq!(clock.now(), datetime!(2027-01-01 00:00:00 UTC));
    }
}
