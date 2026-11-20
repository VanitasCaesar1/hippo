//! The night map (`docs/designs/app-night-map.md`): the top bar, the
//! floating panel and Ask bar, and the views Morning, Brain, Agents and
//! Guard drawn on a dotted ground.

pub mod brain;
pub mod kit;
pub mod morning;

use std::path::PathBuf;

use gpui::{AnyElement, Context, FontWeight, ScrollHandle, Window, div, prelude::*};
use ulid::Ulid;

use self::kit::{Btn, Nt, Tone};
use super::Shell;
use super::theme::Look;
use crate::state::{Section, Tab};

/// The top bar's height.
pub const TOP_H: f32 = 56.;
/// Below this window width Morning and Brain take the feed and the board.
pub const NARROW: f32 = 900.;
/// The floating panel's width, and below 1200 points.
pub const PANEL_W: f32 = 360.;
pub const PANEL_W_SMALL: f32 = 332.;
/// The gap around floating things.
pub const EDGE: f32 = 18.;

/// What is selected in a view.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // Brain, Agents and Guard select these
pub enum Sel {
    Rule(Ulid),
    Fix(Ulid),
    Note(PathBuf),
    Session(String),
    Incident(String),
}

/// The night map's own state.
#[allow(dead_code)] // Brain, Agents and Guard read their fields
pub struct NightUi {
    pub morning: Option<Sel>,
    pub brain: Option<PathBuf>,
    pub agents: Option<String>,
    pub guard: Option<String>,
    pub scroll: ScrollHandle,
    pub panel_scroll: ScrollHandle,
    /// The Ask bar's answer card shows.
    pub answer_open: bool,
    /// A dialog drawn as a view for screenshots (`HIPPO_APP_DEV_DIALOG`).
    pub dev_dialog: Option<String>,
    /// What to select once the models load (`HIPPO_APP_DEV_SELECT`).
    pub dev_select: Option<String>,
    /// Brain's zoom at launch (`HIPPO_APP_DEV_ZOOM`).
    pub dev_zoom: Option<f32>,
    /// Brain's map: pan, zoom, the press, frame times.
    pub map: brain::MapView,
}

impl Default for NightUi {
    fn default() -> NightUi {
        NightUi {
            morning: None,
            brain: None,
            agents: None,
            guard: None,
            scroll: ScrollHandle::new(),
            panel_scroll: ScrollHandle::new(),
            answer_open: false,
            dev_dialog: None,
            dev_select: None,
            dev_zoom: None,
            map: brain::MapView::default(),
        }
    }
}

/// Whether a tab is one of the night map's views.
pub fn is_night(tab: &Tab) -> bool {
    matches!(tab, Tab::Morning | Tab::Brain | Tab::Agents | Tab::Guard)
}

/// Whether a night view is drawn on the night canvas; the others show
/// the view they replace until they are built.
pub fn draws(tab: &Tab) -> bool {
    matches!(tab, Tab::Morning | Tab::Brain)
}

impl Shell {
    /// The top bar's counts: rules waiting, sessions waiting on you,
    /// important guard incidents not marked "Not a problem".
    pub(crate) fn night_counts(&self) -> (usize, usize, usize) {
        let Some(l) = self.loaded.as_ref() else {
            return (0, 0, 0);
        };
        let rules = l
            .night
            .as_ref()
            .map_or(l.lessons.len(), |n| n.morning.figures.rules_waiting);
        let waiting = l.fleet.as_ref().map_or(0, |f| f.waiting().count());
        let guard = l.night.as_ref().map_or(0, |n| {
            let now = time::OffsetDateTime::now_utc();
            let dismissed = &self.incidents_ui.dismissed;
            n.guard(
                &|id| dismissed.contains_key(id),
                now,
                super::status::local_offset(),
            )
            .important
        });
        (rules, waiting, guard)
    }

    /// The workspace's folder name.
    pub(crate) fn workspace_name(&self) -> String {
        self.loaded
            .as_ref()
            .and_then(|l| l.setup.workspace())
            .and_then(|ws| {
                ws.root
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "hippo".to_owned())
    }

    /// The top bar: the mark and the workspace, the four views with their
    /// counts, search, the indexed state and the theme toggle.
    pub(crate) fn render_top_bar(
        &mut self,
        look: &Look,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Nt::of(look);
        let w = f32::from(window.viewport_size().width) / f32::from(look.px(1.));
        let (rules, waiting, guard) = self.night_counts();
        let section = self.state.tabs.section();
        let nav = |id: &'static str,
                   s: Section,
                   icon: &'static str,
                   n: usize,
                   tone: Tone,
                   cx: &mut Context<Self>| {
            let on = section == s;
            let hover = t.hover;
            div()
                .id(id)
                .role(gpui::Role::Tab)
                .aria_label(s.name())
                .h(look.px(36.))
                .px(look.px(12.))
                .flex()
                .items_center()
                .gap(look.px(8.))
                .rounded(look.px(9.))
                .cursor_pointer()
                .text_size(look.text(14.5))
                .font_weight(if on {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                })
                .text_color(if on { t.text } else { t.muted })
                .when(on, |d| d.bg(t.selected))
                .when(!on, move |d| d.hover(move |h| h.bg(hover)))
                .child(kit::icon(
                    look,
                    icon,
                    16.,
                    if on { t.text } else { t.muted },
                ))
                .when(w >= 1000., |d| d.child(s.name()))
                .when(n > 0, |d| d.child(kit::count(look, &t, n, tone)))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.night.answer_open = false;
                    this.select_section(s, window, cx);
                }))
        };
        let ws = self.workspace_name();
        let indexed = self
            .loaded
            .as_ref()
            .and_then(|l| l.status.as_ref())
            .map(|s| {
                (
                    s.last_ingest.map(super::kit::hm),
                    !s.errors.is_empty() || s.ingest_stale,
                )
            });
        let left = div()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap(look.px(12.))
            .pl(look.px(86.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(8.))
                    .child(
                        gpui::svg()
                            .path("hippo-mark.svg")
                            .size(look.px(26.))
                            .text_color(t.text),
                    )
                    .when(w >= 1100., |d| {
                        d.child(
                            div()
                                .text_size(look.text(17.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(t.text)
                                .child("hippo"),
                        )
                    }),
            )
            .child(
                div()
                    .id("workspace-switcher")
                    .role(gpui::Role::Button)
                    .aria_label(format!("Workspace {ws}"))
                    .h(look.px(30.))
                    .px(look.px(10.))
                    .flex()
                    .items_center()
                    .gap(look.px(6.))
                    .rounded(look.px(8.))
                    .border(look.fixed(1.))
                    .border_color(t.line2)
                    .text_size(look.text(14.))
                    .text_color(t.muted)
                    .cursor_pointer()
                    .child(ws)
                    .child(kit::icon(look, "down", 14., t.faint))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.select_section(Section::Manage, window, cx);
                    })),
            );
        let nav_bar = div()
            .flex()
            .items_center()
            .gap(look.px(2.))
            .p(look.px(4.))
            .rounded(look.px(12.))
            .bg(t.surface)
            .border(look.fixed(1.))
            .border_color(t.line)
            .child(nav(
                "nav-morning",
                Section::Morning,
                "morning",
                rules,
                Tone::You,
                cx,
            ))
            .child(nav(
                "nav-brain",
                Section::Brain,
                "brain",
                0,
                Tone::Plain,
                cx,
            ))
            .child(nav(
                "nav-agents",
                Section::Agents,
                "agents",
                waiting,
                Tone::You,
                cx,
            ))
            .child(nav(
                "nav-guard",
                Section::Guard,
                "guard",
                guard,
                Tone::Bad,
                cx,
            ));
        let hover = t.hover;
        let right = div()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .justify_end()
            .gap(look.px(12.))
            .pr(look.px(16.))
            .child(
                div()
                    .id("search")
                    .role(gpui::Role::Button)
                    .aria_label("Find a note or session")
                    .h(look.px(32.))
                    .px(look.px(10.))
                    .flex()
                    .items_center()
                    .gap(look.px(8.))
                    .rounded(look.px(9.))
                    .border(look.fixed(1.))
                    .border_color(t.line2)
                    .bg(t.surface)
                    .text_size(look.text(14.))
                    .text_color(t.faint)
                    .cursor_pointer()
                    .hover(move |h| h.bg(hover))
                    .child(kit::icon(look, "search", 15., t.faint))
                    .when(w >= 1200., |d| {
                        d.child("Find a note or session").child(
                            div()
                                .font_family(kit::MONO)
                                .text_size(look.text(11.))
                                .child("⌘P"),
                        )
                    })
                    .on_click(cx.listener(|this, _, window, cx| this.open_finder(window, cx))),
            )
            .when_some(indexed, |d, (at, bad)| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(look.px(6.))
                        .font_family(kit::MONO)
                        .text_size(look.text(13.))
                        .text_color(if bad { t.bad } else { t.muted })
                        .child(div().size(look.px(7.)).rounded_full().bg(if bad {
                            t.bad
                        } else {
                            t.ok
                        }))
                        .when(w >= 1200., |d| {
                            d.child(match at {
                                Some(at) if !bad => format!("Indexed {at}"),
                                Some(_) => "Indexing failed".to_owned(),
                                None => "Not indexed yet".to_owned(),
                            })
                        }),
                )
            })
            .child(
                kit::icon_button(
                    look,
                    &t,
                    "theme-toggle",
                    if t.dark { "sun" } else { "moon" },
                    if t.dark {
                        "Use the light theme"
                    } else {
                        "Use the dark theme"
                    },
                )
                .on_click(cx.listener(|this, _, window, cx| this.toggle_night_theme(window, cx))),
            );
        div()
            .id("top-bar")
            .flex_shrink_0()
            .h(look.px(TOP_H))
            .w_full()
            .flex()
            .items_center()
            .bg(t.canvas)
            .border_b(look.fixed(1.))
            .border_color(t.line)
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, e: &gpui::MouseDownEvent, window, _| {
                    if e.click_count == 2 {
                        window.titlebar_double_click();
                    } else {
                        this.title_drag = true;
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, _, window, _| {
                if this.title_drag {
                    this.title_drag = false;
                    window.start_window_move();
                }
            }))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, _| this.title_drag = false),
            )
            .child(left)
            .child(nav_bar)
            .child(right)
            .into_any_element()
    }

    /// `HIPPO_APP_DEV_SELECT`, once the models are loaded.
    fn apply_dev_select(&mut self) {
        let Some(sel) = self.night.dev_select.clone() else {
            return;
        };
        let Some(n) = self.loaded.as_ref().and_then(|l| l.night.as_ref()) else {
            return;
        };
        let (kind, arg) = sel.split_once(':').unwrap_or((sel.as_str(), "0"));
        let k: usize = arg.parse().unwrap_or(0);
        match kind {
            "rule" => self.night.morning = n.morning.rules.get(k).map(|r| Sel::Rule(r.id)),
            "fix" => {
                self.night.morning = n
                    .morning
                    .applied
                    .iter()
                    .chain(&n.morning.rejected)
                    .nth(k)
                    .map(|f| Sel::Fix(f.proposal))
            }
            "note" => self.night.brain = Some(PathBuf::from(arg)),
            "session" => self.night.agents = Some(arg.to_owned()),
            "incident" => self.night.guard = Some(arg.to_owned()),
            _ => {}
        }
        self.night.dev_select = None;
    }

    /// Brain's map is on screen.
    pub(crate) fn brain_shown(&self) -> bool {
        self.state.tabs.active_tab() == Some(&Tab::Brain)
    }

    /// Return, E and Delete on Morning: Keep, Edit or Dismiss the selected
    /// rule, else the first.
    pub(crate) fn morning_act(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let rules: Vec<Ulid> = self
            .loaded
            .as_ref()
            .and_then(|l| l.night.as_ref())
            .map(|n| n.morning.rules.iter().map(|r| r.id).collect())
            .unwrap_or_default();
        let rule = match &self.night.morning {
            Some(Sel::Rule(r)) if rules.contains(r) => Some(*r),
            _ => rules.first().copied(),
        };
        let Some(rule) = rule else { return };
        match id {
            "InboxKeep" => self.keep_lesson(rule, None, cx),
            "InboxEdit" => self.edit_lesson(rule, window, cx),
            _ => self.dismiss_lesson(rule, cx),
        }
    }

    /// Goes to a section's own view.
    pub(crate) fn select_section(
        &mut self,
        s: Section,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.tabs.select(s);
        if self.state.tabs.active_tab() != Some(&s.root()) {
            self.state.tabs.back_to(s.root());
        }
        window.focus(&self.pane_focus, cx);
        cx.notify();
    }

    /// The theme toggle: Hippo Dark and Hippo Light.
    pub(crate) fn toggle_night_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dark = Look::from_app(cx).c.dark;
        let id = if dark {
            "builtin:Hippo Light"
        } else {
            "builtin:Hippo Dark"
        };
        if let Ok(active) = self.themes.commit(id, window.appearance()) {
            self.show_theme(active, window, cx);
        }
    }

    /// The floating Ask bar: the field, the agent that answers, send.
    pub(crate) fn render_night_ask(
        &mut self,
        look: &Look,
        width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Nt::of(look);
        let asker = self
            .loaded
            .as_ref()
            .and_then(|l| l.night.as_ref())
            .and_then(|n| n.asker.clone());
        let answer = self
            .night
            .answer_open
            .then(|| self.render_answer_card(look, &t, cx));
        div()
            .flex()
            .flex_col()
            .gap(look.px(10.))
            .w(look.px(width))
            .children(answer)
            .child(
                div()
                    .h(look.px(56.))
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(look.px(10.))
                    .pl(look.px(20.))
                    .pr(look.px(8.))
                    .rounded(look.px(18.))
                    .bg(t.surface)
                    .border(look.fixed(1.))
                    .border_color(t.line2)
                    .shadow(kit::float_shadow(look, &t))
                    .text_size(look.text(15.))
                    .child(div().flex_1().min_w_0().child(self.ask_field.clone()))
                    .when_some(asker, |d, a| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(look.px(8.))
                                .h(look.px(34.))
                                .px(look.px(10.))
                                .rounded(look.px(10.))
                                .bg(t.inset)
                                .text_size(look.text(13.5))
                                .text_color(t.text)
                                .when(a.label != "Your model command answers", |d| {
                                    d.child(kit::badge(look, &a.agent))
                                })
                                .child(a.label),
                        )
                    })
                    .child(
                        div()
                            .id("ask-send")
                            .role(gpui::Role::Button)
                            .aria_label("Ask")
                            .size(look.px(40.))
                            .rounded(look.px(12.))
                            .bg(t.you)
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|h| h.opacity(0.9))
                            .child(kit::icon(look, "up", 18., t.you_ink))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.night_ask(window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    /// Asks from the floating bar: the answer opens above it.
    pub(crate) fn night_ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.ask_field.read(cx).text().trim().is_empty() {
            return;
        }
        self.night.answer_open = true;
        self.submit_ask_here(false, window, cx);
    }

    fn render_answer_card(&mut self, look: &Look, t: &Nt, cx: &mut Context<Self>) -> AnyElement {
        let a = &self.ask;
        let status = if a.searching {
            Some("Searching every session and note…".to_owned())
        } else if a.answering {
            Some("Writing the answer…".to_owned())
        } else {
            None
        };
        let text = match &a.answer {
            Some(Ok((Some(ans), _))) => Some(super::kit::plain(&ans.text)),
            Some(Ok((None, Some(why)))) => Some(why.clone()),
            Some(Err(e)) => Some(e.clone()),
            _ => None,
        };
        let sources: Vec<String> = match &a.result {
            Some(Ok(asked)) => asked
                .sources
                .iter()
                .take(5)
                .map(|s| s.title.clone())
                .collect(),
            Some(Err(e)) => vec![e.clone()],
            None => Vec::new(),
        };
        let open_all = cx.listener(|this, _, _, cx| {
            this.night.answer_open = false;
            this.state.tabs.open(Tab::Ask);
            cx.notify();
        });
        kit::card(look, t)
            .rounded(look.px(18.))
            .p(look.px(18.))
            .shadow(kit::float_shadow(look, t))
            .flex()
            .flex_col()
            .gap(look.px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(8.))
                    .child(
                        div()
                            .flex_1()
                            .text_size(look.text(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.text)
                            .child(a.question.clone()),
                    )
                    .child(
                        kit::icon_button(look, t, "answer-close", "x", "Close the answer")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.night.answer_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .children(status.map(|s| div().text_size(look.text(13.)).text_color(t.faint).child(s)))
            .children(text.map(|s| {
                div()
                    .text_size(look.text(14.))
                    .line_height(look.text(21.))
                    .text_color(t.text)
                    .line_clamp(8)
                    .child(s)
            }))
            .when(!sources.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(look.px(6.))
                        .child(
                            div()
                                .text_size(look.text(12.))
                                .text_color(t.faint)
                                .child("Sources"),
                        )
                        .children(
                            sources
                                .into_iter()
                                .map(|s| kit::chip(look, t, s, Tone::Plain)),
                        )
                        .child(
                            kit::button(look, t, "answer-open", None, "Open", Btn::Quiet, true)
                                .on_click(open_all),
                        ),
                )
            })
            .into_any_element()
    }

    /// The content area of a night view: the ground, the view, and the
    /// floating panel and Ask bar; below [`NARROW`], one column.
    pub(crate) fn render_night(
        &mut self,
        tab: &Tab,
        look: &Look,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Nt::of(look);
        self.apply_dev_select();
        let unit = f32::from(look.px(1.));
        let vw = f32::from(window.viewport_size().width) / unit;
        let vh = f32::from(window.viewport_size().height) / unit;
        let narrow = vw < NARROW;
        let panel_w = if vw < 1200. { PANEL_W_SMALL } else { PANEL_W };
        let left_w = if narrow { vw } else { vw - panel_w - 2. * EDGE };
        let ask_w = (left_w - 2. * 24.).clamp(280., 880.);
        let view_h = vh - TOP_H;
        let scrolls = narrow || !matches!(tab, Tab::Brain);
        let body = match tab {
            Tab::Morning => self.render_morning(look, &t, left_w, narrow, panel_w, cx),
            Tab::Brain => self.render_brain(look, &t, left_w, view_h, narrow, cx),
            _ => div().into_any_element(),
        };
        let panel = if narrow {
            None
        } else {
            match tab {
                Tab::Morning => Some(self.render_start_panel(look, &t, cx)),
                Tab::Brain => Some(self.render_note_panel(look, &t, cx)),
                _ => None,
            }
        };
        let ask = self.render_night_ask(look, ask_w, cx);

        div()
            .relative()
            .size_full()
            .bg(t.canvas)
            .child(kit::ground(look, &t, (0., 0.), 1.))
            .child(if scrolls {
                div()
                    .id("night-scroll")
                    .absolute()
                    .top_0()
                    .left_0()
                    .bottom_0()
                    .w(look.px(left_w))
                    .overflow_y_scroll()
                    .track_scroll(&self.night.scroll)
                    .child(body)
                    .child(div().h(look.px(120.)))
                    .into_any_element()
            } else {
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .child(body)
                    .into_any_element()
            })
            .children(panel.map(|p| {
                div()
                    .id("night-panel")
                    .absolute()
                    .top(look.px(EDGE))
                    .right(look.px(EDGE))
                    .bottom(look.px(EDGE))
                    .w(look.px(panel_w))
                    .rounded(look.px(18.))
                    .bg(t.surface)
                    .border(look.fixed(1.))
                    .border_color(t.line)
                    .shadow(kit::float_shadow(look, &t))
                    .overflow_y_scroll()
                    .track_scroll(&self.night.panel_scroll)
                    .p(look.px(24.))
                    .child(p)
            }))
            .child(
                div()
                    .absolute()
                    .bottom(look.px(EDGE))
                    .left_0()
                    .w(look.px(left_w))
                    .flex()
                    .justify_center()
                    .child(ask),
            )
            .into_any_element()
    }
}
