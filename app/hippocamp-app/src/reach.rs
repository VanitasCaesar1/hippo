//! How hippocamp reaches the person outside its window:
//! what the menu-bar item says and lists, and which fleet changes become a
//! notification. Pure: the fleet in, text and events out; the status item
//! and the notification center are in `menubar.rs` and the shell.

use std::collections::{HashMap, HashSet};

use hippocamp_view::fleet::{self, Fleet, Row, State, Why};
use time::{Duration, OffsetDateTime};

/// What a menu row or a notification's action does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    /// Open the session where it lives (T3 Code's thread, its terminal),
    /// or resume it there when nothing holds it.
    Open(String),
    /// Resume a session stopped at a limit (on Use next when its own
    /// profile is out).
    Resume(String),
    /// Show the session's x-ray in the window.
    Xray(String),
    OpenHippocamp,
    /// Open the window on the Inbox.
    Inbox,
    Brief,
    Quit,
}

/// One Needs you row of the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuRow {
    pub title: String,
    /// The reason, as the item's subtitle: `asked you · 12 min`.
    pub reason: String,
    pub act: Act,
}

/// The menu-bar item and its menu.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Menu {
    /// Beside the glyph: `1 waiting`, `1 stopped` when someone
    /// needs the person; with live agents only, their count (`2`); empty
    /// when nothing is.
    pub badge: String,
    /// `Needs you · 3` (sentence case, no small capitals).
    pub needs_head: String,
    pub needs: Vec<MenuRow>,
    /// `No agent needs you · 2 live` when `needs` is empty.
    pub nothing: Option<String>,
    /// `Working · 2` and one row per live agent, its live line as subtitle.
    pub live_head: Option<String>,
    pub live: Vec<MenuRow>,
    /// `Inbox · 14 to review`, when anything is.
    pub inbox: Option<String>,
    /// `Headroom`.
    pub headroom_head: Option<String>,
    /// Each headroom row's bar: what is left of the tighter
    /// window and whether it is inferred, in the rows' order.
    pub bars: Vec<(u8, bool)>,
    /// One row per Claude profile, in Use next order: `work  100%  use
    /// next`, `default  16%  Wed 01:30`, `home  ~100%  reset Sun 04:30`.
    pub headroom: Vec<String>,
}

/// The most live rows the menu lists.
pub const LIVE_ROWS: usize = 5;

/// The most Needs you rows the menu lists.
pub const MENU_ROWS: usize = 8;
/// A row's title is cut to this many characters.
const TITLE_CHARS: usize = 48;

fn cut(s: &str, n: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= n {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
    out = out.trim_end().to_owned();
    out.push('…');
    out
}

/// `12m`, `2h 5m`, `just now`: the one duration form.
fn waited(at: OffsetDateTime, now: OffsetDateTime) -> String {
    let d = now - at;
    if d.whole_minutes() < 1 {
        "just now".into()
    } else {
        fleet::span(d.whole_seconds())
    }
}

/// A row's title for the menu and notifications: a session the index has
/// not read yet is titled by its id's first characters in Fleet; here it
/// reads `New session in <project>` until the next ingest names it.
pub fn title_of(row: &Row) -> String {
    let id8: String = row.id.chars().take(8).collect();
    if row.title.is_empty() || row.title == id8 || row.title == row.id {
        return match &row.project {
            Some(p) => format!("New session in {p}"),
            None => "New session".to_owned(),
        };
    }
    crate::ui::kit::title(&row.title)
}

/// The item's text (the shared tally): `1 needs you`, `2 need
/// you` when someone needs the person; `2` with working agents only;
/// empty when nothing is.
pub fn badge(t: &crate::shelving::Tally) -> String {
    match (t.needs_you, t.working) {
        (0, 0) => String::new(),
        (0, w) => w.to_string(),
        (1, _) => "1 needs you".to_owned(),
        (n, _) => format!("{n} need you"),
    }
}

/// `<project> · <profile>` for a row's subtitle.
pub fn whereabouts(row: &Row) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.extend(row.project.clone());
    parts.extend(
        row.profile
            .as_deref()
            .map(|p| fleet::profile_label(p).to_owned()),
    );
    parts.join(" · ")
}

/// Each [`headroom_rows`] row's bar: the percent left of the tighter
/// window (0 at a limit) and whether it is inferred.
pub fn headroom_bars(f: &Fleet) -> Vec<(u8, bool)> {
    crate::ui::home::ordered(&f.headroom, f.use_next.as_deref())
        .into_iter()
        .filter(|h| !h.codex)
        .filter_map(|h| {
            let tight = [h.five, h.seven].into_iter().flatten().min_by(|a, b| {
                a.frac
                    .total_cmp(&b.frac)
                    .then_with(|| fleet::inferred(b).cmp(&fleet::inferred(a)))
            });
            let left = if h.usable() { h.left()? } else { 0.0 };
            Some((
                (left.clamp(0.0, 1.0) * 100.0).round() as u8,
                tight.as_ref().is_some_and(fleet::inferred),
            ))
        })
        .collect()
}

/// One menu row per Claude profile in Use next order: the name,
/// what is left of the tighter window (`~` when inferred, `0%` at a
/// limit), then `use next` or its reset.
pub fn headroom_rows(f: &Fleet, now: OffsetDateTime) -> Vec<String> {
    let offset = crate::ui::status::local_offset();
    crate::ui::home::ordered(&f.headroom, f.use_next.as_deref())
        .into_iter()
        .filter(|h| !h.codex)
        .filter_map(|h| {
            // The tighter window; on a tie the inferred one, so `~` shows.
            let tight = [h.five, h.seven].into_iter().flatten().min_by(|a, b| {
                a.frac
                    .total_cmp(&b.frac)
                    .then_with(|| fleet::inferred(b).cmp(&fleet::inferred(a)))
            });
            let left = if h.usable() { h.left()? } else { 0.0 };
            let guess = tight.as_ref().is_some_and(fleet::inferred);
            let pct = format!("{}{}% left", if guess { "~" } else { "" }, fleet::pct(left));
            let tail = if f.use_next.as_deref() == Some(h.profile.as_str()) {
                "use next".to_owned()
            } else {
                match tight.and_then(|l| l.resets_at.or(l.reset_passed).map(|r| (r, l))) {
                    Some((r, l)) => {
                        let w = fleet::when(r, now, offset);
                        if l.reset_passed.is_some() {
                            format!("reset {w}")
                        } else {
                            w
                        }
                    }
                    None => String::new(),
                }
            };
            Some(
                format!("{}   {pct}   {tail}", fleet::profile_label(&h.profile))
                    .trim_end()
                    .to_owned(),
            )
        })
        .collect()
}

/// A Needs you row's reason in words, as the menu and Home say it.
pub fn reason(why: Why, row: &Row, now: OffsetDateTime) -> String {
    match (&row.state, why) {
        (State::Waiting { at, plan, .. }, Why::Waiting) => {
            if *plan {
                format!("plan to approve · {}", waited(*at, now))
            } else {
                format!("asked you · {}", waited(*at, now))
            }
        }
        (State::Stopped { at, .. }, Why::Stopped) => {
            format!("stopped at its limit {}", crate::ui::kit::hm(*at))
        }
        _ => {
            let n = row.claims.unverified;
            format!(
                "{n} claim{} unverified · shows the evidence",
                if n == 1 { "" } else { "s" }
            )
        }
    }
}

/// The menu for `fleet` (none read yet: an empty menu that says so), with
/// `inbox` items to review. Its counts are the sidebar's.
pub fn menu(
    f: Option<&Fleet>,
    marks: &crate::shelving::Marks<'_>,
    inbox: usize,
    now: OffsetDateTime,
) -> Menu {
    let Some(f) = f else {
        return Menu {
            needs_head: "Needs you".into(),
            nothing: Some("Reading the fleet…".into()),
            ..Menu::default()
        };
    };
    let shelves = crate::shelving::Shelves::of(f, marks);
    let tally = shelves.tally();
    let needs = f.needs_you();
    let rows: Vec<MenuRow> = needs
        .iter()
        .take(MENU_ROWS)
        .map(|n| MenuRow {
            title: cut(&title_of(n.row), TITLE_CHARS),
            reason: {
                let w = whereabouts(n.row);
                let r = reason(n.why, n.row, now);
                if w.is_empty() {
                    r
                } else {
                    format!("{r} · {w}")
                }
            },
            act: match n.why {
                Why::Waiting => Act::Open(n.row.id.clone()),
                Why::Stopped => Act::Resume(n.row.id.clone()),
                Why::Check => Act::Xray(n.row.id.clone()),
            },
        })
        .collect();
    let working = tally.working;
    // What working agents are doing, as Home and Fleet say it,
    // and the sidebar's Working shelf.
    let live: Vec<MenuRow> = shelves
        .rows(fleet::Shelf::Working)
        .iter()
        .take(LIVE_ROWS)
        .map(|r| MenuRow {
            title: cut(&title_of(r), TITLE_CHARS),
            reason: fleet::live_facts(r, now),
            act: Act::Xray(r.id.clone()),
        })
        .collect();
    let rooms = headroom_rows(f, now);
    Menu {
        badge: badge(&tally),
        needs_head: if needs.is_empty() {
            "Needs you".into()
        } else {
            format!("Needs you · {}", tally.needs_you)
        },
        // "working", Home's and the sidebar's word and number ("1 live" and
        // "1 agent working" read as two different things).
        nothing: needs.is_empty().then(|| match working {
            0 => "No agent needs you".to_owned(),
            n => format!("No agent needs you · {n} working"),
        }),
        needs: rows,
        live_head: (working > 0).then(|| format!("Working · {working}")),
        live,
        inbox: (inbox > 0).then(|| format!("Inbox · {inbox} to review")),
        headroom_head: (!rooms.is_empty()).then(|| "Headroom".to_owned()),
        bars: headroom_bars(f),
        headroom: rooms,
    }
}

/// A notification's kind (each has its own switch).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Waiting,
    Stopped,
    Reset,
}

/// Which kinds are on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Switches {
    pub waiting: bool,
    pub stopped: bool,
    pub reset: bool,
}

impl Switches {
    fn on(&self, k: Kind) -> bool {
        match k {
            Kind::Waiting => self.waiting,
            Kind::Stopped => self.stopped,
            Kind::Reset => self.reset,
        }
    }

    pub fn any(&self) -> bool {
        self.waiting || self.stopped || self.reset
    }
}

/// A notification to post.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    pub kind: Kind,
    pub id: String,
    pub title: String,
    pub body: String,
    /// The one action button: `Open` or `Resume`.
    pub action: &'static str,
}

/// At most one notification per session in this long.
pub const PER_SESSION: Duration = Duration::hours(1);

/// Remembers what the fleet looked like, to announce only what changed.
#[derive(Debug, Default)]
pub struct Watcher {
    seeded: bool,
    /// `(kind, session, the state's time)` already seen: a wait is
    /// announced once, whatever happens to the notification.
    seen: HashSet<(Kind, String, i64)>,
    /// Each profile's `usable()` at the last read.
    usable: HashMap<String, bool>,
    /// When each session was last announced.
    sent: HashMap<String, OffsetDateTime>,
}

impl Watcher {
    /// The notifications this read of the fleet calls for. The first read
    /// only remembers. `quiet(id)` is true while the window shows that
    /// session in front; a quiet event is still marked seen.
    pub fn observe(
        &mut self,
        f: &Fleet,
        now: OffsetDateTime,
        switches: Switches,
        quiet: &dyn Fn(&str) -> bool,
    ) -> Vec<Alert> {
        let mut events: Vec<(Kind, &Row, i64, String)> = Vec::new();
        for row in &f.rows {
            match &row.state {
                State::Waiting { at, asked, plan } => {
                    let body = if *plan {
                        "has a plan to approve".to_owned()
                    } else {
                        match asked {
                            Some(q) => format!("asked you: {}", cut(q, 120)),
                            None => "is waiting on you".to_owned(),
                        }
                    };
                    events.push((Kind::Waiting, row, at.unix_timestamp(), body));
                }
                State::Stopped {
                    at,
                    reset,
                    carried_on: false,
                } => {
                    let mut body = "stopped at its usage limit".to_owned();
                    if let Some(r) = reset {
                        body.push_str(&format!(" · {}", hippocamp_view::text::twenty_four(r)));
                    }
                    events.push((Kind::Stopped, row, at.unix_timestamp(), body));
                    let profile = row.profile.clone().unwrap_or_default();
                    let now_usable = f.room(&profile).is_some_and(|h| h.usable());
                    let was_usable = self.usable.get(&profile).copied();
                    if now_usable && was_usable == Some(false) {
                        events.push((
                            Kind::Reset,
                            row,
                            at.unix_timestamp(),
                            format!(
                                "{}'s limit reset; still to resume",
                                fleet::profile_label(&profile)
                            ),
                        ));
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for (kind, row, at, body) in events {
            let key = (kind, row.id.clone(), at);
            if !self.seen.insert(key) || !self.seeded {
                continue;
            }
            if !switches.on(kind) || quiet(&row.id) {
                continue;
            }
            if self
                .sent
                .get(&row.id)
                .is_some_and(|t| now - *t < PER_SESSION)
            {
                continue;
            }
            self.sent.insert(row.id.clone(), now);
            // A reset names the profile already: the project alone.
            let w = if kind == Kind::Reset {
                row.project.clone().unwrap_or_default()
            } else {
                whereabouts(row)
            };
            out.push(Alert {
                kind,
                id: row.id.clone(),
                title: cut(&title_of(row), 80),
                // Where it is, so two sessions can be told apart.
                body: if w.is_empty() {
                    body
                } else {
                    format!("{w} · {body}")
                },
                action: if kind == Kind::Waiting {
                    "Open"
                } else {
                    "Resume"
                },
            });
        }
        self.usable = f
            .headroom
            .iter()
            .map(|h| (h.profile.clone(), h.usable()))
            .collect();
        self.seeded = true;
        out
    }
}

#[cfg(test)]
mod tests;
