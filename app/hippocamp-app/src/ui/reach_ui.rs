//! Reaching the person from the shell: the menu-bar item and the notifications kept in
//! step with each fleet read, Open and Resume that act, the resume confirm
//! sheet, the morning digest and the settings.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use futures::StreamExt as _;
use futures::channel::mpsc::UnboundedSender;
use gpui::{
    Context, FocusHandle, FontWeight, SharedString, SystemNotification, SystemNotificationAction,
    Window, div, prelude::*,
};
use hippocamp_view::fleet::{Fleet, Row};
use time::OffsetDateTime;

use super::Shell;
use super::kit::{self, Kind};
use super::theme::Look;
use crate::launch::{self, Opened};
use crate::reach::{self, Act, Switches};
use crate::state::Section;

/// A resume on another profile, waiting for its confirmation.
#[derive(Debug, Clone)]
pub struct ResumeConfirm {
    pub title: String,
    pub profile: String,
    pub command: String,
}

/// The morning digest's parts: what is new since the
/// person last looked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Digest {
    pub notes: usize,
    pub rules: usize,
    pub claims: usize,
    /// `03:31`, the run's report, and whether it changed any note.
    pub dreamed: Option<(String, PathBuf, bool)>,
    /// Rules waiting in all, new or not.
    pub waiting: usize,
}

impl Digest {
    pub fn is_empty(&self) -> bool {
        self.notes == 0 && self.rules == 0 && self.claims == 0 && self.dreamed.is_none()
    }

    /// Nothing new since the person last looked.
    pub fn nothing_new(&self) -> bool {
        self.notes == 0 && self.rules == 0 && self.claims == 0
    }
}

/// A viewing that starts after this long away counts the digest from the
/// last one.
pub const AWAY: time::Duration = time::Duration::minutes(30);

/// When the digest counts from: the last time Home or the Inbox
/// was seen, but never before 18:00 yesterday.
pub fn digest_since(seen_at: Option<OffsetDateTime>, now: OffsetDateTime) -> OffsetDateTime {
    let overnight = overnight_since(now);
    seen_at.map_or(overnight, |t| t.max(overnight))
}

pub struct ReachUi {
    item: Option<crate::menubar::StatusItem>,
    sender: Option<UnboundedSender<Act>>,
    watcher: reach::Watcher,
    /// The menu-bar item is on; read by the window's close handler.
    pub menubar: Rc<Cell<bool>>,
    pub switches: Switches,
    pub window_active: bool,
    /// What the last Open or Resume did.
    pub said: Option<Result<String, String>>,
    pub confirm: Option<ResumeConfirm>,
    pub confirm_focus: FocusHandle,
    /// Open at login's state, read when Manage shows; its last error.
    pub login: Option<crate::menubar::login::Status>,
    pub login_error: Option<String>,
    pub digest_dismissed: Option<String>,
    /// Home or the Inbox last on screen, and when that was last
    /// written to the preferences.
    pub seen_at: Option<OffsetDateTime>,
    seen_saved: Option<OffsetDateTime>,
    /// This viewing's digest counts from here.
    pub digest_from: Option<OffsetDateTime>,
    /// The terminal setting.
    pub terminal: Option<String>,
    /// Sessions opened in Hippocamp (the Settled rule).
    pub opened: std::collections::BTreeMap<String, i64>,
    /// Sessions settled or un-settled by hand, and snoozed.
    pub settled: std::collections::BTreeMap<String, crate::prefs::Mark>,
    pub snoozed: std::collections::BTreeMap<String, crate::prefs::Snooze>,
    _acts: Option<gpui::Task<()>>,
}

impl ReachUi {
    pub fn new(prefs: &crate::prefs::Prefs, focus: FocusHandle) -> ReachUi {
        ReachUi {
            item: None,
            sender: None,
            watcher: reach::Watcher::default(),
            menubar: Rc::new(Cell::new(prefs.menubar)),
            switches: Switches {
                waiting: prefs.notify_waiting,
                stopped: prefs.notify_stopped,
                reset: prefs.notify_reset,
            },
            window_active: true,
            said: None,
            confirm: None,
            confirm_focus: focus,
            login: None,
            login_error: None,
            digest_dismissed: prefs.digest_dismissed.clone(),
            seen_at: prefs
                .inbox_seen_at
                .and_then(|t| OffsetDateTime::from_unix_timestamp(t).ok()),
            seen_saved: None,
            digest_from: None,
            terminal: prefs.terminal.clone(),
            opened: prefs.opened_sessions.clone(),
            settled: prefs.settled_sessions.clone(),
            snoozed: prefs.snoozed_sessions.clone(),
            _acts: None,
        }
    }

    /// The fleet is read on a timer whatever the window shows.
    pub fn watching(&self) -> bool {
        self.menubar.get() || self.switches.any()
    }
}

/// A notification's tag: `hippocamp:<kind>:<session>`.
fn tag(kind: reach::Kind, id: &str) -> String {
    let k = match kind {
        reach::Kind::Waiting => "waiting",
        reach::Kind::Stopped => "stopped",
        reach::Kind::Reset => "reset",
    };
    format!("hippocamp:{k}:{id}")
}

/// What a notification's response asks for: its button opens or resumes;
/// a click on the banner shows the session's x-ray.
pub fn response_act(tag: &str, button: bool) -> Option<Act> {
    let rest = tag.strip_prefix("hippocamp:")?;
    let (kind, id) = rest.split_once(':')?;
    let id = id.to_owned();
    Some(match (button, kind) {
        (true, "waiting") => Act::Open(id),
        (true, _) => Act::Resume(id),
        (false, _) => Act::Xray(id),
    })
}

/// The local date as `2026-11-10`.
pub fn local_date(now: OffsetDateTime) -> String {
    let d = now.to_offset(super::status::local_offset()).date();
    format!("{:04}-{:02}-{:02}", d.year(), u8::from(d.month()), d.day())
}

/// Overnight begins at 18:00 local the day before `now`.
pub fn overnight_since(now: OffsetDateTime) -> OffsetDateTime {
    let local = now.to_offset(super::status::local_offset());
    let day = local.date().previous_day().unwrap_or(local.date());
    day.with_hms(18, 0, 0)
        .map(|t| t.assume_offset(local.offset()))
        .unwrap_or(local)
}

impl Shell {
    /// Starts the reach features: the act channel, the status item, the notification
    /// responses, and closing the window hides the app while the item is
    /// on.
    pub(crate) fn start_reach(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<Act>();
        self.reach.sender = Some(tx.clone());
        self.sync_status_item();
        let task = cx.spawn_in(window, async move |this, cx| {
            while let Some(act) = rx.next().await {
                if this
                    .update_in(cx, |this, window, cx| this.do_act(act, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        self.reach._acts = Some(task);
        cx.on_system_notification_response(move |r, _| {
            if let Some(act) = response_act(&r.tag, r.action_id.is_some()) {
                tx.unbounded_send(act).ok();
            }
        });
        let on = self.reach.menubar.clone();
        window.on_window_should_close(cx, move |_, cx| {
            if on.get() {
                cx.hide();
                false
            } else {
                true
            }
        });
    }

    /// Makes or removes the status item to match the setting.
    fn sync_status_item(&mut self) {
        if cfg!(test) {
            return;
        }
        match (self.reach.menubar.get(), self.reach.item.is_some()) {
            (true, false) => {
                if let Some(tx) = self.reach.sender.clone() {
                    self.reach.item = crate::menubar::StatusItem::new(tx);
                }
            }
            (false, true) => self.reach.item = None,
            _ => {}
        }
        let now = OffsetDateTime::now_utc();
        let inbox = self.inbox_count();
        let fleet = self.loaded.as_ref().and_then(|l| l.fleet.as_ref());
        let menu = reach::menu(fleet, &self.marks(), inbox, now);
        if let Some(item) = self.reach.item.as_mut() {
            item.update(&menu);
        }
    }

    /// After each fleet read: the menu, then the notifications its changes
    /// call for.
    pub(crate) fn sync_reach(&mut self, cx: &mut Context<Self>) {
        self.wake_snoozes();
        self.sync_status_item();
        let Some(fleet) = self.loaded.as_ref().and_then(|l| l.fleet.clone()) else {
            return;
        };
        let now = OffsetDateTime::now_utc();
        let shown: Vec<String> = fleet
            .rows
            .iter()
            .filter(|r| self.shows_session(&r.id))
            .map(|r| r.id.clone())
            .collect();
        let quiet = |id: &str| shown.iter().any(|s| s == id);
        let alerts = self
            .reach
            .watcher
            .observe(&fleet, now, self.reach.switches, &quiet);
        for a in alerts {
            cx.show_system_notification(SystemNotification {
                tag: SharedString::from(tag(a.kind, &a.id)),
                title: SharedString::from(a.title),
                body: SharedString::from(a.body),
                actions: vec![SystemNotificationAction {
                    id: SharedString::from("act"),
                    label: SharedString::from(a.action),
                }],
            });
        }
    }

    /// The window is in front and shows session `id`: Home or Fleet, or
    /// the session's own page or x-ray.
    fn shows_session(&self, id: &str) -> bool {
        if !self.reach.window_active {
            return false;
        }
        match self.state.tabs.active_tab() {
            Some(crate::state::Tab::Xray(s)) | Some(crate::state::Tab::Session { id: s, .. }) => {
                s == id
            }
            None => matches!(self.state.tabs.section(), Section::Home | Section::Fleet),
            _ => false,
        }
    }

    fn bring_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.activate(true);
        window.activate_window();
    }

    /// Does what a menu row, a menu item or a notification asks.
    pub(crate) fn do_act(&mut self, act: Act, window: &mut Window, cx: &mut Context<Self>) {
        match act {
            Act::Quit => cx.quit(),
            Act::OpenHippocamp => self.bring_forward(window, cx),
            Act::Inbox => {
                self.bring_forward(window, cx);
                self.show_section(Section::Inbox, window, cx);
            }
            Act::Brief => {
                self.bring_forward(window, cx);
                self.show_section(Section::Home, window, cx);
                self.open_brief_mode(window, cx);
            }
            Act::Xray(id) => {
                self.bring_forward(window, cx);
                self.show_section(Section::Fleet, window, cx);
                self.open_xray(&id, window, cx);
            }
            Act::Open(id) | Act::Resume(id) => {
                let row = self
                    .loaded
                    .as_ref()
                    .and_then(|l| l.fleet.as_ref())
                    .and_then(|f| f.row(&id).cloned());
                match row {
                    Some(row) => self.open_session(row, window, cx),
                    None => {
                        self.bring_forward(window, cx);
                        self.show_section(Section::Fleet, window, cx);
                        self.open_xray(&id, window, cx);
                    }
                }
            }
        }
    }

    /// The profile to resume `row` on instead of its own: Use next, when
    /// its own is at a limit and it is a Claude Code session outside T3
    /// Code.
    pub(crate) fn resume_elsewhere(row: &Row, fleet: &Fleet) -> Option<String> {
        let own_blocked = row
            .profile
            .as_deref()
            .and_then(|p| fleet.room(p))
            .is_some_and(|h| !h.usable());
        fleet.use_next.clone().filter(|n| {
            own_blocked
                && Some(n.as_str()) != row.profile.as_deref()
                && row.tool == "claude-code"
                && row.client != Some("T3 Code")
        })
    }

    /// Open or Resume: on another profile through the confirm sheet when
    /// its own is out; else where it lives ([`launch::open_session`]).
    /// Records that the person opened session `id` (a finished
    /// session with no pull request settles once opened).
    pub(crate) fn mark_opened(&mut self, id: &str) {
        if self.reach.opened.contains_key(id) {
            return;
        }
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let next = crate::prefs::mark_opened(&self.reach.opened, id, now);
        self.reach.opened = next.clone();
        self.save_pref(|p| p.opened_sessions = next);
    }

    pub(crate) fn open_session(&mut self, row: Row, window: &mut Window, cx: &mut Context<Self>) {
        self.mark_opened(&row.id);
        let fleet = self.loaded.as_ref().and_then(|l| l.fleet.clone());
        let elsewhere = row
            .state
            .needs_resume()
            .then(|| fleet.as_ref().and_then(|f| Self::resume_elsewhere(&row, f)))
            .flatten();
        if let (Some(p), Some(f)) = (elsewhere, fleet.as_ref()) {
            let dir = f.room(&p).and_then(|h| h.dir.clone());
            let target = hippocamp_core::resume::Target {
                id: &row.id,
                tool: &row.tool,
                transcript: &row.transcript,
                cwd: row.cwd.as_deref(),
            };
            if let Some(command) = dir
                .as_deref()
                .and_then(|d| hippocamp_core::resume::command_on(&target, d))
            {
                self.bring_forward(window, cx);
                self.reach.confirm = Some(ResumeConfirm {
                    title: row.title.clone(),
                    profile: p,
                    command,
                });
                window.focus(&self.reach.confirm_focus, cx);
                cx.notify();
                return;
            }
        }
        let title = row.title.clone();
        let term = self.term();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(async move { launch::open_session(&row, &term) })
                .await;
            this.update(cx, |this, cx| {
                this.reach.said = Some(match r {
                    Ok(Opened::Said(s)) => Ok(s),
                    Ok(Opened::T3Fallback) => {
                        this.open_t3(cx);
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(title));
                        Ok("T3 Code brought forward; its thread was not found, so the title is copied to search for".into())
                    }
                    Err(e) => Err(e),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn confirm_resume(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.reach.confirm.take() else {
            return;
        };
        let term = self.term();
        let header = launch::resume_header(&c.title, &c.profile);
        self.reach.said = Some(
            launch::run_in_terminal(&c.command, &header, &term)
                .map(|()| format!("resumed on {} in {}", c.profile, term.name())),
        );
        window.focus(&self.focus_handle_for_page(), cx);
        cx.notify();
    }

    /// The handle the page's keys go to once a sheet closes.
    fn focus_handle_for_page(&self) -> FocusHandle {
        self.fleet_focus.clone()
    }

    /// What the last Open or Resume did, one muted line (an error in the
    /// error colour).
    pub(crate) fn render_reach_said(&self, look: &Look) -> Option<gpui::AnyElement> {
        let said = self.reach.said.as_ref()?;
        let (color, text) = match said {
            Ok(s) => (look.c.text_muted, s.clone()),
            Err(e) => (look.c.error, e.clone()),
        };
        Some(
            div()
                .px(look.px(2.))
                .text_size(look.text(12.))
                .text_color(color)
                .truncate()
                .child(text)
                .into_any_element(),
        )
    }

    /// Resume on another profile: the command, Run in Terminal or Cancel.
    pub(crate) fn render_resume_confirm(
        &self,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let c = self.reach.confirm.as_ref()?;
        let name = hippocamp_view::fleet::profile_label(&c.profile).to_owned();
        Some(
            div()
                .id("resume-confirm-overlay")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .justify_center()
                .items_start()
                .pt(look.px(80.))
                .px(look.px(16.))
                .bg(gpui::black().opacity(0.4))
                .occlude()
                .child(
                    div()
                        .id("resume-confirm")
                        .track_focus(&self.reach.confirm_focus)
                        .role(gpui::Role::Dialog)
                        .aria_label(format!("Resume on {name}"))
                        .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, window, cx| {
                            match e.keystroke.key.as_str() {
                                "escape" => {
                                    this.reach.confirm = None;
                                    cx.notify();
                                }
                                "enter" if crate::keyguard::allow(cx) => {
                                    this.confirm_resume(window, cx)
                                }
                                "enter" => {}
                                _ => return,
                            }
                            cx.stop_propagation();
                        }))
                        .flex()
                        .flex_col()
                        .gap(look.px(10.))
                        .w(look.px(620.))
                        .max_w_full()
                        .p(look.px(18.))
                        .rounded(look.px(10.))
                        .bg(look.c.elevated)
                        .border(look.fixed(1.))
                        .border_color(look.c.border)
                        .shadow_lg()
                        .text_size(look.text(13.))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(look.text(15.))
                                .child(format!("Resume on {name}")),
                        )
                        .child(div().text_color(look.c.text_muted).child(format!(
                            "“{}” stopped at its own profile's limit. This copies its transcript into {name}'s folder (cp -n: nothing is overwritten, the original stays) and resumes it there, in Terminal.",
                            c.title
                        )))
                        .child(
                            div()
                                .p(look.px(10.))
                                .rounded(look.px(6.))
                                .bg(look.c.window)
                                .font_family("Menlo")
                                .text_size(look.text(11.5))
                                .child(format!("$ {}", c.command)),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap(look.px(8.))
                                .child(
                                    kit::button(look, "resume-cancel", "Cancel  esc", Kind::Plain)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.reach.confirm = None;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    kit::button(
                                        look,
                                        "resume-run",
                                        "Run in Terminal  ↩",
                                        Kind::Primary,
                                    )
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.confirm_resume(window, cx)
                                    })),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    /// The morning digest's parts, from what the app has read.
    pub(crate) fn digest(&self, now: OffsetDateTime) -> Digest {
        let overnight = overnight_since(now);
        let since = self
            .reach
            .digest_from
            .unwrap_or_else(|| digest_since(self.reach.seen_at, now));
        let after = |t: std::time::SystemTime| OffsetDateTime::from(t) > since;
        let notes = self
            .review_items()
            .iter()
            .filter(|r| after(r.receipt.datetime()))
            .count();
        let rules = self
            .lessons()
            .iter()
            .filter(|l| after(l.id.datetime()))
            .count();
        let claims = self
            .loaded
            .as_ref()
            .and_then(|l| l.fleet.as_ref())
            .map(|f| {
                f.rows
                    .iter()
                    .filter(|r| r.needs_check())
                    .filter(|r| match r.state {
                        hippocamp_view::fleet::State::Finished { at }
                        | hippocamp_view::fleet::State::Interrupted { at, .. } => at > since,
                        _ => true,
                    })
                    .count()
            })
            .unwrap_or(0);
        let dreamed = self
            .loaded
            .as_ref()
            .and_then(|l| l.panels.as_ref())
            .and_then(|p| p.dreaming.as_ref().ok())
            .and_then(|d| {
                d.runs.iter().find(|r| {
                    let f = crate::rows::run_facts(r);
                    !f.dry_run && !f.failed
                })
            })
            .and_then(|r| {
                let at = OffsetDateTime::from(r.written?);
                let changed = crate::rows::run_facts(r).applied > 0;
                (at >= overnight).then(|| (super::kit::hm(at), r.report.path.clone(), changed))
            });
        Digest {
            notes,
            rules,
            claims,
            dreamed,
            waiting: self.lessons().len(),
        }
    }

    /// Home's digest line, on the day's first open until dismissed.
    pub(crate) fn render_digest(
        &self,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let now = OffsetDateTime::now_utc();
        let today = local_date(now);
        if self.reach.digest_dismissed.as_deref() == Some(today.as_str()) {
            return None;
        }
        let d = self.digest(now);
        if d.is_empty() {
            return None;
        }
        let plural = |n: usize, one: &str, many: &str| {
            if n == 1 {
                format!("1 {one}")
            } else {
                format!("{n} {many}")
            }
        };
        let link = |id: &'static str, text: String| {
            kit::link(look, id, text).text_color(look.c.text_muted)
        };
        let mut parts: Vec<gpui::AnyElement> = Vec::new();
        if d.notes > 0 {
            parts.push(
                link(
                    "digest-notes",
                    plural(d.notes, "note corrected", "notes corrected"),
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.show_section(Section::Inbox, window, cx);
                    this.inbox.filter = super::lessons::Filter::Overnight;
                    cx.notify();
                }))
                .into_any_element(),
            );
        }
        if d.rules > 0 {
            parts.push(
                link(
                    "digest-rules",
                    plural(d.rules, "rule proposed", "rules proposed"),
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.show_section(Section::Inbox, window, cx);
                    this.inbox.filter = super::lessons::Filter::All;
                    cx.notify();
                }))
                .into_any_element(),
            );
        }
        if d.claims > 0 {
            parts.push(
                link(
                    "digest-claims",
                    plural(d.claims, "claim unverified", "claims unverified"),
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    // Unverified claims are hints on their Fleet rows.
                    this.fleet_ui.group = super::fleet::Group::State;
                    this.show_section(Section::Fleet, window, cx);
                }))
                .into_any_element(),
            );
        }
        if let Some((at, path, changed)) = d.dreamed.clone() {
            // With nothing new, the run says so in its own words.
            let words = match (d.nothing_new(), changed) {
                (true, false) => format!("dreamed {at}, nothing changed"),
                (true, true) => format!("dreamed {at}, nothing new since you looked"),
                _ => format!("dreamed {at}"),
            };
            parts.push(
                link("digest-dreamed", words)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.state
                            .tabs
                            .open(crate::state::Tab::Report(path.clone()));
                        this.after_state_change(window, cx);
                    }))
                    .into_any_element(),
            );
        }
        // A 40-point banner: moon, `Overnight`, the parts muted.
        let mut row = div()
            .flex()
            .items_center()
            .gap(look.px(8.))
            .h(look.px(40.))
            .pl(look.px(14.))
            .pr(look.px(8.))
            .rounded(look.px(12.))
            .bg(look.c.surface)
            .border(look.fixed(1.))
            .border_color(look.c.border)
            .text_size(look.text(13.))
            .overflow_hidden()
            .child(kit::icon(look, "moon", 16., look.c.text_muted))
            .child(
                div()
                    .flex_shrink_0()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child("Overnight"),
            );
        for (i, p) in parts.into_iter().enumerate() {
            if i > 0 {
                row = row.child(div().text_color(kit::dim(look)).child("·"));
            }
            row = row.child(p);
        }
        if d.nothing_new() && d.waiting > 0 {
            row = row
                .child(div().text_color(kit::dim(look)).child("·"))
                .child(
                    kit::link(
                        look,
                        "digest-waiting",
                        format!(
                            "{} rule{} still waiting",
                            d.waiting,
                            if d.waiting == 1 { "" } else { "s" }
                        ),
                    )
                    .text_color(look.c.text_muted)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.show_section(Section::Inbox, window, cx);
                    })),
                );
        }
        Some(
            row.child(div().flex_1())
                .child(
                    div()
                        .id("digest-dismiss")
                        .role(gpui::Role::Button)
                        .aria_label("Dismiss until tomorrow")
                        .cursor_pointer()
                        .size(look.px(24.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(look.px(6.))
                        .hover(|s| s.bg(look.c.raised))
                        .child(kit::icon(look, "x", 14., look.c.faint))
                        .tooltip(super::tooltip::Tooltip::text("Dismiss until tomorrow"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let today = local_date(OffsetDateTime::now_utc());
                            this.reach.digest_dismissed = Some(today.clone());
                            this.save_pref(|p| p.digest_dismissed = Some(today.clone()));
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }

    /// Home or the Inbox is on screen: a viewing that starts after
    /// [`AWAY`] counts the digest from the previous one; the time is kept
    /// in the preferences at most every 5 minutes.
    pub(crate) fn note_seen(&mut self) {
        if !self.reach.window_active {
            return;
        }
        let now = OffsetDateTime::now_utc();
        if self.reach.seen_at.is_none_or(|t| now - t > AWAY) || self.reach.digest_from.is_none() {
            self.reach.digest_from = Some(digest_since(self.reach.seen_at, now));
        }
        self.reach.seen_at = Some(now);
        // The first sighting after launch is not written: a short look
        // writes nothing; one kept open is written every 5 minutes.
        let Some(saved) = self.reach.seen_saved else {
            self.reach.seen_saved = Some(now);
            return;
        };
        if now - saved > time::Duration::minutes(5) {
            self.reach.seen_saved = Some(now);
            let at = now.unix_timestamp();
            self.save_pref(|p| p.inbox_seen_at = Some(at));
        }
    }

    /// The terminal Resume and Brief's starts use.
    pub(crate) fn term(&self) -> launch::Term {
        launch::term_for(self.reach.terminal.as_deref(), launch::ghostty_app())
    }

    /// Sets the terminal (`ghostty` or `terminal`) and keeps it.
    pub(crate) fn set_terminal(&mut self, which: &'static str, cx: &mut Context<Self>) {
        self.reach.terminal = Some(which.to_owned());
        self.save_pref(|p| p.terminal = Some(which.to_owned()));
        cx.notify();
    }

    /// Writes one preference; a failure becomes a notice.
    pub(crate) fn save_pref(&mut self, change: impl FnOnce(&mut crate::prefs::Prefs)) {
        if let Err(e) = crate::prefs::update(self.themes.prefs_path(), change) {
            self.state
                .notices
                .push(format!("The setting could not be saved: {e}"));
        }
    }

    pub(crate) fn set_menubar(&mut self, on: bool, cx: &mut Context<Self>) {
        self.reach.menubar.set(on);
        self.save_pref(|p| p.menubar = on);
        self.sync_status_item();
        cx.notify();
    }

    pub(crate) fn set_notify(&mut self, kind: reach::Kind, on: bool, cx: &mut Context<Self>) {
        match kind {
            reach::Kind::Waiting => self.reach.switches.waiting = on,
            reach::Kind::Stopped => self.reach.switches.stopped = on,
            reach::Kind::Reset => self.reach.switches.reset = on,
        }
        self.save_pref(|p| match kind {
            reach::Kind::Waiting => p.notify_waiting = on,
            reach::Kind::Stopped => p.notify_stopped = on,
            reach::Kind::Reset => p.notify_reset = on,
        });
        cx.notify();
    }

    pub(crate) fn set_login(&mut self, on: bool, cx: &mut Context<Self>) {
        self.reach.login_error = crate::menubar::login::set(on).err();
        self.reach.login = crate::menubar::login::status();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_response_opens_resumes_or_shows_the_x_ray() {
        assert_eq!(
            response_act("hippocamp:waiting:s-1", true),
            Some(Act::Open("s-1".into()))
        );
        assert_eq!(
            response_act("hippocamp:stopped:s-1", true),
            Some(Act::Resume("s-1".into()))
        );
        assert_eq!(
            response_act("hippocamp:reset:codex-1:2", true),
            Some(Act::Resume("codex-1:2".into()))
        );
        assert_eq!(
            response_act("hippocamp:waiting:s-1", false),
            Some(Act::Xray("s-1".into()))
        );
        assert_eq!(response_act("other", true), None);
        assert_eq!(tag(reach::Kind::Reset, "s-1"), "hippocamp:reset:s-1");
    }

    #[test]
    fn overnight_starts_at_six_the_evening_before() {
        let now = time::macros::datetime!(2026-11-10 09:00 UTC);
        let since = overnight_since(now);
        let local = since.to_offset(super::super::status::local_offset());
        assert_eq!((local.hour(), local.minute()), (18, 0));
        assert!(since < now && now - since < time::Duration::hours(40));
    }
}
