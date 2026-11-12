//! The Needs attention tab: every note the linter flags, grouped by what to
//! do about it, worst first. The overview shows the top of each group.

use std::path::Path;

use gpui::{Context, Role, SharedString, div, prelude::*};
use hippo_view::brain::Attention;
use time::Date;
use time::macros::format_description;

use super::Shell;
use super::theme::Look;
use crate::state::Tab;

/// `due 24 Oct, 15 days ago`.
pub fn overdue_text(due: Date, today: Date) -> String {
    let days = (today - due).whole_days();
    let when = due
        .format(format_description!("[day] [month repr:short] [year]"))
        .unwrap_or_default();
    match days {
        ..=0 => format!("review due {when}"),
        1 => format!("review was due {when}, 1 day ago"),
        n => format!("review was due {when}, {n} days ago"),
    }
}

/// The folder of a note below the notes directory, `Flows/`, or `""`.
pub fn folder_of(path: &Path, notes_dir: &Path) -> String {
    let below = path.strip_prefix(notes_dir).unwrap_or(path);
    match below.parent() {
        Some(p) if !p.as_os_str().is_empty() => format!("{}/", p.display()),
        _ => String::new(),
    }
}

/// `Guides 20 · Releases 13 · Archive 8 · 4 more folders`.
pub fn folders_line(folders: &[(String, usize)], top: usize) -> String {
    let mut parts: Vec<String> = folders
        .iter()
        .take(top)
        .map(|(f, n)| {
            let name = if f.is_empty() { "top level" } else { f };
            format!("{name} {n}")
        })
        .collect();
    if folders.len() > top {
        parts.push(format!("{} more folders", folders.len() - top));
    }
    parts.join(" · ")
}

/// What to do, per group.
pub const DO_BROKEN: &str = "The frontmatter does not parse, so hippo reads none of it.";
pub const DO_DANGLING: &str = "A [[link]] that names no note: fix the name or write the note.";
pub const DO_OVERDUE: &str =
    "Re-check each against the code or the decision it records, then move review_after forward.";
pub const DO_NONE: &str =
    "No type, status or review date: hippo cannot tell how current these are.";

impl Shell {
    pub(super) fn today(&self) -> Date {
        time::OffsetDateTime::now_utc()
            .to_offset(super::status::local_offset())
            .date()
    }

    /// The rows of one group, at most `limit`, each opening its note.
    pub(super) fn attention_rows(
        &self,
        prefix: &str,
        a: &Attention,
        notes_dir: &Path,
        limit: usize,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> Vec<(
        &'static str,
        String,
        &'static str,
        Vec<gpui::AnyElement>,
        usize,
    )> {
        let today = self.today();
        let mut groups = Vec::new();
        let row = |i: usize, path: &Path, title: &str, meta: String, cx: &mut Context<Self>| {
            self.overview_row(
                SharedString::from(format!("{prefix}-{}-{i}", path.display())),
                title.to_owned(),
                Some(meta),
                Tab::Note(path.to_path_buf()),
                look,
                cx,
            )
        };
        if !a.broken.is_empty() {
            let rows = a
                .broken
                .iter()
                .take(limit)
                .enumerate()
                .map(|(i, (p, t, m))| row(i, p, t, format!("{} · {m}", p.display()), cx))
                .collect();
            groups.push((
                "Broken frontmatter",
                String::new(),
                DO_BROKEN,
                rows,
                a.broken.len(),
            ));
        }
        if !a.dangling.is_empty() {
            let rows = a
                .dangling
                .iter()
                .take(limit)
                .enumerate()
                .map(|(i, (p, t, l))| {
                    row(
                        i,
                        p,
                        t,
                        format!("[[{l}]] names no note · {}", folder_of(p, notes_dir)),
                        cx,
                    )
                })
                .collect();
            groups.push((
                "Broken links",
                String::new(),
                DO_DANGLING,
                rows,
                a.dangling.len(),
            ));
        }
        if !a.overdue.is_empty() {
            let rows = a
                .overdue
                .iter()
                .take(limit)
                .enumerate()
                .map(|(i, (p, t, due))| {
                    let folder = folder_of(p, notes_dir);
                    let meta = if folder.is_empty() {
                        overdue_text(*due, today)
                    } else {
                        format!("{} · {folder}", overdue_text(*due, today))
                    };
                    row(i, p, t, meta, cx)
                })
                .collect();
            groups.push((
                "Past their review date",
                String::new(),
                DO_OVERDUE,
                rows,
                a.overdue.len(),
            ));
        }
        if !a.no_frontmatter.is_empty() {
            let rows = a
                .no_frontmatter
                .iter()
                .take(limit)
                .enumerate()
                .map(|(i, (p, t))| row(i, p, t, p.display().to_string(), cx))
                .collect();
            groups.push((
                "No frontmatter",
                folders_line(&a.no_frontmatter_folders, 4),
                DO_NONE,
                rows,
                a.no_frontmatter.len(),
            ));
        }
        groups
    }

    pub(super) fn render_attention_tab(
        &mut self,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let mut column = div()
            .id("attention-tab")
            .role(Role::Article)
            .flex()
            .flex_col()
            .w_full()
            .max_w(look.px(720.))
            .p(look.px(16.))
            .text_size(look.text(13.));
        let Some(p) = self.loaded.as_ref().and_then(|l| l.panels.as_ref()) else {
            return column.into_any_element();
        };
        let (a, notes_dir) = (p.attention.clone(), p.notes_dir.clone());
        column = column
            .child(
                div()
                    .text_size(look.text(15.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(format!("Needs attention · {} notes", count_notes(&a))),
            )
            .child(
                div()
                    .text_size(look.text(11.))
                    .text_color(look.c.text_muted)
                    .child("From hippo's note linter. Fix a note in your editor (⌘O on its tab); ⌘R reloads."),
            );
        if a.is_empty() {
            return column
                .child(
                    div()
                        .pt(look.px(16.))
                        .child("Nothing: every note lints clean."),
                )
                .into_any_element();
        }
        for (title, folders, what, rows, n) in
            self.attention_rows("att", &a, &notes_dir, usize::MAX, look, cx)
        {
            column = column
                .child(Self::section_heading(look, &format!("{title} · {n}")))
                .child(
                    div()
                        .pb(look.px(4.))
                        .text_size(look.text(11.))
                        .text_color(look.c.text_muted)
                        .child(what),
                )
                .when(!folders.is_empty(), |d| {
                    d.child(
                        div()
                            .pb(look.px(4.))
                            .text_size(look.text(11.))
                            .text_color(look.c.text_muted)
                            .child(format!("By folder: {folders}")),
                    )
                })
                .children(rows);
        }
        column.into_any_element()
    }
}

/// How many distinct notes have a finding.
pub fn count_notes(a: &Attention) -> usize {
    let mut paths: Vec<&Path> = a
        .broken
        .iter()
        .map(|x| x.0.as_path())
        .chain(a.dangling.iter().map(|x| x.0.as_path()))
        .chain(a.overdue.iter().map(|x| x.0.as_path()))
        .chain(a.no_frontmatter.iter().map(|x| x.0.as_path()))
        .collect();
    paths.sort();
    paths.dedup();
    paths.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    #[test]
    fn overdue_reads_as_days_late() {
        let today = date!(2026 - 11 - 08);
        assert_eq!(
            overdue_text(date!(2026 - 10 - 24), today),
            "review was due 24 Oct 2026, 15 days ago"
        );
        assert_eq!(
            overdue_text(date!(2026 - 11 - 07), today),
            "review was due 07 Nov 2026, 1 day ago"
        );
    }

    #[test]
    fn folders_read_as_a_short_line() {
        let f = vec![
            ("Guides".to_owned(), 20),
            ("Releases".to_owned(), 13),
            (String::new(), 7),
            ("Archive".to_owned(), 8),
        ];
        assert_eq!(
            folders_line(&f, 3),
            "Guides 20 · Releases 13 · top level 7 · 1 more folders"
        );
    }

    #[test]
    fn the_count_is_of_notes_not_findings() {
        let mut a = Attention::default();
        a.overdue
            .push(("n/a.md".into(), "A".into(), date!(2026 - 01 - 01)));
        a.no_frontmatter.push(("n/a.md".into(), "A".into()));
        a.no_frontmatter.push(("n/b.md".into(), "B".into()));
        assert_eq!(count_notes(&a), 2);
    }
}
