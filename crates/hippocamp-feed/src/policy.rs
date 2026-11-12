//! Every fixed number of the feed, in one place.
//!
//! Later code and the `/about` page read these constants, so the code holds
//! one copy of each.

use std::time::Duration;

// --- Accounts and standing ---

/// A feed account becomes established after this many days.
pub const ESTABLISHED_FEED_AGE_DAYS: i64 = 3;
/// The GitHub account must be at least this many days old to be established.
pub const ESTABLISHED_GITHUB_AGE_DAYS: i64 = 90;
/// An author may edit an item this long after creating it.
pub const EDIT_WINDOW_HOURS: i64 = 2;
/// A sign-in session lasts this many days.
pub const SESSION_DAYS: i64 = 30;
/// A sign-in `state` is valid this many minutes.
pub const OAUTH_STATE_MINUTES: i64 = 10;
/// An indefinite restriction is stored as this time.
pub const INDEFINITE: &str = "9999-12-31T23:59:59.999999Z";

// --- Per-member limits: `count` actions in a sliding `window` ---

/// A count of actions allowed in a sliding window of seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limit {
    pub count: u32,
    pub window_secs: i64,
}

const HOUR: i64 = 3_600;
const DAY: i64 = 86_400;

pub const POSTS_ESTABLISHED: Limit = Limit {
    count: 5,
    window_secs: DAY,
};
pub const POSTS_NEW: Limit = Limit {
    count: 1,
    window_secs: DAY,
};
pub const COMMENTS_ESTABLISHED_HOUR: Limit = Limit {
    count: 60,
    window_secs: HOUR,
};
pub const COMMENTS_ESTABLISHED_DAY: Limit = Limit {
    count: 300,
    window_secs: DAY,
};
pub const COMMENTS_NEW: Limit = Limit {
    count: 5,
    window_secs: DAY,
};
pub const VOTES: Limit = Limit {
    count: 300,
    window_secs: HOUR,
};
pub const REPORTS_ESTABLISHED: Limit = Limit {
    count: 20,
    window_secs: DAY,
};
pub const REPORTS_NEW: Limit = Limit {
    count: 5,
    window_secs: DAY,
};
pub const MEASUREMENTS_ESTABLISHED: Limit = Limit {
    count: 20,
    window_secs: DAY,
};

// --- Per-address limits (held in memory) ---

/// A per-address quota: `count` requests per `per`, all allowed as a burst.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddressQuota {
    pub count: u32,
    pub per: Duration,
}

pub const SIGN_IN_QUOTA: AddressQuota = AddressQuota {
    count: 20,
    per: Duration::from_secs(600),
};
pub const API_QUOTA: AddressQuota = AddressQuota {
    count: 120,
    per: Duration::from_secs(60),
};
pub const ALL_QUOTA: AddressQuota = AddressQuota {
    count: 300,
    per: Duration::from_secs(60),
};

/// Database work runs at most this many requests at a time.
pub const DB_PERMITS: usize = 8;
/// A request waits this long for a database turn before it gets 503.
pub const DB_WAIT: Duration = Duration::from_secs(5);

// --- Ranking ---

/// Hacker News's gravity, from its published ranking function.
pub const RANK_GRAVITY: f64 = 1.8;
/// Hours added to a post's age before the gravity is applied.
pub const RANK_OFFSET_HOURS: f64 = 2.0;
/// How many upvotes one point of evidence is worth.
pub const RANK_EVIDENCE_WEIGHT: f64 = 50.0;
/// The front page lists posts created in this many days.
pub const FRONT_WINDOW_DAYS: i64 = 30;

// --- Evidence ---

/// A content needs this many counted measurements to have a score.
pub const EVIDENCE_MIN_MEASUREMENTS: u32 = 3;
/// The score is `median * n / (n + EVIDENCE_SHRINK)`.
pub const EVIDENCE_SHRINK: u32 = 2;
/// A benefit this far from zero, in either direction, files a system report.
pub const EVIDENCE_FLAG_BENEFIT: f64 = 0.5;
/// The fewest runs of each arm in a measurement.
pub const MEASURE_RUNS_MIN: u32 = 3;
/// The most runs of each arm in a measurement.
pub const MEASURE_RUNS_MAX: u32 = 10;
/// An eval set must have at least this many planted errors and kept claims.
pub const EVAL_SET_MIN_COUNT: u32 = 5;

// --- Lengths ---

/// Titles, in Unicode scalar values.
pub const TITLE_MAX: usize = 120;
/// A link URL as stored, in bytes.
pub const LINK_MAX_BYTES: usize = 2_000;
/// A discussion's body, in bytes.
pub const POST_BODY_MAX_BYTES: usize = 20_000;
/// A comment's body, in bytes.
pub const COMMENT_BODY_MAX_BYTES: usize = 10_000;
/// A report's note, in characters.
pub const REPORT_NOTE_MAX: usize = 1_000;
/// A measurement's model name, in characters.
pub const MODEL_MAX: usize = 200;
/// A measurement's report link, in bytes.
pub const REPORT_URL_MAX_BYTES: usize = 300;
/// Personal CSS and a theme pack's stylesheet, in bytes.
pub const CSS_MAX_BYTES: usize = hippocamp_theme::CSS_MAX;
/// A pack post's title, in characters.
pub const PACK_TITLE_MAX: usize = 120;
/// A `return_to` value, in bytes.
pub const RETURN_TO_MAX_BYTES: usize = 512;
/// `FEED_CONTACT`, in characters.
pub const CONTACT_MAX: usize = 200;

// --- Threads ---

/// A reply is at most this many levels below a top-level comment.
pub const COMMENT_DEPTH_MAX: u32 = 10;
/// A post takes at most this many comments.
pub const COMMENTS_PER_POST_MAX: u32 = 1_000;

// --- Pages ---

/// Rows on a page of posts or comments.
pub const PAGE_SIZE: usize = 30;
/// The last page of posts or comments that can be asked for.
pub const PAGE_MAX: usize = 20;
/// Rows on a page of the moderation log, queue or admin lists.
pub const LIST_PAGE_SIZE: usize = 50;

// --- Reports and rings ---

/// Distinct established reporters that hide an item automatically.
pub const AUTO_HIDE_REPORTERS: u32 = 3;
/// A resolved report's note and snapshot are deleted after this many days.
pub const REPORT_TEXT_DAYS: i64 = 90;
/// Two accounts acted together if they acted this many minutes apart or less.
pub const RING_WINDOW_MINUTES: i64 = 10;
/// Accounts are linked when they acted together on this many owners' items.
pub const RING_MIN_TOGETHER: u32 = 3;
/// The ring check reads this many days back.
pub const RING_DAYS: i64 = 30;

// --- Backups and health ---

/// Backup copies are kept this many days by default.
pub const BACKUP_KEEP_DAYS: u32 = 14;
/// `/healthz` lists `backup` as stale after this many hours without one.
pub const STALE_BACKUP_HOURS: i64 = 36;
/// `/healthz` lists `sync` as stale after this many hours without a success.
pub const STALE_SYNC_HOURS: i64 = 6;
/// The sync check starts once the server has run this many hours.
pub const SYNC_GRACE_HOURS: i64 = 1;
/// `/healthz` lists `reports` as stale when one has been open this long.
pub const STALE_REPORT_HOURS: i64 = 48;
/// Graceful shutdown waits this long for requests in flight.
pub const SHUTDOWN_WAIT: Duration = Duration::from_secs(10);

// --- Registry entries ---

/// A pack name, in bytes (at least).
pub const PACK_NAME_MIN: usize = 2;
/// A pack name, in bytes (at most).
pub const PACK_NAME_MAX: usize = 40;
/// An index entry's description, in characters.
pub const PACK_DESCRIPTION_MAX: usize = 300;
/// An index entry names at most this many maintainers (and at least one).
pub const PACK_MAINTAINERS_MAX: usize = 10;
/// A pack of passes has at most this many passes (and at least one).
pub const PACK_PASSES_MAX: usize = 20;
/// A GitHub login, in characters.
pub const GITHUB_LOGIN_MAX: usize = 39;
/// A repository name, in characters.
pub const REPO_NAME_MAX: usize = 100;
/// The web theme contract version the feed serves.
pub const THEME_CONTRACT: i64 = 1;
/// The registry index is read up to this many bytes.
pub const INDEX_MAX_BYTES: u64 = 5 * 1024 * 1024;

/// The licences the feed lists packs under: the registry's list.
pub const LICENCES: &[&str] = &[
    "MIT",
    "MIT-0",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "0BSD",
    "Zlib",
    "Unlicense",
    "CC0-1.0",
    "CC-BY-4.0",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_match_the_design() {
        assert_eq!(CSS_MAX_BYTES, 65_536);
        assert_eq!(LICENCES.len(), 11);
        assert_eq!(PAGE_SIZE * PAGE_MAX, 600);
    }
}
