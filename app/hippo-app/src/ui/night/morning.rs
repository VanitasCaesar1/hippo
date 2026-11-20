//! Morning: the last night pass, the four figures, the strip, and three
//! columns joined by lines (what you said, the rules drafted from it, what
//! the pass applied and what its reviewer rejected); below 900 points, a
//! feed. The panel is what a new session starts with.

use gpui::{AnyElement, Context, FontWeight, div, prelude::*};
use hippo_view::night::morning::{Fix, Morning, RuleCard, Said};

use super::kit::{self, Btn, Nt, Tone, Wire};
use super::{EDGE, Sel};
use crate::ui::Shell;
use crate::ui::theme::Look;

/// A piece of evidence's card height, in points.
pub const EV_H: f32 = 86.;
/// A rule card's height.
pub const RULE_H: f32 = 154.;
/// An applied change's card height: padding, the title row, `was` and
/// `now` at two lines each, and the receipt row.
pub const FIX_H: f32 = 214.;
/// A rejected change's card height: the title row, the diff and the
/// reviewer's reason at four lines.
pub const REJ_H: f32 = 262.;
/// Between cards in a stack, and between a rule's groups.
pub const GAP: f32 = 12.;
pub const GROUP_GAP: f32 = 26.;
/// The gap the evidence lines turn in.
pub const WIRE_GAP: f32 = 56.;
pub const COL_GAP: f32 = 28.;
/// The left column's inner padding.
pub const PAD: f32 = 28.;

/// Where Morning's columns go, in points from the columns' top left.
#[derive(Debug, Clone, PartialEq)]
pub struct Columns {
    /// `None` when the evidence column folds away (under 900 points of
    /// column width).
    pub evidence_x: Option<f32>,
    pub evidence_w: f32,
    pub rules_x: f32,
    pub rules_w: f32,
    pub fixes_x: f32,
    pub fixes_w: f32,
    /// Each rule's card top, and each of its evidence cards' tops.
    pub rules: Vec<(f32, Vec<f32>)>,
    pub height: f32,
}

/// Lays out the columns for `width` points.
pub fn columns(m: &Morning, width: f32) -> Columns {
    let three = width >= 900.;
    let (evidence_x, evidence_w, rules_x, rules_w) = if three {
        let ev = (width * 0.24).round();
        (Some(0.), ev, ev + WIRE_GAP, (width * 0.39).round())
    } else {
        (None, 0., 0., ((width - COL_GAP) * 0.55).round())
    };
    let fixes_x = rules_x + rules_w + COL_GAP;
    let fixes_w = (width - fixes_x).max(200.);
    let mut y = 0.;
    let mut rules = Vec::new();
    for r in &m.rules {
        let n = if three { r.evidence.len().min(4) } else { 0 };
        let ev_h = if n == 0 {
            0.
        } else {
            n as f32 * (EV_H + GAP) - GAP
        };
        let gh = ev_h.max(RULE_H);
        let rule_y = y + (gh - RULE_H) / 2.;
        let ev_top = y + (gh - ev_h) / 2.;
        let evs = (0..n).map(|k| ev_top + k as f32 * (EV_H + GAP)).collect();
        rules.push((rule_y, evs));
        y += gh + GROUP_GAP;
    }
    let left_h = (y - GROUP_GAP).max(0.);
    let fix_h = fixes_height(m);
    Columns {
        evidence_x,
        evidence_w,
        rules_x,
        rules_w,
        fixes_x,
        fixes_w,
        rules,
        height: left_h.max(fix_h),
    }
}

/// The applied and rejected column's height: its cards and the second
/// heading.
fn fixes_height(m: &Morning) -> f32 {
    let a = m.applied.len() as f32 * (FIX_H + GAP);
    let r = if m.rejected.is_empty() {
        0.
    } else {
        40. + m.rejected.len() as f32 * (REJ_H + GAP)
    };
    a + r
}

/// The lines from each piece of evidence to its rule, in points.
pub fn wire_points(c: &Columns) -> Vec<Vec<(f32, f32)>> {
    let Some(ex) = c.evidence_x else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (rule_y, evs) in &c.rules {
        let n = evs.len();
        for (k, ey) in evs.iter().enumerate() {
            let from = (ex + c.evidence_w, ey + EV_H / 2.);
            // Spread the ends along the rule card's left edge.
            let spread = if n <= 1 {
                0.
            } else {
                (k as f32 - (n - 1) as f32 / 2.) * 10.
            };
            let to = (c.rules_x, rule_y + RULE_H / 2. + spread);
            let turn = ex + c.evidence_w + WIRE_GAP * 0.45;
            out.push(vec![from, (turn, from.1), (turn, to.1), to]);
        }
    }
    out
}

impl Shell {
    pub(crate) fn render_morning(
        &mut self,
        look: &Look,
        t: &Nt,
        left_w: f32,
        narrow: bool,
        _panel_w: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(m) = self
            .loaded
            .as_ref()
            .and_then(|l| l.night.as_ref())
            .map(|n| (n.morning.clone(), n.strip.clone()))
        else {
            return self.render_night_empty(look, t, "Reading hippo's files…");
        };
        let (m, strip) = m;
        let pad = if narrow { 18. } else { PAD };
        let inner = left_w - 2. * pad;
        let head = div()
            .flex()
            .flex_col()
            .gap(look.px(8.))
            .child(
                div()
                    .text_size(look.text(13.5))
                    .text_color(t.muted)
                    .child(m.kicker.clone()),
            )
            .child(
                div()
                    .text_size(look.text(if narrow { 24. } else { 28. }))
                    .line_height(look.text(if narrow { 29. } else { 33. }))
                    .font_weight(FontWeight::BOLD)
                    .text_color(t.text)
                    .child(m.headline.clone()),
            )
            .child(
                div()
                    .text_size(look.text(15.))
                    .line_height(look.text(22.))
                    .text_color(t.muted)
                    .child(m.lead.clone()),
            );
        let figures = self.render_figures(look, t, &m, narrow || inner < 760.);
        let strip_el = self.render_strip(look, t, &strip, inner);
        let body = if narrow {
            self.render_morning_feed(look, t, &m, cx)
        } else {
            self.render_morning_columns(look, t, &m, inner, cx)
        };
        let tail = narrow.then(|| {
            div().mt(look.px(28.)).child(
                kit::card(look, t)
                    .rounded(look.px(18.))
                    .p(look.px(20.))
                    .child(self.render_start_panel(look, t, cx)),
            )
        });
        div()
            .flex()
            .flex_col()
            .gap(look.px(22.))
            .px(look.px(pad))
            .pt(look.px(26.))
            .child(head)
            .child(figures)
            .child(strip_el)
            .child(body)
            .children(tail)
            .into_any_element()
    }

    pub(crate) fn render_night_empty(&self, look: &Look, t: &Nt, text: &str) -> AnyElement {
        div()
            .p(look.px(PAD))
            .text_size(look.text(15.))
            .text_color(t.muted)
            .child(text.to_owned())
            .into_any_element()
    }

    /// B's four figures, in a row or two by two.
    fn render_figures(&self, look: &Look, t: &Nt, m: &Morning, two: bool) -> AnyElement {
        let f = &m.figures;
        let cell = |n: usize, words: &str, color| {
            kit::card(look, t)
                .flex_1()
                .min_w(look.px(150.))
                .flex()
                .flex_col()
                .gap(look.px(4.))
                .p(look.px(16.))
                .child(
                    div()
                        .font_family(kit::MONO)
                        .text_size(look.text(26.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color)
                        .child(n.to_string()),
                )
                .child(
                    div()
                        .text_size(look.text(13.))
                        .text_color(t.muted)
                        .child(words.to_owned()),
                )
        };
        let cells = [
            cell(
                f.rules_waiting,
                if f.rules_waiting == 1 {
                    "rule waiting for you"
                } else {
                    "rules waiting for you"
                },
                if f.rules_waiting > 0 { t.you } else { t.text },
            ),
            cell(
                f.notes_fixed,
                if f.notes_fixed == 1 {
                    "note fixed and confirmed"
                } else {
                    "notes fixed and confirmed"
                },
                if f.notes_fixed > 0 { t.ok } else { t.text },
            ),
            cell(
                f.fixes_rejected,
                if f.fixes_rejected == 1 {
                    "fix rejected by the second model"
                } else {
                    "fixes rejected by the second model"
                },
                if f.fixes_rejected > 0 { t.bad } else { t.text },
            ),
            cell(
                f.notes_checked,
                if f.notes_checked == 1 {
                    "note checked against what happened"
                } else {
                    "notes checked against what happened"
                },
                t.text,
            ),
        ];
        let [a, b, c, d] = cells;
        if two {
            div()
                .flex()
                .flex_col()
                .gap(look.px(10.))
                .child(div().flex().gap(look.px(10.)).child(a).child(b))
                .child(div().flex().gap(look.px(10.)).child(c).child(d))
                .into_any_element()
        } else {
            div()
                .flex()
                .gap(look.px(10.))
                .child(a)
                .child(b)
                .child(c)
                .child(d)
                .into_any_element()
        }
    }

    /// The strip and its three labels.
    fn render_strip(
        &self,
        look: &Look,
        t: &Nt,
        s: &hippo_view::night::strip::Strip,
        width: f32,
    ) -> AnyElement {
        let night_x = (s.x(s.night_start) * width).clamp(0., (width - 220.).max(0.));
        div()
            .flex()
            .flex_col()
            .gap(look.px(8.))
            .child(kit::strip(look, t, s, 54.))
            .child(
                div()
                    .relative()
                    .h(look.px(20.))
                    .text_size(look.text(13.5))
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .flex()
                            .gap(look.px(6.))
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(t.text)
                                    .child("Day"),
                            )
                            .child(div().text_color(t.muted).child(s.day.clone())),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(look.px(night_x))
                            .flex()
                            .gap(look.px(6.))
                            .text_color(t.night)
                            .child(div().font_weight(FontWeight::SEMIBOLD).child("Night"))
                            .child(s.night.clone()),
                    )
                    .child(
                        div()
                            .absolute()
                            .right_0()
                            .flex()
                            .gap(look.px(6.))
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(t.text)
                                    .child("Morning"),
                            )
                            .child(
                                div()
                                    .font_family(kit::MONO)
                                    .text_color(t.muted)
                                    .child(s.morning.clone()),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// The three columns, placed from [`columns`], with the lines drawn
    /// from the same numbers.
    fn render_morning_columns(
        &mut self,
        look: &Look,
        t: &Nt,
        m: &Morning,
        width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = columns(m, width);
        let unit = f32::from(look.px(1.));
        let heading = |x: f32, w: f32, text: &str, n: Option<(usize, Tone)>| {
            div()
                .absolute()
                .left(look.px(x))
                .top_0()
                .w(look.px(w))
                .child(kit::label(look, t, text, n))
        };
        let head_h = 34.;
        let mut area = div()
            .relative()
            .w(look.px(width))
            .h(look.px(c.height + head_h));
        if let Some(ex) = c.evidence_x {
            area = area.child(heading(
                ex,
                c.evidence_w,
                "Yesterday, in your sessions",
                None,
            ));
        }
        area = area
            .child(heading(
                c.rules_x,
                c.rules_w,
                "Waiting for you",
                Some((m.rules.len(), Tone::You)),
            ))
            .child(heading(
                c.fixes_x,
                c.fixes_w,
                "Applied overnight",
                Some((m.applied.len(), Tone::Plain)),
            ));
        // Lines first, under the cards.
        let you_line = t.you_line;
        let wires: Vec<Wire> = wire_points(&c)
            .into_iter()
            .map(|pts| Wire {
                points: pts
                    .into_iter()
                    .map(|(x, y)| (x * unit, (y + head_h) * unit))
                    .collect(),
                color: you_line,
                width: 1.5 * unit,
                rings: true,
            })
            .collect();
        area = area.child(kit::wires(wires, t.canvas));
        if m.rules.is_empty() {
            area = area.child(
                div()
                    .absolute()
                    .left(look.px(c.rules_x))
                    .top(look.px(head_h))
                    .w(look.px(c.rules_w))
                    .child(
                        kit::card(look, t)
                            .text_size(look.text(14.))
                            .text_color(t.muted)
                            .child("No rule waits for you. hippo drafts one when you correct an agent twice, or a command fails the same way three times."),
                    ),
            );
        }
        for (i, r) in m.rules.iter().enumerate() {
            let (rule_y, evs) = &c.rules[i];
            if let Some(ex) = c.evidence_x {
                for (k, ey) in evs.iter().enumerate() {
                    if let Some(e) = r.evidence.get(k) {
                        area = area.child(
                            div()
                                .absolute()
                                .left(look.px(ex))
                                .top(look.px(ey + head_h))
                                .w(look.px(c.evidence_w))
                                .h(look.px(EV_H))
                                .child(self.evidence_card(look, t, e)),
                        );
                    }
                }
            }
            area = area.child(
                div()
                    .absolute()
                    .left(look.px(c.rules_x))
                    .top(look.px(rule_y + head_h))
                    .w(look.px(c.rules_w))
                    .h(look.px(RULE_H))
                    .child(self.rule_card(look, t, r, i, c.evidence_x.is_none(), cx)),
            );
        }
        let mut y = head_h;
        if m.applied.is_empty() {
            area = area.child(
                div()
                    .absolute()
                    .left(look.px(c.fixes_x))
                    .top(look.px(y))
                    .w(look.px(c.fixes_w))
                    .child(
                        kit::card(look, t)
                            .text_size(look.text(14.))
                            .text_color(t.muted)
                            .child("Nothing was applied overnight."),
                    ),
            );
            y += 70.;
        }
        for (i, f) in m.applied.iter().enumerate() {
            area = area.child(
                div()
                    .absolute()
                    .left(look.px(c.fixes_x))
                    .top(look.px(y))
                    .w(look.px(c.fixes_w))
                    .h(look.px(FIX_H))
                    .child(self.fix_card(look, t, f, i, false, cx)),
            );
            y += FIX_H + GAP;
        }
        if !m.rejected.is_empty() {
            area = area.child(
                div()
                    .absolute()
                    .left(look.px(c.fixes_x))
                    .top(look.px(y + 8.))
                    .w(look.px(c.fixes_w))
                    .child(kit::label(
                        look,
                        t,
                        "Rejected by the second model",
                        Some((m.rejected.len(), Tone::Plain)),
                    )),
            );
            y += 40.;
            for (i, f) in m.rejected.iter().enumerate() {
                area = area.child(
                    div()
                        .absolute()
                        .left(look.px(c.fixes_x))
                        .top(look.px(y))
                        .w(look.px(c.fixes_w))
                        .h(look.px(REJ_H))
                        .child(self.fix_card(look, t, f, i, true, cx)),
                );
                y += REJ_H + GAP;
            }
        }
        area.into_any_element()
    }

    /// B's feed: rules with their evidence inside, then what was applied
    /// and rejected.
    fn render_morning_feed(
        &mut self,
        look: &Look,
        t: &Nt,
        m: &Morning,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut col = div().flex().flex_col().gap(look.px(12.));
        col = col.child(kit::label(
            look,
            t,
            "Waiting for you",
            Some((m.rules.len(), Tone::You)),
        ));
        for (i, r) in m.rules.iter().enumerate() {
            col = col.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(look.px(8.))
                    .child(
                        div()
                            .h(look.px(RULE_H))
                            .child(self.rule_card(look, t, r, i, true, cx)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(look.px(6.))
                            .pl(look.px(14.))
                            .children(
                                r.evidence
                                    .iter()
                                    .take(3)
                                    .map(|e| self.said_line(look, t, e)),
                            ),
                    ),
            );
        }
        col = col.child(div().h(look.px(8.))).child(kit::label(
            look,
            t,
            "Applied overnight",
            Some((m.applied.len(), Tone::Plain)),
        ));
        for (i, f) in m.applied.iter().enumerate() {
            col = col.child(
                div()
                    .h(look.px(FIX_H))
                    .child(self.fix_card(look, t, f, i, false, cx)),
            );
        }
        if !m.rejected.is_empty() {
            col = col.child(div().h(look.px(8.))).child(kit::label(
                look,
                t,
                "Rejected by the second model",
                Some((m.rejected.len(), Tone::Plain)),
            ));
            for (i, f) in m.rejected.iter().enumerate() {
                col = col.child(
                    div()
                        .h(look.px(REJ_H))
                        .child(self.fix_card(look, t, f, i, true, cx)),
                );
            }
        }
        col.into_any_element()
    }

    /// One line of evidence inside a feed card.
    fn said_line(&self, look: &Look, t: &Nt, e: &Said) -> AnyElement {
        div()
            .flex()
            .items_start()
            .gap(look.px(8.))
            .children(e.agent.as_ref().map(|a| kit::badge(look, a)))
            .child(
                div()
                    .flex_shrink_0()
                    .font_family(kit::MONO)
                    .text_size(look.text(12.5))
                    .text_color(t.faint)
                    .child(e.when.clone()),
            )
            .child(match (&e.said, &e.failed) {
                (Some(s), _) => kit::said(look, t, s, 2).into_any_element(),
                (None, Some(f)) => div()
                    .font_family(kit::MONO)
                    .text_size(look.text(12.5))
                    .text_color(t.bad)
                    .line_clamp(2)
                    .child(f.clone())
                    .into_any_element(),
                _ => div().into_any_element(),
            })
            .into_any_element()
    }

    fn evidence_card(&self, look: &Look, t: &Nt, e: &Said) -> AnyElement {
        let title = e
            .session_title
            .clone()
            .unwrap_or_else(|| e.session.chars().take(8).collect());
        kit::card(look, t)
            .size_full()
            .rounded(look.px(12.))
            .p(look.px(12.))
            .flex()
            .flex_col()
            .gap(look.px(6.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(8.))
                    .children(e.agent.as_ref().map(|a| kit::badge(look, a)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(look.text(13.5))
                            .text_color(t.muted)
                            .truncate()
                            .child(title),
                    )
                    .child(
                        div()
                            .font_family(kit::MONO)
                            .text_size(look.text(12.5))
                            .text_color(t.faint)
                            .child(e.when.clone()),
                    ),
            )
            .child(match (&e.said, &e.failed) {
                (Some(s), _) => div()
                    .flex()
                    .gap(look.px(6.))
                    .child(
                        div()
                            .text_size(look.text(13.))
                            .text_color(t.faint)
                            .child("you"),
                    )
                    .child(kit::said(look, t, s, 2))
                    .into_any_element(),
                (None, Some(f)) => div()
                    .font_family(kit::MONO)
                    .text_size(look.text(12.5))
                    .line_height(look.text(18.))
                    .text_color(t.bad)
                    .line_clamp(2)
                    .child(f.clone())
                    .into_any_element(),
                _ => div().into_any_element(),
            })
            .into_any_element()
    }

    fn rule_card(
        &mut self,
        look: &Look,
        t: &Nt,
        r: &RuleCard,
        i: usize,
        folded: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.night.morning == Some(Sel::Rule(r.id));
        let busy = self.inbox.busy == Some(r.id);
        let id = r.id;
        let failure = r.kind == "failure";
        let verdict = if r.agrees {
            kit::chip(look, t, "second model agrees", Tone::Ok)
        } else {
            kit::chip(look, t, "not reviewed", Tone::Plain)
        };
        let from = r.from.clone();
        let _ = folded;
        kit::select_card(look, t, selected)
            .id(("rule", i))
            .size_full()
            .p(look.px(16.))
            .flex()
            .flex_col()
            .gap(look.px(10.))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.night.morning = Some(Sel::Rule(id));
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(10.))
                    .child(kit::tile(
                        look,
                        t,
                        if failure { "x" } else { "said" },
                        Tone::You,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(look.text(14.))
                            .text_color(t.muted)
                            .child(from),
                    )
                    .child(verdict),
            )
            .child(
                div()
                    .flex_1()
                    .text_size(look.text(15.5))
                    .line_height(look.text(22.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.text)
                    .line_clamp(2)
                    .child(r.rule.clone()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(6.))
                    .child(
                        kit::button(
                            look,
                            t,
                            ("keep", i),
                            None,
                            if busy { "Keeping…" } else { "Keep" },
                            Btn::Go,
                            false,
                        )
                        .debug_selector(move || format!("keep-{i}"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.keep_lesson(id, None, cx);
                        })),
                    )
                    .child(
                        kit::button(look, t, ("edit", i), None, "Edit", Btn::Second, false)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.edit_lesson(id, window, cx);
                            })),
                    )
                    .child(
                        kit::button(look, t, ("dismiss", i), None, "Dismiss", Btn::Quiet, false)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.dismiss_lesson(id, cx);
                            })),
                    )
                    .child(div().flex_1())
                    .children(r.note_label.clone().map(|n| {
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(look.text(13.))
                            .text_color(t.faint)
                            .child(format!(
                                "into {}",
                                n.rsplit('/').next().unwrap_or(&n).to_owned()
                            ))
                    })),
            )
            .into_any_element()
    }

    fn fix_card(
        &mut self,
        look: &Look,
        t: &Nt,
        f: &Fix,
        i: usize,
        rejected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.night.morning == Some(Sel::Fix(f.proposal));
        let proposal = f.proposal;
        let undo = f.undo.clone();
        let receipt = f.receipt.map(|r| {
            let s = r.to_string();
            format!("{}…", &s[..10.min(s.len())])
        });
        let chip = if rejected {
            kit::chip(look, t, "rejected", Tone::Bad)
        } else if f.reverted {
            kit::chip(look, t, "undone", Tone::Plain)
        } else {
            kit::chip(look, t, "fixed last night", Tone::Ok)
        };
        let mut card = kit::select_card(look, t, selected)
            .id((if rejected { "rejected" } else { "fix" }, i))
            .size_full()
            .overflow_hidden()
            .p(look.px(16.))
            .flex()
            .flex_col()
            .gap(look.px(10.))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.night.morning = Some(Sel::Fix(proposal));
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(10.))
                    .child(kit::tile(
                        look,
                        t,
                        if rejected { "x" } else { "check" },
                        if rejected { Tone::Bad } else { Tone::Ok },
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(look.text(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.text)
                            .child(f.title.clone()),
                    )
                    .child(chip),
            )
            .child(kit::diff(look, t, f.was.as_deref(), &f.now));
        if rejected {
            card = card.child(kit::quote(look, t.bad, t.muted, &f.reason, 13.5).line_clamp(4));
        } else {
            card = card.child(div().flex_1()).child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(8.))
                    .child(kit::icon(look, "receipt", 14., t.faint))
                    .child(
                        div()
                            .flex_1()
                            .font_family(kit::MONO)
                            .text_size(look.text(12.5))
                            .text_color(t.faint)
                            .child(receipt.unwrap_or_default()),
                    )
                    .when_some(undo.filter(|_| !f.reverted), |d, args| {
                        d.child(
                            kit::button(
                                look,
                                t,
                                ("undo", i),
                                Some("undo"),
                                "Undo",
                                Btn::Second,
                                true,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.start_job(
                                        crate::flow::Purpose::Review,
                                        vec![args.clone()],
                                        cx,
                                    );
                                },
                            )),
                        )
                    }),
            );
        }
        card.into_any_element()
    }

    /// "Your agents now start with": the start block for a task.
    pub(crate) fn render_start_panel(
        &mut self,
        look: &Look,
        t: &Nt,
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(l) = self.loaded.as_ref() else {
            return div().into_any_element();
        };
        let waiting: Vec<String> = l
            .night
            .as_ref()
            .map(|n| n.morning.rules.iter().map(|r| r.rule.clone()).collect())
            .unwrap_or_default();
        let reaches = l
            .night
            .as_ref()
            .map(|n| n.reaches.clone())
            .unwrap_or_default();
        let kept = l.kept_rules.clone();
        let block = l.start_block.clone();
        let group = |text: &str, n: Option<usize>| {
            div().mt(look.px(16.)).mb(look.px(8.)).child(kit::label(
                look,
                t,
                text,
                n.map(|n| (n, Tone::Plain)),
            ))
        };
        let row = |icon: &str, title: String, right: String| {
            div()
                .flex()
                .items_center()
                .gap(look.px(10.))
                .h(look.px(40.))
                .px(look.px(12.))
                .mb(look.px(6.))
                .rounded(look.px(10.))
                .bg(t.inset)
                .child(kit::icon(look, icon, 15., t.muted))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(look.text(14.))
                        .text_color(t.text)
                        .child(title),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(look.text(13.))
                        .text_color(t.faint)
                        .child(right),
                )
        };
        let bullet = |text: String, filled: bool| {
            div()
                .flex()
                .items_start()
                .gap(look.px(10.))
                .mb(look.px(8.))
                .child(
                    div()
                        .mt(look.px(6.))
                        .size(look.px(8.))
                        .flex_shrink_0()
                        .rounded(look.px(2.))
                        .when(filled, |d| d.bg(t.you))
                        .when(!filled, |d| d.border(look.fixed(1.5)).border_color(t.you)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(look.text(14.))
                        .line_height(look.text(20.))
                        .text_color(t.text)
                        .child(text),
                )
        };
        let mut p = div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(10.))
                    .child(kit::tile(look, t, "start", Tone::Night))
                    .child(
                        div()
                            .text_size(look.text(13.5))
                            .text_color(t.muted)
                            .child("Start block"),
                    ),
            )
            .child(
                div()
                    .mt(look.px(14.))
                    .text_size(look.text(22.))
                    .line_height(look.text(27.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(t.text)
                    .child("Your agents now start with"),
            );
        if let Some((task, b)) = block {
            let on: Vec<&hippo_core::brief::Part> = b.parts.iter().collect();
            let bytes: usize = on.iter().map(|p| p.bytes).sum();
            p = p.child(
                div()
                    .mt(look.px(10.))
                    .text_size(look.text(14.))
                    .line_height(look.text(21.))
                    .text_color(t.muted)
                    .child(format!(
                        "Before its first prompt, every new session gets the notes its task needs: about {} of a {} budget, not the whole pile.",
                        hippo_view::memory::human(bytes as u64),
                        hippo_view::memory::human(b.budget as u64)
                    )),
            );
            if !task.is_empty() {
                p = p.child(
                    div()
                        .mt(look.px(14.))
                        .flex()
                        .items_center()
                        .gap(look.px(8.))
                        .h(look.px(40.))
                        .px(look.px(12.))
                        .rounded(look.px(10.))
                        .bg(t.inset)
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(look.text(13.))
                                .text_color(t.faint)
                                .child("For the task"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(look.text(14.))
                                .text_color(t.text)
                                .child(task),
                        ),
                );
            }
            let required: Vec<_> = on
                .iter()
                .filter(|p| p.group == hippo_core::brief::Group::Required)
                .collect();
            if !required.is_empty() {
                p = p.child(group("Always", None));
                for r in required {
                    p = p.child(row("note", r.title.clone(), r.reason.clone()));
                }
            }
            let picked: Vec<_> = on
                .iter()
                .filter(|p| p.group == hippo_core::brief::Group::Relevant)
                .collect();
            if !picked.is_empty() {
                p = p.child(group("Picked for this task", None));
                for r in picked.into_iter().take(4) {
                    let folder = r
                        .note
                        .as_ref()
                        .and_then(|n| n.parent())
                        .and_then(|d| d.file_name())
                        .map(|d| d.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    p = p.child(row("note", r.title.clone(), folder));
                }
            }
        }
        if !kept.is_empty() {
            p = p.child(group("Rules you kept", Some(kept.len())));
            for r in kept.into_iter().take(6) {
                p = p.child(bullet(r, true));
            }
        }
        if !waiting.is_empty() {
            p = p.child(div().mt(look.px(16.)).mb(look.px(8.)).child(kit::label(
                look,
                t,
                "Not yet: waiting for you",
                Some((waiting.len(), Tone::You)),
            )));
            for r in waiting {
                p = p.child(bullet(r, false));
            }
        }
        p = p.child(group("Reaches", None));
        if reaches.is_empty() {
            p = p.child(
                div()
                    .text_size(look.text(13.5))
                    .text_color(t.muted)
                    .child("No agent has hippo's session-start hook yet. hippo hooks install adds it to Claude Code."),
            );
        }
        for r in reaches {
            p = p.child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(10.))
                    .h(look.px(40.))
                    .px(look.px(12.))
                    .mb(look.px(6.))
                    .rounded(look.px(10.))
                    .bg(t.inset)
                    .child(kit::badge(look, &r.agent))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(look.text(14.))
                            .text_color(t.text)
                            .child(r.agent.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(look.text(13.))
                            .text_color(t.faint)
                            .child(r.how),
                    ),
            );
        }
        let _ = EDGE;
        p.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hippo_view::night::morning::Figures;

    fn morning(rules: usize, ev: usize, applied: usize, rejected: usize) -> Morning {
        let said = |k: usize| Said {
            session: format!("s{k}"),
            ordinal: 1,
            agent: None,
            profile: None,
            session_title: None,
            at: time::OffsetDateTime::UNIX_EPOCH,
            when: "10:07".into(),
            said: Some("no".into()),
            failed: None,
        };
        let rule = |i: usize| RuleCard {
            id: ulid::Ulid::from_parts(i as u64, 1),
            kind: "correction",
            from: "From 2 corrections".into(),
            title: "t".into(),
            rule: "r".into(),
            note: None,
            note_label: None,
            new_note: false,
            verdict: None,
            agrees: true,
            evidence: (0..ev).map(said).collect(),
            shared: false,
        };
        let fix = |i: usize| Fix {
            proposal: ulid::Ulid::from_parts(i as u64, 2),
            receipt: None,
            note: "n.md".into(),
            title: "n".into(),
            was: None,
            now: "x".into(),
            reason: String::new(),
            at: String::new(),
            undo: None,
            reverted: false,
        };
        Morning {
            kicker: String::new(),
            headline: String::new(),
            lead: String::new(),
            pass: None,
            figures: Figures::default(),
            rules: (0..rules).map(rule).collect(),
            applied: (0..applied).map(fix).collect(),
            rejected: (0..rejected).map(fix).collect(),
        }
    }

    #[test]
    fn rules_sit_beside_their_evidence_and_never_overlap() {
        let m = morning(3, 2, 2, 1);
        let c = columns(&m, 1000.);
        assert!(c.evidence_x.is_some());
        let mut last = f32::MIN;
        for (rule_y, evs) in &c.rules {
            assert!(*rule_y >= last);
            last = rule_y + RULE_H;
            assert_eq!(evs.len(), 2);
            let mid = (evs[0] + evs[1] + EV_H) / 2.;
            assert!((mid - (rule_y + RULE_H / 2.)).abs() < 0.5);
        }
        assert_eq!(wire_points(&c).len(), 6);
        for w in wire_points(&c) {
            for p in w.windows(2) {
                assert!(p[0].0 == p[1].0 || p[0].1 == p[1].1);
            }
        }
    }

    #[test]
    fn a_narrow_column_folds_the_evidence_away() {
        let m = morning(2, 2, 1, 0);
        let c = columns(&m, 700.);
        assert_eq!(c.evidence_x, None);
        assert!(wire_points(&c).is_empty());
        assert!(c.fixes_x + c.fixes_w <= 700.5);
    }
}
