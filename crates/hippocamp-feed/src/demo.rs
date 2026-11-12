//! `hippocamp-feed seed-demo`: synthetic data for a local walkthrough.
//!
//! Dev mode only (the command checks `FEED_DEV`). The layout is fixed, with
//! no random numbers, so two runs on equal databases at equal times make
//! equal rows. The data goes in through the same functions the pages use:
//! `posts::create_discussion`, `comments::create`, `votes::vote` and
//! `measurements::enter`, each at the time the row pretends to have happened.
//!
//! Choices made so the demo reads naturally: ask and show posts are found by
//! a title that *starts* with `Ask hippocamp:` or `Show hippocamp:`, so those four
//! titles read `Ask hippocamp: [demo] …` and `Show hippocamp: [demo] …`; the other
//! eight start with `[demo] `. Every title holds `[demo] `. The two show posts
//! carry a link URL on `example.com` as well as a body, so six posts have a
//! link in all and the two ask posts and four others are body-only.

use std::path::Path;

use anyhow::{bail, Context};
use rusqlite::{params, Transaction};
use time::{Duration, OffsetDateTime};

use crate::clock;
use crate::comments;
use crate::config::EvalSet;
use crate::db::{self, UserId};
use crate::measurements::{self, MeasurementForm};
use crate::posts::{self, Discussion};
use crate::users;
use crate::votes::{self, Target};

/// The marker every demo title holds.
pub const MARKER: &str = "[demo] ";
/// The demo eval set's id; the walkthrough sets `FEED_EVAL_SETS=000000000000:7:8`.
pub const EVAL_SET: &str = "000000000000";
pub const PLANTED: u32 = 7;
pub const KEPT: u32 = 8;
pub const MEMBERS: usize = 8;
pub const COMMENTS: usize = 40;
pub const VOTES: usize = 150;

/// The packs that get measurements, and the demo members who measure each
/// (indexes into the members, 0 for `demo-1`) with the four counts
/// `(corrected_without, damaged_without, corrected_with, damaged_with)` over
/// 3 runs. None is a maintainer of the fixture's packs; no benefit reaches
/// 0.5, so none is flagged.
/// One demo member's measurement: the member index and the four counts.
type Entry = (usize, [i64; 4]);

const MEASURED: &[(&str, &[Entry])] = &[
    (
        "glossary-terms",
        &[(0, [8, 2, 12, 1]), (1, [9, 1, 13, 2]), (2, [7, 2, 11, 2])],
    ),
    (
        "decision-log",
        &[(3, [8, 3, 11, 3]), (4, [10, 2, 12, 2]), (5, [9, 2, 13, 3])],
    ),
    ("review-dates", &[(6, [9, 1, 12, 1])]),
];

/// `(title after any prefix, link, body)`; the first two are asks, the next
/// two shows, then four with links and four with a body only.
const DISCUSSIONS: [(&str, &str, &str, &str); 12] = [
    (
        "Ask hippocamp: ",
        "how do you review a pack before adding it?",
        "",
        "I read the passes and the reads list first. What else do you check?",
    ),
    (
        "Ask hippocamp: ",
        "does dreaming get better with a bigger eval set?",
        "",
        "I measured with 7 planted errors and wonder how noisy that is.",
    ),
    (
        "Show hippocamp: ",
        "my notes folder after a week of dreaming",
        "https://example.com/demo/notes-after-a-week",
        "Before and after, with the claims that changed.",
    ),
    (
        "Show hippocamp: ",
        "a pack that keeps a decision log",
        "https://example.com/demo/decision-log-notes",
        "How I use it, and what it left out.",
    ),
    (
        "",
        "Why measuring beats a star count",
        "https://example.com/demo/measuring",
        "",
    ),
    (
        "",
        "A short history of plain-text notes",
        "https://example.com/demo/plain-text",
        "",
    ),
    (
        "",
        "What a sixth of a planted error costs you",
        "https://example.com/demo/sixths",
        "",
    ),
    (
        "",
        "Reading an eval report line by line",
        "https://example.com/demo/reading-reports",
        "",
    ),
    (
        "",
        "Weekly thread: what did you measure?",
        "",
        "Post the pack, the model and the numbers.",
    ),
    (
        "",
        "Notes on keeping claims short",
        "",
        "Short claims are easier to check and easier to keep.",
    ),
    (
        "",
        "Is three runs enough?",
        "",
        "Three runs is the floor. I use five when I can afford them.",
    ),
    (
        "",
        "Pack ideas wanted",
        "",
        "Which passes would you like to see listed next?",
    ),
];

const COMMENT_BODIES: [&str; 10] = [
    "Same here; the reads list is where I start.",
    "I would check the model name in the report too.",
    "That matches what I saw on a smaller set.",
    "Worth saying which hippocamp version this was.",
    "Thanks, this is the clearest write-up so far.",
    "Do you have the full report to link?",
    "I got a lower number with the same pack.",
    "Three runs were enough for me to see the direction.",
    "Good question; I do not know yet.",
    "Agreed, and the median hides less than the mean would.",
];

/// What a seeding added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seeded {
    pub members: usize,
    pub discussions: usize,
    pub comments: usize,
    pub votes: usize,
    pub measurements: usize,
}

/// The demo eval set, as `FEED_EVAL_SETS=000000000000:7:8` configures it.
pub fn eval_sets() -> Vec<EvalSet> {
    vec![EvalSet {
        id: EVAL_SET.to_string(),
        planted: PLANTED,
        kept: KEPT,
    }]
}

/// Seeds the database at `path` as of `now`, in one transaction.
pub fn seed(path: &Path, now: OffsetDateTime) -> anyhow::Result<Seeded> {
    let mut conn =
        db::open(path).with_context(|| format!("cannot open the database {}", path.display()))?;
    db::check_version(&conn)?;
    let tx = conn.transaction()?;
    let seeded = seed_tx(&tx, now)?;
    tx.commit()?;
    Ok(seeded)
}

/// [`seed`] inside a transaction of the caller.
pub fn seed_tx(tx: &Transaction, now: OffsetDateTime) -> anyhow::Result<Seeded> {
    let listed: i64 = tx.query_row(
        "SELECT COUNT(*) FROM packs WHERE listed = 1 AND kind = 'passes'",
        [],
        |r| r.get(0),
    )?;
    if listed == 0 {
        bail!("no packs are listed: run sync-registry first");
    }
    let existing: i64 = tx.query_row(
        "SELECT COUNT(*) FROM users WHERE login LIKE 'demo-%' OR github_id < 0",
        [],
        |r| r.get(0),
    )?;
    if existing > 0 {
        bail!("demo members already exist: seed-demo runs once on a database");
    }
    // The demo measures those of its packs that are listed: all three on the
    // fixture's index, only `glossary-terms` and `decision-log` on the
    // repository's own registry, which has no `review-dates`.
    let mut measured: Vec<(&str, &[Entry])> = Vec::new();
    for &(pack, rows) in MEASURED {
        let found: i64 = tx.query_row(
            "SELECT COUNT(*) FROM packs WHERE name = ?1 AND listed = 1 AND kind = 'passes'",
            [pack],
            |r| r.get(0),
        )?;
        if found > 0 {
            measured.push((pack, rows));
        }
    }
    if measured.is_empty() {
        bail!("none of the packs the demo measures is listed: run sync-registry first");
    }

    // Members: created 10 days ago, GitHub accounts two years old.
    let joined = clock::storage(now - Duration::days(10));
    let github = clock::storage(now - Duration::days(730));
    let mut members = Vec::new();
    for n in 1..=MEMBERS {
        tx.execute(
            "INSERT INTO users (github_id, login, github_created_at, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![-(n as i64), format!("demo-{n}"), github, joined],
        )?;
        let id = UserId(tx.last_insert_rowid());
        members.push(users::load_member(tx, id)?.context("the member just inserted")?);
    }

    // Discussions, oldest last: post `i` is `30 + 6 i` hours old.
    let mut posts = Vec::new();
    for (i, (prefix, title, link, body)) in DISCUSSIONS.iter().enumerate() {
        let at = now - Duration::hours(30 + 6 * i as i64);
        let discussion = Discussion::parse(&format!("{prefix}{MARKER}{title}"), link, body)?;
        let author = &members[i % MEMBERS];
        let id = posts::create_discussion(tx, author, &discussion, at)?;
        posts.push((id, at));
    }

    // Comments: 16 top-level, 14 replies to them, 10 replies to those.
    let mut level: [Vec<(i64, i64)>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut comment_times = Vec::new();
    for k in 0..COMMENTS {
        let (depth, parent) = match k {
            0..=15 => (0, None),
            16..=29 => (1, Some(level[0][k - 16])),
            _ => (2, Some(level[1][k - 30])),
        };
        let post_index = match parent {
            Some((_, post_index)) => post_index as usize,
            None => k % posts.len(),
        };
        let (post, post_at) = posts[post_index];
        let author = &members[(k * 3 + 1) % MEMBERS];
        let at = post_at + Duration::minutes(5 * (k as i64 + 1));
        let body = format!(
            "{} (comment {})",
            COMMENT_BODIES[k % COMMENT_BODIES.len()],
            k + 1
        );
        let id = comments::create(tx, author, post, parent.map(|(id, _)| id), &body, at)?;
        level[depth].push((id, post_index as i64));
        comment_times.push((id, at));
    }

    // Votes: walk the member-and-item pairs in a fixed order that is not
    // sorted, skip a member's own items, and stop at 150. Each vote comes
    // 7 minutes after the one before it, and after its item.
    let mut items: Vec<(Target, UserId, OffsetDateTime)> = Vec::new();
    for (i, (id, at)) in posts.iter().enumerate() {
        items.push((Target::Post(*id), members[i % MEMBERS].id, *at));
    }
    for (k, (id, at)) in comment_times.iter().enumerate() {
        items.push((Target::Comment(*id), members[(k * 3 + 1) % MEMBERS].id, *at));
    }
    let pairs = MEMBERS * items.len();
    let mut cast = 0usize;
    let mut step = 0usize;
    while cast < VOTES {
        let pair = (step * 37) % pairs;
        step += 1;
        if step > pairs {
            bail!("the demo has too few items for {VOTES} votes");
        }
        let (voter, (target, author, item_at)) = (&members[pair % MEMBERS], items[pair / MEMBERS]);
        if voter.id == author {
            continue;
        }
        let at = item_at + Duration::minutes(7 * (cast as i64 + 1));
        votes::vote(tx, voter, target, at)?;
        cast += 1;
    }

    // Measurements, two hours apart, through the page's own function.
    let sets = eval_sets();
    let mut entered = 0usize;
    for (pack, rows) in measured {
        for (who, [cw, dw, cwith, dwith]) in rows {
            let at = now - Duration::hours(40 - 2 * entered as i64);
            let hash = measurements::pack_facts(tx, pack)?
                .and_then(|f| f.hashes.first().cloned())
                .with_context(|| format!("{pack} has no recorded content"))?;
            let form = MeasurementForm {
                pack_hash: hash,
                eval_set: EVAL_SET.to_string(),
                hippocamp: "0.9.0".to_string(),
                model: "demo-model".to_string(),
                runs: "3".to_string(),
                corrected_without: cw.to_string(),
                damaged_without: dw.to_string(),
                corrected_with: cwith.to_string(),
                damaged_with: dwith.to_string(),
                report_url: format!("https://example.com/demo/report-{}", entered + 1),
            };
            measurements::enter(tx, &members[*who], pack, &form, &sets, at)?;
            entered += 1;
        }
    }

    Ok(Seeded {
        members: MEMBERS,
        discussions: posts.len(),
        comments: comment_times.len(),
        votes: cast,
        measurements: entered,
    })
}
