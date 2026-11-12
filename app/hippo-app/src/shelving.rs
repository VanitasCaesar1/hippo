//! Where each of today's sessions sits, and the counts every view shows.
//!
//! The rule is [`hippo_view::fleet::shelf`] (the Settled rule) with
//! the person's own marks on top: a session settled or un-settled by hand,
//! and one snoozed until a time (T3 Code's Settle, Un-settle and Snooze,
//! MIT). A session that needs the person is never moved by a mark. Every
//! count the app shows (Home's headline, the sidebar's shelves and its
//! toggle's badge, Fleet's groups, the menu-bar item) comes from
//! [`Shelves::tally`], so no two of them can disagree.

use std::collections::BTreeMap;

use hippo_view::fleet::{self, Fleet, Row, Shelf};
use time::{Duration, OffsetDateTime, UtcOffset, Weekday};

use crate::prefs::{Mark, Snooze};

/// The person's marks and the clock.
#[derive(Clone, Copy)]
pub struct Marks<'a> {
    pub opened: &'a BTreeMap<String, i64>,
    pub settled: &'a BTreeMap<String, Mark>,
    pub snoozed: &'a BTreeMap<String, Snooze>,
    /// Unix seconds.
    pub now: i64,
}

/// Where a row goes: a shelf, or hidden as snoozed until a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Shelf(Shelf),
    Snoozed { until: i64 },
}

impl Marks<'_> {
    /// A hand-made settle holds while nothing newer happened in the
    /// session; an un-settle holds until it is pruned.
    fn hand(&self, row: &Row) -> Option<bool> {
        let m = self.settled.get(&row.id)?;
        let newer = row.last.is_some_and(|t| t.unix_timestamp() > m.at);
        (!(m.settled && newer)).then_some(m.settled)
    }

    /// The Settled-rule shelf, with the person's marks.
    pub fn place(&self, row: &Row) -> Place {
        let base = fleet::shelf(row, |id| self.opened.contains_key(id));
        if base == Shelf::NeedsYou {
            return Place::Shelf(base);
        }
        if let Some(s) = self.snoozed.get(&row.id).filter(|s| s.until > self.now) {
            return Place::Snoozed { until: s.until };
        }
        Place::Shelf(match (base, self.hand(row)) {
            (Shelf::ToReview, Some(true)) => Shelf::Settled,
            (Shelf::Settled, Some(false)) => Shelf::ToReview,
            (b, _) => b,
        })
    }
}

/// Today's sessions by shelf, in the sidebar's order.
pub struct Shelves<'a> {
    /// Needs you (Home's order: the longest wait first), Working, To
    /// review, Settled; empty shelves are kept, so callers index freely.
    pub shelves: [(Shelf, Vec<&'a Row>); 4],
    /// Snoozed sessions, the soonest to wake first, with their wake time.
    pub snoozed: Vec<(&'a Row, i64)>,
}

/// The counts every view shows, from one [`Shelves`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tally {
    pub needs_you: usize,
    pub working: usize,
    pub to_review: usize,
    pub settled: usize,
    pub snoozed: usize,
}

/// Whether `row` sits under another row of `rows`: a T3 Code delegated
/// session whose parent is among them.
pub fn nested_in(row: &Row, rows: &[&Row]) -> bool {
    row.delegated_by
        .as_deref()
        .is_some_and(|p| p != row.id && rows.iter().any(|r| r.id == p))
}

/// How many rows of `rows` stand on their own: a nested one counts with
/// its parent.
pub fn count_own(rows: &[&Row]) -> usize {
    rows.iter().filter(|r| !nested_in(r, rows)).count()
}

/// The shelf a family goes on: the more urgent of its members'.
fn rank(s: Shelf) -> usize {
    match s {
        Shelf::NeedsYou => 0,
        Shelf::Working => 1,
        Shelf::ToReview => 2,
        Shelf::Settled => 3,
    }
}

impl<'a> Shelves<'a> {
    pub fn of(f: &'a Fleet, marks: &Marks<'_>) -> Shelves<'a> {
        let mut shelves: [(Shelf, Vec<&Row>); 4] = [
            (Shelf::NeedsYou, Vec::new()),
            (Shelf::Working, Vec::new()),
            (Shelf::ToReview, Vec::new()),
            (Shelf::Settled, Vec::new()),
        ];
        let mut snoozed: Vec<(&Row, i64)> = Vec::new();
        // Needs you in Home's order.
        let needs: Vec<&str> = f.needs_you().iter().map(|n| n.row.id.as_str()).collect();
        // A delegated session whose parent is in today's fleet
        // sits right under it, unless it needs the person itself.
        let root_of = |r: &Row| -> Option<&'a Row> {
            let mut at = f.row(r.delegated_by.as_deref()?)?;
            for _ in 0..8 {
                match at.delegated_by.as_deref().and_then(|p| f.row(p)) {
                    Some(up) if up.id != r.id => at = up,
                    _ => break,
                }
            }
            (at.id != r.id).then_some(at)
        };
        let mut kids: BTreeMap<&str, Vec<&Row>> = BTreeMap::new();
        for r in &f.rows {
            if needs.contains(&r.id.as_str()) {
                continue;
            }
            if let Some(root) = root_of(r) {
                kids.entry(root.id.as_str()).or_default().push(r);
            }
        }
        let nested = |r: &Row| kids.values().flatten().any(|k| k.id == r.id);
        for id in &needs {
            if let Some(r) = f.row(id) {
                shelves[0].1.push(r);
                shelves[0]
                    .1
                    .extend(kids.get(r.id.as_str()).into_iter().flatten());
            }
        }
        for r in &f.rows {
            if needs.contains(&r.id.as_str()) || nested(r) {
                continue;
            }
            let family = kids.get(r.id.as_str()).cloned().unwrap_or_default();
            match marks.place(r) {
                Place::Snoozed { until } => {
                    snoozed.push((r, until));
                    snoozed.extend(family.into_iter().map(|k| (k, until)));
                }
                Place::Shelf(s) => {
                    let i = family
                        .iter()
                        .filter_map(|k| match marks.place(k) {
                            Place::Shelf(ks) => Some(rank(ks)),
                            Place::Snoozed { .. } => None,
                        })
                        .chain(std::iter::once(rank(s)))
                        .min()
                        .unwrap_or(rank(s));
                    shelves[i].1.push(r);
                    shelves[i].1.extend(family);
                }
            }
        }
        // The soonest to wake first, a family kept together.
        let mut order: Vec<(&Row, i64)> = Vec::new();
        let mut tops: Vec<(&Row, i64)> = snoozed
            .iter()
            .copied()
            .filter(|(r, _)| !nested(r))
            .collect();
        tops.sort_by_key(|(_, until)| *until);
        for (r, until) in tops {
            order.push((r, until));
            order.extend(
                kids.get(r.id.as_str())
                    .into_iter()
                    .flatten()
                    .map(|k| (*k, until)),
            );
        }
        Shelves {
            shelves,
            snoozed: order,
        }
    }

    /// The rows of one shelf.
    pub fn rows(&self, s: Shelf) -> &[&'a Row] {
        &self
            .shelves
            .iter()
            .find(|(k, _)| *k == s)
            .expect("four shelves")
            .1
    }

    /// How many of the Working shelf's sessions are on `profile` (the
    /// Headroom pane's `2 working`).
    pub fn working_on(&self, profile: &str) -> usize {
        let rows = self.rows(Shelf::Working);
        rows.iter()
            .filter(|r| r.profile.as_deref() == Some(profile) && !nested_in(r, rows))
            .count()
    }

    pub fn tally(&self) -> Tally {
        Tally {
            needs_you: count_own(self.rows(Shelf::NeedsYou)),
            working: count_own(self.rows(Shelf::Working)),
            to_review: count_own(self.rows(Shelf::ToReview)),
            settled: count_own(self.rows(Shelf::Settled)),
            snoozed: count_own(&self.snoozed.iter().map(|(r, _)| *r).collect::<Vec<_>>()),
        }
    }
}

/// `groups` placed as `snap` had them (a live refresh never moves
/// a row under the pointer). Rows still there keep their group and order;
/// new rows join their own group at its end; gone rows go. Groups left
/// empty are dropped.
pub fn freeze<K: PartialEq + Clone, T>(
    groups: Vec<(K, Vec<T>)>,
    snap: &[(K, Vec<String>)],
    id: impl Fn(&T) -> &str,
) -> Vec<(K, Vec<T>)> {
    let mut pool: Vec<(K, T)> = Vec::new();
    for (k, rows) in groups {
        pool.extend(rows.into_iter().map(|r| (k.clone(), r)));
    }
    let mut out: Vec<(K, Vec<T>)> = Vec::new();
    for (k, ids) in snap {
        let mut rows = Vec::new();
        for want in ids {
            if let Some(i) = pool.iter().position(|(_, r)| id(r) == want) {
                rows.push(pool.remove(i).1);
            }
        }
        out.push((k.clone(), rows));
    }
    for (k, r) in pool {
        match out.iter_mut().find(|(g, _)| *g == k) {
            Some(g) => g.1.push(r),
            None => out.push((k, vec![r])),
        }
    }
    out.retain(|(_, rows)| !rows.is_empty());
    out
}

/// The ids of `groups`, for [`freeze`].
pub fn snapshot<K: Clone, T>(
    groups: &[(K, Vec<T>)],
    id: impl Fn(&T) -> &str,
) -> Vec<(K, Vec<String>)> {
    groups
        .iter()
        .map(|(k, rows)| (k.clone(), rows.iter().map(|r| id(r).to_owned()).collect()))
        .collect()
}

/// Snooze ids whose session now needs the person: their snooze ends at
/// once (a raised hand wakes it, as T3 Code's `threadRaisedHandWhileSnoozed`)
/// and does not come back when it stops needing them.
pub fn woken(f: &Fleet, marks: &Marks<'_>) -> Vec<String> {
    marks
        .snoozed
        .keys()
        .filter(|id| {
            f.row(id).is_some_and(|r| {
                fleet::shelf(r, |i| marks.opened.contains_key(i)) == Shelf::NeedsYou
            })
        })
        .cloned()
        .collect()
}

/// Whether the sidebar offers Settle on a row of `shelf` (To review only),
/// Snooze (Working and To review), Un-settle (Settled) or Wake (snoozed).
/// A row that needs the person has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Controls {
    pub settle: bool,
    pub snooze: bool,
    pub unsettle: bool,
    pub wake: bool,
}

pub fn controls(place: Place) -> Controls {
    let none = Controls {
        settle: false,
        snooze: false,
        unsettle: false,
        wake: false,
    };
    match place {
        Place::Snoozed { .. } => Controls { wake: true, ..none },
        Place::Shelf(Shelf::NeedsYou) => none,
        Place::Shelf(Shelf::Working) => Controls {
            snooze: true,
            ..none
        },
        Place::Shelf(Shelf::ToReview) => Controls {
            settle: true,
            snooze: true,
            ..none
        },
        Place::Shelf(Shelf::Settled) => Controls {
            unsettle: true,
            ..none
        },
    }
}

/// One snooze choice: `In 1 hour` waking at `19:47`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preset {
    pub label: String,
    /// The wake time as the menu shows it: `19:47`, or `Mon 09:00` on
    /// another day.
    pub when: String,
    pub until: OffsetDateTime,
}

/// T3 Code's default snooze menu (`DEFAULT_SNOOZE_PRESETS`, MIT): in 1
/// hour, in 3 hours, this evening at 18:00 (while that is ahead), tomorrow
/// at 09:00, next week (Monday) at 09:00; soonest first, one per wake
/// minute. `now` is read in `offset`.
pub fn presets(now: OffsetDateTime, offset: UtcOffset) -> Vec<Preset> {
    let local = now.to_offset(offset);
    let at = |days: i64, h: u8| -> Option<OffsetDateTime> {
        let day = local.date().checked_add(Duration::days(days))?;
        let t = time::Time::from_hms(h, 0, 0).ok()?;
        Some(day.with_time(t).assume_offset(offset))
    };
    let days_to_monday = {
        let d = (7 + Weekday::Monday.number_days_from_monday() as i64
            - local.weekday().number_days_from_monday() as i64)
            % 7;
        if d == 0 { 7 } else { d }
    };
    let mut out: Vec<(String, OffsetDateTime)> = vec![
        ("In 1 hour".into(), now + Duration::hours(1)),
        ("In 3 hours".into(), now + Duration::hours(3)),
    ];
    if let Some(t) = at(0, 18).filter(|t| *t > now) {
        out.push(("This evening".into(), t));
    }
    if let Some(t) = at(1, 9) {
        out.push(("Tomorrow".into(), t));
    }
    if let Some(t) = at(days_to_monday, 9) {
        out.push((
            if days_to_monday == 1 {
                "Tomorrow".into()
            } else {
                "Next week".into()
            },
            t,
        ));
    }
    out.sort_by_key(|(_, t)| t.unix_timestamp() / 60);
    let mut seen = std::collections::HashSet::new();
    out.into_iter()
        .filter(|(_, t)| seen.insert(t.unix_timestamp() / 60))
        .map(|(label, until)| {
            let l = until.to_offset(offset);
            let hm = format!("{:02}:{:02}", l.hour(), l.minute());
            let names_day = label != "Next week" && !label.starts_with("In ");
            let when = if names_day || l.date() == local.date() {
                hm
            } else {
                format!("{} {hm}", &l.weekday().to_string()[..3])
            };
            Preset { label, when, until }
        })
        .collect()
}

/// How long until a snoozed session wakes: `40m`, `3h`, `2d` (T3 Code's
/// `snoozeWakeLabel`).
pub fn wake_label(until: i64, now: i64) -> String {
    let s = until - now;
    if s <= 0 {
        "now".into()
    } else if s < 3600 {
        format!("{}m", ((s + 59) / 60).max(1))
    } else if s < 86_400 {
        format!("{}h", (s + 3599) / 3600)
    } else {
        format!("{}d", (s + 86_399) / 86_400)
    }
}

#[cfg(test)]
mod tests;
