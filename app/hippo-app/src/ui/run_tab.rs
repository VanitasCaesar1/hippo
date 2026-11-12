//! A dreaming run's tab: what the run changed, what it rejected, what failed
//! validation, which notes it checked and which it left out, read from its
//! report (`hippo_view::digest`), with the report file itself one click
//! away.

use std::path::{Path, PathBuf};

use gpui::{Context, Role, SharedString, div, prelude::*};
use hippo_view::digest;
use hippo_view::dreaming::Run;
use hippo_view::text::visible;

use super::Shell;
use super::theme::Look;
use crate::rows::{self, RunFacts};
use crate::state::Tab;

/// `2 sessions, 1 notes, 172 turns, 0 human-confirmed` without the parts
/// that say nothing: zero counts and `1 notes` of a per-note pass.
pub fn short_facts(facts: &str) -> String {
    facts
        .split(", ")
        .filter(|p| !p.starts_with("0 ") && *p != "1 notes")
        .map(|p| match p {
            "1 sessions" => "1 session",
            p => p,
        })
        .collect::<Vec<_>>()
        .join(", ")
}

impl Shell {
    /// A note's title from the Brain rows, else its file name.
    pub(super) fn note_title(&self, path: &Path) -> String {
        self.loaded
            .as_ref()
            .and_then(|l| l.panels.as_ref())
            .and_then(|p| p.brain.rows.iter().find(|r| r.path == path))
            .map(|r| r.title.clone())
            .unwrap_or_else(|| rows::note_name(&path.display().to_string()))
    }

    fn run_for(&self, path: &Path) -> Option<Run> {
        self.loaded
            .as_ref()?
            .panels
            .as_ref()?
            .dreaming
            .as_ref()
            .ok()?
            .runs
            .iter()
            .find(|r| r.report.path == path)
            .cloned()
    }

    /// A one-line link to a note, with text after it.
    fn note_line(
        &self,
        id: SharedString,
        note: Option<&str>,
        after: String,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let mut line = div().flex().flex_wrap().gap_x(look.px(6.)).py(look.px(2.));
        if let Some(note) = note {
            let path = PathBuf::from(note);
            let title = self.note_title(&path);
            let tab = Tab::Note(path);
            line = line.child(
                div()
                    .id(id)
                    .role(Role::Link)
                    .cursor_pointer()
                    .text_color(look.c.text_accent)
                    .child(title)
                    .on_click(cx.listener(move |this, e: &gpui::ClickEvent, window, cx| {
                        this.state.tabs.follow(tab.clone(), e.click_count() >= 2);
                        this.after_state_change(window, cx);
                    })),
            );
        }
        // Wraps under the title, and within the column, when it is long.
        line.child(div().min_w_0().text_color(look.c.text_muted).child(after))
            .into_any_element()
    }

    pub(super) fn render_run_tab(
        &mut self,
        path: &Path,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        self.load_report(path);
        let column = div()
            .id("run-tab")
            .role(Role::Article)
            .flex()
            .flex_col()
            .w_full()
            .max_w(look.px(720.))
            .p(look.px(16.))
            .gap(look.px(2.))
            .text_size(look.text(13.));
        let Some(run) = self.run_for(path) else {
            return column
                .child(
                    div()
                        .text_color(look.c.text_muted)
                        .child("This run's report is gone; press ⌘R to reload."),
                )
                .into_any_element();
        };
        let facts = rows::run_facts(&run);
        let offset = super::status::local_offset();
        let digest = run.digest.clone().unwrap_or_default();

        let mut column = column
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_start()
                    .gap(look.px(12.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(look.px(220.))
                            .text_size(look.text(15.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(format!("Dreaming run · {}", facts.when_label(offset))),
                    )
                    .child(self.file_buttons(path.to_path_buf(), look, cx)),
            )
            .child(
                div()
                    .text_color(if facts.failed {
                        look.c.error
                    } else {
                        look.c.text
                    })
                    .child(headline(&facts)),
            );
        if let Some(e) = &facts.error {
            column = column.child(
                div()
                    .pt(look.px(4.))
                    .text_color(look.c.error)
                    .child(format!("Error: {}", visible(e))),
            );
        }
        if facts.dry_run {
            column = column.child(
                div()
                    .pt(look.px(4.))
                    .text_color(look.c.text_muted)
                    .child("A dry run: it asked the model and the reviewer but wrote no proposal, receipt or note."),
            );
        }

        // What it changed, and what the reviewer turned down.
        if !run.changes.is_empty() {
            column = column.child(Self::section_heading(look, "Changes"));
            for change in &run.changes {
                let (title, meta) = rows::change_text(change);
                let verdict = digest
                    .verdicts
                    .get(&change.proposal_id)
                    .map(|v| format!(" · {}", visible(&v.notes)))
                    .unwrap_or_default();
                column = column.child(self.overview_row(
                    SharedString::from(format!("run-change-{}", change.proposal_id)),
                    title,
                    Some(format!("{meta}{verdict}")),
                    Tab::Proposal(change.proposal_id),
                    look,
                    cx,
                ));
            }
        }
        if !run.missing.is_empty() {
            column = column.child(Self::section_heading(look, "Proposed, not written"));
            column = column.child(div().text_color(look.c.text_muted).child(format!(
                "{} — a dry run writes no proposal file. Their verdicts:",
                rows::count(run.missing.len(), "proposal", "proposals")
            )));
            for id in &run.missing {
                if let Some(v) = digest.verdicts.get(id) {
                    column = column.child(div().text_color(look.c.text_muted).child(format!(
                        "{} · {}",
                        v.verdict,
                        visible(&v.notes)
                    )));
                }
            }
        }

        if !digest.failures.is_empty() {
            column = column
                .child(Self::section_heading(look, "Failed validation"))
                .child(
                    div()
                        .pb(look.px(2.))
                        .text_size(look.text(11.))
                        .text_color(look.c.text_muted)
                        .child("Proposals that broke a rule before review: never reviewed, nothing written."),
                );
            for (i, f) in digest.failures.iter().enumerate() {
                column = column.child(self.note_line(
                    SharedString::from(format!("run-invalid-{i}")),
                    f.note.as_deref(),
                    format!("{} · {}", f.pass, f.reason),
                    look,
                    cx,
                ));
            }
        }

        if !digest.checked.is_empty() {
            column = column.child(Self::section_heading(
                look,
                &format!("Checked · {}", rows::count(facts.checked, "note", "notes")),
            ));
            for (i, c) in digest.checked.iter().enumerate() {
                let cut = digest
                    .turns_cut
                    .iter()
                    .find(|t| t.pass == c.pass && t.note == c.note)
                    .map(|t| {
                        format!(
                            "; {} lower-ranked turns cut for the budget ({})",
                            t.count,
                            digest::size(t.bytes)
                        )
                    })
                    .unwrap_or_default();
                let text = format!("{} · {}{cut}", c.pass, short_facts(&c.facts));
                column = column.child(self.note_line(
                    SharedString::from(format!("run-checked-{i}")),
                    c.note.as_deref(),
                    text,
                    look,
                    cx,
                ));
            }
        }

        if !digest.skipped.is_empty() {
            column = column.child(Self::section_heading(look, "Left out"));
            for (i, s) in digest.skipped.iter().enumerate() {
                column = column.child(self.note_line(
                    SharedString::from(format!("run-skipped-{i}")),
                    Some(&s.note),
                    format!("{} · {}", s.pass, s.why),
                    look,
                    cx,
                ));
            }
        }

        if !digest.other.is_empty() {
            column = column.child(Self::section_heading(look, "Other report lines"));
            for (section, line) in &digest.other {
                column = column.child(
                    div()
                        .text_color(look.c.text_muted)
                        .child(format!("{section}: {line}")),
                );
            }
        }

        column = column.child(Self::section_heading(look, "Window"));
        for w in &digest.window {
            column = column.child(div().text_color(look.c.text_muted).child(w.clone()));
        }
        if let Some(i) = &digest.ingest {
            column = column.child(div().text_color(look.c.text_muted).child(i.clone()));
        }

        // The report file, folded.
        let open = self.dream_ui.raw_open.contains(path);
        let key = path.to_path_buf();
        column = column.child(
            div()
                .id("run-raw-toggle")
                .role(Role::Button)
                .pt(look.px(16.))
                .cursor_pointer()
                .text_size(look.text(11.))
                .text_color(look.c.text_accent)
                .child(if open {
                    "▾ Hide the report file"
                } else {
                    "▸ Show the report file"
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.dream_ui.raw_open.remove(&key) {
                        this.dream_ui.raw_open.insert(key.clone());
                    }
                    cx.notify();
                })),
        );
        if open {
            let text = match self.dream_ui.reports.get(path) {
                Some(Ok(t)) => visible(t),
                Some(Err(e)) => e.clone(),
                None => String::new(),
            };
            column = column
                .child(
                    div()
                        .text_size(look.text(11.))
                        .text_color(look.c.text_muted)
                        .child(visible(&path.display().to_string())),
                )
                .child(
                    div()
                        .pt(look.px(4.))
                        .font_family("Menlo")
                        .text_size(look.text(12.))
                        .line_height(gpui::relative(1.45))
                        .child(text),
                );
        }
        column.into_any_element()
    }
}

/// `2 changes · 1 rejected · 3 notes checked`; `Failed · …` for a run that
/// did not finish. A run that finished says what it did, not "ok", so that
/// "ok" never stands next to a rejection or a validation failure.
pub fn headline(f: &RunFacts) -> String {
    if f.failed {
        format!("Failed · {}", f.details())
    } else if f.dry_run {
        format!("Dry run, wrote nothing · {}", f.details())
    } else {
        let mut s = f.outcome();
        if let Some(c) = s.get_mut(0..1) {
            c.make_ascii_uppercase();
        }
        format!("{s} · {}", f.details())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facts_drop_the_parts_that_say_nothing() {
        assert_eq!(
            short_facts("2 sessions, 1 notes, 172 turns, 0 human-confirmed"),
            "2 sessions, 172 turns"
        );
        assert_eq!(
            short_facts("1 sessions, 1 notes, 26 turns, 1 human-confirmed"),
            "1 session, 26 turns, 1 human-confirmed"
        );
        assert_eq!(short_facts("no note to work on"), "no note to work on");
    }

    #[test]
    fn a_headline_counts_what_the_run_did() {
        let f = RunFacts {
            applied: 2,
            rejected: 1,
            checked: 3,
            ..RunFacts::default()
        };
        assert_eq!(headline(&f), "2 changes · 1 rejected · 3 notes checked");
        let failed = RunFacts {
            failed: true,
            checked: 1,
            ..RunFacts::default()
        };
        assert_eq!(headline(&failed), "Failed · 1 note checked");
        let undone = RunFacts {
            applied: 1,
            reverted: 1,
            checked: 2,
            ..RunFacts::default()
        };
        assert_eq!(headline(&undone), "1 change · 1 reverted · 2 notes checked");
    }
}
