//! A note's tab, the note page: its kicker, title and facts, its sections
//! with their sizes and dates, what dreaming did with it, who links here and
//! where it links, the sessions that read or edited it, its lint findings,
//! its change receipts and its body with `[[links]]` that open notes.
//!  and "Look and first
//! run".

use std::path::PathBuf;

use std::rc::Rc;

use gpui::{Context, Role, SharedString, div, prelude::*};
use hippocamp_core::records::Verdict;
use hippocamp_view::brain::{BODY_LIMIT, NoteView};
use hippocamp_view::notepage::NotePage;
use hippocamp_view::text::visible;

use super::Shell;
use super::kit;
use super::markdown::{NoteLinks, markdown_element};
use super::theme::Look;
use crate::state::Tab;

/// The body as Markdown for [`markdown_element`]: each `[[link]]` becomes
/// `[label](hippocamp-note:<n>)`, `n` indexing the returned targets, or
/// `[label](hippocamp-dangling:0)` when it names no note.
pub fn markdown_source(view: &NoteView) -> (String, Vec<PathBuf>) {
    let mut text = String::new();
    let mut targets = Vec::new();
    let mut at = 0;
    let mut spans = view.links.clone();
    spans.sort_by_key(|(r, _)| r.start);
    for (range, target) in spans {
        if range.start < at || range.end > view.body.len() {
            continue;
        }
        text.push_str(&view.body[at..range.start]);
        let raw = &view.body[range.clone()];
        let inner = raw
            .strip_prefix("[[")
            .and_then(|r| r.strip_suffix("]]"))
            .unwrap_or(raw);
        // `[[target|label]]` shows its label.
        let label: String = inner
            .split_once('|')
            .map_or(inner, |(_, l)| l)
            .chars()
            .filter(|c| !matches!(c, '[' | ']'))
            .collect();
        match target {
            Some(t) => {
                text.push_str(&format!("[{label}](hippocamp-note:{})", targets.len()));
                targets.push(t);
            }
            None => text.push_str(&format!("[{label}](hippocamp-dangling:0)")),
        }
        at = range.end;
    }
    text.push_str(&view.body[at..]);
    (text, targets)
}

/// A lint finding as what it means and what to do.
pub fn lint_sentence(f: &hippocamp_core::notes::LintFinding, today: time::Date) -> String {
    use hippocamp_core::notes::LintFinding as L;
    match f {
        L::Overdue { review_after, .. } => format!(
            "{}: re-check it, then move review_after forward.",
            super::attention_tab::overdue_text(*review_after, today)
                .replacen("review", "Review", 1)
        ),
        L::MissingFrontmatter { .. } => {
            "No frontmatter: add type, status, updated and review_after so hippocamp can date it."
                .into()
        }
        L::DanglingLink { link, .. } => format!("[[{}]] names no note.", visible(link)),
        L::BadFrontmatter { message, .. } => format!("Frontmatter: {}", visible(message)),
        L::Error { message, .. } => format!("Frontmatter does not parse: {}", visible(message)),
    }
}

impl Shell {
    pub(super) fn render_note_tab(
        &self,
        path: &PathBuf,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let page_root = div()
            .id("note-tab")
            .role(Role::Article)
            .size_full()
            .overflow_y_scroll()
            .px(look.px(24.))
            .pt(look.px(18.))
            .pb(look.px(24.))
            .text_size(look.text(13.));
        let view = match self.note_views.get(path) {
            Some(Ok(view)) => view.clone(),
            Some(Err(e)) => {
                return page_root
                    .child(div().text_color(look.c.error).child(e.clone()))
                    .into_any_element();
            }
            None => {
                return page_root
                    .child(div().text_color(look.c.text_muted).child("Reading…"))
                    .into_any_element();
            }
        };
        let page = self.note_pages.get(path).cloned().flatten();
        let title = self.tab_title(&Tab::Note(path.clone()));

        let (source, targets) = markdown_source(&view);
        let entity = cx.entity().downgrade();
        let open_note: NoteLinks = Rc::new(move |n, window, cx| {
            let Some(target) = targets.get(n).cloned() else {
                return;
            };
            entity
                .update(cx, |this, cx| {
                    // A followed link shows its note in the preview tab.
                    this.state.tabs.follow(Tab::Note(target), false);
                    this.after_state_change(window, cx);
                })
                .ok();
        });
        let text = markdown_element(
            &format!("note-body-{}", path.display()),
            &source,
            look,
            Some(&open_note),
        );
        let file = self.active_file();
        let dim = look.c.text_muted.opacity(0.72);

        let mut body = div()
            .flex()
            .flex_col()
            .flex_basis(look.px(580.))
            .flex_shrink(1.)
            .min_w(look.px(320.))
            .gap(look.px(4.));
        if let Some(p) = &page
            && !p.kicker.is_empty()
        {
            // Sentence case, muted: the frontmatter is context, not a shout.
            let mut kicker = p.kicker.join(" · ");
            if let Some(c) = kicker.get(..1) {
                kicker = c.to_uppercase() + &kicker[1..];
            }
            body = body.child(
                div()
                    .text_size(look.text(11.5))
                    .text_color(look.c.text_muted)
                    .child(kicker),
            );
        }
        // Health as a chip beside the title.
        let dangling = page.as_ref().map_or(0, |p| p.dangling);
        let overdue = view
            .lint
            .iter()
            .any(|f| matches!(f, hippocamp_core::notes::LintFinding::Overdue { .. }));
        let findings = view.lint.len() + dangling;
        let (health, health_color) = if overdue {
            ("review overdue".to_owned(), look.c.warning)
        } else if findings > 0 {
            (
                format!("{findings} finding{}", if findings == 1 { "" } else { "s" }),
                look.c.warning,
            )
        } else {
            ("fresh".to_owned(), kit::ok(look))
        };
        body = body
            .child(
                // The buttons go under a title they would squeeze.
                div()
                    .flex()
                    .flex_wrap()
                    .items_start()
                    .gap(look.px(12.))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(look.px(10.))
                            .flex_1()
                            .min_w(look.px(220.))
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_size(look.text(20.))
                                    .line_height(look.px(26.))
                                    .child(title),
                            )
                            .child(kit::dot_pill(look, health, health_color)),
                    )
                    .when_some(file, |d, file| d.child(self.file_buttons(file, look, cx))),
            )
            .child(match &page {
                Some(p) => {
                    let mut line = String::new();
                    let mut highlights = Vec::new();
                    for (i, (f, strong)) in p.facts.iter().enumerate() {
                        if i > 0 {
                            line.push_str(" · ");
                        }
                        let at = line.len();
                        line.push_str(f);
                        if *strong {
                            highlights.push((
                                at..line.len(),
                                gpui::HighlightStyle {
                                    color: Some(look.c.text_muted),
                                    font_weight: Some(gpui::FontWeight::MEDIUM),
                                    ..Default::default()
                                },
                            ));
                        }
                    }
                    div()
                        .text_size(look.text(11.))
                        .text_color(dim)
                        .child(gpui::StyledText::new(line).with_highlights(highlights))
                        .into_any_element()
                }
                None => {
                    let mut facts = vec![visible(&view.path.display().to_string())];
                    facts.extend(view.frontmatter.iter().map(|(k, v)| format!("{k} {v}")));
                    div()
                        .text_size(look.text(11.))
                        .text_color(look.c.text_muted)
                        .child(facts.join(" · "))
                        .into_any_element()
                }
            })
            .children(
                page.as_ref()
                    .and_then(|p| p.alert.clone())
                    // A status sentence, muted; warnings are for
                    // what is wrong.
                    .map(|a| div().pt(look.px(8.)).text_color(look.c.text_muted).child(a)),
            )
            .children(self.note_stat_cards(path, page.as_ref(), look))
            .children(self.note_status(&view, look))
            .children(page.as_ref().map(|p| self.section_table(p, look)))
            .children(self.receipt_rows(&view, look, cx))
            .child(
                div()
                    .pt(look.px(16.))
                    .mt(look.px(8.))
                    .border_t(look.fixed(1.))
                    .border_color(look.c.border_variant)
                    .text_color(look.c.text)
                    .child(text),
            )
            .when(view.cut, |d| {
                d.child(
                    div()
                        .pt(look.px(8.))
                        .text_color(look.c.text_muted)
                        .child(format!(
                            "Showing the first {} KB. Open in Editor (⌘O) for the rest.",
                            BODY_LIMIT / 1024
                        )),
                )
            });

        // The side column goes under the note when the pane is narrow.
        page_root
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_x(look.px(32.))
                    .gap_y(look.px(20.))
                    .child(body)
                    .children(page.as_ref().map(|p| self.note_side(p, look, cx))),
            )
            .into_any_element()
    }

    /// The note's sections: heading, a size bar, size and the newest date
    /// each names.
    fn section_table(&self, p: &NotePage, look: &Look) -> gpui::AnyElement {
        let dim = look.c.text_muted.opacity(0.72);
        let head = |t: &'static str| {
            div()
                .text_size(look.text(11.))
                .text_color(dim)
                .child(super::kit::sentence(&t.to_uppercase()))
        };
        let mut table = div().flex().flex_col().pt(look.px(14.)).child(
            div()
                .flex()
                .items_center()
                .gap(look.px(12.))
                .pb(look.px(4.))
                .border_b(look.fixed(1.))
                .border_color(look.c.border)
                .child(head("Section").flex_1())
                .child(head("Size").w(look.px(56.)).flex().justify_end())
                .child(head("Last dated").w(look.px(76.)).flex().justify_end()),
        );
        let largest = p.largest.max(1) as f32;
        for s in &p.sections {
            let share = (s.bytes as f32 / largest).clamp(0.02, 1.0);
            table = table.child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(12.))
                    .py(look.px(2.))
                    .border_b(look.fixed(1.))
                    .border_color(look.c.border_variant)
                    .child(
                        div()
                            .flex_1()
                            .min_w(look.px(80.))
                            .truncate()
                            .child(s.heading.clone()),
                    )
                    .child(
                        // The bar gives way first when the pane is narrow.
                        div()
                            .w(look.px(150.))
                            .flex_shrink(1.)
                            .min_w(look.px(32.))
                            .h(look.px(5.))
                            .rounded(look.fixed(1.))
                            .bg(look.c.border_variant)
                            .child(
                                div()
                                    .h_full()
                                    .w(gpui::relative(share))
                                    .rounded(look.fixed(1.))
                                    .bg(look.c.text_muted.opacity(0.55)),
                            ),
                    )
                    .child(
                        div()
                            .w(look.px(56.))
                            .flex()
                            .justify_end()
                            .font_family("Menlo")
                            .text_size(look.text(11.))
                            .text_color(dim)
                            .child(hippocamp_view::digest::size(s.bytes as u64)),
                    )
                    .child(
                        div()
                            .w(look.px(56.))
                            .flex()
                            .justify_end()
                            .text_size(look.text(11.))
                            .text_color(look.c.text_muted)
                            .child(s.dated.clone().unwrap_or_else(|| "—".to_owned())),
                    ),
            );
        }
        table.into_any_element()
    }

    /// The right column: dreaming, links in and out, and the sessions that
    /// read or edited the note.
    fn note_side(&self, p: &NotePage, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let dim = look.c.text_muted.opacity(0.72);
        let section = |title: String| {
            div().flex().flex_col().mb(look.px(16.)).child(
                div()
                    .mb(look.px(4.))
                    .pb(look.px(4.))
                    .border_b(look.fixed(1.))
                    .border_color(look.c.border_variant)
                    .text_size(look.text(11.))
                    .text_color(dim)
                    .child(title.clone()),
            )
        };
        let mut col = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(look.px(220.))
            .text_size(look.text(12.))
            .line_height(look.px(17.5));

        let mut from = section(format!("Linked from · {}", p.linked_from.len()));
        for (i, n) in p.linked_from.iter().take(15).enumerate() {
            let path = n.path.clone();
            from = from.child(
                div()
                    .id(SharedString::from(format!("linked-from-{i}")))
                    .role(Role::Link)
                    .flex()
                    .items_baseline()
                    .min_w_0()
                    .py(look.px(2.))
                    .cursor_pointer()
                    // One link style, the text colour, underlined
                    // on hover.
                    .child(
                        div()
                            .flex_shrink(1.)
                            .min_w_0()
                            .truncate()
                            .text_color(look.c.text)
                            .hover(|h| h.underline())
                            .child(n.title.clone()),
                    )
                    .children(n.folder.clone().map(|f| {
                        div()
                            .flex_shrink_0()
                            .pl(look.px(6.))
                            .text_size(look.text(11.))
                            .text_color(dim)
                            .child(f)
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.state.tabs.follow(Tab::Note(path.clone()), false);
                        this.after_state_change(window, cx);
                    })),
            );
        }
        if p.linked_from.len() > 15 {
            from = from.child(
                div()
                    .text_color(dim)
                    .child(format!("{} more", p.linked_from.len() - 15)),
            );
        }
        if p.linked_from.is_empty() {
            from = from.child(
                div()
                    .text_color(look.c.text_muted)
                    .child("No note links here: an island."),
            );
        }
        col = col.child(from);

        let mut to = section(format!("Links to · {}", p.links_to.len()));
        let mut names = div().flex().flex_wrap().text_color(look.c.text);
        for (i, n) in p.links_to.iter().enumerate() {
            let path = n.path.clone();
            let last = i + 1 == p.links_to.len();
            names = names.child(
                div()
                    .id(SharedString::from(format!("links-to-{i}")))
                    .role(Role::Link)
                    .cursor_pointer()
                    .hover(|h| h.underline())
                    .child(if last {
                        n.title.clone()
                    } else {
                        format!("{}, ", n.title)
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.state.tabs.follow(Tab::Note(path.clone()), false);
                        this.after_state_change(window, cx);
                    })),
            );
        }
        to = to.child(names);
        if p.dangling > 0 {
            to = to.child(div().pt(look.px(2.)).text_color(look.c.warning).child(
                crate::rows::count(p.dangling, "link names no note", "links name no note"),
            ));
        }
        col = col.child(to);

        if p.reader_count > 0 {
            let mut readers = section(format!("Read and edited by · {}", p.reader_count));
            for (i, r) in p.readers.iter().enumerate() {
                let id = r.id.clone();
                readers = readers.child(
                    div()
                        .id(SharedString::from(format!("reader-{i}")))
                        .role(Role::Link)
                        .flex()
                        .items_baseline()
                        .min_w_0()
                        .py(look.px(2.))
                        .cursor_pointer()
                        .child(
                            div()
                                .flex_shrink(1.)
                                .min_w_0()
                                .truncate()
                                .child(r.title.clone()),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .pl(look.px(6.))
                                .text_size(look.text(11.))
                                .text_color(dim)
                                .child(format!("{} · {}", r.kind, r.when)),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.state.tabs.open(Tab::Session {
                                id: id.clone(),
                                ordinal: None,
                            });
                            this.after_state_change(window, cx);
                        })),
                );
            }
            col = col.child(readers);
        }
        col.into_any_element()
    }

    /// Under the title: each lint finding as what to do, and what the
    /// newest dreaming run did with this note.
    fn note_status(&self, view: &NoteView, look: &Look) -> Vec<gpui::AnyElement> {
        let today = self.today();
        let mut out: Vec<gpui::AnyElement> = view
            .lint
            .iter()
            .map(|f| {
                div()
                    .pt(look.px(4.))
                    .text_color(if f.is_error() {
                        look.c.error
                    } else {
                        look.c.warning
                    })
                    .child(lint_sentence(f, today))
                    .into_any_element()
            })
            .collect();
        if let Some(line) = self
            .note_pages
            .get(&view.path)
            .is_none_or(Option::is_none)
            .then(|| self.dreaming_line(&view.path))
            .flatten()
        {
            out.push(
                div()
                    .pt(look.px(4.))
                    .text_size(look.text(11.))
                    .text_color(look.c.text_muted)
                    .child(line)
                    .into_any_element(),
            );
        }
        out
    }

    /// What the newest dreaming run did with the note at `path`, when it
    /// looked at it.
    fn dreaming_line(&self, path: &std::path::Path) -> Option<String> {
        let run = self
            .loaded
            .as_ref()?
            .panels
            .as_ref()?
            .dreaming
            .as_ref()
            .ok()?
            .runs
            .first()?;
        let d = run.digest.as_ref()?;
        let key = path.display().to_string();
        let when = crate::rows::run_facts(run).when_label(super::status::local_offset());
        let changed = run.changes.iter().filter(|c| {
            c.proposal
                .record
                .as_ref()
                .is_ok_and(|p| p.note.as_path() == path)
        });
        let changed = changed.count();
        let failed: Vec<&str> = d
            .failures
            .iter()
            .filter(|f| f.note.as_deref() == Some(key.as_str()))
            .map(|f| f.reason.as_str())
            .collect();
        let skipped = d.skipped.iter().find(|s| s.note == key);
        let checked = d.notes_checked().contains(&key.as_str());
        let what = if changed > 0 {
            format!(
                "proposed {}",
                crate::rows::count(changed, "change", "changes")
            )
        } else if let Some(reason) = failed.first() {
            format!("its proposal failed validation ({reason}); nothing written")
        } else if let Some(s) = skipped {
            format!("left it out: {}", s.why)
        } else if checked {
            "checked it; no change".into()
        } else {
            return None;
        };
        Some(format!("Dreaming, {when}: {what}."))
    }

    /// The note's change receipts, each opening its proposal.
    fn receipt_rows(
        &self,
        view: &NoteView,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        if view.receipts.is_empty() {
            return Vec::new();
        }
        let mut out = vec![Self::section_heading(look, "Changes by dreaming").into_any_element()];
        // Newest first.
        let mut receipts = view.receipts.clone();
        receipts.reverse();
        for r in &receipts {
            let verdict = match r.verdict {
                Verdict::Confirm => "confirmed",
                Verdict::Reject => "rejected",
                Verdict::Revert => "reverted",
            };
            let when = r
                .applied
                .map(|t| format!("applied {}", t.date()))
                .unwrap_or_else(|| format!("{}", r.at.date()));
            let proposal = r.proposal;
            out.push(
                div()
                    .id(SharedString::from(format!("receipt-{}", r.id)))
                    .role(Role::Link)
                    .flex()
                    .flex_col()
                    .justify_center()
                    .h(look.px(40.))
                    .px(look.px(8.))
                    .mx(look.px(-8.))
                    .rounded(look.px(4.))
                    .cursor_pointer()
                    .hover(|h| h.bg(look.c.hover))
                    .child(div().child(format!("{verdict} · {when}")))
                    .child(
                        div()
                            .text_size(look.text(11.))
                            .text_color(look.c.text_muted)
                            .child(format!("receipt {} · proposal {}", r.id, r.proposal)),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.state.tabs.open(Tab::Proposal(proposal));
                        this.after_state_change(window, cx);
                    }))
                    .into_any_element(),
            );
        }
        out
    }

    /// Open in Editor and Reveal in Finder, as small buttons.
    pub(super) fn file_buttons(
        &self,
        file: PathBuf,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // The standard button size, the key inside the button.
        let button = |id: &'static str, label: &'static str, keys: &'static str| {
            kit::button(look, id, format!("{label}  {keys}"), kit::Kind::Plain).aria_label(label)
        };
        let reveal = file.clone();
        div()
            .flex()
            .flex_shrink_0()
            .gap(look.px(6.))
            .child(
                button("open-in-editor", "Open in editor", "⌘O")
                    .on_click(cx.listener(move |_, _, _, cx| cx.open_with_system(&file))),
            )
            .child(
                button("reveal-in-finder", "Reveal in Finder", "⇧⌘R")
                    .on_click(cx.listener(move |_, _, _, cx| cx.reveal_path(&reveal))),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ops::Range;

    fn view(body: &str, links: Vec<(Range<usize>, Option<PathBuf>)>) -> NoteView {
        NoteView {
            path: PathBuf::from("notes/a.md"),
            frontmatter: Vec::new(),
            body: body.into(),
            cut: false,
            links,
            lint: Vec::new(),
            receipts: Vec::new(),
        }
    }

    #[test]
    fn links_become_note_links_and_dangling_ones_are_marked() {
        let text = "See [[Deploy notes]] and [[Gone|the old one]].";
        let deploy = text.find("[[Deploy").unwrap();
        let gone = text.find("[[Gone").unwrap();
        let v = view(
            text,
            vec![
                (
                    deploy..deploy + "[[Deploy notes]]".len(),
                    Some(PathBuf::from("notes/Deploy notes.md")),
                ),
                (gone..gone + "[[Gone|the old one]]".len(), None),
            ],
        );
        let (source, targets) = markdown_source(&v);
        assert_eq!(
            source,
            "See [Deploy notes](hippocamp-note:0) and [the old one](hippocamp-dangling:0)."
        );
        assert_eq!(targets, vec![PathBuf::from("notes/Deploy notes.md")]);
        let spans = hippocamp_view::markdown::inline(&source);
        assert_eq!(spans[1].link.as_deref(), Some("hippocamp-note:0"));
    }
}
