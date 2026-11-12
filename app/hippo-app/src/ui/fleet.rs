//! Fleet: today's sessions by state and each profile's headroom, with
//! Resume. The data is
//! `hippo_view::fleet`; Resume copies `hippo_view::fleet::resume`'s command
//! and handoff to the clipboard and writes nothing.

use gpui::{Context, FontWeight, Role, Window, div, prelude::*};
use hippo_view::fleet::{Fleet, Row, Shelf, State};

use super::Shell;
use super::kit::{self, Kind, Status};
use super::theme::Look;

/// How Fleet groups its rows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Group {
    #[default]
    State,
    Project,
    Account,
}

/// The Fleet view's choices.
#[derive(Debug, Clone, Default)]
pub struct FleetUi {
    pub group: Group,
    pub profile: Option<String>,
    /// The selected row's session id (↑ ↓; ↩ opens its x-ray, ⌘C or R
    /// copies its resume command).
    pub selected: Option<String>,
    /// Groups folded by hand; Settled starts folded until `unfolded`.
    pub folded: std::collections::HashSet<String>,
    pub unfolded: std::collections::HashSet<String>,
    /// The groups as the pointer found them: while it is over the list, a
    /// live refresh does not move rows.
    pub frozen: Option<Vec<(GroupKey, Vec<String>)>>,
}

/// A group's key and label.
pub type GroupKey = (String, String);

/// Whether row `r` holds every word of `filter` in its title, project,
/// profile or tool (the `/` filter).
fn filter_keeps(r: &Row, filter: &str) -> bool {
    let hay = format!(
        "{} {} {} {} {}",
        r.title,
        r.project.as_deref().unwrap_or(""),
        r.profile
            .as_deref()
            .map(kit::profile_name)
            .unwrap_or_default(),
        r.client.unwrap_or(""),
        r.tool
    )
    .to_lowercase();
    filter
        .to_lowercase()
        .split_whitespace()
        .all(|w| hay.contains(w))
}

/// Below these window widths the table drops columns.
const NO_COUNTS: f32 = 960.;
const NO_PROJECT: f32 = 860.;

impl Shell {
    /// The rows Fleet shows, in their order on screen (folded groups
    /// left out), for ↑ ↓.
    pub(crate) fn fleet_rows(&self) -> Vec<Row> {
        let Some(fleet) = self.loaded.as_ref().and_then(|l| l.fleet.as_ref()) else {
            return Vec::new();
        };
        let ui = &self.fleet_ui;
        self.fleet_groups(fleet)
            .into_iter()
            .filter(|(k, _, _)| {
                !(ui.folded.contains(k) || (starts_folded(k) && !ui.unfolded.contains(k)))
            })
            .flat_map(|(_, _, rows)| rows.into_iter().cloned())
            .collect()
    }

    /// Fleet's keys: ↑ ↓ select a row, ↩ opens its x-ray.
    pub(super) fn fleet_key(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if key == "l" {
            self.replay.lanes = true;
            cx.notify();
            return true;
        }
        if key == "/" {
            self.fleet_filter_shown = true;
            window.focus(
                &gpui::Focusable::focus_handle(self.fleet_filter.read(cx), cx),
                cx,
            );
            return true;
        }
        let rows = self.fleet_rows();
        if rows.is_empty() {
            return false;
        }
        let at = self
            .fleet_ui
            .selected
            .as_ref()
            .and_then(|id| rows.iter().position(|r| r.id == *id));
        match key {
            "down" | "j" => {
                let i = at.map_or(0, |i| (i + 1).min(rows.len() - 1));
                self.fleet_ui.selected = Some(rows[i].id.clone());
            }
            "up" | "k" => {
                let i = at.map_or(0, |i| i.saturating_sub(1));
                self.fleet_ui.selected = Some(rows[i].id.clone());
            }
            "enter" => {
                let Some(i) = at else { return false };
                let id = rows[i].id.clone();
                self.open_xray(&id, window, cx);
            }
            "r" => return self.copy_selected_resume(cx),
            _ => return false,
        }
        cx.notify();
        true
    }

    /// ⌘C on Fleet: the selected row's resume command and handoff.
    pub(crate) fn copy_selected_resume(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(id) = self.fleet_ui.selected.clone() else {
            return false;
        };
        let Some(row) = self.fleet_rows().into_iter().find(|r| r.id == id) else {
            return false;
        };
        self.copy_resume(row, None, cx);
        true
    }

    /// Copies `row`'s resume command and handoff to the clipboard, on its
    /// own profile or on `other`.
    pub(crate) fn copy_resume(&mut self, row: Row, other: Option<String>, cx: &mut Context<Self>) {
        let Some(home) = self
            .loaded
            .as_ref()
            .and_then(|l| l.setup.config())
            .map(|c| c.home.clone())
        else {
            return;
        };
        let room = other.and_then(|p| {
            self.loaded
                .as_ref()
                .and_then(|l| l.fleet.as_ref())
                .and_then(|f| f.room(&p).cloned())
        });
        let key = format!("resume:{}", row.id);
        cx.spawn(async move |this, cx| {
            let text = cx
                .background_executor()
                .spawn(async move {
                    let index = hippo_core::index::Index::open(&home, false).ok();
                    hippo_view::fleet::resume(&row, index.as_ref(), room.as_ref())
                        .map(|r| r.clipboard())
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(text) = text {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                    this.copied = Some(key);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The Resume button for a stopped row (it acts): on the
    /// profile with the most headroom when the row's own profile is at its
    /// limit, through the confirm sheet; ⌘C still copies the command.
    pub(super) fn resume_button(
        &self,
        row: &Row,
        fleet: &Fleet,
        key: usize,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let other = Self::resume_elsewhere(row, fleet);
        let t3 = row.client == Some("T3 Code");
        let label = match (&other, t3) {
            (_, true) => "Open in T3 Code".to_owned(),
            (Some(p), false) => format!("Resume on {}", kit::profile_name(p)),
            (None, false) => "Resume".to_owned(),
        };
        let tip = match (&other, t3) {
            (_, true) => "Opens this thread in T3 Code, where you pick the account to go on with.".to_owned(),
            (Some(p), false) => format!(
                "Shows the command first: it copies the transcript into {}'s folder (cp -n: the original stays, nothing is overwritten), then resumes there in Terminal. ⌘C copies it instead.",
                kit::profile_name(p)
            ),
            (None, false) => "Resumes it in Terminal (or brings forward the app it still runs in). ⌘C copies the command and a short handoff instead.".to_owned(),
        };
        let row = row.clone();
        kit::button(
            look,
            ("resume", key),
            label,
            if other.is_some() {
                Kind::Primary
            } else {
                Kind::Plain
            },
        )
        .tooltip(super::tooltip::Tooltip::text(tip))
        .on_click(cx.listener(move |this, _, window, cx| {
            cx.stop_propagation();
            this.open_session(row.clone(), window, cx);
        }))
        .into_any_element()
    }

    pub(super) fn render_fleet_tab(
        &mut self,
        look: &Look,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(fleet) = self.loaded.as_ref().and_then(|l| l.fleet.clone()) else {
            let why = self
                .loaded
                .as_ref()
                .and_then(|l| l.fleet_error.clone())
                .unwrap_or_else(|| "Reading the fleet…".to_owned());
            return div()
                .p(look.px(20.))
                .text_color(look.c.text_muted)
                .child(why)
                .into_any_element();
        };
        // The day as lanes.
        if self.replay.lanes {
            let lanes = self.render_lanes(look, window, cx);
            return div()
                .id("fleet-keys")
                .key_context("Fleet")
                .track_focus(&self.fleet_focus)
                .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, window, cx| {
                    let k = &e.keystroke;
                    if k.modifiers.platform || k.modifiers.control || k.modifiers.alt {
                        return;
                    }
                    if this.replay_key(k.key.as_str(), window, cx) {
                        cx.stop_propagation();
                    }
                }))
                .size_full()
                .child(lanes)
                .into_any_element();
        }
        let width = f32::from(window.viewport_size().width);
        // Below 900 points the accounts are one menu button and the filter
        // field shows only after `/`.
        let narrow = width < super::home::COMPACT;
        let counts = width >= NO_COUNTS;
        let project_col = width >= NO_PROJECT;
        let ui = self.fleet_ui.clone();
        let now = time::OffsetDateTime::now_utc();

        let toolbar = self.fleet_toolbar(&fleet, narrow, project_col, look, cx);
        let col = |w: f32| div().flex_shrink_0().w(look.px(w)).min_w_0();
        let header = div()
            .flex()
            .items_center()
            .gap(look.px(14.))
            .h(look.px(32.))
            .px(look.px(10.))
            .border_b(look.fixed(1.))
            .border_color(look.c.border)
            .text_size(look.text(12.))
            .text_color(look.c.faint)
            .child(div().w(look.px(24.)).flex_shrink_0())
            .child(div().flex_1().min_w_0().child("Session"))
            .when(project_col, |d| d.child(col(PROJECT_W).child("Project")))
            .when(counts, |d| {
                d.child(
                    col(CLAIMS_W).child(
                        div()
                            .id("fleet-claims-head")
                            .child("Claims")
                            .tooltip(super::tooltip::Tooltip::text(
                                "What it said it did, checked against the transcript: ✓ shown by a command or edit, ⚠ not",
                            )),
                    ),
                )
                    .child(col(PRS_W).child("Pull requests"))
            })
            .child(col(STATE_W).flex().justify_end().child("State"));

        let groups = self.fleet_groups(&fleet);
        let mut body: Vec<gpui::AnyElement> = Vec::new();
        let mut i = 0usize;
        for (key, label, rows) in groups {
            let folded =
                ui.folded.contains(&key) || (starts_folded(&key) && !ui.unfolded.contains(&key));
            // A delegated session sits indented under its parent
            // and counts with it.
            let n = crate::shelving::count_own(&rows);
            body.push(self.fleet_group_header(&key, &label, n, folded, look, cx));
            if folded {
                continue;
            }
            for r in &rows {
                let row = self.fleet_row(r, &fleet, i, project_col, counts, now, look, cx);
                body.push(if crate::shelving::nested_in(r, &rows) {
                    nested(look, row, 26.)
                } else {
                    row
                });
                i += 1;
            }
        }
        if body.is_empty() {
            body.push(
                div()
                    .px(look.px(10.))
                    .py(look.px(14.))
                    .text_color(look.c.text_muted)
                    .child(if self.fleet_filter_text.trim().is_empty() {
                        "No session today yet."
                    } else {
                        "No session matches the filter."
                    })
                    .into_any_element(),
            );
        }
        let page = div()
            .flex()
            .flex_col()
            .w_full()
            .px(look.px(16.))
            .pt(look.px(8.))
            .pb(look.px(20.))
            .text_size(look.text(13.))
            .child(toolbar)
            .children(self.render_reach_said(look))
            .child(header)
            .children(body);
        div()
            .id("fleet-keys")
            .key_context("Fleet")
            .track_focus(&self.fleet_focus)
            .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, window, cx| {
                let k = &e.keystroke;
                if k.modifiers.platform || k.modifiers.control || k.modifiers.alt {
                    return;
                }
                if this.fleet_key(k.key.as_str(), window, cx) {
                    cx.stop_propagation();
                }
            }))
            .relative()
            .size_full()
            .child(
                div()
                    .id("fleet")
                    .size_full()
                    .on_hover(cx.listener(|this, over: &bool, _, cx| {
                        if *over {
                            if this.fleet_ui.frozen.is_none() {
                                let snap =
                                    this.loaded
                                        .as_ref()
                                        .and_then(|l| l.fleet.as_ref())
                                        .map(|f| {
                                            this.fleet_groups_live(f)
                                                .into_iter()
                                                .map(|(k, l, r)| {
                                                    (
                                                        (k, l),
                                                        r.iter().map(|r| r.id.clone()).collect(),
                                                    )
                                                })
                                                .collect()
                                        });
                                this.fleet_ui.frozen = snap;
                            }
                        } else if this.fleet_ui.frozen.take().is_some() {
                            cx.notify();
                        }
                    }))
                    .overflow_y_scroll()
                    .track_scroll(&self.page_scroll)
                    .child(page),
            )
            .children(super::scrollbar::vertical(&self.page_scroll, look))
            .into_any_element()
    }

    /// The rows Fleet shows, grouped: `(key, label, rows)` in order, empty
    /// groups left out. State groups are the sidebar's shelves:
    /// Needs you, Working, Finished (to review) and Settled.
    pub(crate) fn fleet_groups<'a>(&self, fleet: &'a Fleet) -> Vec<(String, String, Vec<&'a Row>)> {
        let live = self.fleet_groups_live(fleet);
        match &self.fleet_ui.frozen {
            Some(snap) => crate::shelving::freeze(
                live.into_iter().map(|(k, l, r)| ((k, l), r)).collect(),
                snap,
                |r: &&Row| r.id.as_str(),
            )
            .into_iter()
            .map(|((k, l), r)| (k, l, r))
            .collect(),
            None => live,
        }
    }

    /// The groups as the fleet has them now.
    fn fleet_groups_live<'a>(&self, fleet: &'a Fleet) -> Vec<(String, String, Vec<&'a Row>)> {
        let ui = &self.fleet_ui;
        let text = self.fleet_filter_text.clone();
        let keep =
            |r: &&Row| (ui.profile.is_none() || r.profile == ui.profile) && filter_keeps(r, &text);
        match ui.group {
            Group::State => state_groups(&crate::shelving::Shelves::of(fleet, &self.marks()))
                .into_iter()
                .map(|(key, label, rows)| {
                    (
                        key.to_owned(),
                        label.to_owned(),
                        rows.into_iter().filter(keep).collect::<Vec<_>>(),
                    )
                })
                .filter(|(_, _, rows)| !rows.is_empty())
                .collect(),
            Group::Project | Group::Account => {
                let mut out: Vec<(String, String, Vec<&Row>)> = Vec::new();
                for r in fleet.rows.iter().filter(keep) {
                    let label = match ui.group {
                        Group::Project => r.project.clone().unwrap_or_else(|| "No project".into()),
                        _ => r
                            .profile
                            .as_deref()
                            .map(kit::profile_name)
                            .unwrap_or_else(|| r.tool.clone()),
                    };
                    let key = format!("g:{label}");
                    match out.iter_mut().find(|g| g.0 == key) {
                        Some(g) => g.2.push(r),
                        None => out.push((key, label, vec![r])),
                    }
                }
                out
            }
        }
    }

    fn fleet_toolbar(
        &self,
        fleet: &Fleet,
        narrow: bool,
        wide_filter: bool,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let ui = &self.fleet_ui;
        let groups = [
            (Group::State, "State"),
            (Group::Project, "Project"),
            (Group::Account, "Account"),
        ];
        let group_seg = groups.iter().fold(kit::segmented(look), |d, (g, name)| {
            let g = *g;
            d.child(
                kit::segment(look, *name, None, *name, ui.group == g).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.fleet_ui.group = g;
                        cx.notify();
                    },
                )),
            )
        });
        let mut profiles: Vec<String> = fleet
            .rows
            .iter()
            .filter_map(|r| r.profile.clone())
            .collect();
        profiles.sort_by_key(|p| kit::profile_name(p).to_lowercase());
        profiles.dedup();
        let accounts: gpui::AnyElement = if narrow {
            let label = match &ui.profile {
                Some(p) => kit::profile_name(p),
                None => "All accounts".to_owned(),
            };
            let open = self.fleet_profiles_open;
            let items: Vec<gpui::AnyElement> = std::iter::once(None)
                .chain(profiles.iter().map(Some))
                .enumerate()
                .map(|(i, p)| {
                    let p2 = p.cloned();
                    div()
                        .id(("account-menu-item", i))
                        .flex()
                        .items_center()
                        .gap(look.px(8.))
                        .h(look.px(28.))
                        .px(look.px(8.))
                        .rounded(look.px(6.))
                        .cursor_pointer()
                        .hover(|h| h.bg(look.c.hover))
                        .children(p.map(|p| kit::mark(look, Some(p), 16.)))
                        .child(
                            p.map_or_else(|| "All accounts".to_owned(), |p| kit::profile_name(p)),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.fleet_ui.profile = p2.clone();
                            this.fleet_profiles_open = false;
                            cx.notify();
                        }))
                        .into_any_element()
                })
                .collect();
            div()
                .relative()
                .child(kit::segmented(look).child(
                    kit::segment(look, "account-menu", None, format!("{label} ▾"), false).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.fleet_profiles_open = !this.fleet_profiles_open;
                            cx.notify();
                        }),
                    ),
                ))
                .when(open, |d| {
                    d.child(gpui::deferred(
                        gpui::anchored().child(
                            div()
                                .id("account-menu-pop")
                                .mt(look.px(32.))
                                .min_w(look.px(180.))
                                .p(look.px(4.))
                                .rounded(look.px(10.))
                                .bg(look.c.surface)
                                .border(look.fixed(1.))
                                .border_color(look.c.border)
                                .shadow_lg()
                                .flex()
                                .flex_col()
                                .children(items)
                                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                    this.fleet_profiles_open = false;
                                    cx.notify();
                                })),
                        ),
                    ))
                })
                .into_any_element()
        } else {
            let all = kit::segment(
                look,
                "account-all",
                None,
                "All accounts",
                ui.profile.is_none(),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.fleet_ui.profile = None;
                cx.notify();
            }));
            profiles
                .iter()
                .fold(kit::segmented(look).child(all), |d, p| {
                    let on = ui.profile.as_deref() == Some(p.as_str());
                    let p2 = p.clone();
                    d.child(
                        kit::segment_lead(
                            look,
                            gpui::SharedString::from(format!("account-{p}")),
                            kit::mark(look, Some(p), 16.),
                            kit::profile_name(p),
                            on,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.fleet_ui.profile = Some(p2.clone());
                            cx.notify();
                        })),
                    )
                })
                .into_any_element()
        };
        let filter_box = div()
            .flex_shrink_0()
            .w(look.px(if wide_filter { 176. } else { 150. }))
            .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, window, cx| {
                if e.keystroke.key == "escape" {
                    this.fleet_filter
                        .update(cx, |f, cx| f.set_text("", window, cx));
                    this.fleet_filter_text.clear();
                    this.fleet_filter_shown = false;
                    window.focus(&this.fleet_focus, cx);
                    cx.stop_propagation();
                }
            }))
            .child(self.fleet_filter.clone());
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(look.px(10.))
            .pb(look.px(12.))
            .child(self.replay_switch(look, cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(8.))
                    .child(kit::icon(look, "layers", 16., look.c.faint))
                    .child(group_seg),
            )
            .child(accounts)
            .child(div().flex_1())
            .when(!narrow || self.fleet_filter_shown, |d| d.child(filter_box))
            .into_any_element()
    }

    fn fleet_group_header(
        &self,
        key: &str,
        label: &str,
        n: usize,
        folded: bool,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let k = key.to_owned();
        let warn = key == "needs";
        div()
            .id(gpui::SharedString::from(format!("fleet-group-{key}")))
            .role(Role::Button)
            .aria_label(format!("{label} {n}"))
            .flex()
            .items_center()
            .gap(look.px(6.))
            .h(look.px(34.))
            .px(look.px(10.))
            .border_b(look.fixed(1.))
            .border_color(look.c.border)
            .text_size(look.text(12.5))
            .font_weight(FontWeight::MEDIUM)
            .text_color(look.c.text_muted)
            .cursor_pointer()
            .child(label.to_owned())
            .child(if warn {
                kit::count_badge(look, n, true).into_any_element()
            } else {
                div()
                    .text_color(look.c.faint)
                    .child(n.to_string())
                    .into_any_element()
            })
            .child(kit::icon(
                look,
                if folded {
                    "chevron-right"
                } else {
                    "chevron-down"
                },
                14.,
                look.c.faint,
            ))
            .on_click(cx.listener(move |this, _, _, cx| {
                let ui = &mut this.fleet_ui;
                let settled_default = starts_folded(&k);
                let now_folded =
                    ui.folded.contains(&k) || (settled_default && !ui.unfolded.contains(&k));
                if now_folded {
                    ui.folded.remove(&k);
                    ui.unfolded.insert(k.clone());
                } else {
                    ui.unfolded.remove(&k);
                    ui.folded.insert(k.clone());
                }
                cx.notify();
            }))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn fleet_row(
        &self,
        r: &Row,
        fleet: &Fleet,
        i: usize,
        project_col: bool,
        counts: bool,
        now: time::OffsetDateTime,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let col = |w: f32| div().flex_shrink_0().w(look.px(w)).min_w_0();
        let selected = self.fleet_ui.selected.as_deref() == Some(r.id.as_str());
        let settled = matches!(
            self.marks().place(r),
            crate::shelving::Place::Shelf(Shelf::Settled)
        );
        let working = r.state.is_live() || r.call.is_some();
        let open = selected && working && r.turn.as_ref().is_some_and(|t| t.calls.len() > 1);
        // No tooltip over the whole row (it covered the open row's
        // trail); when it started, its turns and client are in the x-ray.

        // The second line: the tool line while it works, the question
        // while it waits, else what it left.
        let second: gpui::AnyElement = match (&r.state, &r.call) {
            (State::Waiting { asked, plan, .. }, _) => div()
                .text_size(look.text(12.5))
                .text_color(look.c.text_muted)
                .line_clamp(2)
                .text_ellipsis()
                .child(match (asked, plan) {
                    (_, true) => "A plan waits for your approval.".to_owned(),
                    (Some(q), _) => format!("“{q}”"),
                    (None, _) => "It asked you something.".to_owned(),
                })
                .into_any_element(),
            (_, Some(call)) => {
                // The same step run again reads once, `×n`.
                let runs = r.turn.as_ref().map(|t| kit::runs(&t.calls));
                let (shown, n) = match runs.as_ref().and_then(|v| v.first()) {
                    Some(run) if kit::same_step(&run.call, call) => (run.call.clone(), run.n),
                    _ => (call.clone(), 1),
                };
                let age = call_age(&shown, now);
                kit::tool_line(look, &shown, n, Some(age), false).into_any_element()
            }
            _ => div()
                .truncate()
                .text_size(look.text(12.5))
                .text_color(if r.claims.unverified > 0 {
                    look.c.warn
                } else {
                    look.c.faint
                })
                .child(sub_line(r))
                .into_any_element(),
        };
        let trail = open.then(|| self.fleet_trail(r, now, !project_col, look));
        let (status, detail) = row_state(r, now);
        let state_cell = div()
            .flex()
            .flex_col()
            .items_end()
            .gap(look.px(8.))
            .child(kit::status(look, status, detail))
            .when(r.state.is_waiting(), |d| {
                let row = r.clone();
                d.child(
                    kit::button(look, ("fleet-open-thread", i), "Open thread", Kind::Primary)
                        .child(kit::icon(look, "arrow-up-right", 13., look.c.primary_text))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.open_session(row.clone(), window, cx);
                        })),
                )
            })
            .when(r.state.needs_resume(), |d| {
                d.child(self.resume_button(r, fleet, 500 + i, look, cx))
            });
        let claims_cell = if r.claims.unverified > 0 {
            div()
                .flex()
                .items_center()
                .gap(look.px(4.))
                .text_color(look.c.warn)
                .child(kit::icon(look, "triangle-alert", 13., look.c.warn))
                .child(r.claims.unverified.to_string())
        } else if r.claims.verified > 0 {
            div()
                .flex()
                .items_center()
                .gap(look.px(4.))
                .text_color(look.c.ok)
                .child(kit::icon(look, "check", 13., look.c.ok))
                .child(r.claims.verified.to_string())
        } else {
            div().text_color(look.c.faint).child("—")
        };
        // The cell says what it counts on hover.
        let claims_tip = match (r.claims.unverified, r.claims.verified) {
            (0, 0) => None,
            (0, v) => Some(format!(
                "{v} claim{} checked against the transcript",
                if v == 1 { "" } else { "s" }
            )),
            (u, _) => Some(format!(
                "{u} claim{} the transcript does not show",
                if u == 1 { "" } else { "s" }
            )),
        };
        let claims_cell = div()
            .id(("fleet-claims", i))
            .child(claims_cell)
            .when_some(claims_tip, |d, t| {
                d.tooltip(super::tooltip::Tooltip::text(t))
            });
        let (opened, merged) = pr_counts(r);
        let prs_cell = match (opened, merged) {
            (0, 0) => div().text_color(look.c.faint).child("—"),
            (o, m) => div()
                .flex()
                .items_center()
                .gap(look.px(5.))
                .text_color(look.c.text_muted)
                .child(kit::icon(
                    look,
                    if o > 0 {
                        "git-pull-request"
                    } else {
                        "git-merge"
                    },
                    13.,
                    look.c.text_muted,
                ))
                .when(o > 0, |d| {
                    d.child(div().text_color(look.c.text).child(format!("{o} open")))
                })
                .when(o > 0 && m > 0, |d| {
                    d.child(div().text_color(look.c.faint).child("·"))
                })
                .when(m > 0, |d| d.child(format!("{m} merged"))),
        };
        let project = div()
            .flex()
            .flex_col()
            .text_size(look.text(12.5))
            .child(
                div()
                    .truncate()
                    .text_color(look.c.text_muted)
                    .child(r.project.clone().unwrap_or_else(|| "—".into())),
            )
            .children(r.branch.as_ref().map(|b| {
                // The branch under the project, cut in the middle so its
                // distinctive end shows; nothing where none was named.
                // The client is in the row's tooltip.
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(4.))
                    .min_w_0()
                    .text_color(look.c.faint)
                    .child(kit::icon(look, "git-branch", 12., look.c.faint))
                    .child(div().min_w_0().truncate().child(kit::middle(b, 18)))
            }));
        let id = r.id.clone();
        let main = div()
            .flex()
            .items_start()
            .gap(look.px(14.))
            .child(kit::mark(look, r.profile.as_deref(), 24.).mt(look.px(1.)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(look.px(3.))
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .text_size(look.text(13.5))
                            // Titles at 400, T3 Code's weight as
                            // GPUI draws it.
                            .text_color(if settled {
                                look.c.text_muted
                            } else {
                                look.c.text
                            })
                            .child(kit::title(&r.title)),
                    )
                    .child(second),
            )
            .when(project_col, |d| d.child(col(PROJECT_W).child(project)))
            .when(counts, |d| {
                d.child(col(CLAIMS_W).pt(look.px(1.)).child(claims_cell))
                    .child(col(PRS_W).pt(look.px(1.)).child(prs_cell))
            })
            .child(col(STATE_W).flex().justify_end().child(state_cell));
        let row = div()
            .id(("fleet-row", i))
            .role(Role::Row)
            .aria_label(r.title.clone())
            .aria_selected(selected)
            .flex()
            .flex_col()
            .px(look.px(10.))
            .py(look.px(10.))
            .border_b(look.fixed(1.))
            .border_color(look.c.border)
            .cursor_pointer()
            .when(open, |d| {
                d.rounded(look.px(10.)).border_1().bg(look.c.raised)
            })
            .child(main)
            .children(trail)
            .on_click(cx.listener(move |this, e: &gpui::ClickEvent, window, cx| {
                // A click selects (a live row opens its trail); a second
                // click, a double-click or ↩ opens the x-ray.
                let again = this.fleet_ui.selected.as_deref() == Some(id.as_str());
                if again || e.click_count() >= 2 {
                    this.open_xray(&id, window, cx);
                } else {
                    this.fleet_ui.selected = Some(id.clone());
                    cx.notify();
                }
            }));
        kit::row_fill(row, look, selected && !open).into_any_element()
    }

    /// The open live row's trail: its turn's calls before the
    /// newest, then the branch foot.
    fn fleet_trail(
        &self,
        r: &Row,
        now: time::OffsetDateTime,
        narrow: bool,
        look: &Look,
    ) -> gpui::AnyElement {
        // Runs of the same step, after the newest.
        let runs: Vec<kit::Run> = r
            .turn
            .as_ref()
            .map(|t| {
                kit::runs(&t.calls)
                    .into_iter()
                    .skip(1)
                    .take(TRAIL)
                    .collect()
            })
            .unwrap_or_default();
        let lines = runs.iter().map(|run| {
            let c = &run.call;
            div()
                .flex()
                .items_center()
                .gap(look.px(10.))
                .h(look.px(24.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(kit::tool_line(look, c, run.n, None, false)),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(look.text(12.))
                        .text_color(look.c.faint)
                        .child(short_age(now - c.at)),
                )
        });
        // Each sub-agent writing now, under its own calls.
        let subs = live_subs(look, &r.live_subs, now).map(|d| d.mt(look.px(6.)));
        div()
            .flex()
            .flex_col()
            .pl(look.px(38.))
            .pt(look.px(6.))
            .children(lines)
            .children(subs)
            .child(
                div()
                    .mt(look.px(8.))
                    .pt(look.px(10.))
                    .border_t(look.fixed(1.))
                    .border_color(look.c.border)
                    .child(branch_foot(r, now, narrow, look)),
            )
            .into_any_element()
    }

    /// The selected row, for the title bar's actions.
    pub(crate) fn fleet_selected_row(&self) -> Option<Row> {
        let id = self.fleet_ui.selected.as_ref()?;
        // Only a row on screen: a filter or a folded group hides it.
        self.fleet_rows().into_iter().find(|r| &r.id == id)
    }
}

/// Fleet's State groups (including Snoozed): the sidebar's
/// shelves, `(key, label, rows)`, in its order, empty ones kept.
pub fn state_groups<'a>(
    s: &crate::shelving::Shelves<'a>,
) -> Vec<(&'static str, &'static str, Vec<&'a Row>)> {
    vec![
        ("needs", "Needs you", s.rows(Shelf::NeedsYou).to_vec()),
        ("working", "Working", s.rows(Shelf::Working).to_vec()),
        // The sidebar's word for the same sessions.
        ("finished", "To review", s.rows(Shelf::ToReview).to_vec()),
        (
            "snoozed",
            "Snoozed",
            s.snoozed.iter().map(|(r, _)| *r).collect(),
        ),
        ("settled", "Settled", s.rows(Shelf::Settled).to_vec()),
    ]
}

/// Groups that start folded: Settled and Snoozed.
pub fn starts_folded(key: &str) -> bool {
    matches!(key, "settled" | "snoozed")
}

/// A delegated session's row under its parent's: inset by
/// `inset` points behind a faint rule, as T3 Code draws a sub-thread.
pub fn nested(look: &Look, row: gpui::AnyElement, inset: f32) -> gpui::AnyElement {
    div()
        .flex()
        .child(
            div()
                .flex_shrink_0()
                .w(look.px(inset))
                .flex()
                .justify_end()
                .pr(look.px(8.))
                .child(div().w(look.fixed(1.)).h_full().bg(look.c.border)),
        )
        .child(div().flex_1().min_w_0().child(row))
        .into_any_element()
}

/// Calls the open row's trail lists under the newest.
const TRAIL: usize = 4;

/// Live sub-agents listed by name before `+n more`.
pub const LIVE_SUBS: usize = 3;

/// Each sub-agent writing now with its current step: `◌ Crop
/// covers to square · editing crop.rs   12s`, at most [`LIVE_SUBS`], then
/// `+n more`. `None` when none is writing.
pub fn live_subs(
    look: &Look,
    subs: &[hippo_view::fleet::LiveSub],
    now: time::OffsetDateTime,
) -> Option<gpui::Div> {
    if subs.is_empty() {
        return None;
    }
    let lines = subs.iter().take(LIVE_SUBS).map(|s| {
        div()
            .flex()
            .items_center()
            .gap(look.px(8.))
            .h(look.px(24.))
            .text_size(look.text(12.5))
            .child(kit::icon(look, "circle-dashed", 13., look.c.info))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .gap(look.px(6.))
                    .child(
                        div()
                            .flex_shrink_0()
                            .max_w(gpui::relative(0.5))
                            .truncate()
                            .text_color(look.c.text)
                            .child(s.title.clone().unwrap_or_else(|| "Sub-agent".to_owned())),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(look.c.text_muted)
                            .child(s.step.clone()),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(look.text(12.))
                    .text_color(look.c.faint)
                    .child(short_age(now - s.at)),
            )
    });
    let more = subs.len().saturating_sub(LIVE_SUBS);
    Some(div().flex().flex_col().children(lines).when(more > 0, |d| {
        d.child(
            div()
                .pl(look.px(21.))
                .text_size(look.text(12.))
                .text_color(look.c.faint)
                .child(format!("+{more} more")),
        )
    }))
}

/// Column widths.
const PROJECT_W: f32 = 150.;
const CLAIMS_W: f32 = 52.;
const PRS_W: f32 = 132.;
const STATE_W: f32 = 140.;

/// A row's pull requests as `(open, merged)`: one it opened that another
/// session merged counts as merged (the sidebar, Fleet and the branch foot
/// agree).
pub fn pr_counts(row: &Row) -> (usize, usize) {
    let open = row
        .pulls
        .iter()
        .filter(|(u, d)| {
            *d == "opened"
                && !row.merged_elsewhere.contains(u)
                && !row.pulls.iter().any(|(v, e)| v == u && *e == "merged")
        })
        .count();
    let merged =
        row.pulls.iter().filter(|(_, d)| *d == "merged").count() + row.merged_elsewhere.len();
    (open, merged)
}

/// `now`, `3m`, `2h`.
pub fn short_age(d: time::Duration) -> String {
    let s = d.whole_seconds().max(0);
    if s < 60 {
        "now".to_owned()
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86_400 {
        format!("{}h", s / 3600)
    } else {
        format!("{}d", s / 86_400)
    }
}

/// A running call's time so far (`2m 10s`), else its age (`3m`).
pub fn call_age(c: &hippo_view::fleet::tail::Activity, now: time::OffsetDateTime) -> String {
    let s = (now - c.at).whole_seconds().max(0);
    if c.running {
        if s < 60 {
            format!("{s}s")
        } else if s < 3600 {
            format!("{}m {}s", s / 60, s % 60)
        } else {
            format!("{}h {}m", s / 3600, (s % 3600) / 60)
        }
    } else {
        short_age(now - c.at)
    }
}

/// A row's state as the glyph and word of the State column.
pub fn row_state(r: &Row, now: time::OffsetDateTime) -> (Status, Option<String>) {
    if let Some(s) = super::sidebar::row_status(r, now) {
        return s;
    }
    match &r.state {
        State::Finished { at } => (Status::Done, Some(kit::hm(*at))),
        State::Stopped { at, .. } => (Status::Done, Some(format!("carried on {}", kit::hm(*at)))),
        State::Quiet { since: Some(t) } => (Status::Quiet, Some(format!("since {}", kit::hm(*t)))),
        State::Quiet { since: None } => (Status::Quiet, None),
        State::Interrupted { at, .. } => (Status::Interrupted, Some(kit::hm(*at))),
        State::Live => (Status::Working, None),
        State::Waiting { .. } => (Status::Input, None),
    }
}

/// The branch foot: `⑂ branch +a −r · n files · PR state`
/// at the left; running sub-tasks and the context size at the right.
pub fn branch_foot(r: &Row, now: time::OffsetDateTime, narrow: bool, look: &Look) -> gpui::Div {
    let _ = now;
    let (opened, merged) = pr_counts(r);
    let pr = match (opened, merged) {
        (0, 0) => None,
        (o, 0) => Some(format!(
            "{o} pull request{} open",
            if o == 1 { "" } else { "s" }
        )),
        (_, m) => Some(format!("{m} merged")),
    };
    let running = r
        .tasks
        .iter()
        .filter(|t| t.running(&r.state))
        .count()
        .max(r.live_subs.len());
    let mut left = div()
        .flex()
        .items_center()
        .gap(look.px(6.))
        .min_w_0()
        .flex_1()
        .overflow_hidden()
        .text_size(look.text(12.))
        .text_color(look.c.text_muted);
    left = match &r.branch {
        Some(b) => left
            .child(kit::icon(look, "git-branch", 14., look.c.faint))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_family(kit::MONO)
                    .text_size(look.text(11.5))
                    .text_color(look.c.text)
                    .font_weight(FontWeight::MEDIUM)
                    .child(b.clone()),
            ),
        None => left,
    };
    // The facts after the branch are one line that ends in an
    // ellipsis when the row is narrow, never cut mid-word; under 900
    // points the context size, the lowest, is dropped whole.
    let mut facts: Vec<String> = Vec::new();
    // Its sub-agents' lines count too, each file once.
    let lines = r.total_lines();
    if let Some(l) = lines {
        left = left.children(kit::diff_counts(look, l.added, l.removed));
        facts.push(format!(
            "{} file{}",
            l.files,
            if l.files == 1 { "" } else { "s" }
        ));
        if r.sub.editing > 0 {
            facts.push(format!(
                "with {} sub-agent{}",
                r.sub.editing,
                if r.sub.editing == 1 { "" } else { "s" }
            ));
        }
    }
    facts.extend(pr);
    if !facts.is_empty() {
        let lead = if r.branch.is_some() || lines.is_some() {
            "· "
        } else {
            ""
        };
        left = left.child(
            div()
                .min_w_0()
                .truncate()
                .text_color(look.c.faint)
                .child(format!("{lead}{}", facts.join(" · "))),
        );
    }
    let right = div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(look.px(12.))
        .text_size(look.text(12.))
        .text_color(look.c.text_muted)
        .when(running > 0, |d| {
            d.child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(5.))
                    .child(kit::icon(look, "circle-dot-dashed", 13., look.c.text_muted))
                    .child(format!(
                        "{running} sub-task{} running",
                        if running == 1 { "" } else { "s" }
                    )),
            )
        })
        .children(
            r.turn
                .as_ref()
                .and_then(|t| t.tokens)
                .filter(|_| !narrow)
                .map(|n| {
                    div()
                        .text_color(look.c.faint)
                        .child(format!("{} tokens", hippo_view::fleet::compact(n as f64)))
                }),
        );
    div()
        .flex()
        .items_center()
        .gap(look.px(12.))
        .child(left)
        .child(right)
}

/// A row's second line (one pattern, never the account, which
/// the mark shows): what it is doing, `<activity> · running 33m`; how it
/// stopped, `stopped 14:31 · resets 6pm`; else what it did, `ran 2h 10m ·
/// 13 edits`. An unverified claim leads in any state.
fn sub_line(r: &Row) -> String {
    let now = time::OffsetDateTime::now_utc();
    let mut f: Vec<String> = Vec::new();
    let ran = |f: &mut Vec<String>| {
        if let (Some(s), Some(l)) = (r.started, r.last) {
            f.push(format!("ran {}", kit::span(l - s)));
        }
    };
    match &r.state {
        State::Live | State::Quiet { .. } if r.activity.is_some() => {
            f.extend(r.activity.clone());
            if let Some(s) = r.started {
                f.push(format!("running {}", kit::span(now - s)));
            }
        }
        State::Live => {
            if let Some(s) = r.started {
                f.push(format!("running {}", kit::span(now - s)));
            }
        }
        State::Stopped {
            at,
            reset,
            carried_on,
        } => {
            f.push(format!("stopped {}", kit::hm(*at)));
            if *carried_on {
                f.push("a later session carried it on".into());
            }
            f.extend(reset.clone());
        }
        State::Interrupted { detail, .. } => {
            f.extend(detail.clone());
            if f.is_empty() {
                ran(&mut f);
            }
        }
        State::Finished { .. } => ran(&mut f),
        State::Waiting { asked, plan, .. } => f.push(match (asked, plan) {
            (_, true) => "a plan to approve".to_owned(),
            (Some(q), _) => format!("asked: {q}"),
            (None, _) => "asked you something".to_owned(),
        }),
        State::Quiet { since } => {
            if let Some(t) = since {
                f.push(format!("active {}", kit::hm(*t)));
            }
        }
    }
    if let Some(line) = r.claims.line().filter(|_| r.claims.unverified > 0) {
        f.insert(0, line);
    } else if !r.state.is_live() && r.edits > 0 {
        f.push(format!(
            "{} edit{}",
            r.edits,
            if r.edits == 1 { "" } else { "s" }
        ));
    }
    f.join(" · ")
}
