//! The overnight dreaming changes no one has marked (`hippo_view::home`'s
//! review), each with Looks right (`hippo review <receipt> --ok`) and Undo
//! (the change, whose Revert asks first), and the recent dreaming runs. The
//! Inbox itself, with the lessons, is `lessons.rs`.

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    Context, FontWeight, HighlightStyle, Hsla, IndentAdjustment, LineFragment, LineWrapper, Pixels,
    Role, StyledText, canvas, div, prelude::*,
};
use hippo_view::home::{Need, ReviewItem};
use hippo_view::worddiff::{Mark, layout};

use super::Shell;
use super::kit::{self, Kind};
use super::theme::Look;
use crate::flow::Purpose;
use crate::state::Tab;

fn dim(look: &Look) -> Hsla {
    look.c.text_muted.opacity(0.75)
}

/// How many lines a review row's text may take; one more in a narrow
/// pane, where two lines hold only a few words.
const REVIEW_LINES: usize = 2;
const NARROW_LINES: usize = 3;
/// The width of a review row's two actions, so that they are never cut.
const ACTIONS_WIDTH: f32 = 150.;
/// Below this text width a review row is narrow.
const NARROW: f32 = 420.;

/// The lines `text` wraps to at `width`.
fn lines(wrapper: &mut LineWrapper, text: &str, width: Pixels) -> usize {
    wrapper
        .wrap_line(
            &[LineFragment::text(text)],
            width,
            IndentAdjustment::NoIndent,
        )
        .count()
        + 1
}

/// `text`'s first `n` words, without trailing punctuation, and an ellipsis;
/// `text` itself when it has no more than `n`.
fn words(text: &str, n: usize) -> String {
    let all: Vec<&str> = text.split(' ').collect();
    if all.len() <= n {
        return text.to_owned();
    }
    let cut = all[..n].join(" ");
    format!(
        "{}…",
        cut.trim_end_matches(|c: char| c.is_whitespace() || ",;:–—-(".contains(c))
    )
}

/// The most words of `text` (at least one) for which `make` still fits.
fn fit(text: &str, fits: &mut dyn FnMut(&str) -> bool) -> Option<String> {
    let n = text.split(' ').count();
    if fits(text) {
        return Some(text.to_owned());
    }
    // The largest k in 1..n with words(text, k) fitting.
    let (mut lo, mut hi) = (1, n - 1);
    if hi < lo || !fits(&words(text, lo)) {
        return None;
    }
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if fits(&words(text, mid)) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    Some(words(text, lo))
}

/// Excerpts of a change's word diff to try, from the most shared text
/// around the changes to the least: context words on each side, changes
/// shown, words per side.
const EXCERPTS: [(usize, usize, usize); 9] = [
    (4, 3, 40),
    (3, 2, 24),
    (3, 1, 16),
    (2, 1, 12),
    (1, 1, 8),
    (1, 1, 5),
    (0, 1, 4),
    (0, 1, 2),
    (0, 1, 1),
];

/// A review row's change and the spans to strike (removed words) or mark
/// (words put in): the largest excerpt of what changed, word by word, for
/// which `head`, the change and `tail` fit in [`REVIEW_LINES`] lines of
/// `width` ([`NARROW_LINES`] when narrow). An addition, or a change of no
/// words (markup only), shows as quotes cut to fit instead. A width of zero
/// (not measured yet) takes a middling excerpt.
pub(crate) fn fit_change(
    wrapper: &mut LineWrapper,
    width: Pixels,
    head: &str,
    item: &ReviewItem,
    tail: &str,
    one_line: bool,
) -> (String, Vec<(Range<usize>, Mark)>) {
    let most = if one_line {
        1
    } else if width < Pixels::from(NARROW) {
        NARROW_LINES
    } else {
        REVIEW_LINES
    };
    let mut fits = |change: &str| lines(wrapper, &format!("{head}{change}{tail}"), width) <= most;
    if let Some(diff) = item.diff.as_ref().filter(|d| d.changed()) {
        let excerpt = |(c, n, w): (usize, usize, usize)| layout(&diff.excerpt(c, n, w));
        if width <= Pixels::ZERO {
            return excerpt(EXCERPTS[3]);
        }
        return EXCERPTS
            .iter()
            .map(|&e| excerpt(e))
            .find(|(text, _)| fits(text))
            .unwrap_or_else(|| excerpt(EXCERPTS[EXCERPTS.len() - 1]));
    }
    if width <= Pixels::ZERO {
        return (item.change.clone(), Vec::new());
    }
    (fit_quotes(&mut fits, item), Vec::new())
}

/// An addition, or a change of no words, as quotes cut at words to fit:
/// the new text first, then the old.
fn fit_quotes(fits: &mut dyn FnMut(&str) -> bool, item: &ReviewItem) -> String {
    match &item.old {
        None => {
            let shown = fit(&item.new, &mut |n| fits(&format!("added “{n}”")));
            format!("added “{}”", shown.unwrap_or_else(|| words(&item.new, 1)))
        }
        Some(old) => {
            if let Some(n) = fit(&item.new, &mut |n| fits(&format!("“{old}” → {n}"))) {
                return format!("“{old}” → {n}");
            }
            let o = fit(old, &mut |o| fits(&format!("“{o}” → …"))).unwrap_or_else(|| words(old, 1));
            format!("“{o}” → …")
        }
    }
}

impl Shell {
    /// The applied dreaming changes no one has marked.
    pub(crate) fn review_items(&self) -> Vec<ReviewItem> {
        self.loaded
            .as_ref()
            .and_then(|l| l.home.as_ref())
            .map(|h| {
                h.needs
                    .iter()
                    .filter_map(|n| match n {
                        Need::Review { items, .. } => Some(items.clone()),
                        _ => None,
                    })
                    .flatten()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether `hippo review` is running.
    /// Whether `hippo review` is running.
    pub(crate) fn reviewing_now(&self) -> bool {
        self.reviewing()
    }

    fn reviewing(&self) -> bool {
        self.flow.job.as_ref().is_some_and(|j| {
            j.purpose == Purpose::Review && j.result.lock().map(|r| r.is_none()).unwrap_or(false)
        })
    }

    /// Runs `hippo review <receipts> --ok`, shown before it runs.
    pub(crate) fn mark_right(&mut self, receipts: Vec<String>, cx: &mut Context<Self>) {
        if self.reviewing() || receipts.is_empty() {
            return;
        }
        let mut args = vec!["review".to_owned()];
        args.extend(receipts);
        args.push("--ok".to_owned());
        self.flow.log.clear();
        self.flow.review = None;
        self.start_job(Purpose::Review, vec![args], cx);
    }

    /// The review rows: each change's note, what it changed word by word,
    /// and Looks right and Undo in their own fixed space. `width` holds the
    /// rows' width at the last draw, which the quotes are fitted to;
    /// `one_line` fits each to one line (the Home's preview).
    pub(super) fn render_review_rows(
        &self,
        items: &[ReviewItem],
        width: &Rc<Cell<Pixels>>,
        one_line: bool,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let busy = self.reviewing();
        let measured = width.get();
        // Narrow rows get icon buttons, so that the change keeps room.
        let narrow = measured > Pixels::ZERO && measured < look.px(NARROW);
        let actions_width = if narrow { 64. } else { ACTIONS_WIDTH };
        let text_width = (measured - look.px(actions_width) - look.px(12.)).max(Pixels::ZERO);
        let font = gpui::font(
            look.font_family
                .clone()
                .unwrap_or_else(|| ".SystemUIFont".to_string()),
        );
        let mut wrapper = cx.text_system().line_wrapper(font, look.text(13.));
        let rows: Vec<gpui::AnyElement> = items
            .iter()
            .enumerate()
            .map(|(k, it)| {
                let note = it.note.clone();
                let proposal = it.proposal;
                let receipt = it.receipt.to_string();
                let label = it.label.clone();
                let head = format!("{label} · ");
                let tail = it
                    .edited_since
                    .as_ref()
                    .map(|e| format!(" · {e}"))
                    .unwrap_or_default();
                let (change, marks) =
                    fit_change(&mut wrapper, text_width, &head, it, &tail, one_line);
                // The note in the accent colour, what changed (the removed
                // words struck, the new ones in the text colour) with a
                // little shared text around it, and a note edited since in
                // amber. A click opens the note; the old and new text are
                // in the tooltip.
                let mut text = format!("{head}{change}");
                let mut highlights = vec![(
                    0..label.len(),
                    HighlightStyle {
                        color: Some(look.c.text_muted),
                        ..Default::default()
                    },
                )];
                for (r, mark) in marks {
                    let r = r.start + head.len()..r.end + head.len();
                    highlights.push((
                        r,
                        match mark {
                            Mark::Old => HighlightStyle {
                                color: Some(look.c.deleted),
                                background_color: Some(look.c.deleted_background),
                                strikethrough: Some(gpui::StrikethroughStyle {
                                    thickness: look.fixed(1.),
                                    color: Some(look.c.deleted),
                                }),
                                ..Default::default()
                            },
                            Mark::New => HighlightStyle {
                                color: Some(look.c.text),
                                font_weight: Some(FontWeight::MEDIUM),
                                ..Default::default()
                            },
                        },
                    ));
                }
                if !tail.is_empty() {
                    let start = text.len();
                    text.push_str(&tail);
                    highlights.push((
                        start..text.len(),
                        HighlightStyle {
                            color: Some(look.c.warning),
                            ..Default::default()
                        },
                    ));
                }
                let full = it.full.clone();
                let tip = match &it.edited_by {
                    Some(by) => format!("{full}\n\nEdited since in the session “{by}”."),
                    None => full,
                };
                let undo = it.undo.is_some();
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(12.))
                    .py(look.px(8.))
                    .when(k > 0, |d| {
                        d.border_t(look.fixed(1.))
                            .border_color(look.c.border_variant)
                    })
                    .child(
                        div()
                            .id(("review-note", k))
                            .role(Role::Link)
                            .aria_label(label.clone())
                            .flex_1()
                            .min_w_0()
                            .text_color(look.c.text_muted)
                            .text_size(look.text(13.))
                            .cursor_pointer()
                            .when(one_line, |d| d.truncate())
                            .child(StyledText::new(text).with_highlights(highlights))
                            .tooltip(super::tooltip::Tooltip::text(tip))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.state.tabs.open(Tab::Note(note.clone()));
                                this.after_state_change(window, cx);
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_shrink_0()
                            .justify_end()
                            .gap(look.px(6.))
                            .w(look.px(actions_width))
                            .child(
                                kit::button(
                                    look,
                                    ("review-right", k),
                                    match (busy, narrow) {
                                        (true, _) => "…",
                                        (false, true) => "✓",
                                        (false, false) => "Looks right",
                                    },
                                    if busy { Kind::Disabled } else { Kind::Plain },
                                )
                                .text_color(kit::ok(look))
                                .tooltip(super::tooltip::Tooltip::text(format!(
                                    "Runs hippo review {receipt} --ok"
                                )))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.mark_right(vec![receipt.clone()], cx);
                                })),
                            )
                            .child(
                                kit::button(
                                    look,
                                    ("review-undo", k),
                                    if narrow { "↶" } else { "Undo" },
                                    if undo { Kind::Plain } else { Kind::Disabled },
                                )
                                .tooltip(super::tooltip::Tooltip::text(if undo {
                                    "Open the change; its Revert asks first"
                                } else {
                                    "A newer change to this note must be undone first, or its snapshot is gone"
                                }))
                                .when(undo, |b| {
                                    b.on_click(cx.listener(move |this, _, window, cx| {
                                        this.state.tabs.open(Tab::Proposal(proposal));
                                        this.after_state_change(window, cx);
                                    }))
                                }),
                            ),
                    )
                    .into_any_element()
            })
            .collect();
        drop(wrapper);
        let width_cell = width.clone();
        let shell = cx.entity_id();
        let measure = canvas(
            move |bounds, _, cx| {
                if width_cell.get() != bounds.size.width {
                    width_cell.set(bounds.size.width);
                    // Draw again with the quotes fitted, after this frame:
                    // a notify inside a draw is lost.
                    cx.defer(move |cx| cx.notify(shell));
                }
            },
            |_, _, _, _| {},
        )
        .w_full()
        .h(Pixels::ZERO);
        div()
            .flex()
            .flex_col()
            .w_full()
            .child(measure)
            .children(rows)
            .into_any_element()
    }

    /// What the last Looks right did: its command and whether it worked.
    pub(super) fn review_result(&self, look: &Look) -> Option<gpui::Div> {
        let (line, result) = self.flow.review.as_ref()?;
        let (text, err) = match result {
            Ok(()) => (format!("{line}: done"), false),
            Err(e) => (e.clone(), true),
        };
        Some(
            div()
                .mt(look.px(6.))
                .font_family("Menlo")
                .text_size(look.text(11.))
                .truncate()
                .text_color(if err { look.c.error } else { dim(look) })
                .child(text),
        )
    }

    /// The recent dreaming runs, each opening its run tab.
    pub(super) fn runs_card(&self, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let runs: Vec<hippo_view::dreaming::Run> = self
            .loaded
            .as_ref()
            .and_then(|l| l.panels.as_ref())
            .and_then(|p| p.dreaming.as_ref().ok())
            .map(|d| d.runs.iter().take(6).cloned().collect())
            .unwrap_or_default();
        let run_rows = runs.iter().enumerate().map(|(i, run)| {
            let facts = crate::rows::run_facts(run);
            let path = run.report.path.clone();
            let when = run
                .written
                .map(|t| {
                    let t = time::OffsetDateTime::from(t).to_offset(super::status::local_offset());
                    format!(
                        "{} {:02}:{:02}",
                        crate::state::edition_title(t.date()),
                        t.hour(),
                        t.minute()
                    )
                })
                .unwrap_or_else(|| crate::state::report_title(&path));
            div()
                .id(("inbox-run", i))
                .role(Role::Link)
                .flex()
                .items_center()
                .gap(look.px(12.))
                .py(look.px(6.))
                .px(look.px(8.))
                .rounded(look.px(6.))
                .cursor_pointer()
                .hover(|h| h.bg(look.c.hover))
                .child(
                    div()
                        .flex_shrink_0()
                        .w(look.px(110.))
                        .text_color(look.c.text)
                        .child(when),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(look.c.text_muted)
                        .child(super::run_tab::headline(&facts)),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.state.tabs.open(Tab::Report(path.clone()));
                    this.after_state_change(window, cx);
                }))
        });
        kit::card(look)
            .flex_shrink_0()
            .p(look.px(14.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(10.))
                    .pb(look.px(4.))
                    .child(kit::label(look, "Dreaming runs", look.c.text_muted))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(dim(look))
                            .child("each opens what it checked, changed and left out"),
                    ),
            )
            .children(run_rows)
            .when(runs.is_empty(), |d| {
                d.child(
                    div()
                        .text_color(look.c.text_muted)
                        .child("No dreaming run yet."),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cut_keeps_whole_words() {
        assert_eq!(words("one two, three", 2), "one two…");
        assert_eq!(words("one two", 5), "one two");
        // Each word counts four characters; eleven fit.
        let mut fits = |t: &str| t.chars().count() <= 11;
        assert_eq!(
            fit("alpha beta gamma delta", &mut fits).as_deref(),
            Some("alpha beta…")
        );
        assert_eq!(fit("tiny", &mut fits).as_deref(), Some("tiny"));
        let mut never = |_: &str| false;
        assert_eq!(fit("alpha beta", &mut never), None);
    }
}
