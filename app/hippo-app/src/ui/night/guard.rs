//! Guard: one row of three cards per incident (who, what it ran, what it
//! touched) joined by lines; the panel says what it means, what to do,
//! and starts a session to fix it the way Brief does.

use gpui::{AnyElement, Context, FontWeight, div, prelude::*};
use hippo_view::night::guard::{Guard, GuardItem, Severity};
use time::OffsetDateTime;

use super::kit::{self, Btn, Nt, Tone, Wire};
use crate::ui::Shell;
use crate::ui::theme::Look;

/// A row's height.
pub const ROW_H: f32 = 128.;
/// Below this column width the rows stack.
pub const STACK_BELOW: f32 = 600.;

/// The icon for a kind.
fn kind_icon(kind: &str) -> &'static str {
    match kind {
        "remote-script" => "script",
        "credential-read" => "key",
        "secret-to-model" => "tomodel",
        "fresh-clone-exec" => "clone",
        _ => "guard",
    }
}

/// The gap between columns for `width` points.
pub fn link(width: f32) -> f32 {
    (width * 0.05).clamp(28., 48.)
}

/// The three columns' x and width for `width` points: who, what it ran,
/// what it touched.
pub fn columns(width: f32) -> [(f32, f32); 3] {
    let gap = link(width);
    let who = (width * 0.24).round();
    let ran = (width * 0.38).round();
    let touched = width - who - ran - 2. * gap;
    [(0., who), (who + gap, ran), (who + ran + 2. * gap, touched)]
}

impl Shell {
    fn guard_model(&self) -> Option<Guard> {
        let n = self.loaded.as_ref()?.night.as_ref()?;
        let dismissed = &self.incidents_ui.dismissed;
        Some(n.guard(
            &|id| dismissed.contains_key(id),
            OffsetDateTime::now_utc(),
            crate::ui::status::local_offset(),
        ))
    }

    fn guard_selected<'a>(&self, g: &'a Guard) -> Option<&'a GuardItem> {
        self.night
            .guard
            .as_ref()
            .and_then(|id| {
                g.items
                    .iter()
                    .find(|i| &i.id == id || i.kind == id.as_str())
            })
            .or_else(|| g.items.first())
    }

    pub(crate) fn render_guard(
        &mut self,
        look: &Look,
        t: &Nt,
        left_w: f32,
        narrow: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(g) = self.guard_model() else {
            return self.render_night_empty(look, t, "Reading this week's incidents…");
        };
        let pad = if narrow { 18. } else { super::morning::PAD };
        let inner = left_w - 2. * pad;
        let sel = self.guard_selected(&g).map(|i| i.id.clone());
        let head = div()
            .flex()
            .items_start()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(look.px(8.))
                    .child(
                        div()
                            .text_size(look.text(13.5))
                            .text_color(t.muted)
                            .child(g.kicker.clone()),
                    )
                    .child(
                        div()
                            .text_size(look.text(if narrow { 24. } else { 28. }))
                            .font_weight(FontWeight::BOLD)
                            .text_color(t.text)
                            .child(g.headline.clone()),
                    )
                    .child(
                        div()
                            .text_size(look.text(15.))
                            .text_color(t.muted)
                            .child(g.lead.clone()),
                    ),
            )
            .child(
                kit::button(
                    look,
                    t,
                    "guard-all",
                    None,
                    "All incidents",
                    Btn::Quiet,
                    true,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.select_section(crate::state::Section::Incidents, window, cx)
                })),
            );
        let important: Vec<&GuardItem> = g
            .items
            .iter()
            .filter(|i| i.severity == Severity::Important)
            .collect();
        let minor: Vec<&GuardItem> = g
            .items
            .iter()
            .filter(|i| i.severity == Severity::Minor)
            .collect();
        let mut body = div()
            .flex()
            .flex_col()
            .gap(look.px(20.))
            .px(look.px(pad))
            .pt(look.px(26.))
            .child(head);
        if g.items.is_empty() {
            body = body.child(
                kit::card(look, t)
                    .text_size(look.text(14.))
                    .text_color(t.muted)
                    .child("Nothing this week: no installer piped from the internet, no credential file read, no secret sent to a model, no fresh clone run."),
            );
            return body.into_any_element();
        }
        // Stacked when the three columns have no room.
        if narrow || inner < STACK_BELOW {
            for (k, i) in g.items.iter().enumerate() {
                let selected = sel.as_deref() == Some(i.id.as_str());
                body = body.child(self.guard_ran_card(look, t, i, k, selected, true, cx));
            }
            if narrow {
                body = body.child(
                    kit::card(look, t)
                        .rounded(look.px(18.))
                        .p(look.px(20.))
                        .child(self.render_guard_panel(look, t, cx)),
                );
            }
            return body.into_any_element();
        }
        let cols = columns(inner);
        let labels = div().relative().h(look.px(20.)).children(
            ["Who", "What it ran", "What it touched"]
                .iter()
                .enumerate()
                .map(|(k, w)| {
                    div()
                        .absolute()
                        .left(look.px(cols[k].0))
                        .text_size(look.text(13.))
                        .text_color(t.muted)
                        .child(w.to_string())
                }),
        );
        body = body.child(labels);
        let mut k = 0;
        for (group, items) in [("", important), ("Worth a look", minor)] {
            if items.is_empty() {
                continue;
            }
            if !group.is_empty() {
                body = body.child(kit::label(look, t, group, Some((items.len(), Tone::Plain))));
            }
            for i in items {
                let selected = sel.as_deref() == Some(i.id.as_str());
                body = body.child(self.guard_row(look, t, i, k, selected, &cols, cx));
                k += 1;
            }
        }
        body.into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn guard_row(
        &mut self,
        look: &Look,
        t: &Nt,
        i: &GuardItem,
        k: usize,
        selected: bool,
        cols: &[(f32, f32); 3],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let unit = f32::from(look.px(1.));
        let color = if i.severity == Severity::Important {
            t.bad.opacity(0.7)
        } else {
            t.line2
        };
        let y = ROW_H / 2. * unit;
        let wires = vec![
            Wire {
                points: vec![((cols[0].0 + cols[0].1) * unit, y), (cols[1].0 * unit, y)],
                color,
                width: 1.5 * unit,
                rings: true,
            },
            Wire {
                points: vec![((cols[1].0 + cols[1].1) * unit, y), (cols[2].0 * unit, y)],
                color,
                width: 1.5 * unit,
                rings: true,
            },
        ];
        let who = kit::card(look, t)
            .size_full()
            .flex()
            .flex_col()
            .gap(look.px(6.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(8.))
                    .child(kit::badge(look, &i.agent))
                    .child(
                        div()
                            .text_size(look.text(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.text)
                            .child(i.agent.name.clone()),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .text_size(look.text(13.5))
                    .text_color(t.muted)
                    .truncate()
                    .child(
                        i.session_title
                            .clone()
                            .unwrap_or_else(|| i.session.chars().take(8).collect()),
                    ),
            )
            .child(
                div()
                    .text_size(look.text(12.5))
                    .text_color(t.faint)
                    .truncate()
                    .child(
                        [
                            i.project.clone(),
                            Some(format!("{}, turn {}", i.when, i.ordinal)),
                        ]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" · "),
                    ),
            );
        let touched = kit::card(look, t)
            .size_full()
            .flex()
            .flex_col()
            .justify_center()
            .gap(look.px(6.))
            .child(
                div()
                    .font_family(kit::MONO)
                    .text_size(look.text(13.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.text)
                    .truncate()
                    .child(i.target.clone()),
            )
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .text_size(look.text(13.))
                    .text_color(t.muted)
                    .truncate()
                    .child(i.target_note.clone()),
            );
        div()
            .relative()
            .w_full()
            .h(look.px(ROW_H))
            .child(kit::wires(wires, t.canvas))
            .child(
                div()
                    .absolute()
                    .left(look.px(cols[0].0))
                    .w(look.px(cols[0].1))
                    .h(look.px(ROW_H))
                    .child(who),
            )
            .child(
                div()
                    .absolute()
                    .left(look.px(cols[1].0))
                    .w(look.px(cols[1].1))
                    .h(look.px(ROW_H))
                    .child(self.guard_ran_card(look, t, i, k, selected, false, cx)),
            )
            .child(
                div()
                    .absolute()
                    .left(look.px(cols[2].0))
                    .w(look.px(cols[2].1))
                    .h(look.px(ROW_H))
                    .child(touched),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn guard_ran_card(
        &mut self,
        look: &Look,
        t: &Nt,
        i: &GuardItem,
        k: usize,
        selected: bool,
        full: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = i.id.clone();
        let tone = if i.severity == Severity::Important {
            Tone::Bad
        } else {
            Tone::Plain
        };
        kit::select_card(look, t, selected)
            .id(("guard-ran", k))
            .when(!full, |d| d.size_full())
            .flex()
            .flex_col()
            .gap(look.px(10.))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.night.guard = Some(id.clone());
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(10.))
                    .child(kit::tile(look, t, kind_icon(i.kind), tone))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(look.text(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.text)
                            .line_clamp(2)
                            .child(i.title.clone()),
                    ),
            )
            .child(
                div()
                    .px(look.px(12.))
                    .py(look.px(8.))
                    .rounded(look.px(10.))
                    .bg(t.inset)
                    .font_family(kit::MONO)
                    .text_size(look.text(13.))
                    .text_color(t.text)
                    .truncate()
                    .child(i.command.clone()),
            )
            .into_any_element()
    }

    /// The panel: what it is, what to do, and the ways to act.
    pub(crate) fn render_guard_panel(
        &mut self,
        look: &Look,
        t: &Nt,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(g) = self.guard_model() else {
            return div().into_any_element();
        };
        let Some(i) = self.guard_selected(&g).cloned() else {
            return div()
                .text_size(look.text(14.))
                .text_color(t.muted)
                .child("Guard looks for four things in the transcripts: an installer piped from the internet, a credential file read, a secret sent to a model, and code run from a repository cloned a moment before. It warns; it does not block.")
                .into_any_element();
        };
        let tone = if i.severity == Severity::Important {
            Tone::Bad
        } else {
            Tone::Plain
        };
        let (fix, show, dismiss) = (i.clone(), i.clone(), i.id.clone());
        let confirming = self.night.dev_dialog.as_deref() == Some("fix")
            || self.night.fixing.as_deref() == Some(i.id.as_str());
        let spec = |k: &str, v: String, mono: bool| {
            div()
                .flex()
                .gap(look.px(12.))
                .py(look.px(6.))
                .child(
                    div()
                        .w(look.px(110.))
                        .flex_shrink_0()
                        .text_size(look.text(14.))
                        .text_color(t.muted)
                        .child(k.to_owned()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .when(mono, |d| d.font_family(kit::MONO))
                        .text_size(look.text(14.))
                        .text_color(t.text)
                        .child(v),
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
                    .child(kit::tile(look, t, kind_icon(i.kind), tone))
                    .child(
                        div()
                            .flex_1()
                            .text_size(look.text(13.5))
                            .text_color(t.muted)
                            .child(i.kind_label),
                    )
                    .child(kit::chip(
                        look,
                        t,
                        if i.severity == Severity::Important {
                            "important"
                        } else {
                            "minor"
                        },
                        if i.severity == Severity::Important {
                            Tone::Bad
                        } else {
                            Tone::Plain
                        },
                    )),
            )
            .child(
                div()
                    .mt(look.px(14.))
                    .text_size(look.text(22.))
                    .line_height(look.text(27.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(t.text)
                    .child(i.title.clone()),
            )
            .child(
                div()
                    .mt(look.px(10.))
                    .text_size(look.text(14.))
                    .line_height(look.text(21.))
                    .text_color(t.muted)
                    .child(i.sentence.clone()),
            )
            .child(
                div()
                    .mt(look.px(16.))
                    .mb(look.px(8.))
                    .text_size(look.text(13.))
                    .text_color(t.muted)
                    .child("What to do"),
            )
            .child(kit::quote(look, t.bad, t.text, &i.what_to_do, 14.5));
        if confirming {
            let fix2 = fix.clone();
            p = p.child(
                div()
                    .mt(look.px(16.))
                    .p(look.px(14.))
                    .rounded(look.px(12.))
                    .border(look.fixed(1.5))
                    .border_color(t.you_line)
                    .flex()
                    .flex_col()
                    .gap(look.px(10.))
                    .child(
                        div()
                            .text_size(look.text(14.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.text)
                            .child(format!("Start {} with this brief?", fix.fix_agent.name)),
                    )
                    .child(
                        div()
                            .text_size(look.text(13.5))
                            .line_height(look.text(20.))
                            .text_color(t.muted)
                            .child(fix.fix_brief.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(look.px(8.))
                            .child(
                                kit::button(
                                    look,
                                    t,
                                    "guard-fix-go",
                                    Some("fix"),
                                    "Start the session",
                                    Btn::Go,
                                    false,
                                )
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.guard_fix(&fix2, cx)),
                                ),
                            )
                            .child(
                                kit::button(
                                    look,
                                    t,
                                    "guard-fix-cancel",
                                    None,
                                    "Cancel",
                                    Btn::Quiet,
                                    false,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.night.fixing = None;
                                        this.night.dev_dialog = None;
                                        cx.notify();
                                    },
                                )),
                            ),
                    ),
            );
        } else {
            let id = i.id.clone();
            p = p
                .child(
                    div()
                        .mt(look.px(16.))
                        .flex()
                        .flex_wrap()
                        .gap(look.px(8.))
                        .child(
                            kit::button(look, t, "guard-show", Some("thread"), "Show session", Btn::Second, false)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.state.tabs.open(crate::state::Tab::Session {
                                        id: show.session.clone(),
                                        ordinal: Some(show.ordinal),
                                    });
                                    cx.notify();
                                })),
                        )
                        .child(
                            kit::button(look, t, "guard-fix", Some("fix"), format!("Fix with {}", fix.fix_agent.name), Btn::Go, false)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.night.fixing = Some(id.clone());
                                    cx.notify();
                                })),
                        ),
                )
                .child(
                    div()
                        .mt(look.px(10.))
                        .text_size(look.text(13.))
                        .line_height(look.text(19.))
                        .text_color(t.faint)
                        .child(format!(
                            "Fix with {} opens a new session with this brief: “{}” You read it before it runs.",
                            fix.fix_agent.name, fix.fix_brief
                        )),
                );
        }
        if let Some(said) = &self.night.fix_said {
            p = p.child(
                div()
                    .mt(look.px(8.))
                    .text_size(look.text(13.))
                    .text_color(t.muted)
                    .child(said.clone()),
            );
        }
        p = p
            .child(
                div()
                    .mt(look.px(16.))
                    .mb(look.px(8.))
                    .text_size(look.text(13.))
                    .text_color(t.muted)
                    .child("The command"),
            )
            .child(
                div()
                    .p(look.px(12.))
                    .rounded(look.px(10.))
                    .bg(t.inset)
                    .font_family(kit::MONO)
                    .text_size(look.text(13.))
                    .text_color(t.text)
                    .child(i.command.clone()),
            )
            .child(
                div()
                    .mt(look.px(14.))
                    .flex()
                    .flex_col()
                    .child(spec("Agent", i.agent.name.clone(), false))
                    .child(spec(
                        "Session",
                        i.session_title
                            .clone()
                            .unwrap_or_else(|| i.session.chars().take(8).collect()),
                        false,
                    ))
                    .child(spec(
                        "When",
                        format!("{}, turn {}", i.when, i.ordinal),
                        false,
                    ))
                    .child(spec(
                        "Touched",
                        format!("{}\n{}", i.target, i.target_note),
                        true,
                    ))
                    .child(spec("Found by", "a rule, no model".to_owned(), false)),
            )
            .child(
                div().mt(look.px(16.)).flex().gap(look.px(6.)).child(
                    kit::button(
                        look,
                        t,
                        "guard-dismiss",
                        None,
                        "Not a problem",
                        Btn::Quiet,
                        false,
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.dismiss_incident(&dismiss, window, cx);
                        this.night.guard = None;
                        cx.notify();
                    })),
                ),
            );
        p.into_any_element()
    }

    /// Starts a session to fix an incident: the brief in a private file
    /// and a one-shot script, opened in the terminal, as Brief starts one.
    pub(crate) fn guard_fix(&mut self, i: &GuardItem, cx: &mut Context<Self>) {
        let agent = if i.fix_agent.code == "cx" {
            hippo_view::briefing::Agent::Codex
        } else {
            hippo_view::briefing::Agent::Claude
        };
        let repo = self
            .loaded
            .as_ref()
            .and_then(|l| l.setup.config())
            .and_then(|cfg| crate::ui::brief::repo_of_session(&cfg.home, &i.session))
            .or_else(|| {
                self.loaded
                    .as_ref()
                    .and_then(|l| l.setup.workspace())
                    .map(|w| w.root.clone())
            });
        let Some(repo) = repo else { return };
        let term = self.term();
        let r = crate::ui::brief::write_start_files(agent, &i.fix_brief, &repo, None).and_then(
            |script| crate::launch::open_script(&term, &script).map_err(std::io::Error::other),
        );
        self.night.fix_said = Some(match r {
            Ok(()) => format!("Started {} in {}.", i.fix_agent.name, repo.display()),
            Err(e) => format!("Could not start {}: {e}", i.fix_agent.name),
        });
        self.night.fixing = None;
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_columns_fill_the_width_with_room_for_the_lines() {
        for w in [STACK_BELOW, 1000., 1300.] {
            let c = columns(w);
            assert_eq!(c[0].0, 0.);
            let gap = link(w);
            assert!((c[1].0 - (c[0].0 + c[0].1) - gap).abs() < 0.01);
            assert!((c[2].0 - (c[1].0 + c[1].1) - gap).abs() < 0.01);
            assert!((c[2].0 + c[2].1 - w).abs() < 0.5, "{w}: {c:?}");
            assert!(c[2].1 >= 150., "{w}: what it touched has room: {c:?}");
        }
    }

    #[test]
    fn each_guard_kind_has_its_icon() {
        assert_eq!(kind_icon("remote-script"), "script");
        assert_eq!(kind_icon("credential-read"), "key");
        assert_eq!(kind_icon("secret-to-model"), "tomodel");
        assert_eq!(kind_icon("fresh-clone-exec"), "clone");
        assert_eq!(kind_icon("guard-bypass"), "guard");
    }
}
