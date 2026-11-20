//! The night map's view models (`docs/designs/app-night-map.md`): what
//! Morning, Brain, Agents and Guard draw, and what `hippo view <model>
//! --json` prints. Each model is a plain value that serialises to JSON, so
//! the app, the CLI and the site's web replica read the same numbers.
//!
//! [`morning`] is the last nightly pass and what waits on the person,
//! [`strip`] the day, night and morning band, [`map`] the notes with their
//! links and laid-out rectangles for each zoom level, [`note`] one note's
//! panel, [`guard`] the guard's incidents in plain words, and [`counts`]
//! the top bar's badges. Nothing here writes.

pub mod counts;
pub mod guard;
pub mod map;
pub mod morning;
pub mod note;
pub mod reach;
pub mod recent;
pub mod strip;

use serde::Serialize;
use time::{OffsetDateTime, UtcOffset};

/// An agent, as a badge draws it: its code (`cc`), its two letters (`CC`)
/// and its name (`Claude Code`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Agent {
    pub code: String,
    pub short: String,
    pub name: String,
}

/// The agent behind a session's tool name (`claude-code`, `codex`, …).
pub fn agent(tool: &str) -> Agent {
    let (code, short, name) = match tool {
        "claude-code" | "claude" => ("cc", "CC", "Claude Code"),
        "codex" => ("cx", "Cx", "Codex"),
        "cursor" => ("cu", "Cu", "Cursor"),
        "gemini" | "gemini-cli" => ("gm", "Gm", "Gemini CLI"),
        "opencode" => ("oc", "OC", "OpenCode"),
        "dsh" => ("ds", "DS", "DeepSeek Harness"),
        "muse" => ("mu", "Mu", "Muse"),
        "command-code" => ("co", "Co", "Command Code"),
        other => {
            let short: String = other
                .chars()
                .filter(|c| c.is_alphanumeric())
                .take(2)
                .collect();
            return Agent {
                code: short.to_lowercase(),
                short: capital(&short),
                name: other.to_owned(),
            };
        }
    };
    Agent {
        code: code.into(),
        short: short.into(),
        name: name.into(),
    }
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// `08:41`.
pub fn hm(t: OffsetDateTime, offset: UtcOffset) -> String {
    let t = t.to_offset(offset);
    format!("{:02}:{:02}", t.hour(), t.minute())
}

/// `Thu 10:07` for a time before today, `10:07` for today.
pub fn day_hm(t: OffsetDateTime, now: OffsetDateTime, offset: UtcOffset) -> String {
    let (lt, ln) = (t.to_offset(offset), now.to_offset(offset));
    if lt.date() == ln.date() {
        hm(t, offset)
    } else {
        format!("{} {}", weekday(lt.date()), hm(t, offset))
    }
}

/// `Thu`.
pub fn weekday(d: time::Date) -> &'static str {
    use time::Weekday::*;
    match d.weekday() {
        Monday => "Mon",
        Tuesday => "Tue",
        Wednesday => "Wed",
        Thursday => "Thu",
        Friday => "Fri",
        Saturday => "Sat",
        Sunday => "Sun",
    }
}

/// `Friday 20 November`.
pub fn long_date(d: time::Date) -> String {
    use time::Weekday::*;
    let day = match d.weekday() {
        Monday => "Monday",
        Tuesday => "Tuesday",
        Wednesday => "Wednesday",
        Thursday => "Thursday",
        Friday => "Friday",
        Saturday => "Saturday",
        Sunday => "Sunday",
    };
    format!("{day} {} {}", d.day(), d.month())
}

/// `4 min 12 s`, `38 s`, `1 h 5 min`.
pub fn took(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < 60 {
        format!("{secs} s")
    } else if secs < 3600 {
        format!("{} min {} s", secs / 60, secs % 60)
    } else {
        format!("{} h {} min", secs / 3600, (secs % 3600) / 60)
    }
}

/// `n thing` or `n things`.
pub fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A one-sentence summary of a note: its frontmatter `summary:` when it
/// has one, else the first sentence of its first paragraph as plain text,
/// at most `max` characters.
pub fn summary_of(note: &hippo_core::notes::Note, max: usize) -> String {
    let raw = String::from_utf8_lossy(&note.raw_frontmatter);
    if let Some(s) = raw.lines().find_map(|l| l.strip_prefix("summary:")) {
        let s = s.trim().trim_matches('"').trim_matches('\'').trim();
        if !s.is_empty() {
            return crate::edition::sentences(&crate::text::visible(s), max);
        }
    }
    let body = String::from_utf8_lossy(&note.body);
    let mut para = String::new();
    let mut fenced = false;
    for line in body.lines() {
        let t = line.trim();
        if t.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced || t.starts_with('#') || t.starts_with('|') || t.starts_with("<!--") {
            if !para.is_empty() {
                break;
            }
            continue;
        }
        if t.is_empty() {
            if !para.is_empty() {
                break;
            }
            continue;
        }
        if !para.is_empty() {
            para.push(' ');
        }
        para.push_str(t.trim_start_matches(['-', '*', '>', ' ']));
    }
    let plain = crate::edition::plain(&para.replace("[[", "").replace("]]", ""));
    let plain = crate::text::visible(plain.trim());
    let first = match plain.find(". ") {
        Some(i) => plain[..=i].to_owned(),
        None => plain,
    };
    crate::edition::sentences(&first, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agents_have_badges() {
        assert_eq!(agent("claude-code").short, "CC");
        assert_eq!(agent("codex").code, "cx");
        assert_eq!(agent("gemini").name, "Gemini CLI");
        assert_eq!(agent("zed").short, "Ze");
    }

    #[test]
    fn durations_read_plainly() {
        assert_eq!(took(38), "38 s");
        assert_eq!(took(252), "4 min 12 s");
        assert_eq!(took(3900), "1 h 5 min");
    }

    #[test]
    fn summaries_prefer_frontmatter_then_first_sentence() {
        let note = |fm: &str, body: &str| hippo_core::notes::Note {
            path: "n.md".into(),
            raw_frontmatter: fm.as_bytes().to_vec(),
            body: body.as_bytes().to_vec(),
            frontmatter: Default::default(),
            frontmatter_error: None,
            frontmatter_warnings: Vec::new(),
        };
        assert_eq!(
            summary_of(
                &note("---\nsummary: How previews run.\n---\n", "# T\n\nOther."),
                80
            ),
            "How previews run."
        );
        assert_eq!(
            summary_of(
                &note(
                    "",
                    "# Title\n\nPreviews use [[Staging]] data. They run on 4310.\n"
                ),
                80
            ),
            "Previews use Staging data."
        );
    }
}

/// Everything the night map's views read, loaded once.
#[derive(Debug, Clone)]
pub struct Night {
    pub morning: morning::Morning,
    pub strip: strip::Strip,
    pub map: map::NotesMap,
    /// The guard's incidents of the last 7 days, before "Not a problem".
    pub incidents: Vec<hippo_core::incidents::Incident>,
    /// Top-level sessions in those 7 days.
    pub week_sessions: usize,
    pub titles: std::collections::HashMap<String, String>,
    pub asker: Option<counts::Asker>,
    /// Every configured runner, for Ask's runner menu.
    pub runners: Vec<counts::Asker>,
    pub reaches: Vec<reach::Reach>,
}

impl Night {
    /// Guard, leaving out what `dismissed` names.
    pub fn guard(
        &self,
        dismissed: &dyn Fn(&str) -> bool,
        now: OffsetDateTime,
        offset: UtcOffset,
    ) -> guard::Guard {
        guard::build(
            &self.incidents,
            dismissed,
            self.week_sessions,
            7,
            &|id| self.titles.get(id).cloned(),
            now,
            offset,
        )
    }
}

/// Reads what [`Night`] holds. Blocks; writes nothing.
pub fn load(
    cfg: &hippo_core::config::Config,
    ws: &hippo_core::config::Workspace,
    now: OffsetDateTime,
    offset: UtcOffset,
) -> hippo_core::error::Result<Night> {
    use std::collections::{BTreeSet, HashMap};
    use std::path::PathBuf;
    let history = hippo_core::dream::history(ws)?;
    let dreaming = crate::dreaming::load(ws, &history)?;
    let lessons = hippo_core::lessons::list(&cfg.home).unwrap_or_default();
    let rollup = hippo_core::rollup::Rollup::open(&cfg.home).ok().flatten();
    let brain = crate::brain::load(ws, &history);
    let note_titles: HashMap<PathBuf, String> = brain
        .rows
        .iter()
        .map(|r| (r.path.clone(), r.title.clone()))
        .collect();
    let morning = morning::build(&morning::Inputs {
        ws,
        history: &history,
        dreaming: &dreaming,
        lessons: &lessons,
        rollup: rollup.as_ref(),
        titles: &note_titles,
        now,
        offset,
    });
    let pass = morning.pass.as_ref().map(|p| (p.started, p.ended));
    let strip = strip::build(rollup.as_ref(), pass, now, offset);
    let fixed: BTreeSet<PathBuf> = morning
        .applied
        .iter()
        .filter(|f| !f.reverted)
        .map(|f| f.note.clone())
        .collect();
    let rejected: BTreeSet<PathBuf> = morning.rejected.iter().map(|f| f.note.clone()).collect();
    let map = map::build(&map::Inputs {
        ws,
        brain: &brain,
        rollup: rollup.as_ref(),
        lessons: &lessons,
        fixed: &fixed,
        rejected: &rejected,
        now,
        offset,
    });
    let since = now - time::Duration::days(7);
    let (incidents, week_sessions, titles) = match &rollup {
        Some(r) => (
            r.incidents(since).unwrap_or_default(),
            r.sessions_in(since, now)
                .unwrap_or_default()
                .iter()
                .filter(|s| s.facts.parent.is_none() && !s.facts.automated)
                .count(),
            r.titles().unwrap_or_default(),
        ),
        None => (Vec::new(), 0, HashMap::new()),
    };
    let plan = hippo_core::runners::plan(cfg).ok();
    let asker = plan.as_ref().and_then(counts::asker);
    let runners = plan.as_ref().map(counts::runners).unwrap_or_default();
    Ok(Night {
        morning,
        strip,
        map,
        incidents,
        week_sessions,
        titles,
        asker,
        runners,
        reaches: reach::reaches(&ws.root),
    })
}
