//! The card: one day, or seven, of a
//! person's agents as one 1200 × 675 image and a text version.
//!
//! Privacy is structural: everything drawn comes from [`Facts`], whose
//! fields are numbers, dates and the span's kind. No string read from the
//! index, the rollup, a transcript, a record or the configuration can reach
//! the card, so no project, title, path, repository, account or profile
//! name can appear on it.

mod font;
mod png;
mod render;

use time::{Date, Duration};

pub use render::{render, HEIGHT, WIDTH};

/// What the card covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Span {
    /// Today, in local time.
    Today(Date),
    /// An earlier day.
    Day(Date),
    /// The seven days `first..=last`.
    Week { first: Date, last: Date },
}

/// Everything the card shows: numbers and dates only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Facts {
    pub span: Span,
    pub work: Duration,
    pub threads: usize,
    pub subs: usize,
    pub merged: usize,
    pub waited: Duration,
    pub questions: usize,
    pub corrections: usize,
}

/// A piece of a line, bold or not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub bold: bool,
}

fn plain(text: impl Into<String>) -> Run {
    Run {
        text: text.into(),
        bold: false,
    }
}

fn bold(text: impl Into<String>) -> Run {
    Run {
        text: text.into(),
        bold: true,
    }
}

/// The words of the card, in the order they are drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Words {
    /// `Tuesday 10 November 2026`, or `4 to 10 November 2026`.
    pub date: String,
    /// `My agents did`.
    pub lead: String,
    /// `56 hours 34 minutes`.
    pub work: String,
    /// `of work today.`
    pub tail: String,
    /// The four lines under the sentence.
    pub lines: [Vec<Run>; 4],
}

pub const NAME: &str = "hippocamp";
pub const TAGLINE: &str = "one brain for all your coding agents";

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

fn month(date: Date) -> &'static str {
    MONTHS[usize::from(u8::from(date.month())) - 1]
}

fn weekday(date: Date) -> String {
    date.weekday().to_string()
}

fn count(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        one.to_owned()
    } else {
        many.to_owned()
    }
}

/// `Tuesday 10 November 2026`; a week: `4 to 10 November 2026`,
/// `29 October to 4 November 2026`, `29 December 2026 to 4 January 2027`.
pub fn date_line(span: Span) -> String {
    match span {
        Span::Today(d) | Span::Day(d) => {
            format!("{} {} {} {}", weekday(d), d.day(), month(d), d.year())
        }
        Span::Week { first, last } => {
            let end = format!("{} {} {}", last.day(), month(last), last.year());
            if first.year() != last.year() {
                format!("{} {} {} to {end}", first.day(), month(first), first.year())
            } else if first.month() != last.month() {
                format!("{} {} to {end}", first.day(), month(first))
            } else {
                format!("{} to {end}", first.day())
            }
        }
    }
}

/// `56 hours 34 minutes`, `1 hour 1 minute`, `5 hours`, `42 minutes`,
/// `under a minute`.
pub fn work_time(d: Duration) -> String {
    let minutes = d.whole_minutes().max(0);
    if minutes == 0 {
        return "under a minute".to_owned();
    }
    let (h, m) = (minutes / 60, minutes % 60);
    let hours = format!("{h} {}", if h == 1 { "hour" } else { "hours" });
    let mins = format!("{m} {}", if m == 1 { "minute" } else { "minutes" });
    match (h, m) {
        (0, _) => mins,
        (_, 0) => hours,
        _ => format!("{hours} {mins}"),
    }
}

/// `1h 29m`, `42m`, as the overview writes a wait; `under a minute`.
pub fn wait_time(d: Duration) -> String {
    let minutes = d.whole_minutes().max(0);
    if minutes == 0 {
        "under a minute".to_owned()
    } else if minutes >= 60 {
        format!("{}h {:02}m", minutes / 60, minutes % 60)
    } else {
        format!("{minutes}m")
    }
}

/// The card's words for `facts`.
pub fn words(facts: &Facts) -> Words {
    let tail = match facts.span {
        Span::Today(_) => "of work today.".to_owned(),
        Span::Day(d) => format!("of work on {}.", weekday(d)),
        Span::Week { .. } => "of work in seven days.".to_owned(),
    };
    let threads = {
        let mut line = vec![
            bold(facts.threads.to_string()),
            plain(format!(" {}", count(facts.threads, "thread", "threads"))),
        ];
        if facts.subs == 0 {
            line.push(plain(" and no sub-agents"));
        } else {
            line.push(plain(" and "));
            line.push(bold(facts.subs.to_string()));
            line.push(plain(format!(
                " {}",
                count(facts.subs, "sub-agent", "sub-agents")
            )));
        }
        line
    };
    let merged = if facts.merged == 0 {
        vec![plain("no pull requests merged")]
    } else {
        vec![
            bold(facts.merged.to_string()),
            plain(format!(
                " {} merged",
                count(facts.merged, "pull request", "pull requests")
            )),
        ]
    };
    let waited = match facts.questions {
        0 => vec![plain("they never waited on me")],
        1 => vec![
            plain("they waited on me "),
            bold(wait_time(facts.waited)),
            plain(", for 1 question"),
        ],
        q => vec![
            plain("they waited on me "),
            bold(wait_time(facts.waited)),
            plain(format!(", over {q} questions")),
        ],
    };
    let corrected = vec![
        plain("I had to correct them "),
        bold(facts.corrections.to_string()),
        plain(format!(" {}", count(facts.corrections, "time", "times"))),
    ];
    Words {
        date: date_line(facts.span),
        lead: "My agents did".to_owned(),
        work: work_time(facts.work),
        tail,
        lines: [threads, merged, waited, corrected],
    }
}

fn line_text(runs: &[Run]) -> String {
    runs.iter().map(|r| r.text.as_str()).collect()
}

/// The text version, without the `wrote` line.
pub fn text(words: &Words) -> String {
    let mut out = format!(
        "{}\n{} {} {}\n",
        words.date, words.lead, words.work, words.tail
    );
    for line in &words.lines {
        out.push_str(&format!("  {}\n", line_text(line)));
    }
    out.push_str(&format!("\n{NAME} · {TAGLINE}\n"));
    out
}

/// Every string drawn on the card, for the privacy test.
#[cfg(test)]
pub fn drawn(words: &Words) -> Vec<String> {
    let mut all = vec![
        words.date.clone(),
        words.lead.clone(),
        words.work.clone(),
        words.tail.clone(),
        NAME.to_owned(),
        TAGLINE.to_owned(),
    ];
    all.extend(words.lines.iter().map(|l| line_text(l)));
    all
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    fn facts(span: Span) -> Facts {
        Facts {
            span,
            work: Duration::minutes(56 * 60 + 34),
            threads: 13,
            subs: 538,
            merged: 66,
            waited: Duration::minutes(89),
            questions: 7,
            corrections: 0,
        }
    }

    fn lines(f: &Facts) -> Vec<String> {
        words(f).lines.iter().map(|l| line_text(l)).collect()
    }

    #[test]
    fn the_reference_day_reads_as_approved() {
        let w = words(&facts(Span::Today(date!(2026 - 11 - 10))));
        assert_eq!(w.date, "Tuesday 10 November 2026");
        assert_eq!(
            format!("{} / {} / {}", w.lead, w.work, w.tail),
            "My agents did / 56 hours 34 minutes / of work today."
        );
        assert_eq!(
            lines(&facts(Span::Today(date!(2026 - 11 - 10)))),
            [
                "13 threads and 538 sub-agents",
                "66 pull requests merged",
                "they waited on me 1h 29m, over 7 questions",
                "I had to correct them 0 times",
            ]
        );
        assert_eq!(w.lines[0][0], bold("13"));
    }

    #[test]
    fn edges_keep_the_wording_true() {
        let mut f = facts(Span::Day(date!(2026 - 11 - 3)));
        f.threads = 1;
        f.subs = 1;
        f.merged = 1;
        f.questions = 1;
        f.waited = Duration::minutes(12);
        f.corrections = 1;
        assert_eq!(words(&f).tail, "of work on Tuesday.");
        assert_eq!(
            lines(&f),
            [
                "1 thread and 1 sub-agent",
                "1 pull request merged",
                "they waited on me 12m, for 1 question",
                "I had to correct them 1 time",
            ]
        );
        f.subs = 0;
        f.merged = 0;
        f.questions = 0;
        f.waited = Duration::ZERO;
        f.corrections = 3;
        assert_eq!(
            lines(&f),
            [
                "1 thread and no sub-agents",
                "no pull requests merged",
                "they never waited on me",
                "I had to correct them 3 times",
            ]
        );
        f.questions = 2;
        f.waited = Duration::seconds(20);
        assert_eq!(
            lines(&f)[2],
            "they waited on me under a minute, over 2 questions"
        );
    }

    #[test]
    fn work_time_reads_in_words() {
        assert_eq!(work_time(Duration::minutes(61)), "1 hour 1 minute");
        assert_eq!(work_time(Duration::minutes(300)), "5 hours");
        assert_eq!(work_time(Duration::minutes(42)), "42 minutes");
        assert_eq!(work_time(Duration::minutes(1)), "1 minute");
        assert_eq!(work_time(Duration::seconds(30)), "under a minute");
        assert_eq!(work_time(Duration::minutes(18725)), "312 hours 5 minutes");
    }

    #[test]
    fn a_week_names_its_seven_days() {
        let week = |first, last| Span::Week { first, last };
        let f = facts(week(date!(2026 - 11 - 4), date!(2026 - 11 - 10)));
        let w = words(&f);
        assert_eq!(w.date, "4 to 10 November 2026");
        assert_eq!(w.tail, "of work in seven days.");
        assert_eq!(
            date_line(week(date!(2026 - 10 - 29), date!(2026 - 11 - 4))),
            "29 October to 4 November 2026"
        );
        assert_eq!(
            date_line(week(date!(2026 - 12 - 29), date!(2027 - 1 - 4))),
            "29 December 2026 to 4 January 2027"
        );
    }

    #[test]
    fn the_text_version_holds_the_same_words() {
        let t = text(&words(&facts(Span::Today(date!(2026 - 11 - 10)))));
        assert_eq!(
            t,
            "Tuesday 10 November 2026\nMy agents did 56 hours 34 minutes of work today.\n  13 threads and 538 sub-agents\n  66 pull requests merged\n  they waited on me 1h 29m, over 7 questions\n  I had to correct them 0 times\n\nhippocamp · one brain for all your coding agents\n"
        );
    }

    #[test]
    fn every_drawn_string_comes_from_the_fixed_wording() {
        // Only digits, the fixed words and the date's names can appear.
        let allowed = [
            "My",
            "agents",
            "did",
            "of",
            "work",
            "today.",
            "on",
            "in",
            "seven",
            "days.",
            "hours",
            "hour",
            "minutes",
            "minute",
            "under",
            "a",
            "threads",
            "thread",
            "and",
            "no",
            "sub-agents",
            "sub-agent",
            "pull",
            "requests",
            "request",
            "merged",
            "they",
            "waited",
            "me",
            "never",
            "over",
            "for",
            "questions",
            "question",
            "I",
            "had",
            "to",
            "correct",
            "them",
            "times",
            "time",
            "hippocamp",
            "one",
            "brain",
            "all",
            "your",
            "coding",
        ];
        let f = facts(Span::Day(date!(2026 - 11 - 3)));
        for s in drawn(&words(&f)) {
            for word in s.split([' ', ',', '.']).filter(|w| !w.is_empty()) {
                let ok = word
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == 'h' || c == 'm')
                    || allowed.contains(&word)
                    || allowed.contains(&format!("{word}.").as_str())
                    || MONTHS.contains(&word)
                    || [
                        "Monday",
                        "Tuesday",
                        "Wednesday",
                        "Thursday",
                        "Friday",
                        "Saturday",
                        "Sunday",
                    ]
                    .contains(&word);
                assert!(ok, "{word:?} in {s:?}");
            }
        }
    }
}
