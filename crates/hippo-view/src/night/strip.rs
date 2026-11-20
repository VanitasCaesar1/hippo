//! The strip above Morning's columns: yesterday as bars of activity with
//! the person's corrections marked, the night with the pass in it, and this
//! morning up to now.
//!
//! The bars are the rollup's activity buckets ([`BUCKET_SECONDS`], turns
//! plus three per edit, every session summed), so the strip costs one
//! query. The night runs from local midnight to the first activity after
//! the pass ended (to 06:00 when there was none, or no pass).

use hippo_core::incidents::Kind;
use hippo_core::rollup::{Rollup, BUCKET_SECONDS};
use serde::Serialize;
use time::{Duration, OffsetDateTime, Time, UtcOffset};

use super::{count, hm, long_date};

/// The strip.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Strip {
    /// Yesterday's local midnight: the strip's left edge.
    #[serde(with = "time::serde::rfc3339")]
    pub start: OffsetDateTime,
    /// The right edge.
    #[serde(with = "time::serde::rfc3339")]
    pub now: OffsetDateTime,
    pub bucket_seconds: i64,
    /// Activity per bucket from `start`: turns plus three per edit.
    pub beats: Vec<u32>,
    /// The bucket of each correction the person gave in the strip's span.
    pub corrections: Vec<usize>,
    /// The night: from today's midnight to its end.
    #[serde(with = "time::serde::rfc3339")]
    pub night_start: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub night_end: OffsetDateTime,
    /// The pass inside the night, when there was one.
    #[serde(with = "time::serde::rfc3339::option")]
    pub pass_start: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub pass_end: Option<OffsetDateTime>,
    /// `Thursday, 36 sessions, 4 corrections`.
    pub day: String,
    /// `pass at 03:30`, or `no pass`.
    pub night: String,
    /// `08:41`.
    pub morning: String,
}

impl Strip {
    /// Where `t` falls, 0 to 1, from the left edge.
    pub fn x(&self, t: OffsetDateTime) -> f32 {
        let span = (self.now - self.start).whole_seconds().max(1) as f32;
        ((t - self.start).whole_seconds() as f32 / span).clamp(0., 1.)
    }

    /// The bucket count that spans the strip.
    pub fn buckets(&self) -> usize {
        ((self.now - self.start).whole_seconds() / self.bucket_seconds).max(1) as usize + 1
    }
}

/// Builds the strip for `now`, with the pass's start and end when known.
pub fn build(
    rollup: Option<&Rollup>,
    pass: Option<(Option<OffsetDateTime>, Option<OffsetDateTime>)>,
    now: OffsetDateTime,
    offset: UtcOffset,
) -> Strip {
    let local = now.to_offset(offset);
    let midnight = local.replace_time(Time::MIDNIGHT);
    let start = midnight - Duration::days(1);
    let n = ((now - start).whole_seconds() / BUCKET_SECONDS).max(0) as usize + 1;
    let mut beats = vec![0u32; n];
    let mut sessions = 0;
    let mut corrections = Vec::new();
    let mut first_after: Option<OffsetDateTime> = None;
    let (pass_start, pass_end) = pass.unwrap_or((None, None));
    if let Some(r) = rollup {
        for (t, turns, edits) in r
            .activity(start, now + Duration::minutes(1))
            .unwrap_or_default()
        {
            let k = ((t - start).whole_seconds() / BUCKET_SECONDS) as usize;
            if let Some(b) = beats.get_mut(k) {
                *b = b.saturating_add((turns + 3 * edits).min(u64::from(u32::MAX)) as u32);
            }
            let after = pass_end.unwrap_or(midnight);
            if t >= after && t >= midnight && first_after.is_none_or(|f| t < f) {
                first_after = Some(t);
            }
        }
        sessions = r
            .sessions_in(start, midnight)
            .unwrap_or_default()
            .iter()
            .filter(|s| s.facts.parent.is_none() && !s.facts.automated)
            .count();
        for i in r.incidents(start).unwrap_or_default() {
            if i.kind == Kind::Corrected && i.at >= start && i.at <= now {
                corrections.push(((i.at - start).whole_seconds() / BUCKET_SECONDS) as usize);
            }
        }
        corrections.sort_unstable();
    }
    let six = midnight + Duration::hours(6);
    let night_end = first_after
        .filter(|t| *t > midnight)
        .unwrap_or(six)
        .max(pass_end.unwrap_or(midnight))
        .min(now)
        .max(midnight.min(now));
    let yesterday = start.date();
    let day = format!(
        "{}, {}, {}",
        long_date(yesterday).split(' ').next().unwrap_or_default(),
        count(sessions, "session", "sessions"),
        count(
            corrections
                .iter()
                .filter(|k| start + Duration::seconds(**k as i64 * BUCKET_SECONDS) < midnight)
                .count(),
            "correction",
            "corrections"
        ),
    );
    let night = match pass_start.or(pass_end) {
        Some(t) if t >= midnight - Duration::hours(6) => format!("pass at {}", hm(t, offset)),
        _ => "no pass".to_owned(),
    };
    Strip {
        start,
        now,
        bucket_seconds: BUCKET_SECONDS,
        beats,
        corrections,
        night_start: midnight,
        night_end,
        pass_start: pass_start.filter(|t| *t >= start),
        pass_end: pass_end.filter(|t| *t >= start),
        day,
        night,
        morning: hm(now, offset),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn an_empty_strip_spans_yesterday_to_now() {
        let now = datetime!(2026-11-20 08:41 UTC);
        let s = build(None, None, now, UtcOffset::UTC);
        assert_eq!(s.start, datetime!(2026-11-19 00:00 UTC));
        assert_eq!(s.beats.len(), (24 * 4) + 34 + 1);
        assert_eq!(s.night_start, datetime!(2026-11-20 00:00 UTC));
        assert_eq!(s.night_end, datetime!(2026-11-20 06:00 UTC));
        assert_eq!(s.day, "Thursday, 0 sessions, 0 corrections");
        assert_eq!(s.night, "no pass");
        assert_eq!(s.morning, "08:41");
        assert!((s.x(datetime!(2026-11-20 00:00 UTC)) - 24. / 32.683).abs() < 0.01);
    }

    #[test]
    fn the_pass_names_the_night() {
        let now = datetime!(2026-11-20 08:41 UTC);
        let s = build(
            None,
            Some((
                Some(datetime!(2026-11-20 03:30 UTC)),
                Some(datetime!(2026-11-20 03:34 UTC)),
            )),
            now,
            UtcOffset::UTC,
        );
        assert_eq!(s.night, "pass at 03:30");
    }
}
