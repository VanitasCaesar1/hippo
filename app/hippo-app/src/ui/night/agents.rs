//! Agents: one card per account, then the sessions in three columns by
//! what they need from you (waiting, working, recent); the panel is the
//! selected session with the ways into it.

use gpui::{AnyElement, Context, FontWeight, div, prelude::*};
use hippo_view::fleet::{Fleet, Row, State};
use hippo_view::night::{agent, recent};
use time::OffsetDateTime;

use super::kit::{self, Btn, Nt, Tone};
use crate::ui::Shell;
use crate::ui::theme::Look;

/// `12 min`, `1 h 12 min`.
fn mins(secs: i64) -> String {
    let m = (secs / 60).max(0);
    if m < 60 {
        format!("{m} min")
    } else {
        format!("{} h {} min", m / 60, m % 60)
    }
}

fn waited(r: &Row, now: OffsetDateTime) -> Option<i64> {
    match &r.state {
        State::Waiting { at, .. } => Some((now - *at).whole_seconds()),
        _ => None,
    }
}

/// The waiting sessions, oldest wait first; the working ones; recent ones.
pub fn columns(f: &Fleet) -> (Vec<&Row>, Vec<&Row>) {
    let mut waiting: Vec<&Row> = f.rows.iter().filter(|r| r.state.is_waiting()).collect();
    waiting.sort_by_key(|r| match &r.state {
        State::Waiting { at, .. } => *at,
        _ => OffsetDateTime::UNIX_EPOCH,
    });
    let mut working: Vec<&Row> = f
        .rows
        .iter()
        .filter(|r| r.state.is_live() && r.delegated_by.is_none())
        .collect();
    working.sort_by_key(|r| std::cmp::Reverse(r.last));
    (waiting, working)
}

/// The headline and the sentence under it.
pub fn headline(f: &Fleet, offset: time::UtcOffset) -> (String, String) {
    let (waiting, working) = columns(f);
    let head = match waiting.len() {
        0 => "No session is waiting on you".to_owned(),
        1 => "1 session is waiting on you".to_owned(),
        n => format!("{n} sessions are waiting on you"),
    };
    let mut lead = match working.len() {
        0 => "Nothing else is working.".to_owned(),
        1 => "1 more is working.".to_owned(),
        n => format!("{n} more are working."),
    };
    for (profile, d) in &f.drains {
        if let Some(e) = d.empty_at {
            let at = hippo_view::fleet::when(e, OffsetDateTime::now_utc(), offset);
            let reset = hippo_view::fleet::when(d.end, OffsetDateTime::now_utc(), offset);
            let stop = f
                .rows
                .iter()
                .filter(|r| r.profile.as_deref() == Some(profile.as_str()) && r.state.is_live())
                .count();
            lead.push_str(&format!(
                " The {} account runs dry at {at}, before its {reset} reset",
                hippo_view::fleet::profile_label(profile)
            ));
            lead.push_str(&match stop {
                0 => ".".to_owned(),
                1 => "; 1 session will stop.".to_owned(),
                n => format!("; {n} sessions will stop."),
            });
            break;
        }
    }
    (head, lead)
}

impl Shell {
    fn agents_selected<'a>(&self, f: &'a Fleet) -> Option<&'a Row> {
        if let Some(id) = &self.night.agents
            && let Some(r) = f.rows.iter().find(|r| &r.id == id)
        {
            return Some(r);
        }
        let (waiting, working) = columns(f);
        waiting.first().or(working.first()).copied()
    }

    pub(crate) fn render_agents(
        &mut self,
        look: &Look,
        t: &Nt,
        left_w: f32,
        narrow: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(f) = self.loaded.as_ref().and_then(|l| l.fleet.clone()) else {
            let why = self
                .loaded
                .as_ref()
                .and_then(|l| l.fleet_error.clone())
                .unwrap_or_else(|| "Reading today's sessions…".to_owned());
            return self.render_night_empty(look, t, &why);
        };
        let now = OffsetDateTime::now_utc();
        let offset = crate::ui::status::local_offset();
        let (head, lead) = headline(&f, offset);
        let agents: std::collections::BTreeSet<&str> =
            f.rows.iter().map(|r| r.tool.as_str()).collect();
        let pad = if narrow { 18. } else { super::morning::PAD };
        let inner = left_w - 2. * pad;
        let selected = self.agents_selected(&f).map(|r| r.id.clone());
        let (waiting, working) = columns(&f);
        let recent = recent::recent(&f, now, offset);
        // Accounts.
        let mut accounts = div().flex().flex_wrap().gap(look.px(12.));
        for h in hippo_view::fleet::ordered_rooms(&f.headroom) {
            let drain = f.drain(&h.profile);
            let dry = drain.and_then(|d| d.empty_at);
            let next = f.use_next.as_deref() == Some(h.profile.as_str());
            let meter = |label: &str, l: Option<&hippo_core::usage::Left>, reset: Option<i64>| {
                let frac = l.map(|l| l.frac).unwrap_or(0.);
                let lit = hippo_view::fleet::pct(frac);
                let color = if frac < 0.2 { t.bad } else { t.text };
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(10.))
                    .child(
                        div()
                            .w(look.px(56.))
                            .text_size(look.text(13.))
                            .text_color(t.muted)
                            .child(label.to_owned()),
                    )
                    .child(div().flex().gap(look.px(2.)).children((0..10).map(|k| {
                        div()
                            .w(look.px(7.))
                            .h(look.px(10.))
                            .rounded(look.px(2.))
                            .bg(if (k as f64) < frac * 10. {
                                color
                            } else {
                                t.inset
                            })
                    })))
                    .child(
                        div()
                            .w(look.px(44.))
                            .font_family(kit::MONO)
                            .text_size(look.text(13.))
                            .text_color(color)
                            .child(if l.is_some() {
                                format!("{lit}%")
                            } else {
                                "–".to_owned()
                            }),
                    )
                    .children(reset.map(|r| {
                        div()
                            .text_size(look.text(13.))
                            .text_color(t.muted)
                            .child(format!(
                                "resets {}",
                                hippo_view::fleet::when(r, now, offset)
                            ))
                    }))
            };
            let five_reset = h.five.and_then(|l| l.resets_at).or(h.resets());
            let card = kit::card(look, t)
                .flex_1()
                .min_w(look.px(280.))
                .flex()
                .flex_col()
                .gap(look.px(10.))
                .when(dry.is_some(), |d| d.border_color(t.bad))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(look.px(10.))
                        .child(kit::badge(
                            look,
                            &agent(if h.codex { "codex" } else { "claude-code" }),
                        ))
                        .child(
                            div()
                                .flex_1()
                                .text_size(look.text(15.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.text)
                                .child(hippo_view::fleet::profile_label(&h.profile).to_owned()),
                        )
                        .when(next, |d| d.child(kit::chip(look, t, "use next", Tone::Ok)))
                        .when_some(dry, |d, e| {
                            d.child(kit::chip(
                                look,
                                t,
                                format!("empty at {}", hippo_view::fleet::when(e, now, offset)),
                                Tone::Bad,
                            ))
                        }),
                )
                .child(meter("5 hours", h.five.as_ref(), five_reset))
                .child(meter("7 days", h.seven.as_ref(), None));
            accounts = accounts.child(card);
        }
        let col_w = if narrow {
            inner
        } else {
            (inner - 2. * 20.) / 3.
        };
        let sel_card = |id: &str| selected.as_deref() == Some(id);
        let session_card = |this: &mut Shell,
                            r: &Row,
                            right: String,
                            line: String,
                            quote: bool,
                            chips: Vec<(String, Tone)>,
                            sel: bool,
                            cx: &mut Context<Shell>| {
            let id = r.id.clone();
            let a = agent(&r.tool);
            kit::select_card(look, t, sel)
                .id(gpui::SharedString::from(format!("agent-{id}")))
                .w_full()
                .flex()
                .flex_col()
                .gap(look.px(8.))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.night.agents = Some(id.clone());
                    cx.notify();
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(look.px(10.))
                        .child(kit::badge(look, &a))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(look.text(15.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.text)
                                .child(r.title.clone()),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .font_family(kit::MONO)
                                .text_size(look.text(12.5))
                                .text_color(t.faint)
                                .child(right),
                        ),
                )
                .child(if quote {
                    kit::quote(look, t.you, t.text, &line, 14.).into_any_element()
                } else {
                    div()
                        .text_size(look.text(13.5))
                        .text_color(t.muted)
                        .truncate()
                        .child(line)
                        .into_any_element()
                })
                .when(!chips.is_empty(), |d| {
                    d.child(
                        div().flex().flex_wrap().gap(look.px(6.)).children(
                            chips
                                .into_iter()
                                .map(|(w, tone)| kit::chip(look, t, w, tone)),
                        ),
                    )
                })
                .into_any_element()
                .pipe_ref(this)
        };
        let mut c_wait = div()
            .flex()
            .flex_col()
            .gap(look.px(10.))
            .w(look.px(col_w))
            .child(kit::label(
                look,
                t,
                "Waiting on you",
                Some((waiting.len(), Tone::You)),
            ));
        for r in &waiting {
            let w = waited(r, now).unwrap_or(0);
            let line = match &r.state {
                State::Waiting { plan: true, .. } => {
                    let n = r.plan.as_ref().map_or(0, |p| p.steps.len());
                    if n > 0 {
                        format!("A plan with {n} steps is waiting for your approval.")
                    } else {
                        "A plan is waiting for your approval.".to_owned()
                    }
                }
                State::Waiting { asked: Some(q), .. } => crate::ui::kit::plain(q),
                _ => "It asked you something.".to_owned(),
            };
            let mut chips = vec![(format!("waiting {}", mins(w)), Tone::You)];
            if let Some(p) = &r.profile {
                chips.push((hippo_view::fleet::profile_label(p).to_owned(), Tone::Plain));
            }
            if let Some(p) = &r.project {
                chips.push((p.clone(), Tone::Plain));
            }
            let sel = sel_card(&r.id);
            c_wait = c_wait.child(session_card(
                self,
                r,
                String::new(),
                line,
                true,
                chips,
                sel,
                cx,
            ));
        }
        if waiting.is_empty() {
            c_wait = c_wait.child(
                kit::card(look, t)
                    .text_size(look.text(14.))
                    .text_color(t.muted)
                    .child("No session is waiting on you."),
            );
        }
        let mut c_work = div()
            .flex()
            .flex_col()
            .gap(look.px(10.))
            .w(look.px(col_w))
            .child(kit::label(
                look,
                t,
                "Working",
                Some((working.len(), Tone::Plain)),
            ));
        for r in working.iter().take(8) {
            let age = r
                .started
                .map(|s| mins((now - s).whole_seconds()))
                .unwrap_or_default();
            let line = r.activity.clone().unwrap_or_else(|| "Working".to_owned());
            let sel = sel_card(&r.id);
            c_work = c_work.child(session_card(self, r, age, line, false, Vec::new(), sel, cx));
        }
        let mut c_recent = div()
            .flex()
            .flex_col()
            .gap(look.px(10.))
            .w(look.px(col_w))
            .child(kit::label(
                look,
                t,
                "Recent",
                Some((recent.len(), Tone::Plain)),
            ));
        for rc in &recent {
            if let Some(r) = f.rows.iter().find(|r| r.id == rc.id) {
                let sel = sel_card(&r.id);
                c_recent = c_recent.child(session_card(
                    self,
                    r,
                    rc.when.clone(),
                    rc.line.clone(),
                    false,
                    Vec::new(),
                    sel,
                    cx,
                ));
            }
        }
        let cols = if narrow {
            div()
                .flex()
                .flex_col()
                .gap(look.px(24.))
                .child(c_wait)
                .child(c_work)
                .child(c_recent)
        } else {
            div()
                .flex()
                .gap(look.px(20.))
                .items_start()
                .child(c_wait)
                .child(c_work)
                .child(c_recent)
        };
        let links = div()
            .flex()
            .gap(look.px(6.))
            .child(
                kit::button(look, t, "agents-lanes", None, "Lanes", Btn::Quiet, true).on_click(
                    cx.listener(|this, _, window, cx| this.open_fleet_mode("lanes", window, cx)),
                ),
            )
            .child(
                kit::button(look, t, "agents-rows", None, "Rows", Btn::Quiet, true).on_click(
                    cx.listener(|this, _, window, cx| this.open_fleet_mode("rows", window, cx)),
                ),
            );
        let mut body = div()
            .flex()
            .flex_col()
            .gap(look.px(20.))
            .px(look.px(pad))
            .pt(look.px(26.))
            .child(
                div()
                    .flex()
                    .items_start()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(look.px(8.))
                            .child(div().text_size(look.text(13.5)).text_color(t.muted).child(
                                format!(
                                    "Agents · today, across {} and {}",
                                    hippo_view::night::count(
                                        f.headroom.len(),
                                        "account",
                                        "accounts"
                                    ),
                                    hippo_view::night::count(agents.len(), "agent", "agents")
                                ),
                            ))
                            .child(
                                div()
                                    .text_size(look.text(if narrow { 24. } else { 28. }))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(t.text)
                                    .child(head),
                            )
                            .child(
                                div()
                                    .text_size(look.text(15.))
                                    .text_color(t.muted)
                                    .child(lead),
                            ),
                    )
                    .child(links),
            )
            .when(!f.headroom.is_empty(), |d| d.child(accounts))
            .child(cols);
        if narrow {
            body = body.child(
                kit::card(look, t)
                    .rounded(look.px(18.))
                    .p(look.px(20.))
                    .child(self.render_session_panel(look, t, cx)),
            );
        }
        body.into_any_element()
    }

    /// Opens Fleet in one of its modes (`lanes`, `rows`).
    fn open_fleet_mode(&mut self, mode: &str, window: &mut gpui::Window, cx: &mut Context<Self>) {
        self.select_section(crate::state::Section::Fleet, window, cx);
        self.replay.lanes = mode == "lanes";
        if mode == "rows" {
            self.pulse.on = false;
        }
        cx.notify();
    }

    /// The panel: the selected session and the ways into it.
    pub(crate) fn render_session_panel(
        &mut self,
        look: &Look,
        t: &Nt,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(f) = self.loaded.as_ref().and_then(|l| l.fleet.clone()) else {
            return div().into_any_element();
        };
        let Some(r) = self.agents_selected(&f).cloned() else {
            return div()
                .text_size(look.text(14.))
                .text_color(t.muted)
                .child("No session today yet.")
                .into_any_element();
        };
        let now = OffsetDateTime::now_utc();
        let a = agent(&r.tool);
        let mut where_ = vec![a.name.clone()];
        if let Some(p) = &r.profile {
            where_.push(hippo_view::fleet::profile_label(p).to_owned());
        }
        if let Some(p) = &r.project {
            where_.push(p.clone());
        }
        let question = match &r.state {
            State::Waiting {
                asked: Some(q),
                plan: false,
                ..
            } => Some(crate::ui::kit::plain(q)),
            State::Waiting { plan: true, .. } => {
                Some("A plan is waiting for your approval.".to_owned())
            }
            _ => None,
        };
        let (row, row2) = (r.clone(), r.clone());
        let fleet = f.clone();
        let id = r.id.clone();
        let spec = |k: &str, v: String| {
            div()
                .flex()
                .gap(look.px(12.))
                .py(look.px(6.))
                .child(
                    div()
                        .w(look.px(120.))
                        .flex_shrink_0()
                        .text_size(look.text(14.))
                        .text_color(t.muted)
                        .child(k.to_owned()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_family(kit::MONO)
                        .text_size(look.text(13.5))
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
                    .gap(look.px(8.))
                    .child(kit::badge(look, &a))
                    .child(
                        div()
                            .text_size(look.text(13.5))
                            .text_color(t.muted)
                            .child(where_.join(" · ")),
                    ),
            )
            .child(
                div()
                    .mt(look.px(14.))
                    .text_size(look.text(22.))
                    .line_height(look.text(27.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(t.text)
                    .child(r.title.clone()),
            )
            .children(waited(&r, now).map(|w| {
                div().mt(look.px(10.)).flex().child(kit::chip(
                    look,
                    t,
                    format!("waiting {}", mins(w)),
                    Tone::You,
                ))
            }))
            .children(question.map(|q| {
                div()
                    .mt(look.px(14.))
                    .child(kit::quote(look, t.you, t.text, &q, 15.5))
            }))
            .child(
                div()
                    .mt(look.px(16.))
                    .flex()
                    .flex_wrap()
                    .gap(look.px(8.))
                    .child(
                        kit::button(
                            look,
                            t,
                            "panel-open-thread",
                            Some("thread"),
                            "Open thread",
                            Btn::Go,
                            false,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| this.open_session(row.clone(), window, cx),
                        )),
                    )
                    .child(
                        kit::button(
                            look,
                            t,
                            "panel-terminal",
                            Some("terminal"),
                            "Terminal",
                            Btn::Second,
                            false,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.pulse_terminal(&row2, &fleet);
                            cx.notify();
                        })),
                    )
                    .child(
                        kit::button(look, t, "panel-later", None, "Later", Btn::Quiet, false)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let until = OffsetDateTime::now_utc().unix_timestamp() + 3600;
                                this.snooze(&id, Some(until));
                                cx.notify();
                            })),
                    ),
            );
        let mut table = div().mt(look.px(18.)).flex().flex_col();
        if let Some(b) = &r.branch {
            table = table.child(spec("Branch", b.clone()));
        }
        if let Some((url, done)) = r.pulls.last() {
            let n = url.trim_end_matches('/').rsplit('/').next().unwrap_or("");
            let lines = r
                .total_lines()
                .map(|l| format!("  {}", l.counts()))
                .unwrap_or_default();
            table = table.child(spec("Pull request", format!("#{n} {done}{lines}")));
        }
        if let Some(c) = r.claims.short_line() {
            table = table.child(spec("Checks", c));
        }
        if !r.rules.is_empty() {
            table = table.child(spec(
                "Started with",
                hippo_view::night::count(r.rules.len(), "kept rule", "kept rules"),
            ));
        }
        p = p.child(table);
        if let Some(turn) = r.turn.as_ref() {
            let calls: Vec<String> = turn
                .calls
                .iter()
                .rev()
                .take(3)
                .map(|c| c.label.clone())
                .collect();
            if !calls.is_empty() {
                p = p.child(
                    div()
                        .mt(look.px(18.))
                        .mb(look.px(8.))
                        .text_size(look.text(13.))
                        .text_color(t.muted)
                        .child("Last calls"),
                );
                for c in calls.into_iter().rev() {
                    p = p.child(
                        div()
                            .h(look.px(36.))
                            .px(look.px(12.))
                            .mb(look.px(6.))
                            .flex()
                            .items_center()
                            .rounded(look.px(10.))
                            .bg(t.inset)
                            .font_family(kit::MONO)
                            .text_size(look.text(13.))
                            .text_color(t.text)
                            .truncate()
                            .child(c),
                    );
                }
            }
        }
        p = p.child(
            div()
                .mt(look.px(18.))
                .text_size(look.text(13.))
                .text_color(t.faint)
                .child("hippo never writes into a session. These open it where it runs."),
        );
        p.into_any_element()
    }
}

/// `e.pipe_ref(x)`: `e`, with `x` borrowed for the borrow checker.
trait PipeRef: Sized {
    fn pipe_ref<T>(self, _: &mut T) -> Self {
        self
    }
}

impl<E> PipeRef for E {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_read_in_minutes_and_hours() {
        assert_eq!(mins(30), "0 min");
        assert_eq!(mins(12 * 60 + 5), "12 min");
        assert_eq!(mins(72 * 60), "1 h 12 min");
    }

    #[test]
    fn an_empty_fleet_says_nobody_waits() {
        let f = Fleet {
            rows: Vec::new(),
            headroom: Vec::new(),
            use_next: None,
            signed_out: Vec::new(),
            usage_cached: false,
            drains: Vec::new(),
        };
        let (head, lead) = headline(&f, time::UtcOffset::UTC);
        assert_eq!(head, "No session is waiting on you");
        assert_eq!(lead, "Nothing else is working.");
    }
}
