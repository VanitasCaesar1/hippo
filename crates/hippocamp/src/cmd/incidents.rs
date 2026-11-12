//! `hippocamp incidents [--json] [--days N] [--all]`: what went wrong in the
//! last days, which agent did it, at which step, and why when a rule says
//! so.

use std::process::ExitCode;

use anyhow::{anyhow, Result};
use hippocamp_core::incidents::subject::{Subject, SubjectKind};
use hippocamp_core::incidents::{Incident, HINTS_HEADING};
use hippocamp_core::rollup::Rollup;
use time::{Duration, OffsetDateTime};

use super::load_history;

/// `Tue 10 Nov 2026`.
const DAY: &[time::format_description::FormatItem<'static>] = time::macros::format_description!(
    "[weekday repr:short] [day padding:none] [month repr:short] [year]"
);

#[derive(clap::Args)]
pub struct Args {
    /// Days to cover (1 to 14)
    #[arg(long, default_value_t = 7, value_parser = clap::value_parser!(u32).range(1..=14))]
    days: u32,
    /// Also list destructive commands nothing harmful followed
    #[arg(long)]
    all: bool,
    /// Print one JSON array
    #[arg(long)]
    json: bool,
}

pub fn run(args: Args) -> Result<ExitCode> {
    let (config, _) = load_history()?;
    let rollup = Rollup::open(&config.home)?
        .ok_or_else(|| anyhow!("no rollup yet; run hippocamp ingest"))?;
    let now = OffsetDateTime::now_utc();
    let mut list: Vec<Incident> = rollup
        .incidents(now - Duration::days(i64::from(args.days)))?
        .into_iter()
        .filter(|i| args.all || !i.quiet)
        .collect();
    let offset = hippocamp_core::usage::local_offset_at(now);
    // Newest day first; costliest first within a day.
    list.sort_by(|a, b| {
        let day = |i: &Incident| i.at.to_offset(offset).date();
        day(b)
            .cmp(&day(a))
            .then(b.cost_key().cmp(&a.cost_key()))
            .then(b.at.cmp(&a.at))
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&list)?);
        return Ok(ExitCode::SUCCESS);
    }
    // Hints to check are listed apart, after the incidents.
    let (hints, list): (Vec<Incident>, Vec<Incident>) =
        list.into_iter().partition(|i| i.kind.is_hint());
    if list.is_empty() {
        println!("No incidents in the last {} days.", args.days);
    } else {
        print_days(&list, offset);
    }
    if !hints.is_empty() {
        println!("\n{HINTS_HEADING}");
        print_days(&hints, offset);
    }
    Ok(ExitCode::SUCCESS)
}

/// Rows under a heading per local day.
fn print_days(list: &[Incident], offset: time::UtcOffset) {
    let mut day = None;
    for i in list {
        let d = i.at.to_offset(offset).date();
        if day != Some(d) {
            println!("\n{}", d.format(&DAY).unwrap_or_else(|_| d.to_string()));
            day = Some(d);
        }
        let mut cost = Vec::new();
        if let Some(m) = i.minutes.filter(|m| *m > 0) {
            cost.push(format!("{m}m"));
        }
        if let Some(l) = i.lines {
            cost.push(format!("{l} lines"));
        }
        if let Some(t) = i.tokens {
            cost.push(format!("{}k tokens", t / 1000));
        }
        let quiet = if i.quiet { " (quiet)" } else { "" };
        println!(
            "  {}{quiet} · {}",
            head(i.kind.label(), i.cause.label()),
            i.body.showed.text
        );
        let c = &i.body.chain;
        let who = match &c.sub {
            Some(sub) => format!(
                "{} › {sub}",
                c.root_title.as_deref().unwrap_or(c.root.as_str())
            ),
            None => c.root_title.clone().unwrap_or_else(|| c.root.clone()),
        };
        let profile = c.profile.as_deref().unwrap_or("?");
        let hm = |t: OffsetDateTime| {
            let t = t.to_offset(offset);
            format!("{:02}:{:02}", t.hour(), t.minute())
        };
        let cost = if cost.is_empty() {
            String::new()
        } else {
            format!(" · {}", cost.join(", "))
        };
        match &i.body.subject {
            Some(about) => print_subject(about, profile, &who, &hm, offset),
            None => println!(
                "    {profile} · {who} · step {} at {}{cost}",
                i.body.did.ordinal,
                hm(i.body.did.at)
            ),
        }
    }
}

/// A correction's subject under its line: the step with
/// its working time, the earlier work it may be about, what the agent
/// said, or that it is not established.
fn print_subject(
    about: &Subject,
    profile: &str,
    who: &str,
    hm: &dyn Fn(OffsetDateTime) -> String,
    offset: time::UtcOffset,
) {
    match about.kind {
        SubjectKind::Step => {
            let Some(step) = &about.step else { return };
            let sub = about
                .sub
                .as_deref()
                .map(|s| format!(" › {s}"))
                .unwrap_or_default();
            let work = about
                .minutes
                .map(|m| format!(" · {m}m of work"))
                .unwrap_or_default();
            println!(
                "    {profile} · {who}{sub} · step {} at {}{work}",
                step.ordinal,
                hm(step.at)
            );
            println!("      what it did: {}", one_line(&step.text, 120));
        }
        SubjectKind::Earlier => {
            println!("    {profile} · {who} · may be about:");
            for c in &about.candidates {
                let mut parts = vec![
                    c.words.join(", "),
                    format!(
                        "{} ({}, {})",
                        c.title.as_deref().unwrap_or(c.session.as_str()),
                        c.relation.label(),
                        c.profile.as_deref().unwrap_or("?")
                    ),
                ];
                if let Some(under) = &c.under {
                    let n = c.files;
                    parts.push(format!(
                        "{n} file{} under {under}",
                        if n == 1 { "" } else { "s" }
                    ));
                }
                if let Some(p) = &c.pull {
                    parts.push(match &p.title {
                        Some(t) => format!("{} \"{}\"", p.url, one_line(t, 80)),
                        None => p.url.clone(),
                    });
                }
                let d = c.at.to_offset(offset).date();
                parts.push(format!(
                    "{} {}",
                    d.format(&DAY).unwrap_or_else(|_| d.to_string()),
                    hm(c.at)
                ));
                println!("      {}", parts.join(" · "));
            }
        }
        SubjectKind::Said => {
            let Some(said) = &about.said else { return };
            println!(
                "    {profile} · {who} · what it said at {}: \"{}\"",
                hm(said.at),
                one_line(&said_text(&said.text), 120)
            );
        }
        SubjectKind::None => {
            println!("    {profile} · {who} · what it is about is not established");
        }
    }
}

/// `text` on one line, at most `max` characters.
fn one_line(text: &str, max: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= max {
        line
    } else {
        let cut: String = line.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

/// A row's kind and cause, `<kind> · <cause>`, or the label once when they
/// read the same (a claim: `claimed without checking`).
fn head(kind: &str, cause: &str) -> String {
    if kind == cause {
        kind.to_owned()
    } else {
        format!("{kind} · {cause}")
    }
}

/// What the agent said, as the app shows it (`hippocamp-app`'s
/// `incidents::said_text`): a Markdown image link `![alt](path)` reads as
/// the file's name, or its alt text when the path has none.
fn said_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(k) = rest.find("![") {
        let after = &rest[k + 2..];
        let Some(close) = after.find("](") else { break };
        let target = &after[close + 2..];
        let Some(end) = target.find(')') else { break };
        let path = target[..end].split_whitespace().next().unwrap_or("");
        let name = path
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(path);
        out.push_str(&rest[..k]);
        out.push_str(if name.is_empty() {
            &after[..close]
        } else {
            name
        });
        rest = &target[end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use hippocamp_core::incidents::{Cause, Kind};

    #[test]
    fn a_claim_prints_its_label_once() {
        assert_eq!(
            head(Kind::Unverified.label(), Cause::Claimed.label()),
            "claimed without checking"
        );
        assert_eq!(
            head(Kind::Blocked.label(), Cause::Hook.label()),
            format!("{} · {}", Kind::Blocked.label(), Cause::Hook.label())
        );
    }

    #[test]
    fn an_image_link_in_what_it_said_prints_the_files_name() {
        assert_eq!(
            said_text("Here it is: ![the chart](/tmp/out/chart.png) and done."),
            "Here it is: chart.png and done."
        );
        assert_eq!(
            said_text("![a](x/one.png) then ![b](two.png \"a title\")"),
            "one.png then two.png"
        );
        assert_eq!(said_text("![only alt]()"), "only alt");
        assert_eq!(
            said_text("no image, [a link](x.md) stays"),
            "no image, [a link](x.md) stays"
        );
        assert_eq!(said_text("an open ![bracket"), "an open ![bracket");
    }
}
