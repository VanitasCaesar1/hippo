//! Incidents: what went wrong, which agent did it, at which step, and
//! whether it had been told.
//! The list (`Tab::Incidents`), an incident's page (`Tab::Incident`), the
//! title-bar filters and actions, Never again (a lesson through the
//! Inbox, or a hook to copy) and Not an incident (a preference and a toast
//! with Undo). The data is `crate::incidents`; reads only, and the app
//! writes nothing but its preferences.

use std::collections::{BTreeMap, HashSet};

use gpui::{Context, FocusHandle, FontWeight, Role, SharedString, Window, div, prelude::*};
use hippo_core::incidents::{Cause, Incident, Kind as IncidentKind};
use hippo_view::fleet::tail::Activity;
use time::OffsetDateTime;

use super::Shell;
use super::kit::{self, Kind};
use super::theme::Look;
use crate::incidents::{self as inc, Filter, NeverAgain};
use crate::state::{Section, Tab};

/// The confirm dialog's title for Never again on a correction; Manage's
/// runner labels the running command with it.
pub const DRAFT_TITLE: &str = "Draft a rule from this correction?";

/// A filter menu, open under its title-bar button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Menu {
    Cause,
    Account,
    Project,
    /// Under 900 points the three are one menu.
    All,
}

/// One choice in a filter menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    Cause(Option<Cause>),
    Account(Option<String>),
    Project(Option<String>),
}

/// A line at the window's bottom right, with Undo when a mark can be taken
/// back.
#[derive(Debug, Clone)]
pub struct Toast {
    pub seq: u64,
    pub text: String,
    /// The incident an Undo restores.
    pub undo: Option<String>,
}

/// Incidents' state.
pub struct IncidentsUi {
    /// The selected row, by incident id.
    pub selected: Option<String>,
    pub focus: FocusHandle,
    pub filter: Filter,
    pub menu: Option<Menu>,
    /// `dismissed_incidents` and `incidents_seen_at`, as the preferences
    /// hold them.
    pub dismissed: BTreeMap<String, i64>,
    pub seen_at: Option<i64>,
    pub toast: Option<Toast>,
    pub toast_seq: u64,
    /// The incident whose hook sheet is open, and what was copied there.
    pub hook_sheet: Option<String>,
    pub hook_focus: FocusHandle,
    pub copied: Option<&'static str>,
    /// Days whose folded `ran a destructive command` row is open.
    pub quiet_open: HashSet<time::Date>,
    /// Incidents whose "What it said" is shown whole.
    pub answering_open: HashSet<String>,
}

impl IncidentsUi {
    pub fn new(prefs: &crate::prefs::Prefs, cx: &mut Context<Shell>) -> IncidentsUi {
        IncidentsUi {
            selected: None,
            focus: cx.focus_handle(),
            filter: Filter::default(),
            menu: None,
            dismissed: prefs.dismissed_incidents.clone(),
            seen_at: prefs.incidents_seen_at,
            toast: None,
            toast_seq: 0,
            hook_sheet: None,
            hook_focus: cx.focus_handle(),
            copied: None,
            quiet_open: HashSet::new(),
            answering_open: HashSet::new(),
        }
    }
}

const MONO: &str = kit::MONO;

impl Shell {
    fn incident_list(&self) -> &[Incident] {
        self.loaded
            .as_ref()
            .and_then(|l| l.incidents.as_ref())
            .map_or(&[], |d| d.list.as_slice())
    }

    /// The incident `id`, unless it was marked not an incident.
    pub(crate) fn incident(&self, id: &str) -> Option<Incident> {
        if self.incidents_ui.dismissed.contains_key(id) {
            return None;
        }
        self.incident_list().iter().find(|i| i.id == id).cloned()
    }

    /// The sidebar's count: incidents newer than the page was last opened.
    pub(crate) fn incidents_new(&self) -> usize {
        inc::count_new(
            self.incident_list(),
            &self.incidents_ui.dismissed,
            self.incidents_ui.seen_at,
        )
    }

    /// Home's `n incidents today`.
    pub(crate) fn incidents_today(&self) -> usize {
        inc::count_today(
            self.incident_list(),
            &self.incidents_ui.dismissed,
            OffsetDateTime::now_utc(),
            super::status::local_offset(),
        )
    }

    /// Manage's row: `Incidents: 2 marked not an incident of 31 this week`.
    pub(crate) fn incidents_alarm_line(&self) -> Option<String> {
        let (n, m) = inc::false_alarms(self.incident_list(), &self.incidents_ui.dismissed);
        (m > 0).then(|| format!("Incidents: {n} marked not an incident of {m} this week"))
    }

    /// Opening the section sets `incidents_seen_at`.
    pub(crate) fn mark_incidents_seen(&mut self) {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        self.incidents_ui.seen_at = Some(now);
        self.save_pref(|p| p.incidents_seen_at = Some(now));
    }

    /// The list as the filters leave it.
    fn incidents_on_screen(&self) -> Vec<&Incident> {
        let f = &self.incidents_ui.filter;
        inc::shown(self.incident_list(), &self.incidents_ui.dismissed)
            .into_iter()
            .filter(|i| f.keeps(i))
            .collect()
    }

    /// The ids of the selectable rows in screen order (a folded quiet row
    /// is not one until opened), for ↑ ↓.
    fn incident_order(&self) -> Vec<String> {
        let rows = self.incidents_on_screen();
        let mut out = Vec::new();
        for d in inc::days(&rows, super::status::local_offset()) {
            out.extend(d.rows.iter().map(|i| i.id.clone()));
            if self.incidents_ui.quiet_open.contains(&d.date) {
                out.extend(d.quiet.iter().map(|i| i.id.clone()));
            }
        }
        out.extend(inc::hints(&rows).iter().map(|i| i.id.clone()));
        out
    }

    /// ↑ ↓ (J K) select a row, ↩ opens its page.
    pub(super) fn incidents_key(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let order = self.incident_order();
        if order.is_empty() {
            return false;
        }
        let at = self
            .incidents_ui
            .selected
            .as_ref()
            .and_then(|id| order.iter().position(|o| o == id));
        match key {
            "down" | "j" => {
                let i = at.map_or(0, |i| (i + 1).min(order.len() - 1));
                self.incidents_ui.selected = Some(order[i].clone());
            }
            "up" | "k" => {
                let i = at.map_or(0, |i| i.saturating_sub(1));
                self.incidents_ui.selected = Some(order[i].clone());
            }
            "enter" => {
                let Some(i) = at else { return false };
                let id = order[i].clone();
                self.open_incident(&id, window, cx);
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    /// Opens incident `id`'s page.
    pub(crate) fn open_incident(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.incidents_ui.menu = None;
        self.incidents_ui.selected = Some(id.to_owned());
        self.page_scroll.set_offset(gpui::Point::default());
        self.state.tabs.open(Tab::Incident(id.to_owned()));
        window.focus(&self.focus, cx);
        self.after_state_change(window, cx);
    }

    /// Opens the list filtered to nothing (Home's link).
    pub(crate) fn open_incidents(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_section(Section::Incidents, window, cx);
    }

    /// Shows a line at the bottom right for a few seconds.
    pub(crate) fn toast(
        &mut self,
        text: impl Into<String>,
        undo: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.incidents_ui.toast_seq += 1;
        let seq = self.incidents_ui.toast_seq;
        self.incidents_ui.toast = Some(Toast {
            seq,
            text: text.into(),
            undo,
        });
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(6))
                .await;
            this.update(cx, |this, cx| {
                if this
                    .incidents_ui
                    .toast
                    .as_ref()
                    .is_some_and(|t| t.seq == seq)
                {
                    this.incidents_ui.toast = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Not an incident: kept in the preferences, hidden everywhere, with a
    /// toast to take it back.
    pub(crate) fn dismiss_incident(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let next = inc::dismiss(&self.incidents_ui.dismissed, id, now);
        self.incidents_ui.dismissed = next.clone();
        self.save_pref(|p| p.dismissed_incidents = next);
        if self.incidents_ui.selected.as_deref() == Some(id) {
            self.incidents_ui.selected = None;
        }
        // On the incident's own page, back to the list.
        if matches!(self.state.tabs.active_tab(), Some(Tab::Incident(open)) if open == id) {
            self.state.tabs.select(Section::Incidents);
            self.after_state_change(window, cx);
        }
        self.toast("Marked not an incident", Some(id.to_owned()), cx);
    }

    fn undo_dismiss(&mut self, id: &str, cx: &mut Context<Self>) {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let next = inc::restore(&self.incidents_ui.dismissed, id, now);
        self.incidents_ui.dismissed = next.clone();
        self.save_pref(|p| p.dismissed_incidents = next);
        self.incidents_ui.toast = None;
        cx.notify();
    }

    /// Opens the step: the session's x-ray, or the sub-task's page when
    /// the step is in a sub-agent or a delegated task.
    pub(crate) fn open_incident_step(
        &mut self,
        i: &Incident,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A correction opens at its step's turn, or at the correction when
        // there is no step.
        if i.kind == hippo_core::incidents::Kind::Corrected {
            let at = if inc::has_step(i) {
                &i.body.did
            } else {
                &i.body.showed
            };
            let (id, ordinal) = (at.session.clone(), at.ordinal);
            self.sessions_ui.scroll_to = Some((id.clone(), ordinal));
            self.state.tabs.follow(
                Tab::Session {
                    id,
                    ordinal: Some(ordinal),
                },
                false,
            );
            self.after_state_change(window, cx);
            return;
        }
        let chain = &i.body.chain;
        let target = if chain.sub.is_some() && chain.session != chain.root {
            chain.session.clone()
        } else {
            i.body.did.session.clone()
        };
        let home = self
            .loaded
            .as_ref()
            .and_then(|l| l.setup.config())
            .map(|c| c.home.clone());
        let task = (target != chain.root)
            .then(|| home.and_then(|h| task_tab(&h, &target)))
            .flatten();
        match task {
            Some(Tab::Task { session, ord }) => self.open_task(&session, ord, window, cx),
            _ => self.open_xray(&target, window, cx),
        }
    }

    /// Never again, by what the incident is (see
    /// [`crate::incidents::never_again`]).
    pub(crate) fn incident_never_again(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(i) = self.incident(id) else { return };
        let lessons = self
            .loaded
            .as_ref()
            .and_then(|l| l.setup.config())
            .and_then(|c| hippo_core::lessons::list(&c.home).ok())
            .unwrap_or_default();
        match inc::never_again(&i, &lessons) {
            Some(NeverAgain::Open(lesson)) => {
                self.inbox.filter = super::lessons::Filter::All;
                self.inbox.selected = Some(lesson);
                self.show_section(Section::Inbox, window, cx);
            }
            Some(NeverAgain::Said(words)) => self.toast(words, None, cx),
            Some(NeverAgain::Draft) => self.ask_commands(
                DRAFT_TITLE,
                "Runs the lessons scan over the last two days. It makes model calls: the drafter, then a fresh reviewer. The rule reaches the Inbox only if the reviewer confirms.",
                vec![vec![
                    "lessons".into(),
                    "scan".into(),
                    "--days".into(),
                    "2".into(),
                ]],
                window,
                cx,
            ),
            Some(NeverAgain::Hook(_)) => {
                self.incidents_ui.hook_sheet = Some(id.to_owned());
                self.incidents_ui.copied = None;
                window.focus(&self.incidents_ui.hook_focus, cx);
                cx.notify();
            }
            None => {}
        }
    }

    /// The lessons scan Never again started has ended.
    pub(crate) fn incident_scan_done(&mut self, r: &Result<(), String>, cx: &mut Context<Self>) {
        match r {
            Ok(()) => self.toast(
                "The lessons scan finished. A rule reaches the Inbox only if the reviewer confirmed it.",
                None,
                cx,
            ),
            Err(e) => self.toast(format!("The lessons scan failed: {e}"), None, cx),
        }
    }

    fn apply_choice(&mut self, c: Choice, cx: &mut Context<Self>) {
        let f = &mut self.incidents_ui.filter;
        match c {
            Choice::Cause(v) => f.cause = v,
            Choice::Account(v) => f.account = v,
            Choice::Project(v) => f.project = v,
        }
        self.incidents_ui.menu = None;
        cx.notify();
    }

    // ---- the title bar ---------------------------------------------

    /// The list's filters: Cause, Account, Project, or one menu under 900
    /// points.
    pub(super) fn incidents_filter_bar(
        &self,
        narrow: bool,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let list = inc::shown(self.incident_list(), &self.incidents_ui.dismissed);
        let owned: Vec<Incident> = list.into_iter().cloned().collect();
        let f = &self.incidents_ui.filter;
        let bar = || div().flex().flex_shrink_0().items_center().gap(look.px(6.));
        if narrow {
            let on = [f.cause.is_some(), f.account.is_some(), f.project.is_some()]
                .iter()
                .filter(|b| **b)
                .count();
            let label = if on == 0 {
                "Filter ▾".to_owned()
            } else {
                format!("Filter · {on} ▾")
            };
            return bar()
                .child(self.filter_menu(Menu::All, "inc-filter", label, &owned, look, cx))
                .into_any_element();
        }
        let name = |what: &str, v: Option<String>| match v {
            Some(v) => format!("{what}: {v} ▾"),
            None => format!("{what} ▾"),
        };
        bar()
            .child(self.filter_menu(
                Menu::Cause,
                "inc-cause",
                name("Cause", f.cause.map(|c| c.label().to_owned())),
                &owned,
                look,
                cx,
            ))
            .child(self.filter_menu(
                Menu::Account,
                "inc-account",
                name("Account", f.account.as_deref().map(kit::profile_name)),
                &owned,
                look,
                cx,
            ))
            .child(self.filter_menu(
                Menu::Project,
                "inc-project",
                name("Project", f.project.clone()),
                &owned,
                look,
                cx,
            ))
            .into_any_element()
    }

    fn filter_menu(
        &self,
        menu: Menu,
        id: &'static str,
        label: String,
        list: &[Incident],
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let open = self.incidents_ui.menu == Some(menu);
        let f = &self.incidents_ui.filter;
        let mut items: Vec<gpui::AnyElement> = Vec::new();
        let mut n = 0usize;
        let mut push = |items: &mut Vec<gpui::AnyElement>,
                        text: String,
                        on: bool,
                        mark: Option<String>,
                        choice: Choice,
                        cx: &mut Context<Self>| {
            n += 1;
            items.push(
                div()
                    .id(("inc-menu-item", n))
                    .role(Role::MenuItem)
                    .aria_label(text.clone())
                    .flex()
                    .items_center()
                    .gap(look.px(8.))
                    .h(look.px(28.))
                    .px(look.px(8.))
                    .rounded(look.px(6.))
                    .cursor_pointer()
                    .hover(|h| h.bg(look.c.hover))
                    .text_size(look.text(12.5))
                    .child(
                        div()
                            .w(look.px(14.))
                            .flex_shrink_0()
                            .children(on.then(|| kit::icon(look, "check", 13., look.c.text))),
                    )
                    .children(mark.map(|p| kit::mark(look, Some(&p), 16.)))
                    .child(div().min_w_0().truncate().child(text))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.apply_choice(choice.clone(), cx);
                    }))
                    .into_any_element(),
            );
        };
        let heading = |text: &str| {
            div()
                .px(look.px(8.))
                .pt(look.px(6.))
                .pb(look.px(2.))
                .text_size(look.text(11.5))
                .text_color(look.c.faint)
                .child(text.to_owned())
                .into_any_element()
        };
        if matches!(menu, Menu::Cause | Menu::All) {
            if menu == Menu::All {
                items.push(heading("Cause"));
            }
            push(
                &mut items,
                "All".into(),
                f.cause.is_none(),
                None,
                Choice::Cause(None),
                cx,
            );
            for c in inc::causes(list) {
                push(
                    &mut items,
                    c.label().to_owned(),
                    f.cause == Some(c),
                    None,
                    Choice::Cause(Some(c)),
                    cx,
                );
            }
        }
        if matches!(menu, Menu::Account | Menu::All) {
            if menu == Menu::All {
                items.push(heading("Account"));
            }
            push(
                &mut items,
                "All".into(),
                f.account.is_none(),
                None,
                Choice::Account(None),
                cx,
            );
            for p in inc::accounts(list) {
                push(
                    &mut items,
                    kit::profile_name(&p),
                    f.account.as_deref() == Some(p.as_str()),
                    Some(p.clone()),
                    Choice::Account(Some(p)),
                    cx,
                );
            }
        }
        if matches!(menu, Menu::Project | Menu::All) {
            if menu == Menu::All {
                items.push(heading("Project"));
            }
            push(
                &mut items,
                "All".into(),
                f.project.is_none(),
                None,
                Choice::Project(None),
                cx,
            );
            for p in inc::projects(list) {
                push(
                    &mut items,
                    p.clone(),
                    f.project.as_deref() == Some(p.as_str()),
                    None,
                    Choice::Project(Some(p)),
                    cx,
                );
            }
        }
        let active = match menu {
            Menu::Cause => f.cause.is_some(),
            Menu::Account => f.account.is_some(),
            Menu::Project => f.project.is_some(),
            Menu::All => !f.is_none(),
        };
        div()
            .relative()
            .child(
                kit::button(
                    look,
                    id,
                    label,
                    if open || active {
                        Kind::Plain
                    } else {
                        Kind::Ghost
                    },
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.incidents_ui.menu = if this.incidents_ui.menu == Some(menu) {
                        None
                    } else {
                        Some(menu)
                    };
                    cx.notify();
                })),
            )
            .when(open, |d| {
                d.child(gpui::deferred(
                    gpui::anchored().child(
                        div()
                            .id(SharedString::from(format!("{id}-pop")))
                            .mt(look.px(32.))
                            .min_w(look.px(200.))
                            .max_w(look.px(320.))
                            .max_h(look.px(360.))
                            .overflow_y_scroll()
                            .p(look.px(4.))
                            .rounded(look.px(10.))
                            .bg(look.c.surface)
                            .border(look.fixed(1.))
                            .border_color(look.c.border)
                            .shadow_lg()
                            .occlude()
                            .flex()
                            .flex_col()
                            .children(items)
                            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                this.incidents_ui.menu = None;
                                cx.notify();
                            })),
                    ),
                ))
            })
            .into_any_element()
    }

    /// An incident page's title-bar actions: Open the step, Never again
    /// (a correction, or a destructive command a hook can block), Not an
    /// incident.
    pub(super) fn incident_page_actions(
        &self,
        id: &str,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let i = self.incident(id)?;
        let bar = div().flex().flex_shrink_0().items_center().gap(look.px(6.));
        let (open_id, again_id, dismiss_id) = (id.to_owned(), id.to_owned(), id.to_owned());
        let step = i.clone();
        let _ = open_id;
        Some(
            bar.child(
                kit::button_icon(
                    look,
                    "inc-open-step",
                    Some("text-search"),
                    inc::open_label(&i),
                    "",
                    Kind::Plain,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_incident_step(&step, window, cx);
                })),
            )
            .when(inc::offers_never_again(&i), |d| {
                d.child(
                    kit::button(look, "inc-never-again", "Never again", Kind::Plain).on_click(
                        cx.listener(move |this, _, window, cx| {
                            this.incident_never_again(&again_id, window, cx);
                        }),
                    ),
                )
            })
            .child(
                kit::button(look, "inc-dismiss", "Not an incident", Kind::Ghost).on_click(
                    cx.listener(move |this, _, window, cx| {
                        this.dismiss_incident(&dismiss_id, window, cx);
                    }),
                ),
            )
            .into_any_element(),
        )
    }

    // ---- the list ---------------------------------------------------

    pub(super) fn render_incidents_tab(
        &mut self,
        look: &Look,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(data) = self.loaded.as_ref().and_then(|l| l.incidents.as_ref()) else {
            let why = if self.loaded.is_some() {
                "No incidents yet: the index has not been built."
            } else {
                "Reading the incidents…"
            };
            return div()
                .p(look.px(20.))
                .text_color(look.c.text_muted)
                .child(why)
                .into_any_element();
        };
        let width = f32::from(window.viewport_size().width);
        let narrow = width < super::home::COMPACT;
        let offset = super::status::local_offset();
        let now = OffsetDateTime::now_utc();
        let today = now.to_offset(offset).date();
        let sessions = data.sessions.clone();
        let rows = self.incidents_on_screen();
        let board_rows: Vec<&Incident> = rows
            .iter()
            .copied()
            .filter(|i| !i.quiet && !i.kind.is_hint())
            .collect();
        let score = inc::scoreboard(&board_rows, &sessions);
        let days = inc::days(&rows, offset);
        let selected = self.incidents_ui.selected.clone();

        let mut body: Vec<gpui::AnyElement> = Vec::new();
        let mut k = 0usize;
        for d in &days {
            body.push(
                div()
                    .flex()
                    .items_center()
                    .h(look.px(34.))
                    .px(look.px(10.))
                    .border_b(look.fixed(1.))
                    .border_color(look.c.border)
                    .text_size(look.text(12.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(look.c.text_muted)
                    .child(inc::day_label(d.date, today))
                    .into_any_element(),
            );
            for i in &d.rows {
                body.push(self.incident_row(i, k, selected.as_deref(), now, look, cx));
                k += 1;
            }
            if !d.quiet.is_empty() {
                let open = self.incidents_ui.quiet_open.contains(&d.date);
                let date = d.date;
                body.push(
                    div()
                        .id(("inc-quiet", d.date.to_julian_day() as usize))
                        .role(Role::Button)
                        .aria_expanded(open)
                        .flex()
                        .items_center()
                        .gap(look.px(8.))
                        .h(look.px(34.))
                        .px(look.px(10.))
                        .border_b(look.fixed(1.))
                        .border_color(look.c.border)
                        .text_size(look.text(12.5))
                        .text_color(look.c.faint)
                        .cursor_pointer()
                        .hover(|h| h.bg(look.c.hover))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(inc::quiet_label(&d.quiet)),
                        )
                        .child(kit::icon(
                            look,
                            if open {
                                "chevron-down"
                            } else {
                                "chevron-right"
                            },
                            14.,
                            look.c.faint,
                        ))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let q = &mut this.incidents_ui.quiet_open;
                            if !q.remove(&date) {
                                q.insert(date);
                            }
                            cx.notify();
                        }))
                        .into_any_element(),
                );
                if open {
                    for i in &d.quiet {
                        body.push(self.incident_row(i, k, selected.as_deref(), now, look, cx));
                        k += 1;
                    }
                }
            }
        }
        if days.is_empty() {
            body.push(
                div()
                    .px(look.px(10.))
                    .py(look.px(14.))
                    .text_color(look.c.text_muted)
                    .child(if self.incidents_ui.filter.is_none() {
                        "No incidents this week."
                    } else {
                        "No incident matches the filters."
                    })
                    .into_any_element(),
            );
        }
        // Hints to check: apart from the incidents, after them.
        let hints = inc::hints(&rows);
        if !hints.is_empty() {
            body.push(
                div()
                    .flex()
                    .items_center()
                    .h(look.px(34.))
                    .mt(look.px(12.))
                    .px(look.px(10.))
                    .border_b(look.fixed(1.))
                    .border_color(look.c.border)
                    .text_size(look.text(12.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(look.c.text_muted)
                    .child(hippo_core::incidents::HINTS_HEADING)
                    .into_any_element(),
            );
            for i in &hints {
                body.push(self.incident_row(i, k, selected.as_deref(), now, look, cx));
                k += 1;
            }
        }
        let scoreboard = (!score.is_empty()).then(|| self.scoreboard(&score, look));
        let running = self.manage_ui.running.as_deref() == Some(DRAFT_TITLE);
        let page = div()
            .flex()
            .flex_col()
            .w_full()
            .px(look.px(16.))
            .pt(look.px(8.))
            .pb(look.px(20.))
            .text_size(look.text(13.))
            .children(running.then(|| {
                div()
                    .pb(look.px(8.))
                    .text_size(look.text(12.5))
                    .text_color(look.c.text_muted)
                    .child("Drafting a rule: the lessons scan is running.")
            }))
            .children(scoreboard)
            .children(body);
        let _ = narrow;
        div()
            .id("incidents-keys")
            .key_context("Incidents")
            .track_focus(&self.incidents_ui.focus)
            .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, window, cx| {
                let k = &e.keystroke;
                if k.modifiers.platform || k.modifiers.control || k.modifiers.alt {
                    return;
                }
                if this.incidents_key(k.key.as_str(), window, cx) {
                    cx.stop_propagation();
                }
            }))
            .relative()
            .size_full()
            .child(
                div()
                    .id("incidents")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.page_scroll)
                    .child(page),
            )
            .children(super::scrollbar::vertical(&self.page_scroll, look))
            .into_any_element()
    }

    /// `THIS WEEK` and the counts per account and per model, wrapping.
    fn scoreboard(&self, score: &[inc::Score], look: &Look) -> gpui::AnyElement {
        let entries = score.iter().map(|s| {
            let row = div().flex().items_center().gap(look.px(5.)).flex_shrink_0();
            match &s.who {
                inc::Who::Account(p) => row.child(kit::mark(look, Some(p), 16.)).child(
                    div().text_color(look.c.text).child(match s.sessions {
                        Some(n) => format!("{} in {n} sessions", s.count),
                        None => s.count.to_string(),
                    }),
                ),
                inc::Who::Model(m) => row
                    .child(
                        div()
                            .text_color(look.c.text_muted)
                            .child(inc::model_name(m)),
                    )
                    .child(div().text_color(look.c.text).child(s.count.to_string())),
            }
            .into_any_element()
        });
        let mut line = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(look.px(18.))
            .gap_y(look.px(4.))
            .pb(look.px(10.))
            .text_size(look.text(12.5))
            .child(
                div()
                    .text_color(look.c.faint)
                    .font_weight(FontWeight::MEDIUM)
                    .child("Incidents this week, by account and model"),
            );
        // No separators: a wrapped line must not start with one.
        for e in entries {
            line = line.child(e);
        }
        line.into_any_element()
    }

    /// A cause as a chip: amber when it is the person's to act on.
    fn cause_chip(&self, cause: Cause, quiet: bool, look: &Look) -> gpui::Div {
        // A quiet incident's chip is never amber.
        if cause.needs_person() && !quiet {
            kit::pill(look, cause.label(), look.c.warn).h(look.px(20.))
        } else {
            kit::kind_chip(look, cause.label())
        }
    }

    fn incident_row(
        &self,
        i: &Incident,
        k: usize,
        selected: Option<&str>,
        now: OffsetDateTime,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let chain = &i.body.chain;
        let on = selected == Some(i.id.as_str());
        let who = match (&chain.root_title, &chain.sub) {
            (Some(t), _) => kit::title(t),
            (None, _) => crate::rows::id_title(&chain.root),
        };
        let id = i.id.clone();
        let line1 = div()
            .flex()
            .items_center()
            .gap(look.px(10.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(look.text(13.5))
                    .text_color(look.c.text)
                    .child(inc::evidence(i)),
            )
            .child(self.cause_chip(i.cause, i.quiet, look))
            .child(
                div()
                    .flex_shrink_0()
                    .w(look.px(64.))
                    .text_right()
                    .text_size(look.text(12.))
                    .text_color(look.c.faint)
                    .children(inc::cost_text(i)),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .w(look.px(34.))
                    .text_right()
                    .text_size(look.text(12.))
                    .text_color(look.c.faint)
                    .child(inc::age(now - i.at)),
            );
        let line2 = div()
            .flex()
            .items_center()
            .gap(look.px(8.))
            .text_size(look.text(12.5))
            .text_color(look.c.text_muted)
            .child(kit::mark(look, chain.profile.as_deref(), 16.))
            .child(div().min_w_0().truncate().child(who))
            .children(chain.sub.as_ref().map(|s| {
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(4.))
                    .min_w_0()
                    .flex_shrink(2.)
                    .child(div().text_color(look.c.faint).child("›"))
                    .child(div().min_w_0().truncate().child(kit::title(s)))
            }))
            .children(inc::short_form(i).map(|short| {
                // The step, or what it may be about and whose.
                let dot = || div().flex_shrink_0().text_color(look.c.faint).child("·");
                let row = div()
                    .flex()
                    .items_center()
                    .gap(look.px(6.))
                    .min_w_0()
                    .flex_shrink(3.)
                    .child(dot());
                match short {
                    inc::Short::Step(line) => row.child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_family(MONO)
                            .text_size(look.text(12.))
                            .child(line),
                    ),
                    inc::Short::About {
                        words,
                        profile,
                        title,
                    } => row
                        .child(div().flex_shrink_0().child(words))
                        .child(dot())
                        .children(profile.map(|p| kit::mark(look, Some(&p), 14.)))
                        .child(div().min_w_0().truncate().child(kit::title(&title))),
                }
            }))
            .child(div().flex_1())
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(look.c.faint)
                    .child(inc::kind_word(i.kind)),
            );
        let row = div()
            .id(("incident-row", k))
            .role(Role::Row)
            .aria_label(inc::evidence(i))
            .aria_selected(on)
            .flex()
            .flex_col()
            .gap(look.px(4.))
            .px(look.px(10.))
            .py(look.px(10.))
            .border_b(look.fixed(1.))
            .border_color(look.c.border)
            .cursor_pointer()
            .child(line1)
            .child(line2)
            .on_click(cx.listener(move |this, e: &gpui::ClickEvent, window, cx| {
                // A click selects; a second click, a double-click or ↩
                // opens the page (Fleet's rule).
                let again = this.incidents_ui.selected.as_deref() == Some(id.as_str());
                if again || e.click_count() >= 2 {
                    this.open_incident(&id, window, cx);
                } else {
                    this.incidents_ui.selected = Some(id.clone());
                    cx.notify();
                }
            }));
        kit::row_fill(row, look, on).into_any_element()
    }

    // ---- the page ---------------------------------------------------

    pub(super) fn render_incident_tab(
        &mut self,
        id: &str,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(i) = self.incident(id) else {
            return super::xray::quiet_note(
                look,
                "This incident is no longer in the week's list.",
                false,
            );
        };
        let offset = super::status::local_offset();
        let b = &i.body;
        // What a correction is about decides the second block.
        let about = inc::subject(&i).cloned();
        let no_step = !inc::has_step(&i);
        let body = |look: &Look| {
            div()
                .flex()
                .flex_col()
                .gap(look.px(6.))
                .px(look.px(14.))
                .pb(look.px(14.))
        };
        let faint = |look: &Look, t: String| {
            div()
                .text_size(look.text(12.))
                .text_color(look.c.faint)
                .child(t)
        };
        let words = |look: &Look, t: String| div().text_color(look.c.text).child(t);

        // 1. What it was told, and the rule it had.
        let told = (b.told.is_some() || b.rule.is_some()).then(|| {
            let mut card = body(look);
            if let Some(t) = &b.told {
                card = card
                    .child(faint(
                        look,
                        match t.whose.as_str() {
                            "parent" => "the parent agent's brief".to_owned(),
                            _ => "your request".to_owned(),
                        },
                    ))
                    .child(words(look, t.text.clone()));
            }
            if let Some(r) = &b.rule {
                card = card
                    .child(
                        div()
                            .pt(look.px(if b.told.is_some() { 6. } else { 0. }))
                            .text_size(look.text(12.))
                            .text_color(look.c.warn)
                            .child(match &r.used {
                                Some(u) => format!("Had the rule: {} · {u}", r.source),
                                None => format!("Had the rule: {}", r.source),
                            }),
                    )
                    .child(words(look, r.text.clone()));
            }
            kit::pane_card(look, "What it was told", None).child(card)
        });

        // A step's text: the kit's tool line for `Tool: …` or `Tool {…}`,
        // its whole command under it when long; else the words.
        let step_lines =
            |look: &Look, text: &str, at: OffsetDateTime, card: gpui::Div| match inc::parse_tool(
                text,
            ) {
                Some((verb, target)) => {
                    let long = verb == "Bash" && target.chars().count() > 60;
                    let act = Activity {
                        label: text.to_owned(),
                        at,
                        running: false,
                        delegates: false,
                        verb,
                        target: Some(target.clone()),
                        step: None,
                        lines: None,
                    };
                    let diff = inc::edit_preview(text);
                    card.child(kit::tool_line(look, &act, 1, None, false))
                        .children(diff.map(|(_, removed, added)| {
                            let line = |sign: &str, l: String, color| {
                                div()
                                    .truncate()
                                    .text_color(color)
                                    .child(format!("{sign} {l}"))
                            };
                            div()
                                .flex()
                                .flex_col()
                                .min_w_0()
                                .font_family(MONO)
                                .text_size(look.text(12.))
                                .children(
                                    removed.into_iter().map(|l| line("−", l, look.c.diff_del)),
                                )
                                .children(added.into_iter().map(|l| line("+", l, look.c.diff_add)))
                        }))
                        .when(long, |d| {
                            d.child(
                                div()
                                    .font_family(MONO)
                                    .text_size(look.text(12.))
                                    .text_color(look.c.text_muted)
                                    .line_clamp(4)
                                    .text_ellipsis()
                                    .child(target),
                            )
                        })
                }
                None => card.child(div().text_color(look.c.text).child(text.to_owned())),
            };
        // 2. What it did, or what the correction is about.
        let kind = about.as_ref().map(|s| s.kind);
        let did = match kind {
            Some(inc::SubjectKind::Earlier) => {
                let now = OffsetDateTime::now_utc();
                let cands = about
                    .as_ref()
                    .map(|s| s.candidates.clone())
                    .unwrap_or_default();
                let mut card = body(look).child(faint(look, "May be about".to_owned()));
                for (k, c) in cands.into_iter().enumerate() {
                    let words_typed = c.words.join(", ");
                    let title = inc::candidate_title(&c);
                    let facts = inc::candidate_facts(&c, now, offset).join(" · ");
                    let (sid, ord) = (c.session.clone(), c.ordinal);
                    let line1 = div()
                        .flex()
                        .items_center()
                        .gap(look.px(6.))
                        .min_w_0()
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_color(look.c.text)
                                .font_weight(FontWeight::MEDIUM)
                                .child(format!("\u{201c}{words_typed}\u{201d}")),
                        )
                        .child(div().flex_shrink_0().text_color(look.c.faint).child("·"))
                        .child(kit::mark(look, c.profile.as_deref(), 16.))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(look.c.text)
                                .child(kit::title(&title)),
                        );
                    let line2 = div()
                        .text_size(look.text(12.))
                        .text_color(look.c.faint)
                        .line_clamp(2)
                        .text_ellipsis()
                        .child(facts);
                    card = card.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(look.px(10.))
                            .pt(look.px(if k == 0 { 2. } else { 8. }))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .gap(look.px(2.))
                                    .child(line1)
                                    .child(line2),
                            )
                            .child(
                                kit::button(look, ("inc-candidate", k), "Open", Kind::Plain)
                                    .flex_shrink_0()
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if let Some(o) = ord {
                                            this.sessions_ui.scroll_to = Some((sid.clone(), o));
                                        }
                                        this.state.tabs.follow(
                                            Tab::Session {
                                                id: sid.clone(),
                                                ordinal: ord,
                                            },
                                            false,
                                        );
                                        this.after_state_change(window, cx);
                                    })),
                            ),
                    );
                }
                kit::pane_card(look, "What it is about", None).child(card)
            }
            Some(inc::SubjectKind::Said) => {
                let said = about.as_ref().and_then(|s| s.said.clone());
                let open = self.incidents_ui.answering_open.contains(id);
                let key = id.to_owned();
                let at = said.as_ref().map(|s| inc::clock(s.at, offset));
                let text = said
                    .map(|s| kit::plain(&inc::said_text(&s.text)))
                    .unwrap_or_default();
                kit::pane_card(look, "What it said", at).child(
                    body(look).child(
                        div()
                            .id("inc-said")
                            .cursor_pointer()
                            .text_color(look.c.text)
                            .when(!open, |d| d.line_clamp(4).text_ellipsis())
                            .child(text)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.incidents_ui.answering_open.remove(&key) {
                                    this.incidents_ui.answering_open.insert(key.clone());
                                }
                                cx.notify();
                            })),
                    ),
                )
            }
            _ if no_step => kit::pane_card(look, "What it is about", None).child(body(look).child(
                faint(look, "What it is about is not established.".to_owned()),
            )),
            _ => kit::pane_card(look, "What it did", Some(inc::clock(b.did.at, offset)))
                .child(step_lines(look, &b.did.text, b.did.at, body(look))),
        };

        // 3. What showed it was wrong.
        let showed = {
            let mut card = body(look).child(words(look, b.showed.text.clone()));
            if i.kind == IncidentKind::Blocked {
                card = card.child(faint(look, "Prevented: the call did not run.".to_owned()));
            }
            kit::pane_card(
                look,
                "What showed it was wrong",
                Some(inc::clock(b.showed.at, offset)),
            )
            .child(card)
        };

        // 4. What fixed it, only when present.
        let fixed =
            b.fixed.as_ref().map(|f| {
                kit::pane_card(look, "What fixed it", Some(inc::clock(f.at, offset)))
                    .child(step_lines(look, &f.text, f.at, body(look)))
            });

        // The chain.
        let fact = |look: &Look, label: &str, value: gpui::AnyElement| {
            div()
                .flex()
                .items_start()
                .gap(look.px(12.))
                .child(
                    div()
                        .flex_shrink_0()
                        .w(look.px(88.))
                        .text_size(look.text(12.))
                        .text_color(look.c.faint)
                        .child(label.to_owned()),
                )
                .child(div().flex_1().min_w_0().child(value))
        };
        let lane = |c: &hippo_core::incidents::Chain| {
            let root = c
                .root_title
                .as_deref()
                .map(kit::title)
                .unwrap_or_else(|| crate::rows::id_title(&c.root));
            match &c.sub {
                Some(s) => format!("{root} › {}", kit::title(s)),
                None => root,
            }
        };
        let chain = &b.chain;
        let mut facts = body(look);
        facts = facts.child(fact(
            look,
            "Session",
            words(look, lane(chain)).into_any_element(),
        ));
        if let Some(p) = &chain.profile {
            facts = facts.child(fact(
                look,
                "Account",
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(6.))
                    .child(kit::mark(look, Some(p), 16.))
                    .child(kit::profile_name(p))
                    .into_any_element(),
            ));
        }
        let model = match (&chain.model, &chain.effort) {
            (Some(m), Some(e)) => Some(format!("{}, {e} effort", inc::model_name(m))),
            (Some(m), None) => Some(inc::model_name(m)),
            (None, Some(e)) => Some(format!("{e} effort")),
            (None, None) => None,
        };
        if let Some(m) = model {
            facts = facts.child(fact(look, "Model", words(look, m).into_any_element()));
        }
        facts = facts.child(fact(
            look,
            "Tool",
            words(
                look,
                match chain.tool.as_str() {
                    "claude-code" => "Claude Code".to_owned(),
                    "codex" => "Codex".to_owned(),
                    other => other.to_owned(),
                },
            )
            .into_any_element(),
        ));
        facts = facts.when(!no_step, |facts| {
            facts.child(fact(
                look,
                "Step",
                words(
                    look,
                    // A task's own record has no step number: only its time.
                    if b.did.ordinal == 0 && i.kind == IncidentKind::Failed {
                        inc::clock(b.did.at, offset)
                    } else {
                        format!("step {} · {}", b.did.ordinal, inc::clock(b.did.at, offset))
                    },
                )
                .into_any_element(),
            ))
        });
        if let Some(o) = b
            .other
            .as_ref()
            .filter(|_| matches!(i.kind, IncidentKind::Redone | IncidentKind::Collision))
        {
            facts = facts.child(fact(
                look,
                "Other session",
                words(look, lane(o)).into_any_element(),
            ));
        }
        let chain_card = kit::pane_card(look, "The chain", None).child(facts);

        // The cost: only the known parts.
        let parts = inc::cost_parts(&i);
        let cost_card = (!parts.is_empty()).then(|| {
            kit::pane_card(look, "The cost", None).child(
                parts
                    .into_iter()
                    .fold(body(look), |d, p| d.child(words(look, p))),
            )
        });

        // What it had read: each a link to the note in Memory.
        let read_card = (!b.read.is_empty()).then(|| {
            let mut d = body(look);
            for (n, note) in b.read.iter().enumerate() {
                let path = std::path::PathBuf::from(note);
                d = d.child(
                    kit::link(look, ("inc-note", n), kit::middle(note, 64))
                        .min_w_0()
                        .truncate()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.state.tabs.follow(Tab::Note(path.clone()), false);
                            this.page_scroll.set_offset(gpui::Point::default());
                            this.after_state_change(window, cx);
                        })),
                );
            }
            kit::pane_card(look, "What it had read", None).child(d)
        });

        let head = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(look.px(10.))
            .child(
                div()
                    .text_size(look.text(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(sentence(b_kind_label(i.kind))),
            )
            .child(self.cause_chip(i.cause, i.quiet, look))
            .child(
                div()
                    .text_size(look.text(12.))
                    .text_color(look.c.faint)
                    .child(format!(
                        "{} · {}",
                        super::status::format_when(i.at, OffsetDateTime::now_utc(), offset),
                        inc::age(OffsetDateTime::now_utc() - i.at)
                    )),
            );
        let running = self.manage_ui.running.as_deref() == Some(DRAFT_TITLE);
        let column = div()
            .flex()
            .flex_col()
            .gap(look.px(12.))
            .w_full()
            .max_w(look.px(760.))
            .child(head)
            .children(running.then(|| {
                div()
                    .text_size(look.text(12.5))
                    .text_color(look.c.text_muted)
                    .child("Drafting a rule: the lessons scan is running.")
            }))
            .children(told)
            .child(did)
            .child(showed)
            .children(fixed)
            .child(chain_card)
            .children(cost_card)
            .children(read_card);
        div()
            .relative()
            .size_full()
            .child(
                div()
                    .id("incident-page")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.page_scroll)
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .justify_center()
                            .px(look.px(20.))
                            .pt(look.px(8.))
                            .pb(look.px(24.))
                            .text_size(look.text(13.))
                            .child(column),
                    ),
            )
            .children(super::scrollbar::vertical(&self.page_scroll, look))
            .into_any_element()
    }

    // ---- the toast and the hook sheet --------------------------------

    /// The toast at the bottom right, with Undo for a mark.
    pub(super) fn render_incident_toast(
        &self,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let t = self.incidents_ui.toast.as_ref()?;
        let undo = t.undo.clone();
        Some(
            div()
                .absolute()
                .bottom(look.px(40.))
                .right(look.px(16.))
                .max_w(look.px(440.))
                .child(
                    div()
                        .id("incident-toast")
                        .flex()
                        .items_center()
                        .gap(look.px(10.))
                        .px(look.px(14.))
                        .py(look.px(9.))
                        .rounded(look.px(12.))
                        .bg(look.c.raised)
                        .border(look.fixed(1.))
                        .border_color(look.c.border_strong)
                        .shadow_lg()
                        .text_size(look.text(13.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .line_clamp(4)
                                .text_ellipsis()
                                .child(t.text.clone()),
                        )
                        .children(undo.map(|id| {
                            kit::button(look, "incident-undo", "Undo", Kind::Soft).on_click(
                                cx.listener(move |this, _, _, cx| this.undo_dismiss(&id, cx)),
                            )
                        }))
                        .child(
                            kit::icon_btn(look, "incident-toast-close", "x", "Close", false)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.incidents_ui.toast = None;
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    /// `A hook that would have blocked this`: the script and the settings
    /// entry to copy. Hippo installs nothing.
    pub(super) fn render_hook_sheet(
        &self,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let id = self.incidents_ui.hook_sheet.as_ref()?;
        let i = self.incident(id)?;
        let hook = i
            .body
            .detail
            .as_deref()
            .and_then(hippo_core::incidents::hook_for)?;
        let copied = self.incidents_ui.copied;
        let well = |text: &str| {
            div()
                .px(look.px(12.))
                .py(look.px(10.))
                .rounded(look.px(8.))
                .bg(look.c.inset)
                .border(look.fixed(1.))
                .border_color(look.c.border)
                .font_family(MONO)
                .text_size(look.text(11.5))
                .text_color(look.c.text)
                .min_w_0()
                .overflow_hidden()
                .flex()
                .flex_col()
                .children(
                    text.trim_end()
                        .lines()
                        .map(|l| {
                            // Long lines wrap inside the well (nothing is
                            // clipped).
                            div()
                                .w_full()
                                .min_w_0()
                                .child(l.to_owned())
                                .into_any_element()
                        })
                        .collect::<Vec<_>>(),
                )
        };
        let head = |label: String, what: &'static str, text: String, cx: &mut Context<Self>| {
            div()
                .flex()
                .items_center()
                .gap(look.px(8.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(MONO)
                        .text_size(look.text(12.))
                        .text_color(look.c.text_muted)
                        .child(label),
                )
                .child(
                    kit::button_icon(
                        look,
                        SharedString::from(format!("hook-copy-{what}")),
                        Some(if copied == Some(what) {
                            "check"
                        } else {
                            "copy"
                        }),
                        if copied == Some(what) {
                            "Copied"
                        } else {
                            "Copy"
                        },
                        "",
                        Kind::Plain,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()));
                        this.incidents_ui.copied = Some(what);
                        cx.notify();
                    })),
                )
        };
        Some(
            div()
                .id("hook-sheet-overlay")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .justify_center()
                .items_center()
                .px(look.px(16.))
                .bg(gpui::black().opacity(0.5))
                .occlude()
                .child(
                    div()
                        .id("hook-sheet")
                        .key_context("HookSheet")
                        .track_focus(&self.incidents_ui.hook_focus)
                        .role(Role::Dialog)
                        .aria_label("A hook that would have blocked this")
                        .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, window, cx| {
                            if e.keystroke.key == "escape" {
                                this.incidents_ui.hook_sheet = None;
                                window.focus(&this.focus, cx);
                                cx.notify();
                                cx.stop_propagation();
                            }
                        }))
                        .flex()
                        .flex_col()
                        .gap(look.px(10.))
                        .w(look.px(560.))
                        .max_w_full()
                        .max_h_full()
                        .overflow_y_scroll()
                        .p(look.px(20.))
                        .rounded(look.px(16.))
                        .bg(look.c.surface)
                        .border(look.fixed(1.))
                        .border_color(look.c.border_strong)
                        .shadow_lg()
                        .text_size(look.text(13.))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(look.text(15.))
                                .child("A hook that would have blocked this"),
                        )
                        .child(head(hook.file.clone(), "script", hook.script.clone(), cx))
                        .child(well(&hook.script))
                        .child(head(
                            "settings.json".to_owned(),
                            "settings",
                            hook.settings.clone(),
                            cx,
                        ))
                        .child(well(&hook.settings))
                        .child(
                            div()
                                .text_color(look.c.text_muted)
                                .child(format!(
                                    "Save the script as ~/.claude/hooks/{}, make it executable, and add the entry to settings.json. Hippo does not install it.",
                                    hook.file
                                )),
                        )
                        .child(
                            div().flex().justify_end().child(
                                kit::button_key(look, "hook-close", "Close", "esc", Kind::Ghost)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.incidents_ui.hook_sheet = None;
                                        window.focus(&this.focus, cx);
                                        cx.notify();
                                    })),
                            ),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Home's `n incidents today` link, when n > 0.
    pub(super) fn incidents_home_link(
        &self,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let n = self.incidents_today();
        (n > 0).then(|| {
            div()
                .flex()
                .flex_shrink_0()
                .items_baseline()
                .gap(look.px(6.))
                .text_size(look.text(13.))
                .child(div().text_color(look.c.faint).child("·"))
                .child(
                    div()
                        .id("hero-incidents")
                        .role(Role::Link)
                        .flex_shrink_0()
                        .text_color(look.c.accent_text)
                        .cursor_pointer()
                        .hover(|h| h.underline())
                        .child(format!(
                            "{n} incident{} today",
                            if n == 1 { "" } else { "s" }
                        ))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_incidents(window, cx);
                        })),
                )
                .into_any_element()
        })
    }
}

/// `corrected by you` as a heading: `Corrected by you`.
fn sentence(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

fn b_kind_label(k: IncidentKind) -> &'static str {
    k.label()
}

/// The sub-task page of session `session`, when it is a sub-agent or a
/// delegated task whose parent lists it: `Tab::Task`.
fn task_tab(home: &std::path::Path, session: &str) -> Option<Tab> {
    let rollup = hippo_core::rollup::Rollup::open(home).ok().flatten()?;
    let facts = rollup.session(session).ok().flatten();
    let parent = facts
        .and_then(|f| f.parent)
        .or_else(|| rollup.delegated_by(session).ok().flatten().map(|(p, _)| p))?;
    let tasks = rollup.tasks_of(&parent).ok()?;
    let ord = tasks
        .iter()
        .position(|t| t.child.as_deref() == Some(session))?;
    Some(Tab::Task {
        session: parent,
        ord,
    })
}
