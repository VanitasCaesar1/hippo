//! What one dreaming run did, read from its report's bullets: the notes it
//! checked, the notes it left out and why, the proposals that failed
//! validation, and the reviewer's verdicts. The Dreaming panel and the
//! overview draw this instead of the report's raw text.
//!
//! The bullets are the ones `hippocamp_core::dream` writes (in the dreaming
//! report). Most begin with the chunk's window,
//! `<from> to <to> `, then the pass, then a note path that may hold spaces,
//! then `: ` and the rest. A bullet this module does not recognise is kept
//! in [`Digest::other`], never dropped.

use std::collections::BTreeMap;

use ulid::Ulid;

use crate::text::visible;

/// One run's report, by meaning.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Digest {
    /// The run wrote nothing but its report.
    pub dry_run: bool,
    /// The window's chunks, `<from> to <to>: done|failed`.
    pub window: Vec<String>,
    /// The ingest's summary, the first `## Inputs` bullet.
    pub ingest: Option<String>,
    /// What each pass read, per note for a per-note pass.
    pub checked: Vec<Checked>,
    /// Whole notes a pass left out, and why.
    pub skipped: Vec<Skipped>,
    /// Per pass and note, how many lower-ranked turns were cut for the
    /// budget, and their bytes.
    pub turns_cut: Vec<TurnsCut>,
    /// Proposals that failed validation: never reviewed, never written.
    pub failures: Vec<Failure>,
    /// The reviewer's verdict per proposal id, with its notes.
    pub verdicts: BTreeMap<Ulid, Verdict>,
    /// Bullets this module does not recognise, with their section.
    pub other: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    pub pass: String,
    /// The note, for a per-note pass.
    pub note: Option<String>,
    /// `2 sessions, 1 notes, 172 turns, 0 human-confirmed`, or
    /// `no note to work on`.
    pub facts: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub pass: String,
    pub note: String,
    /// In words: `too large: 350 KB, over the 200 KB budget`.
    pub why: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnsCut {
    pub pass: String,
    pub note: Option<String>,
    pub count: usize,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub pass: String,
    pub note: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    /// `confirm`, `reject`, or `not applied` when the note changed after
    /// the prompt.
    pub verdict: String,
    pub notes: String,
}

impl Digest {
    /// The notes the run looked at, each once, in report order.
    pub fn notes_checked(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for c in &self.checked {
            if let Some(n) = c.note.as_deref() {
                if !out.contains(&n) {
                    out.push(n);
                }
            }
        }
        out
    }

    /// The largest session count any pass read, from `<n> sessions`.
    pub fn sessions_read(&self) -> usize {
        self.checked
            .iter()
            .filter_map(|c| c.facts.split(' ').next()?.parse().ok())
            .max()
            .unwrap_or(0)
    }
}

/// Reads a report's text. Never fails: what it cannot place goes to
/// [`Digest::other`].
pub fn parse(text: &str) -> Digest {
    let mut d = Digest::default();
    let mut section = String::new();
    let mut cut: BTreeMap<(String, Option<String>), (usize, u64)> = BTreeMap::new();
    let mut cut_order: Vec<(String, Option<String>)> = Vec::new();
    for line in text.lines() {
        if let Some(h) = line.strip_prefix("## ") {
            section = h.to_owned();
            continue;
        }
        let Some(bullet) = line.strip_prefix("- ") else {
            continue;
        };
        let bullet = visible(bullet);
        match section.as_str() {
            "Window" => {
                if bullet == hippocamp_core::dream::report::DRY_RUN_LINE {
                    d.dry_run = true;
                } else {
                    d.window.push(bullet);
                }
            }
            "Inputs" => {
                if d.ingest.is_none() && d.checked.is_empty() && !starts_with_window(&bullet) {
                    d.ingest = Some(bullet);
                } else if let Some((pass, note, rest)) = split(&bullet, At::Last) {
                    d.checked.push(Checked {
                        pass,
                        note,
                        facts: rest,
                    });
                } else {
                    d.other.push((section.clone(), bullet));
                }
            }
            "Omissions" => match split(&bullet, At::Omission) {
                Some((pass, note, rest)) => {
                    if let Some(bytes) = rest
                        .strip_prefix("omitted: turns (")
                        .and_then(|r| r.strip_suffix(')'))
                        .and_then(|b| b.parse::<u64>().ok())
                    {
                        let key = (pass, note);
                        if !cut.contains_key(&key) {
                            cut_order.push(key.clone());
                        }
                        let e = cut.entry(key).or_default();
                        e.0 += 1;
                        e.1 += bytes;
                    } else if let Some(r) = rest.strip_prefix("omitted: note ") {
                        // `omitted: note <path> (<bytes> bytes)`
                        let (path, bytes) = match r.rsplit_once(" (") {
                            Some((p, b)) => (
                                p.to_owned(),
                                b.trim_end_matches(" bytes)").parse::<u64>().ok(),
                            ),
                            None => (r.to_owned(), None),
                        };
                        d.skipped.push(Skipped {
                            pass,
                            note: path,
                            why: match bytes {
                                Some(b) => format!(
                                    "left out to fit the budget: {}, with the sessions it read",
                                    size(b)
                                ),
                                None => "left out to fit the budget".into(),
                            },
                        });
                    } else if let Some(r) = rest.strip_prefix("skipped: ") {
                        d.skipped.push(Skipped {
                            pass,
                            note: note.unwrap_or_default(),
                            why: skipped_why(r),
                        });
                    } else {
                        d.other.push((section.clone(), bullet));
                    }
                }
                None => d.other.push((section.clone(), bullet)),
            },
            "Validation failures" => match split(&bullet, At::First) {
                Some((pass, note, reason)) => d.failures.push(Failure { pass, note, reason }),
                None => d.other.push((section.clone(), bullet)),
            },
            "Verdicts" => {
                let mut words = bullet.splitn(2, ' ');
                let id = words.next().and_then(|w| w.parse::<Ulid>().ok());
                let rest = words.next().unwrap_or_default();
                match (id, rest.split_once(": ")) {
                    (Some(id), Some((verdict, notes))) if !verdict.starts_with("not applied") => {
                        d.verdicts.insert(
                            id,
                            Verdict {
                                verdict: verdict.to_owned(),
                                notes: notes.to_owned(),
                            },
                        );
                    }
                    (Some(id), _) if rest.starts_with("not applied") => {
                        d.verdicts.insert(
                            id,
                            Verdict {
                                verdict: "not applied".into(),
                                notes: rest.to_owned(),
                            },
                        );
                    }
                    _ => d.other.push((section.clone(), bullet)),
                }
            }
            _ => {}
        }
    }
    d.turns_cut = cut_order
        .into_iter()
        .map(|key| {
            let (count, bytes) = cut[&key];
            TurnsCut {
                pass: key.0,
                note: key.1,
                count,
                bytes,
            }
        })
        .collect();
    d
}

/// Whether a bullet starts with a chunk's window, `<rfc3339> to `.
fn starts_with_window(bullet: &str) -> bool {
    let mut words = bullet.split(' ');
    matches!(
        (words.next(), words.next()),
        (Some(from), Some("to")) if from.len() >= 20 && from.as_bytes()[4] == b'-' && from.contains('T')
    )
}

/// Where a bullet's head (pass and note) ends.
#[derive(Clone, Copy)]
enum At {
    /// At the first `: `: the rest may hold `: `.
    First,
    /// At the last `: `: the note may hold `: `.
    Last,
    /// Before `: omitted: ` or `: skipped: `.
    Omission,
}

/// `[<from> to <to> ]<pass>[ <note>]: <rest>` as its pass, note and rest.
fn split(bullet: &str, at: At) -> Option<(String, Option<String>, String)> {
    let body = if starts_with_window(bullet) {
        // Drop `<from> to <to> `.
        bullet.splitn(4, ' ').nth(3)?
    } else {
        bullet
    };
    let i = match at {
        At::First => body.find(": ")?,
        At::Last => body.rfind(": ")?,
        At::Omission => body
            .find(": omitted: ")
            .or_else(|| body.find(": skipped: "))?,
    };
    let (head, rest) = (&body[..i], &body[i + 2..]);
    let (pass, note) = match head.split_once(' ') {
        Some((pass, note)) => (pass.to_owned(), Some(note.to_owned())),
        None => (head.to_owned(), None),
    };
    if pass.is_empty() {
        return None;
    }
    Some((pass, note, rest.to_owned()))
}

/// A `skipped:` reason in words, sizes in KB.
fn skipped_why(reason: &str) -> String {
    // `the note alone makes a <n>-byte prompt, over <budget>`
    let parsed = reason
        .strip_prefix("the note alone makes a ")
        .and_then(|r| r.split_once("-byte prompt, over "))
        .and_then(|(n, budget)| Some((n.parse::<u64>().ok()?, budget.trim().parse::<u64>().ok()?)));
    match parsed {
        Some((n, budget)) => format!(
            "too large: the note alone makes a {} prompt, over the {} budget",
            size(n),
            size(budget)
        ),
        None => reason.to_owned(),
    }
}

/// Bytes in words: `640 B`, `12 KB`, `1.4 MB`.
pub fn size(bytes: u64) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{} KB", (bytes + 512) / 1024),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: &str = "2026-11-07T17:23:56Z to 2026-11-08T17:23:56Z";

    fn report() -> String {
        format!(
            "status: ok\n\n## Window\n\n- {W}: done\n\n## Inputs\n\n\
             - ingest first: indexed 3 files, skipped 5259, derived 0 sessions\n\
             - {W} consolidate: 173 sessions, 2 notes, 0 turns, 0 human-confirmed\n\
             - {W} verify notes/A long: name.md: 2 sessions, 1 notes, 172 turns, 0 human-confirmed\n\
             - {W} verify notes/B.md: 1 sessions, 1 notes, 26 turns, 0 human-confirmed\n\
             \n## Omissions\n\n\
             - {W} consolidate: omitted: note notes/Log.md (350374 bytes)\n\
             - {W} verify notes/B.md: omitted: turns (622)\n\
             - {W} verify notes/C: x.md: omitted: turns (10)\n\
             - {W} verify notes/B.md: omitted: turns (3105)\n\
             - {W} verify notes/Log.md: skipped: the note alone makes a 365617-byte prompt, over 200000\n\
             - {W} verify: something new\n\
             \n## Proposals\n\n- 01M6WS8Z7ZA7H2PS2RPG29RK25 notes/B.md (verify, replace)\n\
             \n## Validation failures\n\n\
             - verify notes/B.md: new does not quote an evidence turn's text verbatim\n\
             - {W} consolidate: not a proposal: missing field `note`\n\
             \n## Verdicts\n\n\
             - 01M6WS8Z7ZA7H2PS2RPG29RK25 reject: the evidence says 30: not 14\n\
             - 01M6WS8Z7ZA7H2PS2RPG29RK26 not applied: the note changed after the prompt (receipt 01M6WS8Z7ZA7H2PS2RPG29RK27)\n\
             \n## Applied\n"
        )
    }

    #[test]
    fn a_report_reads_as_what_the_run_did() {
        let d = parse(&report());
        assert!(!d.dry_run);
        assert_eq!(d.window, [format!("{W}: done")]);
        assert_eq!(
            d.ingest.as_deref(),
            Some("ingest first: indexed 3 files, skipped 5259, derived 0 sessions")
        );
        assert_eq!(d.checked.len(), 3);
        // A note path may hold spaces and `: `.
        assert_eq!(d.checked[1].note.as_deref(), Some("notes/A long: name.md"));
        assert_eq!(
            d.checked[1].facts,
            "2 sessions, 1 notes, 172 turns, 0 human-confirmed"
        );
        assert_eq!(d.notes_checked(), ["notes/A long: name.md", "notes/B.md"]);
        assert_eq!(d.sessions_read(), 173);
        assert_eq!(
            d.skipped,
            [
                Skipped {
                    pass: "consolidate".into(),
                    note: "notes/Log.md".into(),
                    why: "left out to fit the budget: 342 KB, with the sessions it read".into(),
                },
                Skipped {
                    pass: "verify".into(),
                    note: "notes/Log.md".into(),
                    why: "too large: the note alone makes a 357 KB prompt, over the 195 KB budget"
                        .into(),
                },
            ]
        );
        assert_eq!(
            d.turns_cut,
            [
                TurnsCut {
                    pass: "verify".into(),
                    note: Some("notes/B.md".into()),
                    count: 2,
                    bytes: 3727,
                },
                TurnsCut {
                    pass: "verify".into(),
                    note: Some("notes/C: x.md".into()),
                    count: 1,
                    bytes: 10,
                },
            ]
        );
        assert_eq!(d.failures.len(), 2);
        assert_eq!(d.failures[0].note.as_deref(), Some("notes/B.md"));
        assert_eq!(
            d.failures[1],
            Failure {
                pass: "consolidate".into(),
                note: None,
                reason: "not a proposal: missing field `note`".into(),
            }
        );
        let id: Ulid = "01M6WS8Z7ZA7H2PS2RPG29RK25".parse().unwrap();
        assert_eq!(d.verdicts[&id].verdict, "reject");
        assert_eq!(d.verdicts[&id].notes, "the evidence says 30: not 14");
        let other: Ulid = "01M6WS8Z7ZA7H2PS2RPG29RK26".parse().unwrap();
        assert_eq!(d.verdicts[&other].verdict, "not applied");
        assert_eq!(
            d.other,
            [("Omissions".to_owned(), format!("{W} verify: something new"))]
        );
    }

    #[test]
    fn a_dry_run_and_a_failed_run_parse() {
        let d = parse(&format!(
            "status: failed\nerror: model error\n\n## Window\n\n- {}\n- {W}: failed\n",
            hippocamp_core::dream::report::DRY_RUN_LINE
        ));
        assert!(d.dry_run);
        assert_eq!(d.window, [format!("{W}: failed")]);
        assert_eq!(parse(""), Digest::default());
    }

    #[test]
    fn sizes_read_in_words() {
        assert_eq!(size(640), "640 B");
        assert_eq!(size(69_068), "67 KB");
        assert_eq!(size(2_500_000), "2.4 MB");
    }
}
