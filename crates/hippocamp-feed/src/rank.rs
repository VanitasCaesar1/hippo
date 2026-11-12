//! The lists of posts.
//!
//! Every list is computed when it is read, at the time given, and leaves out
//! the posts the viewer hid. Points are counted votes and evidence comes from
//! `evidence.rs`; nothing here stores a count, a rank or a score.
//!
//! `front`, `ask` and `show` load the visible posts of the last 30 days and
//! sort them in memory; `newest`, `packs` and the hidden list page in SQL;
//! `past`, `by_evidence` and `themes` sort in memory after loading their
//! (one day's, or the listed packs') posts. Each takes the 1-based `page`.

use std::cmp::Ordering;

use rusqlite::{params_from_iter, types::Value, Connection};
use time::{Date, Duration, OffsetDateTime, Time};

use crate::clock;
use crate::db::UserId;
use crate::evidence::{self, Summary};
use crate::policy;
use crate::posts;
use crate::votes::{self, Target};

/// A post as stored, with what a row needs of its author.
#[derive(Debug, Clone)]
pub struct Rec {
    pub id: i64,
    pub author_id: Option<i64>,
    pub author_login: Option<String>,
    pub pack: Option<String>,
    pub title: String,
    pub link: Option<String>,
    pub created_at: String,
    pub state: String,
}

/// The columns [`Rec::from_row`] reads, with `posts p` and `users u`.
pub const REC_COLUMNS: &str =
    "p.id, p.author_id, CASE WHEN u.deleted_at IS NULL THEN u.login END, \
     p.pack, p.title, p.link, p.created_at, p.state";

impl Rec {
    pub fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Rec> {
        Ok(Rec {
            id: r.get(0)?,
            author_id: r.get(1)?,
            author_login: r.get(2)?,
            pack: r.get(3)?,
            title: r.get(4)?,
            link: r.get(5)?,
            created_at: r.get(6)?,
            state: r.get(7)?,
        })
    }

    fn created(&self) -> OffsetDateTime {
        clock::parse_storage(&self.created_at).unwrap_or(OffsetDateTime::UNIX_EPOCH)
    }
}

/// One post of a list with what ordering needed of it.
#[derive(Debug, Clone)]
pub struct Entry {
    pub rec: Rec,
    /// Counted votes.
    pub points: i64,
    /// The post's pack's evidence and current count (for a pack's post).
    pub summary: Option<Summary>,
}

impl Entry {
    /// The pack's evidence score, 0 for a discussion or a pack without.
    fn score(&self) -> f64 {
        self.summary
            .as_ref()
            .and_then(|s| s.evidence.as_ref())
            .map_or(0.0, |e| e.evidence.score)
    }

    /// The front page's rank value at `now`.
    pub fn rank(&self, now: OffsetDateTime) -> f64 {
        rank_value(self.points, self.score(), self.rec.created(), now)
    }
}

/// One page of a list.
#[derive(Debug, Clone, Default)]
pub struct Listing {
    pub entries: Vec<Entry>,
    /// Whether another page exists.
    pub more: bool,
}

/// `rank = (V + 50 × E) / (h + 2)^1.8`, with `h` the age in hours as a
/// fraction (never negative).
pub fn rank_value(points: i64, evidence: f64, created: OffsetDateTime, now: OffsetDateTime) -> f64 {
    let hours = ((now - created).as_seconds_f64() / 3600.0).max(0.0);
    (points as f64 + policy::RANK_EVIDENCE_WEIGHT * evidence)
        / (hours + policy::RANK_OFFSET_HOURS).powf(policy::RANK_GRAVITY)
}

// --- Loading ---

const FRONT_KINDS: &str = "(p.kind = 'discussion' OR (k.kind = 'passes' AND k.listed = 1))";
const PASSES: &str = "(k.kind = 'passes' AND k.listed = 1)";
const THEMES: &str = "(k.kind = 'theme' AND k.listed = 1)";

fn skip(page: usize) -> usize {
    page.saturating_sub(1) * policy::PAGE_SIZE
}

/// The visible posts that satisfy `filter` (over `p`, `u`, `k`; extra
/// parameters from `?2`) that `viewer` has not hidden, in SQL order, with at
/// most `limit` of them from `offset` on.
fn select(
    conn: &Connection,
    viewer: Option<UserId>,
    filter: &str,
    args: Vec<Value>,
    order: &str,
    window: Option<(usize, usize)>,
) -> rusqlite::Result<Vec<Rec>> {
    let limit = window.map_or(String::new(), |(offset, limit)| {
        format!("LIMIT {limit} OFFSET {offset}")
    });
    let sql = format!(
        "SELECT {REC_COLUMNS} FROM posts p
         LEFT JOIN users u ON u.id = p.author_id
         LEFT JOIN packs k ON k.name = p.pack
         WHERE p.state = 'visible' AND {filter}
           AND NOT EXISTS (SELECT 1 FROM hides h WHERE h.user_id = ?1 AND h.post_id = p.id)
         ORDER BY {order} {limit}"
    );
    let mut all = vec![viewer.map_or(Value::Null, |v| Value::Integer(v.0))];
    all.extend(args);
    let mut stmt = conn.prepare(&sql)?;
    let recs = stmt
        .query_map(params_from_iter(all), Rec::from_row)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(recs)
}

/// Counted votes and pack evidence for each record, in order.
fn entries(conn: &Connection, recs: Vec<Rec>, now: OffsetDateTime) -> rusqlite::Result<Vec<Entry>> {
    let targets: Vec<Target> = recs.iter().map(|r| Target::Post(r.id)).collect();
    let counts = votes::counted_votes(conn, &targets, now)?;
    recs.into_iter()
        .map(|rec| {
            let summary = match &rec.pack {
                Some(pack) => Some(evidence::summary(conn, pack, now)?),
                None => None,
            };
            Ok(Entry {
                points: counts[&Target::Post(rec.id)],
                rec,
                summary,
            })
        })
        .collect()
}

/// Cuts a sorted list to one page.
fn paged(mut all: Vec<Entry>, page: usize) -> Listing {
    let from = skip(page).min(all.len());
    let mut rest = all.split_off(from);
    let more = rest.len() > policy::PAGE_SIZE;
    rest.truncate(policy::PAGE_SIZE);
    Listing {
        entries: rest,
        more,
    }
}

/// A page of a list ordered in SQL: one more row than a page is read to know
/// whether another page exists.
fn sql_page(
    conn: &Connection,
    viewer: Option<UserId>,
    now: OffsetDateTime,
    filter: &str,
    order: &str,
    page: usize,
) -> rusqlite::Result<Listing> {
    let mut recs = select(
        conn,
        viewer,
        filter,
        Vec::new(),
        order,
        Some((skip(page), policy::PAGE_SIZE + 1)),
    )?;
    let more = recs.len() > policy::PAGE_SIZE;
    recs.truncate(policy::PAGE_SIZE);
    Ok(Listing {
        entries: entries(conn, recs, now)?,
        more,
    })
}

// --- Orders ---

/// Newer post first, then higher id.
fn newer_first(a: &Entry, b: &Entry) -> Ordering {
    b.rec
        .created_at
        .cmp(&a.rec.created_at)
        .then_with(|| b.rec.id.cmp(&a.rec.id))
}

/// Highest rank first; ties go to the newer post, then the higher id.
fn by_rank(now: OffsetDateTime) -> impl Fn(&Entry, &Entry) -> Ordering {
    move |a, b| {
        b.rank(now)
            .total_cmp(&a.rank(now))
            .then_with(|| newer_first(a, b))
    }
}

fn front_like(
    conn: &Connection,
    viewer: Option<UserId>,
    now: OffsetDateTime,
    page: usize,
    keep: impl Fn(&Rec) -> bool,
) -> rusqlite::Result<Listing> {
    let cutoff = clock::storage(now - Duration::days(policy::FRONT_WINDOW_DAYS));
    let recs = select(
        conn,
        viewer,
        &format!("{FRONT_KINDS} AND p.created_at >= ?2"),
        vec![Value::Text(cutoff)],
        "p.created_at DESC, p.id DESC",
        None,
    )?;
    let recs = recs.into_iter().filter(|r| keep(r)).collect();
    let mut all = entries(conn, recs, now)?;
    all.sort_by(by_rank(now));
    Ok(paged(all, page))
}

/// The front page: visible posts of the last 30 days, discussions and posts of
/// listed packs of passes, by rank.
pub fn front(
    conn: &Connection,
    viewer: Option<UserId>,
    now: OffsetDateTime,
    page: usize,
) -> rusqlite::Result<Listing> {
    front_like(conn, viewer, now, page, |_| true)
}

/// The front order over ask posts only.
pub fn ask(
    conn: &Connection,
    viewer: Option<UserId>,
    now: OffsetDateTime,
    page: usize,
) -> rusqlite::Result<Listing> {
    front_like(conn, viewer, now, page, |r| {
        r.pack.is_none() && posts::is_ask(&r.title)
    })
}

/// The front order over show posts only.
pub fn show(
    conn: &Connection,
    viewer: Option<UserId>,
    now: OffsetDateTime,
    page: usize,
) -> rusqlite::Result<Listing> {
    front_like(conn, viewer, now, page, |r| {
        r.pack.is_none() && posts::is_show(&r.title)
    })
}

/// The same kinds of post, newest first (then higher id), with no age limit.
pub fn newest(
    conn: &Connection,
    viewer: Option<UserId>,
    now: OffsetDateTime,
    page: usize,
) -> rusqlite::Result<Listing> {
    sql_page(
        conn,
        viewer,
        now,
        FRONT_KINDS,
        "p.created_at DESC, p.id DESC",
        page,
    )
}

/// Visible posts of listed packs of passes, newest first by creation (the
/// pack's first listing), then name.
pub fn packs(
    conn: &Connection,
    viewer: Option<UserId>,
    now: OffsetDateTime,
    page: usize,
) -> rusqlite::Result<Listing> {
    sql_page(conn, viewer, now, PASSES, "p.created_at DESC, k.name", page)
}

/// The UTC day before `now`'s.
pub fn yesterday(now: OffsetDateTime) -> Date {
    now.to_offset(time::UtcOffset::UTC).date() - Duration::days(1)
}

/// Posts of the front page's kinds created on `day` (UTC), by points (most
/// first), then newer, then higher id. A day after today lists nothing.
pub fn past(
    conn: &Connection,
    viewer: Option<UserId>,
    now: OffsetDateTime,
    day: Date,
    page: usize,
) -> rusqlite::Result<Listing> {
    if day > now.to_offset(time::UtcOffset::UTC).date() {
        return Ok(Listing::default());
    }
    let start = day.with_time(Time::MIDNIGHT).assume_utc();
    let recs = select(
        conn,
        viewer,
        &format!("{FRONT_KINDS} AND p.created_at >= ?2 AND p.created_at < ?3"),
        vec![
            Value::Text(clock::storage(start)),
            Value::Text(clock::storage(start + Duration::days(1))),
        ],
        "p.created_at DESC, p.id DESC",
        None,
    )?;
    let mut all = entries(conn, recs, now)?;
    all.sort_by(|a, b| b.points.cmp(&a.points).then_with(|| newer_first(a, b)));
    Ok(paged(all, page))
}

/// The three groups of `/evidence`: 0 above zero, 1 not measured or exactly
/// zero, 2 below zero. Signs come from the exact median.
fn evidence_group(entry: &Entry) -> u8 {
    let sign = entry
        .summary
        .as_ref()
        .and_then(|s| s.evidence.as_ref())
        .map_or(0, |e| e.evidence.median.signum());
    match sign {
        1 => 0,
        -1 => 2,
        _ => 1,
    }
}

fn pack_name(entry: &Entry) -> &str {
    entry.rec.pack.as_deref().unwrap_or("")
}

fn counted(entry: &Entry) -> u32 {
    entry
        .summary
        .as_ref()
        .and_then(|s| s.evidence.as_ref())
        .map_or(0, |e| e.evidence.n)
}

/// Listed packs of passes whose post is visible, in three groups: evidence
/// above 0 by score (highest first), then counted measurements (most first),
/// then name; then packs not measured or with evidence of exactly 0, by
/// points (most first), then name; then evidence below 0, by score (highest
/// first), then name.
pub fn by_evidence(
    conn: &Connection,
    viewer: Option<UserId>,
    now: OffsetDateTime,
    page: usize,
) -> rusqlite::Result<Listing> {
    let recs = select(conn, viewer, PASSES, Vec::new(), "k.name", None)?;
    let mut all = entries(conn, recs, now)?;
    all.sort_by(|a, b| {
        evidence_group(a)
            .cmp(&evidence_group(b))
            .then_with(|| match evidence_group(a) {
                0 => b
                    .score()
                    .total_cmp(&a.score())
                    .then_with(|| counted(b).cmp(&counted(a)))
                    .then_with(|| pack_name(a).cmp(pack_name(b))),
                1 => b
                    .points
                    .cmp(&a.points)
                    .then_with(|| pack_name(a).cmp(pack_name(b))),
                _ => b
                    .score()
                    .total_cmp(&a.score())
                    .then_with(|| pack_name(a).cmp(pack_name(b))),
            })
    });
    Ok(paged(all, page))
}

/// Visible posts of listed theme packs, by points (most first), then name.
pub fn themes(
    conn: &Connection,
    viewer: Option<UserId>,
    now: OffsetDateTime,
    page: usize,
) -> rusqlite::Result<Listing> {
    let recs = select(conn, viewer, THEMES, Vec::new(), "k.name", None)?;
    let mut all = entries(conn, recs, now)?;
    all.sort_by(|a, b| {
        b.points
            .cmp(&a.points)
            .then_with(|| pack_name(a).cmp(pack_name(b)))
    });
    Ok(paged(all, page))
}

/// The visible posts `user` has hidden, most recently hidden first.
pub fn hidden_by(
    conn: &Connection,
    user: UserId,
    now: OffsetDateTime,
    page: usize,
) -> rusqlite::Result<Listing> {
    let sql = format!(
        "SELECT {REC_COLUMNS} FROM hides h
         JOIN posts p ON p.id = h.post_id
         LEFT JOIN users u ON u.id = p.author_id
         WHERE h.user_id = ?1 AND p.state = 'visible'
         ORDER BY h.at DESC, p.id DESC LIMIT {} OFFSET {}",
        policy::PAGE_SIZE + 1,
        skip(page)
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut recs: Vec<Rec> = stmt
        .query_map([user.0], Rec::from_row)?
        .collect::<rusqlite::Result<_>>()?;
    let more = recs.len() > policy::PAGE_SIZE;
    recs.truncate(policy::PAGE_SIZE);
    Ok(Listing {
        entries: entries(conn, recs, now)?,
        more,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::posts::testkit::*;
    use time::macros::datetime;

    const NOW: OffsetDateTime = datetime!(2026-11-08 10:00:00 UTC);

    fn old() -> OffsetDateTime {
        NOW - Duration::days(400)
    }

    fn ids(listing: &Listing) -> Vec<i64> {
        listing.entries.iter().map(|e| e.rec.id).collect()
    }

    fn names(listing: &Listing) -> Vec<String> {
        listing
            .entries
            .iter()
            .map(|e| e.rec.pack.clone().unwrap_or_else(|| e.rec.title.clone()))
            .collect()
    }

    #[test]
    fn the_formula() {
        // Created now: (V + 50E) / 2^1.8.
        let v = rank_value(10, 0.0, NOW, NOW);
        assert!((v - 10.0 / 2f64.powf(1.8)).abs() < 1e-12);
        // 14 hours old: / 16^1.8.
        let v = rank_value(5, 0.0, NOW - Duration::hours(14), NOW);
        assert!((v - 5.0 / 16f64.powf(1.8)).abs() < 1e-12);
        // Evidence 0.143 is worth about 7 upvotes.
        let with = rank_value(0, 0.143, NOW, NOW);
        let votes = rank_value(7, 0.0, NOW, NOW);
        assert!((with - votes).abs() / votes < 0.03);
        // Negative evidence lowers the rank below an unvoted post's.
        assert!(rank_value(1, -0.1, NOW, NOW) < 0.0);
        // A post from the future ages as 0 hours.
        assert_eq!(
            rank_value(3, 0.0, NOW + Duration::hours(3), NOW),
            rank_value(3, 0.0, NOW, NOW)
        );
    }

    struct World {
        _dir: tempfile::TempDir,
        conn: Connection,
        voters: Vec<i64>,
    }

    /// Eight established voters and an old author.
    fn world() -> (World, i64) {
        let (dir, conn) = fresh();
        let author = user(&conn, "author", old(), old());
        let voters = (0..8)
            .map(|i| user(&conn, &format!("v{i}"), old(), old()))
            .collect();
        (
            World {
                _dir: dir,
                conn,
                voters,
            },
            author,
        )
    }

    fn vote_n(w: &World, post: i64, n: usize) {
        for v in w.voters.iter().take(n) {
            cast(&w.conn, *v, "post", post, NOW - Duration::hours(1));
        }
    }

    #[test]
    fn front_orders_by_rank_with_the_ties_going_to_the_newer_then_higher_id() {
        let (w, a) = world();
        let c = &w.conn;
        // Same age, different votes.
        let p1 = post(c, a, "one vote", NOW - Duration::hours(5));
        let p3 = post(c, a, "three votes", NOW - Duration::hours(5));
        // Same rank by construction: equal age and points, ids decide.
        let t1 = post(c, a, "tie first", NOW - Duration::hours(5));
        let t2 = post(c, a, "tie second", NOW - Duration::hours(5));
        // Newer with fewer votes can outrank an older one with more.
        let fresh_post = post(c, a, "fresh", NOW - Duration::minutes(10));
        vote_n(&w, p1, 1);
        vote_n(&w, p3, 3);
        vote_n(&w, t1, 1);
        vote_n(&w, t2, 1);
        vote_n(&w, fresh_post, 1);
        let listing = front(c, None, NOW, 1).expect("front");
        assert_eq!(ids(&listing), vec![fresh_post, p3, t2, t1, p1]);
        assert!(!listing.more);
        // The 30-day window.
        let old_post = post(c, a, "old", NOW - Duration::days(31));
        vote_n(&w, old_post, 8);
        assert!(!ids(&front(c, None, NOW, 1).unwrap()).contains(&old_post));
        // /new has no window and goes newest first, higher id first among equal times.
        let new = newest(c, None, NOW, 1).expect("new");
        assert_eq!(ids(&new), vec![fresh_post, t2, t1, p3, p1, old_post]);
    }

    #[test]
    fn a_hidden_post_is_left_out_for_the_viewer_who_hid_it_only() {
        let (w, a) = world();
        let p = post(&w.conn, a, "hidden by one", NOW - Duration::hours(1));
        let q = post(&w.conn, a, "kept", NOW - Duration::hours(2));
        w.conn
            .execute(
                "INSERT INTO hides (user_id, post_id, at) VALUES (?1, ?2, ?3)",
                rusqlite::params![w.voters[0], p, clock::storage(NOW)],
            )
            .unwrap();
        let viewer = Some(UserId(w.voters[0]));
        for f in [front, newest] {
            assert_eq!(ids(&f(&w.conn, viewer, NOW, 1).unwrap()), vec![q]);
            assert_eq!(ids(&f(&w.conn, None, NOW, 1).unwrap()), vec![p, q]);
            assert_eq!(
                ids(&f(&w.conn, Some(UserId(w.voters[1])), NOW, 1).unwrap()),
                vec![p, q]
            );
        }
        let h = hidden_by(&w.conn, UserId(w.voters[0]), NOW, 1).unwrap();
        assert_eq!(ids(&h), vec![p]);
    }

    #[test]
    fn ask_and_show_follow_the_current_title() {
        let (w, a) = world();
        let c = &w.conn;
        let ask_post = post(c, a, "Ask hippocamp: how?", NOW - Duration::hours(1));
        let show_post = post(c, a, "show HIPPOCAMP: this", NOW - Duration::hours(2));
        let plain = post(c, a, "plain", NOW - Duration::hours(3));
        assert_eq!(ids(&ask(c, None, NOW, 1).unwrap()), vec![ask_post]);
        assert_eq!(ids(&show(c, None, NOW, 1).unwrap()), vec![show_post]);
        c.execute(
            "UPDATE posts SET title = 'Ask hippocamp: now' WHERE id = ?1",
            [plain],
        )
        .unwrap();
        assert_eq!(ids(&ask(c, None, NOW, 1).unwrap()), vec![ask_post, plain]);
        // A pack's post is never an ask post, whatever its title.
        let pack = pack_post(c, "ask-pack", a, "[]", NOW - Duration::hours(1));
        c.execute(
            "UPDATE posts SET title = 'Ask hippocamp: pack' WHERE id = ?1",
            [pack],
        )
        .unwrap();
        assert!(!ids(&ask(c, None, NOW, 1).unwrap()).contains(&pack));
    }

    #[test]
    fn packs_are_newest_first_then_name_and_leave_out_delisted_and_themes() {
        let (w, a) = world();
        let c = &w.conn;
        let t = NOW - Duration::hours(3);
        let b = pack_post(c, "bravo", a, "[]", t);
        let al = pack_post(c, "alpha", a, "[]", t);
        let late = pack_post(c, "late", a, "[]", NOW - Duration::hours(1));
        let gone = pack_post(c, "gone", a, "[]", NOW - Duration::hours(2));
        let theme = pack_post(c, "theme-x", a, "[]", NOW - Duration::hours(2));
        c.execute("UPDATE packs SET listed = 0 WHERE name = 'gone'", [])
            .unwrap();
        c.execute("UPDATE packs SET kind = 'theme' WHERE name = 'theme-x'", [])
            .unwrap();
        let listing = packs(c, None, NOW, 1).unwrap();
        assert_eq!(ids(&listing), vec![late, al, b]);
        let front_ids = ids(&front(c, None, NOW, 1).unwrap());
        assert!(!front_ids.contains(&gone) && !front_ids.contains(&theme));
        assert_eq!(ids(&themes(c, None, NOW, 1).unwrap()), vec![theme]);
    }

    #[test]
    fn evidence_lists_three_groups() {
        let (w, a) = world();
        let c = &w.conn;
        let t = NOW - Duration::days(2);
        let mut packs_made = Vec::new();
        for name in [
            "up-hi", "up-more", "up-small", "none-a", "none-b", "zero", "down-lo", "down-hi",
        ] {
            let id = pack_post(c, name, a, "[]", t);
            content(c, name, "h", "1", t);
            packs_made.push((name, id));
        }
        let id = |name: &str| packs_made.iter().find(|p| p.0 == name).unwrap().1;
        let measure_all = |name: &str, dc: i64, dd: i64, n: usize| {
            for v in w.voters.iter().take(n) {
                measure(c, name, "h", *v, dc, dd);
            }
        };
        measure_all("up-hi", 12, 0, 3); // 120/180
        measure_all("up-more", 12, 0, 5); // same median, more measurements
        measure_all("up-small", 6, 0, 3); // 60/180
        measure_all("down-hi", -1, 0, 3); // -10/180
        measure_all("down-lo", -6, 0, 3); // -60/180
        measure_all("zero", 6, 10, 3); // 60 - 60 = 0 exactly
        measure_all("none-a", 12, 0, 2); // not measured
                                         // Votes order the middle group: none-b has one.
        vote_n(&w, id("none-b"), 1);
        let listing = by_evidence(c, None, NOW, 1).unwrap();
        // up-more: 0.667 * 5/7 = 0.476, up-hi: 0.667 * 3/5 = 0.4, up-small: 0.2.
        // Middle group by points then name: none-b (1), then none-a, zero.
        // Below zero by score, highest first: down-hi, down-lo.
        assert_eq!(
            names(&listing),
            vec!["up-more", "up-hi", "up-small", "none-b", "none-a", "zero", "down-hi", "down-lo"]
        );
    }

    #[test]
    fn counted_measurements_leave_out_maintainers_new_members_and_invalid_ones() {
        let (w, a) = world();
        let c = &w.conn;
        let t = NOW - Duration::days(2);
        let newbie = user(c, "newbie", NOW - Duration::days(1), old());
        let maint = user(c, "maint", old(), old());
        pack_post(c, "p", a, r#"["maint"]"#, t);
        content(c, "p", "h", "1", t);
        for v in w.voters.iter().take(2) {
            measure(c, "p", "h", *v, 12, 0);
        }
        // Not counted: a maintainer's and a new member's.
        measure(c, "p", "h", maint, 12, 0);
        measure(c, "p", "h", newbie, 12, 0);
        let s = evidence::summary(c, "p", NOW).unwrap();
        assert!(s.evidence.is_none());
        assert_eq!(s.current_counted, 2);
        // A third counted one gives a score; an invalid one does not count.
        let m3 = measure(c, "p", "h", w.voters[2], 12, 0);
        assert!(evidence::pack_evidence(c, "p", NOW).unwrap().is_some());
        c.execute("UPDATE measurements SET invalid = 1 WHERE id = ?1", [m3])
            .unwrap();
        assert!(evidence::pack_evidence(c, "p", NOW).unwrap().is_none());
        // Once the new member is established, they count.
        c.execute("UPDATE measurements SET invalid = 0 WHERE id = ?1", [m3])
            .unwrap();
        let later = NOW + Duration::days(3);
        let s = evidence::summary(c, "p", later).unwrap();
        assert_eq!(s.evidence.unwrap().evidence.n, 4);
    }

    #[test]
    fn past_lists_one_utc_day_by_points() {
        let (w, a) = world();
        let c = &w.conn;
        let day = datetime!(2026-11-05 00:00:00 UTC);
        let early = post(c, a, "early", day + Duration::hours(1));
        let late = post(c, a, "late", day + Duration::hours(23));
        let voted = post(c, a, "voted", day + Duration::hours(5));
        let before = post(c, a, "before", day - Duration::seconds(1));
        let after = post(c, a, "after", day + Duration::days(1));
        vote_n(&w, voted, 2);
        let d = day.date();
        let listing = past(c, None, NOW, d, 1).unwrap();
        assert_eq!(ids(&listing), vec![voted, late, early]);
        assert!(!ids(&listing).contains(&before) && !ids(&listing).contains(&after));
        // Future days list nothing, and yesterday is the default day.
        let future = NOW.date() + Duration::days(1);
        assert!(past(c, None, NOW, future, 1).unwrap().entries.is_empty());
        assert_eq!(yesterday(NOW), datetime!(2026-11-07 00:00:00 UTC).date());
    }

    #[test]
    fn page_two_follows_page_one() {
        let (w, a) = world();
        let c = &w.conn;
        for i in 0..31 {
            post(c, a, &format!("p{i}"), NOW - Duration::minutes(i + 1));
        }
        let one = newest(c, None, NOW, 1).unwrap();
        assert_eq!(one.entries.len(), 30);
        assert!(one.more);
        let two = newest(c, None, NOW, 2).unwrap();
        assert_eq!(two.entries.len(), 1);
        assert!(!two.more);
        let one = front(c, None, NOW, 1).unwrap();
        assert!(one.more);
        let two = front(c, None, NOW, 2).unwrap();
        assert_eq!(two.entries.len(), 1);
        assert!(front(c, None, NOW, 3).unwrap().entries.is_empty());
    }

    #[test]
    fn negative_evidence_ranks_a_pack_below_an_unvoted_post_and_positive_above() {
        let (w, a) = world();
        let c = &w.conn;
        let t = NOW - Duration::hours(1);
        let pack = pack_post(c, "harm", a, "[]", t);
        content(c, "harm", "h", "1", t);
        for v in w.voters.iter().take(3) {
            measure(c, "harm", "h", *v, -6, 0);
        }
        let plain = post(c, a, "plain", t);
        let helpful = pack_post(c, "help", a, "[]", t);
        content(c, "help", "h", "1", t);
        for v in w.voters.iter().take(3) {
            measure(c, "help", "h", *v, 12, 0);
        }
        let listing = front(c, None, NOW, 1).unwrap();
        assert_eq!(ids(&listing), vec![helpful, plain, pack]);
    }
}
