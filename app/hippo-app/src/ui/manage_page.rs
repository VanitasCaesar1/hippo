//! Manage: real controls over hippo's settings, each change a `hippo`
//! command shown in a confirmation sheet before it runs.

use std::path::PathBuf;
use std::rc::Rc;

use gpui::{
    App, Context, FocusHandle, FontWeight, MouseButton, Role, ScrollHandle, SharedString, Window,
    div, prelude::*, px,
};
use hippo_core::settings::{self, Op};
use hippo_view::manage::{self as model, Manage};

use super::Shell;
use super::controls::{self, Act, TrackBounds};
use super::kit::{self, Kind};
use super::theme::Look;
use crate::flow::Purpose;

/// One `hippo doctor` row: check, status, detail.
pub type DoctorRow = (String, String, String);

/// A change waiting for its confirmation.
#[derive(Debug, Clone)]
pub struct Confirm {
    pub title: String,
    /// What it does, in a sentence.
    pub what: String,
    /// `- before` / `+ after` per configuration change.
    pub diffs: Vec<String>,
    pub commands: Vec<Vec<String>>,
    /// Why it cannot be made (a plan that failed); Change is then off.
    pub error: Option<String>,
    /// What the result line names.
    pub label: String,
    /// The one path the change acts on, shown once.
    pub path: Option<std::path::PathBuf>,
}

/// Manage's state.
pub struct ManageUi {
    pub data: Option<Manage>,
    pub loading: bool,
    pub confirm: Option<Confirm>,
    pub confirm_focus: FocusHandle,
    /// The running command's label.
    pub running: Option<String>,
    pub result: Option<(String, Result<(), String>)>,
    /// The budget slider's value while it is dragged or stepped, in KB.
    pub budget_drag: Option<usize>,
    pub track: TrackBounds,
    /// `hippo doctor`'s rows, once run.
    pub doctor: Option<Result<Vec<DoctorRow>, String>>,
    /// The note chips offered for "add a required note".
    pub adding_required: bool,
    pub scroll: ScrollHandle,
    /// A folder picked with Add a folder, for the next draw's sheet.
    pub pending_folder: Option<String>,
    /// Each card's bounds at the last paint, for the side list's jumps.
    pub card_bounds: Rc<std::cell::RefCell<[Option<gpui::Bounds<gpui::Pixels>>; 8]>>,
    /// Doctor's passing rows are shown (folded by default).
    pub doctor_open: bool,
    /// Doctor rows whose engine text is shown (behind `▸`).
    pub doctor_detail: std::collections::HashSet<usize>,
    /// The Hooks card's `?` is open.
    pub hooks_help: bool,
    /// A pack folder picked with Add a pack, for the next draw's sheet.
    pub pending_pack: Option<std::path::PathBuf>,
}

impl ManageUi {
    pub fn new(cx: &mut App) -> ManageUi {
        ManageUi {
            data: None,
            loading: false,
            confirm: None,
            confirm_focus: cx.focus_handle(),
            running: None,
            result: None,
            budget_drag: None,
            track: Rc::new(std::cell::Cell::new(None)),
            doctor: None,
            adding_required: false,
            scroll: ScrollHandle::new(),
            pending_folder: None,
            card_bounds: Rc::new(std::cell::RefCell::new([None; 8])),
            doctor_open: false,
            doctor_detail: Default::default(),
            hooks_help: false,
            pending_pack: None,
        }
    }
}

const MONO: &str = "Menlo";

/// The cards, in the side list's order, with their icons.
const CARDS: [(&str, &str); 8] = [
    ("Agents", "terminal"),
    ("Hooks", "link"),
    ("Schedule", "clock"),
    ("Context", "stack"),
    ("Dreaming & lessons", "moon"),
    ("Packs", "box"),
    ("Health", "heart"),
    ("Settings", "gear"),
];

impl Shell {
    /// Reads Manage's cards on the background executor.
    pub(crate) fn load_manage(&mut self, cx: &mut Context<Self>) {
        if self.manage_ui.loading {
            return;
        }
        self.manage_ui.loading = true;
        let home = self.installation.home.clone();
        cx.spawn(async move |this, cx| {
            let data = cx
                .background_executor()
                .spawn(async move {
                    let user = hippo_core::config::user_home();
                    model::load(&model::Inputs {
                        home: &home,
                        user_home: user.as_deref(),
                        now: time::OffsetDateTime::now_utc(),
                    })
                })
                .await;
            this.update(cx, |this, cx| {
                this.manage_ui.loading = false;
                this.manage_ui.data = Some(data);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// A control's action on the shell.
    fn act(
        &self,
        cx: &mut Context<Self>,
        f: impl Fn(&mut Shell, &mut Window, &mut Context<Shell>) + 'static,
    ) -> Act {
        let e = cx.entity().downgrade();
        let f = Rc::new(f);
        Rc::new(move |window: &mut Window, app: &mut App| {
            let f = f.clone();
            e.update(app, |this, cx| f(this, window, cx)).ok();
        })
    }

    fn manage_busy(&self) -> bool {
        self.manage_ui.running.is_some()
            || self
                .flow
                .job
                .as_ref()
                .is_some_and(|j| j.result.lock().map(|r| r.is_none()).unwrap_or(false))
    }

    /// Opens the sheet for configuration changes, planned in process (no
    /// write) to show each line before and after, then `extra` commands.
    pub(crate) fn ask_config(
        &mut self,
        title: impl Into<String>,
        what: impl Into<String>,
        ops: Vec<(&'static str, Op)>,
        extra: Vec<Vec<String>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut diffs = Vec::new();
        let mut commands = Vec::new();
        let mut error = None;
        for (key, op) in &ops {
            match settings::plan(&self.installation.home, key, op) {
                Ok(p) => diffs.push(p.diff()),
                Err(e) => error = Some(e.to_string()),
            }
            commands.push(model::config_args(key, op));
        }
        commands.extend(extra);
        let title = title.into();
        self.manage_ui.confirm = Some(Confirm {
            label: title.clone(),
            title,
            what: what.into(),
            diffs,
            commands,
            error,
            path: None,
        });
        window.focus(&self.manage_ui.confirm_focus, cx);
        cx.notify();
    }

    /// Opens the sheet for plain commands.
    pub(crate) fn ask_commands(
        &mut self,
        title: impl Into<String>,
        what: impl Into<String>,
        commands: Vec<Vec<String>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = title.into();
        self.manage_ui.confirm = Some(Confirm {
            label: title.clone(),
            title,
            what: what.into(),
            diffs: Vec::new(),
            commands,
            error: None,
            path: None,
        });
        window.focus(&self.manage_ui.confirm_focus, cx);
        cx.notify();
    }

    /// Change: runs the sheet's commands.
    fn confirm_manage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.manage_ui.confirm.take() else {
            return;
        };
        if c.error.is_some() || self.manage_busy() {
            self.manage_ui.confirm = Some(c);
            return;
        }
        let doctor = c.commands.len() == 1 && c.commands[0] == ["doctor"];
        self.manage_ui.running = Some(c.label.clone());
        self.manage_ui.result = None;
        self.flow.log.clear();
        self.flow.progress = None;
        self.start_job(
            if doctor {
                Purpose::Doctor
            } else {
                Purpose::Manage
            },
            c.commands,
            cx,
        );
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// A Manage command ended.
    pub(crate) fn manage_done(
        &mut self,
        purpose: Purpose,
        r: Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        let label = self.manage_ui.running.take().unwrap_or_default();
        if purpose == Purpose::Doctor {
            let out = self.flow.log.join("\n");
            let rows = model::doctor_rows(&out);
            self.manage_ui.doctor = Some(if rows.is_empty() {
                Err(r
                    .err()
                    .unwrap_or_else(|| "hippo doctor printed no rows".into()))
            } else {
                Ok(rows)
            });
        } else {
            if label == super::incidents_ui::DRAFT_TITLE {
                self.incident_scan_done(&r, cx);
            }
            self.manage_ui.result = Some((label.clone(), r));
            // The toast goes by itself after six seconds, unless another
            // result replaced it.
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(6))
                    .await;
                this.update(cx, |this, cx| {
                    if this
                        .manage_ui
                        .result
                        .as_ref()
                        .is_some_and(|(l, _)| *l == label)
                    {
                        this.manage_ui.result = None;
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
        self.manage_ui.budget_drag = None;
        self.manage_ui.data = None;
        self.load_manage(cx);
        self.reload(cx);
    }

    pub(super) fn render_manage_tab(
        &mut self,
        look: &Look,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if self.manage_ui.data.is_none() && !self.manage_ui.loading {
            self.load_manage(cx);
        }
        if let Some(dir) = self.manage_ui.pending_pack.take() {
            self.ask_commands(
                "Add a pack",
                "Validates the pack, has a fresh-context reviewer read it (one model call), and copies it into .hippo/packs/ only when the review confirms it.",
                vec![vec!["add".into(), dir.display().to_string()]],
                window,
                cx,
            );
            if let Some(c) = self.manage_ui.confirm.as_mut() {
                c.path = Some(dir);
            }
        }
        if let Some(folder) = self.manage_ui.pending_folder.take() {
            self.ask_config(
                format!("Read {folder}"),
                "hippo reads the transcripts in this folder from the next ingest; it never changes them.",
                vec![("transcript_roots", Op::Add(folder))],
                Vec::new(),
                window,
                cx,
            );
        }
        let width = window.viewport_size().width;
        let show_nav = width >= px(1000.);
        let two = width >= px(1180.);
        let Some(data) = self.manage_ui.data.clone() else {
            return div()
                .p(look.px(20.))
                .text_color(look.c.text_muted)
                .child("Reading hippo's settings…")
                .into_any_element();
        };
        let cards: Vec<gpui::AnyElement> = vec![
            self.agents_card(&data, look, cx),
            self.hooks_card(&data, look, cx),
            self.schedule_card(&data, look, cx),
            self.context_card(&data, look, cx),
            self.dreaming_card(&data, look, cx),
            self.packs_card(&data, look, cx),
            self.health_card(look, cx),
            self.settings_card(look, cx),
        ];
        let busy = self.manage_busy();
        let status = self.manage_status(look);
        let nav = show_nav.then(|| {
            let counts: Vec<String> = vec![
                data.agents.iter().filter(|a| a.on).count().to_string(),
                if data.hooks.first().is_some_and(|h| h.all()) {
                    "on".into()
                } else {
                    "off".into()
                },
                match data.jobs {
                    [true, true] => "on".into(),
                    [false, false] => "off".into(),
                    _ => "1 of 2".into(),
                },
                data.shown
                    .as_ref()
                    .and_then(|s| s.budget_bytes)
                    .map(|b| format!("{} KB", b / 1024))
                    .unwrap_or_default(),
                data.passes.iter().filter(|p| p.1).count().to_string(),
                data.packs.len().to_string(),
                {
                    let n = self.health_lines().len();
                    if n == 0 { String::new() } else { n.to_string() }
                },
                String::new(),
            ];
            let scroll = self.manage_ui.scroll.clone();
            let cells = self.manage_ui.card_bounds.clone();
            div()
                .flex()
                .flex_col()
                .flex_shrink_0()
                .w(look.px(196.))
                .gap(look.px(2.))
                .pt(look.px(16.))
                .pl(look.px(12.))
                .children(CARDS.iter().enumerate().map(move |(i, (name, icon))| {
                    let scroll = scroll.clone();
                    let cells = cells.clone();
                    div()
                        .id(SharedString::from(format!("manage-nav-{i}")))
                        .role(Role::Link)
                        .aria_label(*name)
                        .flex()
                        .items_center()
                        .h(look.px(34.))
                        .px(look.px(12.))
                        .gap(look.px(10.))
                        .rounded(look.px(7.))
                        .cursor_pointer()
                        .hover(|h| h.bg(look.c.hover))
                        .child(kit::icon(look, icon, 16., look.c.text_muted))
                        .child(div().flex_1().child(*name))
                        .child(
                            div()
                                .text_size(look.text(12.))
                                .text_color(look.c.text_muted)
                                .child(counts[i].clone()),
                        )
                        .on_click(move |_, window, _| {
                            // Bring the card's top under the list's top.
                            let (Some(card), view) = (cells.borrow()[i], scroll.bounds()) else {
                                return;
                            };
                            let off = scroll.offset();
                            let dy = card.origin.y - view.origin.y - px(16.);
                            let max = scroll.max_offset().y;
                            let y = (off.y - dy).min(px(0.)).max(-max);
                            scroll.set_offset(gpui::point(off.x, y));
                            window.refresh();
                        })
                }))
                .child(div().flex_1())
                .children(
                    self.loaded
                        .as_ref()
                        .and_then(|l| l.setup.workspace())
                        .map(|w| {
                            div()
                                .pt(look.px(16.))
                                .px(look.px(12.))
                                .text_size(look.text(11.5))
                                .text_color(kit::dim(look))
                                .child(format!(
                                    "workspace: {}",
                                    w.root
                                        .file_name()
                                        .map(|n| n.to_string_lossy().into_owned())
                                        .unwrap_or_default()
                                ))
                        }),
                )
                .child(
                    div()
                        .px(look.px(12.))
                        .pb(look.px(12.))
                        .text_size(look.text(11.5))
                        .text_color(kit::dim(look))
                        .truncate()
                        .text_ellipsis_start()
                        .child(format!("$HIPPO_HOME: {}", tilde(&self.installation.home))),
                )
        });
        // Each card measured, for the side list's jumps.
        let measured: Vec<gpui::AnyElement> = cards
            .into_iter()
            .enumerate()
            .map(|(i, c)| {
                let cell = self.manage_ui.card_bounds.clone();
                div()
                    .relative()
                    .w_full()
                    .child(
                        gpui::canvas(
                            move |b, _, _| cell.borrow_mut()[i] = Some(b),
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                    )
                    .child(c)
                    .into_any_element()
            })
            .collect();
        let mut slots: Vec<Option<gpui::AnyElement>> = measured.into_iter().map(Some).collect();
        let mut take = |i: usize| slots[i].take().expect("each card once");
        // One settings column (Zed's settings page), in the side
        // list's order.
        let _ = two;
        let columns: Vec<gpui::AnyElement> = vec![
            div()
                .flex()
                .flex_col()
                .gap(look.px(28.))
                .flex_1()
                .min_w_0()
                .max_w(look.px(720.))
                .children((0..8).map(&mut take))
                .into_any_element(),
        ];
        let grid = div()
            .id("manage-cards")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .track_scroll(&self.manage_ui.scroll)
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(look.px(14.))
                    .px(look.px(20.))
                    .pt(look.px(8.))
                    .pb(look.px(28.))
                    .children(columns),
            );
        let _ = busy;
        let toast = self.manage_toast(look, cx);
        div()
            .id("manage")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .text_size(look.text(13.))
            .children(status)
            .child(div().flex().flex_1().min_h_0().children(nav).child(grid))
            .children(toast)
            .into_any_element()
    }

    /// How the last change ended, as a toast at the bottom right:
    /// it goes by itself after a few seconds, or with its close icon.
    fn manage_toast(&self, look: &Look, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let (label, r) = self.manage_ui.result.as_ref()?;
        let (icon, color, text) = match r {
            Ok(()) => ("circle-check", look.c.ok, format!("{label}: done")),
            Err(e) => ("circle-x", look.c.err, format!("{label}: {e}")),
        };
        Some(
            div()
                .absolute()
                .bottom(look.px(16.))
                .right(look.px(16.))
                .max_w(look.px(440.))
                .child(
                    div()
                        .id("manage-toast")
                        .flex()
                        .items_start()
                        .gap(look.px(10.))
                        .px(look.px(14.))
                        .py(look.px(11.))
                        .rounded(look.px(12.))
                        .bg(look.c.raised)
                        .border(look.fixed(1.))
                        .border_color(look.c.border_strong)
                        .shadow_lg()
                        .text_size(look.text(13.))
                        .child(kit::icon(look, icon, 16., color).mt(look.px(1.)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .line_clamp(4)
                                .text_ellipsis()
                                .child(text),
                        )
                        .child(
                            kit::icon_btn(look, "manage-toast-close", "x", "Close", false)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.manage_ui.result = None;
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    /// The strip over the cards: what is running (with ingest's progress)
    /// or how the last command ended.
    fn manage_status(&self, look: &Look) -> Option<gpui::AnyElement> {
        let (text, color) = if let Some(label) = &self.manage_ui.running {
            let mut t = format!("{label}: running");
            if let Some(p) = self.flow.progress.as_ref().filter(|p| p.files_total > 0) {
                t = format!(
                    "{label}: {}% · {} of {} files",
                    (p.files_done * 100 / p.files_total.max(1)),
                    p.files_done,
                    p.files_total
                );
            } else if let Some(l) = self.flow.log.iter().rev().find(|l| !l.trim().is_empty()) {
                t = format!("{label}: {}", l.trim());
            }
            (t, look.c.text_muted)
        } else {
            return None;
        };
        let frac = self
            .manage_ui
            .running
            .as_ref()
            .and(self.flow.progress.as_ref())
            .filter(|p| p.files_total > 0)
            .map(|p| p.files_done as f32 / p.files_total as f32);
        Some(
            div()
                .flex()
                .flex_col()
                .gap(look.px(4.))
                .flex_shrink_0()
                .px(look.px(16.))
                .py(look.px(8.))
                .border_b(look.fixed(1.))
                .border_color(look.c.border_variant)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(look.px(8.))
                        .child(kit::dot(look, color))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(look.text(12.5))
                                .child(text),
                        ),
                )
                .children(frac.map(|f| kit::meter(look, f, look.c.text)))
                .into_any_element(),
        )
    }

    fn card_head(
        &self,
        look: &Look,
        title: &str,
        sub: &str,
        right: Option<gpui::AnyElement>,
    ) -> gpui::Div {
        // A settings column's heading (Zed's settings page): the
        // title at 15 points, what it is under it, its one action at the
        // right; a hairline below.
        div()
            .flex()
            .items_start()
            .gap(look.px(10.))
            .pb(look.px(8.))
            .border_b(look.fixed(1.))
            .border_color(look.c.border)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(look.px(2.))
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(look.text(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(look.c.text)
                            .child(title.to_owned()),
                    )
                    .when(!sub.is_empty(), |d| {
                        d.child(
                            div()
                                .text_size(look.text(12.5))
                                .text_color(look.c.text_muted)
                                .child({
                                    // Sentence case: the first letter up.
                                    let mut c = sub.chars();
                                    c.next()
                                        .map(|f| f.to_uppercase().chain(c).collect::<String>())
                                        .unwrap_or_default()
                                }),
                        )
                    }),
            )
            .children(right)
    }

    fn agents_card(&self, data: &Manage, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let busy = self.manage_busy();
        let detect = kit::button(look, "manage-detect", "Detect again", Kind::Plain).on_click(
            cx.listener(|this, _, _, cx| {
                this.manage_ui.data = None;
                this.load_manage(cx);
            }),
        );
        let mut rows: Vec<gpui::AnyElement> = Vec::new();
        let mut never: Vec<gpui::AnyElement> = Vec::new();
        for (i, a) in data.agents.iter().enumerate() {
            // A profile folder with no session and no meter: apart,
            // dimmed, with Ignore.
            if a.never_used() {
                let a2 = a.clone();
                let ignore = self.act(cx, move |this, window, cx| {
                    this.ask_config(
                        format!("Ignore {}", a2.name()),
                        format!(
                            "{} has no session and no signed-in account: probably a mistyped CLAUDE_CONFIG_DIR. hippo stops looking at it.",
                            a2.display
                        ),
                        vec![("exclude_roots", Op::Add(a2.display.clone()))],
                        Vec::new(),
                        window,
                        cx,
                    );
                });
                never.push(
                    div()
                        .flex()
                        .items_center()
                        .gap(look.px(10.))
                        .py(look.px(6.))
                        .opacity(0.6)
                        .child(kit::avatar(look, a.profile.as_deref(), None))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .child(div().truncate().child(a.name()))
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(look.text(11.5))
                                        .text_color(kit::dim(look))
                                        .child(format!(
                                            "{} · no sessions · probably a mistyped CLAUDE_CONFIG_DIR",
                                            a.display
                                        )),
                                ),
                        )
                        .child(if a.on {
                            kit::button(look, ("agent-ignore", i), "Ignore", Kind::Plain)
                                .when(!busy, |d| {
                                    let ignore = ignore.clone();
                                    d.on_click(move |_, window, cx| ignore(window, cx))
                                })
                                .into_any_element()
                        } else {
                            kit::pill(look, "ignored", look.c.text_muted).into_any_element()
                        })
                        .into_any_element(),
                );
                continue;
            }
            let a2 = a.clone();
            let act = self.act(cx, move |this, window, cx| {
                let on = !a2.on;
                let what = if on {
                    format!("hippo reads {} again from the next ingest.", a2.display)
                } else {
                    format!(
                        "Ingest, the usage estimate and Fleet stop reading {}. Its sessions already indexed stay searchable.",
                        a2.display
                    )
                };
                this.ask_config(
                    format!("{} {}", if on { "Read" } else { "Stop reading" }, a2.name()),
                    what,
                    model::agent_ops(&a2, on),
                    Vec::new(),
                    window,
                    cx,
                );
            });
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(10.))
                    .py(look.px(8.))
                    .border_b(look.fixed(1.))
                    .border_color(look.c.border_variant.opacity(0.6))
                    .child(kit::avatar(
                        look,
                        a.profile.as_deref().or(if a.tool == "codex" {
                            Some("codex")
                        } else {
                            // A folder added by hand: its mark, not `?`.
                            std::path::Path::new(&a.display)
                                .file_name()
                                .and_then(|n| n.to_str())
                        }),
                        None,
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(look.px(8.))
                                    // A folder added by hand is named by
                                    // its last part, the path below it.
                                    .child(div().truncate().child(
                                        if a.profile.is_none() && a.tool != "codex" {
                                            std::path::Path::new(&a.display)
                                                .file_name()
                                                .map(|n| n.to_string_lossy().into_owned())
                                                .unwrap_or_else(|| a.name())
                                        } else {
                                            a.name()
                                        },
                                    ))
                                    .when(!a.matched, |d| {
                                        d.child(kit::pill(look, "FOUND", look.c.text_muted))
                                    }),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .font_family(MONO)
                                    .text_size(look.text(11.5))
                                    .text_color(kit::dim(look))
                                    .child(format!(
                                        "{} · {} session{}",
                                        middle_ellipsis(
                                            &tilde(std::path::Path::new(&a.display)),
                                            48
                                        ),
                                        a.files,
                                        if a.files == 1 { "" } else { "s" }
                                    )),
                            ),
                    )
                    // A labelled state, not a bare dot.
                    .when(a.live, |d| {
                        d.child(kit::dot_pill(look, "live", kit::ok(look)))
                    })
                    .child(controls::switch(
                        look,
                        ("agent-switch", i),
                        format!("Read {}", a.name()),
                        a.on,
                        !busy,
                        act,
                    ))
                    .into_any_element(),
            );
        }
        let add = kit::button(look, "manage-add-folder", "+ Add a folder", Kind::Plain)
            .on_click(cx.listener(|this, _, _, cx| this.pick_transcript_folder(cx)));
        settings_section(look)
            .p(look.px(16.))
            .gap(look.px(4.))
            .child(self.card_head(
                look,
                "Agents",
                "transcripts hippo reads · read-only",
                Some(detect.into_any_element()),
            ))
            .children(rows)
            .when(!never.is_empty(), |d| {
                d.child(div().pt(look.px(10.)).child(kit::group(
                    look,
                    "Found, never used",
                    look.c.text_muted,
                )))
                .children(never)
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(look.px(10.))
                    .pt(look.px(8.))
                    .child(add)
                    .when(!data.unsupported.is_empty(), |d| {
                        d.child(
                            div()
                                .text_size(look.text(12.))
                                .text_color(kit::dim(look))
                                .child(format!("not read yet: {}", data.unsupported.join(", "))),
                        )
                    }),
            )
            .into_any_element()
    }

    /// Asks for a folder; the next draw opens the sheet adding it to
    /// `transcript_roots`.
    fn pick_transcript_folder(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Read this folder".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(dir) = paths.into_iter().next() else {
                return;
            };
            this.update(cx, |this, cx| {
                this.manage_ui.pending_folder = Some(tilde(&dir));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn hooks_card(&self, data: &Manage, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let busy = self.manage_busy();
        let what = [
            (
                "Session start",
                "the context block, under the budget, with kept rules",
            ),
            ("Every prompt", "notes relevant to the prompt"),
            ("Session end", "the handoff and the session record"),
        ];
        let mut rows: Vec<gpui::AnyElement> = Vec::new();
        for (i, h) in data.hooks.iter().enumerate() {
            let h2 = h.clone();
            let act = move |this: &mut Shell, window: &mut Window, cx: &mut Context<Shell>| {
                let dir = h2.dir.display().to_string();
                let (verb, sub) = if h2.all() {
                    ("Remove hippo's hooks from", "uninstall")
                } else {
                    ("Install hippo's hooks in", "install")
                };
                this.ask_commands(
                    format!("{verb} {}", h2.label),
                    format!(
                        "{} its .claude/settings.json: hippo's three entries only; every other key and hook stays.",
                        if sub == "install" { "Adds to" } else { "Removes from" },
                    ),
                    vec![vec![
                        "hooks".into(),
                        sub.into(),
                        "--workspace".into(),
                        dir,
                    ]],
                    window,
                    cx,
                );
                if let Some(c) = this.manage_ui.confirm.as_mut() {
                    c.path = Some(h2.dir.clone());
                }
            };
            let n = h.on.iter().filter(|b| **b).count();
            let state: gpui::AnyElement = if h.all() {
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(8.))
                    .child(
                        div()
                            .text_size(look.text(12.))
                            .text_color(kit::ok(look))
                            .child("installed"),
                    )
                    .child(
                        kit::button(
                            look,
                            ("hook-remove", i),
                            "Remove",
                            if busy { Kind::Disabled } else { Kind::Plain },
                        )
                        .when(!busy, |b| {
                            b.on_click(
                                cx.listener(move |this, _, window, cx| act(this, window, cx)),
                            )
                        }),
                    )
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(8.))
                    .child(
                        div()
                            .text_size(look.text(12.))
                            .text_color(look.c.text_muted)
                            .child(if n == 0 {
                                "not installed".to_owned()
                            } else {
                                format!("{n} of 3 installed")
                            }),
                    )
                    .child(
                        kit::button(
                            look,
                            ("hook-install", i),
                            "Install",
                            if busy { Kind::Disabled } else { Kind::Plain },
                        )
                        .when(!busy, |b| {
                            b.on_click(
                                cx.listener(move |this, _, window, cx| act(this, window, cx)),
                            )
                        }),
                    )
                    .into_any_element()
            };
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(10.))
                    .py(look.px(7.))
                    .border_b(look.fixed(1.))
                    .border_color(look.c.border_variant.opacity(0.6))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .gap(look.px(8.))
                                    .child(div().truncate().child(h.label.clone()))
                                    .when(h.workspace, |d| {
                                        d.child(kit::pill(
                                            look,
                                            "hippo's workspace",
                                            look.c.text_muted,
                                        ))
                                    }),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_ellipsis_start()
                                    .font_family(MONO)
                                    .text_size(look.text(11.5))
                                    .text_color(kit::dim(look))
                                    .child(tilde(&h.dir)),
                            ),
                    )
                    .child(state)
                    .into_any_element(),
            );
        }
        let help_open = self.manage_ui.hooks_help;
        let help = div()
            .id("hooks-help")
            .role(Role::Button)
            .aria_label("What the hooks do")
            .relative()
            .size(look.px(20.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(look.px(10.))
            .border(look.fixed(1.))
            .border_color(look.c.border)
            .text_size(look.text(11.))
            .text_color(look.c.text_muted)
            .cursor_pointer()
            .child("?")
            .on_click(cx.listener(|this, _, _, cx| {
                this.manage_ui.hooks_help = !this.manage_ui.hooks_help;
                cx.notify();
            }))
            .children(help_open.then(|| {
                gpui::deferred(
                    gpui::anchored().anchor(gpui::Anchor::TopRight).child(
                        div()
                            .id("hooks-help-pop")
                            .mt(look.px(24.))
                            .w(look.px(330.))
                            .p(look.px(12.))
                            .rounded(look.px(8.))
                            .bg(look.c.elevated)
                            .border(look.fixed(1.))
                            .border_color(look.c.border)
                            .shadow_md()
                            .flex()
                            .flex_col()
                            .gap(look.px(6.))
                            .children(what.iter().map(|(t, d)| {
                                div()
                                    .flex()
                                    .gap(look.px(8.))
                                    .text_size(look.text(12.))
                                    .child(div().flex_shrink_0().w(look.px(96.)).child(*t))
                                    .child(div().min_w_0().text_color(kit::dim(look)).child(*d))
                            }))
                            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                this.manage_ui.hooks_help = false;
                                cx.notify();
                            })),
                    ),
                )
            }));
        settings_section(look)
            .p(look.px(16.))
            .gap(look.px(4.))
            .child(self.card_head(
                look,
                "Hooks",
                "what agents get from hippo, per workspace",
                Some(help.into_any_element()),
            ))
            .children(rows)
            .into_any_element()
    }

    /// One launchd job's switch: its install or uninstall command, shown
    /// first.
    fn job_act(&self, job: &'static str, on: bool, cx: &mut Context<Self>) -> Act {
        self.act(cx, move |this, window, cx| {
                let sub = if on { "uninstall" } else { "install" };
                let (title, what) = match (job, on) {
                    ("ingest", true) => ("Stop the ingest job", "Unloads dev.hippo.ingest and removes its file from ~/Library/LaunchAgents. New sessions are not read until it is installed again; the dreaming job is left as it is."),
                    ("ingest", false) => ("Start the ingest job", "Writes dev.hippo.ingest to ~/Library/LaunchAgents and loads it: it reads new transcripts every few minutes and once now."),
                    (_, true) => ("Stop the dreaming job", "Unloads dev.hippo.dream and removes its file from ~/Library/LaunchAgents. Nothing is dreamed until it is installed again; the ingest job is left as it is."),
                    _ => ("Start the dreaming job", "Writes dev.hippo.dream to ~/Library/LaunchAgents and loads it: dreaming and the lessons scan run nightly."),
                };
                this.ask_commands(
                    title,
                    what,
                    vec![vec![
                        "schedule".into(),
                        sub.into(),
                        "--job".into(),
                        job.into(),
                    ]],
                    window,
                    cx,
                );
            })
    }

    fn schedule_card(
        &self,
        data: &Manage,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let busy = self.manage_busy();
        let Some(shown) = data.shown.clone() else {
            return settings_section(look)
                .p(look.px(16.))
                .child("No configuration")
                .into_any_element();
        };
        let [ingest_on, dream_on] = data.jobs;
        // A changed interval or time rewrites only its own job, when that
        // job is installed.
        let reinstall = move |job: &str, on: bool| -> Vec<Vec<String>> {
            if on {
                vec![vec![
                    "schedule".into(),
                    "install".into(),
                    "--job".into(),
                    job.into(),
                ]]
            } else {
                Vec::new()
            }
        };
        let ingest_act = self.job_act("ingest", ingest_on, cx);
        let dream_act = self.job_act("dream", dream_on, cx);
        let interval = shown.ingest_interval_minutes;
        let step_i = |up: bool| {
            let extra = reinstall("ingest", ingest_on);
            cx.listener(move |this: &mut Shell, _: &gpui::ClickEvent, window, cx| {
                let next = model::step_interval(interval, up);
                if next == interval {
                    return;
                }
                this.ask_config(
                    format!("Ingest every {next} min"),
                    "The ingest job reads new transcripts at this interval.",
                    vec![("ingest_interval_minutes", Op::Set(next.to_string()))],
                    extra.clone(),
                    window,
                    cx,
                );
            })
        };
        let dream_at = shown.dream_at.clone();
        let step_d = |up: bool| {
            let extra = reinstall("dream", dream_on);
            let at = dream_at.clone();
            cx.listener(move |this: &mut Shell, _: &gpui::ClickEvent, window, cx| {
                let next = model::step_clock(&at, up);
                this.ask_config(
                    format!("Dream at {next}"),
                    "The nightly dreaming run (and the lessons scan after it) starts at this time.",
                    vec![("dream_at", Op::Set(next))],
                    extra.clone(),
                    window,
                    cx,
                );
            })
        };
        let row = |title: &str, sub: String, control: gpui::AnyElement| {
            div()
                .flex()
                .items_center()
                .gap(look.px(10.))
                .py(look.px(8.))
                .border_b(look.fixed(1.))
                .border_color(look.c.border_variant.opacity(0.6))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(title.to_owned())
                        .child(
                            div()
                                .text_size(look.text(12.))
                                .text_color(kit::dim(look))
                                .child(sub),
                        ),
                )
                .child(control)
        };
        let status = self.loaded.as_ref().and_then(|l| l.status.as_ref());
        let indexed = status
            .and_then(|s| s.last_ingest)
            .map(|t| {
                format!(
                    "last {}",
                    kit::hm(t.to_offset(super::status::local_offset()))
                )
            })
            .unwrap_or_else(|| "not run yet".into());
        let dreamed = format!("nightly at {} · the lessons scan follows", shown.dream_at);
        let run = |id: &'static str,
                   label: &'static str,
                   primary: bool,
                   args: Vec<String>,
                   what: &'static str| {
            kit::button(
                look,
                id,
                label,
                if busy {
                    Kind::Disabled
                } else if primary {
                    Kind::Primary
                } else {
                    Kind::Plain
                },
            )
            .when(!busy, |b| {
                b.on_click(cx.listener(move |this, _, window, cx| {
                    this.ask_commands(label, what, vec![args.clone()], window, cx);
                }))
            })
        };
        settings_section(look)
            .p(look.px(16.))
            .gap(look.px(2.))
            .child(self.card_head(
                look,
                "Schedule",
                "two launchd jobs, no daemon",
                None,
            ))
            .child(row(
                "Ingest",
                if ingest_on {
                    indexed
                } else {
                    format!("off · {indexed}")
                },
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(10.))
                    .child(controls::stepper(
                        look,
                        "interval",
                        format!("every {interval} min"),
                        104.,
                        !busy,
                        step_i(false),
                        step_i(true),
                    ))
                    .child(controls::switch(
                        look,
                        "ingest-switch",
                        "The ingest job",
                        ingest_on,
                        !busy,
                        ingest_act,
                    ))
                    .into_any_element(),
            ))
            .child(row(
                "Dream",
                if dream_on {
                    dreamed
                } else {
                    "off · nothing is dreamed until it is on".into()
                },
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(10.))
                    .child(controls::stepper(
                        look,
                        "dream-at",
                        shown.dream_at.clone(),
                        64.,
                        !busy,
                        step_d(false),
                        step_d(true),
                    ))
                    .child(controls::switch(
                        look,
                        "dream-switch",
                        "The dreaming job",
                        dream_on,
                        !busy,
                        dream_act,
                    ))
                    .into_any_element(),
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(look.px(8.))
                    .pt(look.px(10.))
                    .child(run(
                        "ingest-now",
                        "Ingest now",
                        false,
                        vec!["ingest".into(), "--progress".into()],
                        "Reads new transcripts into the index now. It reads the agents' files and never changes them.",
                    ))
                    .child(run(
                        "dream-now",
                        "Dream now",
                        // The one button that spends model calls is
                        // not the most inviting.
                        false,
                        vec!["dream".into()],
                        "Runs the dreaming passes and the lessons scan now. Every change is reviewed by a fresh-context reviewer and can be undone; it calls the model.",
                    ))
                    .child(run(
                        "dream-dry",
                        "Dry run",
                        false,
                        vec!["dream".into(), "--dry-run".into()],
                        "Runs the dreaming passes and writes only the report: no note, proposal or snapshot changes. It calls the model.",
                    )),
            )
            .into_any_element()
    }

    fn context_card(&self, data: &Manage, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let busy = self.manage_busy();
        let Some(shown) = data.shown.clone() else {
            return settings_section(look)
                .p(look.px(16.))
                .child("No configuration")
                .into_any_element();
        };
        let Some(budget) = shown.budget_bytes else {
            return settings_section(look)
                .p(look.px(16.))
                .gap(look.px(8.))
                .child(self.card_head(
                    look,
                    "Context",
                    "the block agents get at session start",
                    None,
                ))
                .child(
                    div().text_color(kit::dim(look)).child(
                        "No notes folder yet: the step-two sheet gives your agents a memory.",
                    ),
                )
                .into_any_element();
        };
        let (lo, hi) = model::BUDGET_KB;
        let kb = self.manage_ui.budget_drag.unwrap_or(budget / 1024);
        let frac = (kb.saturating_sub(lo)) as f32 / (hi - lo) as f32;
        let track = self.manage_ui.track.clone();
        let t_down = track.clone();
        let t_move = track.clone();
        let commit = |this: &mut Shell,
                      window: &mut Window,
                      cx: &mut Context<Shell>,
                      budget: usize| {
            let Some(kb) = this.manage_ui.budget_drag else {
                return;
            };
            if kb * 1024 == budget {
                this.manage_ui.budget_drag = None;
                cx.notify();
                return;
            }
            this.ask_config(
                format!("Context budget {kb} KB"),
                "The hard limit of the block an agent gets at session start; hippo fails loudly over it.",
                vec![("budget_bytes", Op::Set((kb * 1024).to_string()))],
                Vec::new(),
                window,
                cx,
            );
        };
        let slider = controls::slider(look, "budget-slider", frac, track)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                    if let Some(f) = controls::track_fraction(&t_down, e.position.x) {
                        this.manage_ui.budget_drag =
                            Some(lo + ((hi - lo) as f32 * f).round() as usize);
                        cx.notify();
                    }
                }),
            )
            .on_mouse_move(cx.listener(move |this, e: &gpui::MouseMoveEvent, _, cx| {
                if e.pressed_button != Some(MouseButton::Left)
                    || this.manage_ui.budget_drag.is_none()
                {
                    return;
                }
                if let Some(f) = controls::track_fraction(&t_move, e.position.x) {
                    this.manage_ui.budget_drag = Some(lo + ((hi - lo) as f32 * f).round() as usize);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _: &gpui::MouseUpEvent, window, cx| {
                    commit(this, window, cx, budget)
                }),
            )
            .on_key_down(
                cx.listener(move |this, e: &gpui::KeyDownEvent, window, cx| {
                    let cur = this.manage_ui.budget_drag.unwrap_or(budget / 1024);
                    match e.keystroke.key.as_str() {
                        "left" => this.manage_ui.budget_drag = Some(cur.saturating_sub(1).max(lo)),
                        "right" => this.manage_ui.budget_drag = Some((cur + 1).min(hi)),
                        "enter" if crate::keyguard::allow(cx) => commit(this, window, cx, budget),
                        "enter" => {}
                        _ => return,
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            );
        let (block, required_n) = data.block.unwrap_or((0, 0));
        let chips = data.required.iter().enumerate().map(|(i, (p, use_))| {
            let title = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let ignored = use_.filter(|(_, n)| *n >= 5).map(|(ig, n)| ig * 100 / n.max(1));
            let p2 = p.clone();
            let t2 = title.clone();
            // Neutral until two of three sessions ignore it: the tune
            // tip's rule, one rule everywhere.
            let warn = use_.is_some_and(|(ig, n)| {
                n >= 5 && hippo_view::memory::ignored_level(ig, n) == 2
            });
            div()
                .id(("required-chip", i))
                .flex()
                .items_center()
                .gap(look.px(6.))
                .h(look.px(28.))
                .pl(look.px(12.))
                .pr(look.px(4.))
                .rounded(look.px(6.))
                .border(look.fixed(1.))
                .border_color(if warn { look.c.warning } else { look.c.border })
                .text_color(if warn { look.c.warning } else { look.c.text })
                .text_size(look.text(12.5))
                .child(match ignored {
                    Some(x) => format!("{title} · ignored {x}%"),
                    None => title.clone(),
                })
                .child(
                    div()
                        .id(("required-remove", i))
                        .role(Role::Button)
                        .aria_label(format!("Make {title} relevant-only"))
                        .px(look.px(6.))
                        .rounded(look.px(4.))
                        .cursor_pointer()
                        .text_color(look.c.text_muted)
                        .hover(|h| h.bg(look.c.hover).text_color(look.c.text))
                        .child("×")
                        .when(!busy, |d| {
                            d.on_click(cx.listener(move |this, _, window, cx| {
                                this.ask_config(
                                    format!("Make {t2} relevant-only"),
                                    "It leaves the required list; the context compiler still offers it when a task matches it.",
                                    vec![("required_notes", Op::Remove(p2.display().to_string()))],
                                    Vec::new(),
                                    window,
                                    cx,
                                );
                            }))
                        }),
                )
        });
        let adding = self.manage_ui.adding_required;
        let offers: Vec<(PathBuf, String)> = if adding {
            self.memory_map()
                .map(|m| {
                    m.sorted(hippo_view::memory::SizeBy::Reads)
                        .into_iter()
                        .map(|i| &m.tiles[i])
                        .filter(|t| !t.required)
                        .take(6)
                        .map(|t| (t.path.clone(), t.title.clone()))
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        settings_section(look)
            .p(look.px(16.))
            .gap(look.px(10.))
            .child(self.card_head(
                look,
                "Context budget",
                "hard limit per session",
                Some(
                    div()
                        .text_size(look.text(20.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format!("{kb} KB"))
                        .into_any_element(),
                ),
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(10.))
                    .child(div().text_size(look.text(12.)).text_color(kit::dim(look)).child(format!("{lo}")))
                    .child(slider)
                    .child(div().text_size(look.text(12.)).text_color(kit::dim(look)).child(format!("{hi} KB"))),
            )
            .child(
                div()
                    .text_size(look.text(12.))
                    .text_color(kit::dim(look))
                    .child(format!(
                        "start block now {} · {required_n} required · relevant notes are added per prompt · fails loudly over the line",
                        hippo_view::memory::human(block as u64)
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(look.px(8.))
                    .child(kit::label(look, "REQUIRED", look.c.text_muted))
                    .children(chips)
                    .child(
                        kit::button(look, "required-add", if adding { "Close" } else { "+" }, Kind::Plain)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.manage_ui.adding_required = !this.manage_ui.adding_required;
                                cx.notify();
                            })),
                    ),
            )
            .when(adding, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(look.px(6.))
                        .child(div().text_size(look.text(12.)).text_color(kit::dim(look)).child("most read, not required:"))
                        .children(offers.into_iter().enumerate().map(|(i, (p, t))| {
                            let t2 = t.clone();
                            kit::chip(look, ("required-offer", i), format!("+ {t}"), false).on_click(
                                cx.listener(move |this, _, window, cx| {
                                    this.manage_ui.adding_required = false;
                                    this.ask_config(
                                        format!("Require {t2}"),
                                        "Every session's start block names it as a note to read.",
                                        vec![("required_notes", Op::Add(p.display().to_string()))],
                                        Vec::new(),
                                        window,
                                        cx,
                                    );
                                }),
                            )
                        })),
                )
            })
            .child(
                div().flex().child(
                    kit::button(look, "preview-task", "Preview for a task…", Kind::Plain).on_click(
                        cx.listener(|this, _, window, cx| this.open_brief_mode(window, cx)),
                    ),
                ),
            )
            .into_any_element()
    }

    fn dreaming_card(
        &self,
        data: &Manage,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let busy = self.manage_busy();
        let Some(shown) = data.shown.clone() else {
            return settings_section(look)
                .p(look.px(16.))
                .child("No configuration")
                .into_any_element();
        };
        if shown.workspace.is_none() {
            return settings_section(look)
                .p(look.px(16.))
                .child(self.card_head(look, "Dreaming & lessons", "needs a notes folder", None))
                .into_any_element();
        }
        let row = |title: String, sub: String, control: gpui::AnyElement| {
            div()
                .flex()
                .items_center()
                .gap(look.px(10.))
                .py(look.px(8.))
                .border_b(look.fixed(1.))
                .border_color(look.c.border_variant.opacity(0.6))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(title)
                        .child(
                            div()
                                .text_size(look.text(12.))
                                .text_color(kit::dim(look))
                                .child(sub),
                        ),
                )
                .child(control)
        };
        let mut card = settings_section(look)
            .p(look.px(16.))
            .gap(look.px(2.))
            .child(self.card_head(look, "Dreaming & lessons", "what runs each night", None));
        for (i, (name, on)) in data.passes.iter().enumerate() {
            let (n2, on2) = (name.clone(), *on);
            let act = self.act(cx, move |this, window, cx| {
                let op = if on2 {
                    Op::Add(n2.clone())
                } else {
                    Op::Remove(n2.clone())
                };
                this.ask_config(
                    format!("{} the {n2} pass", if on2 { "Turn off" } else { "Turn on" }),
                    "hippo dream skips a pass in passes_off; hippo dream --pass still runs it.",
                    vec![("passes_off", op)],
                    Vec::new(),
                    window,
                    cx,
                );
            });
            let sub = match name.as_str() {
                "verify" => "checks notes against what sessions did".to_owned(),
                "consolidate" => "merges what the week's sessions settled".to_owned(),
                _ => "a pass from a pack".to_owned(),
            };
            card = card.child(row(
                format!("{name} pass"),
                sub,
                controls::switch(
                    look,
                    ("pass-switch", i),
                    format!("The {name} pass"),
                    *on,
                    !busy,
                    act,
                )
                .into_any_element(),
            ));
        }
        let lessons = shown.lessons.unwrap_or(true);
        let act = self.act(cx, move |this, window, cx| {
            this.ask_config(
                format!("{} the lessons pass", if lessons { "Turn off" } else { "Turn on" }),
                "After dreaming, the lessons scan reads corrections and repeated failures into the Inbox.",
                vec![("lessons", Op::Set(if lessons { "off" } else { "on" }.into()))],
                Vec::new(),
                window,
                cx,
            );
        });
        card = card.child(row(
            "Lessons pass".into(),
            "after dreaming · corrections and repeated failures → Inbox".into(),
            controls::switch(
                look,
                "lessons-switch",
                "The lessons pass",
                lessons,
                !busy,
                act,
            )
            .into_any_element(),
        ));
        let keep = shown.snapshot_keep_days;
        let step = |up: bool| {
            cx.listener(move |this: &mut Shell, _: &gpui::ClickEvent, window, cx| {
                let next = model::step_keep(keep, up);
                if next == keep {
                    return;
                }
                let op = match next {
                    Some(d) => Op::Set(d.to_string()),
                    None => Op::Unset,
                };
                this.ask_config(
                    format!("Snapshots: {}", model::keep_label(next)),
                    "After a dreaming run that wrote, snapshots older than this are deleted; a change whose snapshot is gone can no longer be reverted.",
                    vec![("snapshot_keep_days", op)],
                    Vec::new(),
                    window,
                    cx,
                );
            })
        };
        card.child(row(
            "Snapshots".into(),
            format!(
                "{} kept for Undo",
                hippo_view::memory::human(data.snapshot_bytes)
            ),
            controls::stepper(
                look,
                "keep",
                model::keep_label(keep),
                76.,
                !busy,
                step(false),
                step(true),
            )
            .into_any_element(),
        ))
        .into_any_element()
    }

    /// The packs the workspace has, with Add and Remove through the CLI
    ///hippo has no catalogue the app could read: a pack comes
    /// from a folder (the hippo repository's `packs/`) or a git URL.
    fn packs_card(&self, data: &Manage, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        use hippo_core::packs::install::PackState;
        let busy = self.manage_busy();
        let rows = data.packs.iter().enumerate().map(|(i, p)| {
            let (state, color) = match &p.state {
                PackState::Builtin => ("built in".to_owned(), kit::dim(look)),
                PackState::Local => ("local".to_owned(), kit::dim(look)),
                PackState::Clean => ("clean".to_owned(), kit::ok(look)),
                PackState::Edited(n) => (
                    format!("{n} file{} edited", if *n == 1 { "" } else { "s" }),
                    look.c.warning,
                ),
                PackState::Unreadable(why) => (format!("unreadable: {why}"), look.c.error),
            };
            let name = p.name.clone();
            let removable = !matches!(p.state, PackState::Builtin);
            div()
                .flex()
                .items_center()
                .gap(look.px(10.))
                .py(look.px(7.))
                .border_b(look.fixed(1.))
                .border_color(look.c.border_variant.opacity(0.6))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .flex()
                                .gap(look.px(8.))
                                .child(div().truncate().child(p.name.clone()))
                                .children(p.version.clone().map(|v| {
                                    div().text_color(kit::dim(look)).child(v)
                                })),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_ellipsis_start()
                                .font_family(MONO)
                                .text_size(look.text(11.5))
                                .text_color(kit::dim(look))
                                .child(
                                    p.source
                                        .clone()
                                        .unwrap_or_else(|| "ships with hippo".into()),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(look.text(12.))
                        .text_color(color)
                        .child(state),
                )
                .when(removable, |d| {
                    d.child(
                        kit::button(
                            look,
                            ("pack-remove", i),
                            "Remove",
                            if busy { Kind::Disabled } else { Kind::Plain },
                        )
                        .when(!busy, |b| {
                            b.on_click(cx.listener(move |this, _, window, cx| {
                                this.ask_commands(
                                    format!("Remove the {name} pack"),
                                    "Deletes the pack's folder under .hippo/packs/. Its passes stop running from the next dreaming run; notes it changed stay as they are.",
                                    vec![vec!["packs".into(), "remove".into(), name.clone()]],
                                    window,
                                    cx,
                                );
                            }))
                        }),
                    )
                })
        });
        settings_section(look)
            .p(look.px(16.))
            .gap(look.px(4.))
            .child(self.card_head(
                look,
                "Packs",
                "the dreaming passes this workspace runs",
                Some(
                    kit::button(
                        look,
                        "pack-add",
                        "Add a pack…",
                        if busy { Kind::Disabled } else { Kind::Plain },
                    )
                    .when(!busy, |b| {
                        b.on_click(cx.listener(|this, _, _, cx| this.pick_pack_folder(cx)))
                    })
                    .into_any_element(),
                ),
            ))
            .children(rows)
            .when(data.packs.is_empty(), |d| {
                d.child(
                    div()
                        .py(look.px(6.))
                        .text_color(look.c.text_muted)
                        .child("No packs yet: give your agents a memory first."),
                )
            })
            .child(
                div()
                    .pt(look.px(6.))
                    .text_size(look.text(12.))
                    .text_color(kit::dim(look))
                    .child("Add takes a pack folder (the hippo repository's packs/ has the published ones) and runs hippo add, which has a fresh reviewer read it first. A git URL works from the CLI."),
            )
            .into_any_element()
    }

    fn pick_pack_folder(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Add this pack".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(dir) = paths.into_iter().next() else {
                return;
            };
            this.update(cx, |this, cx| {
                this.manage_ui.pending_pack = Some(dir);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn health_card(&self, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let busy = self.manage_busy();
        let run = kit::button(
            look,
            "run-doctor",
            "Run doctor",
            if busy { Kind::Disabled } else { Kind::Plain },
        )
        .when(!busy, |b| {
            b.on_click(cx.listener(|this, _, window, cx| {
                this.ask_commands(
                    "Run doctor",
                    "Checks the setup and the last runs; it calls the model once to check it answers.",
                    vec![vec!["doctor".into()]],
                    window,
                    cx,
                );
            }))
        });
        let (word, good) = self.health_word();
        let mut card = settings_section(look)
            .p(look.px(16.))
            .gap(look.px(8.))
            .child(self.card_head(look, "Health", &word, Some(run.into_any_element())));
        let _ = good;
        match &self.manage_ui.doctor {
            Some(Ok(rows)) => {
                // Failing and warning rows first and open; the rest one line
                // `n checks pass ▸`.
                let passing = rows.iter().filter(|r| r.1 == "ok").count();
                let open = self.manage_ui.doctor_open;
                let mut order: Vec<usize> =
                    (0..rows.len()).filter(|i| rows[*i].1 != "ok").collect();
                if open {
                    order.extend((0..rows.len()).filter(|i| rows[*i].1 == "ok"));
                }
                if passing > 0 {
                    card = card.child(
                        div()
                            .id("doctor-pass")
                            .role(Role::Button)
                            .flex()
                            .items_center()
                            .gap(look.px(10.))
                            .py(look.px(4.))
                            .cursor_pointer()
                            .text_size(look.text(12.5))
                            .text_color(look.c.text_muted)
                            .child(kit::dot(look, kit::ok(look)))
                            .child(format!(
                                "{passing} check{} pass {}",
                                if passing == 1 { "" } else { "s" },
                                if open { "▾" } else { "▸" }
                            ))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.manage_ui.doctor_open = !this.manage_ui.doctor_open;
                                cx.notify();
                            })),
                    );
                }
                for i in order {
                    let (name, status, detail) = &rows[i];
                    let detail = &doctor_words(detail);
                    // A problem in one plain sentence, the engine's
                    // text behind `▸`.
                    let shown = self.manage_ui.doctor_detail.contains(&i);
                    let plain = (status != "ok").then(|| doctor_sentence(name, detail));
                    let color = match status.as_str() {
                        "ok" => kit::ok(look),
                        "warn" => look.c.warning,
                        _ => look.c.error,
                    };
                    let fix = model::doctor_fix(name, status);
                    let show_memory = name == "linter" && status != "ok";
                    card = card.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(look.px(10.))
                            .py(look.px(4.))
                            .child(kit::dot(look, color))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(look.px(2.))
                                    .text_size(look.text(12.5))
                                    .child(match &plain {
                                        Some(p) => div()
                                            .id(("doctor-row", i))
                                            .flex()
                                            .gap(look.px(6.))
                                            .cursor_pointer()
                                            .child(div().min_w_0().child(p.clone()))
                                            .child(
                                                div()
                                                    .flex_shrink_0()
                                                    .text_color(look.c.text_muted)
                                                    .child(if shown { "▾" } else { "▸" }),
                                            )
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                let d = &mut this.manage_ui.doctor_detail;
                                                if !d.remove(&i) {
                                                    d.insert(i);
                                                }
                                                cx.notify();
                                            }))
                                            .into_any_element(),
                                        None => div()
                                            .truncate()
                                            .child(format!("{name} · {detail}"))
                                            .into_any_element(),
                                    })
                                    .when(shown && plain.is_some(), |d| {
                                        d.child(
                                            div()
                                                .text_size(look.text(11.5))
                                                .text_color(look.c.text_muted)
                                                .child(format!("{name} · {detail}")),
                                        )
                                    }),
                            )
                            .children(fix.map(|args| {
                                let line = format!("hippo {}", args.join(" "));
                                kit::button(
                                    look,
                                    ("doctor-fix", i),
                                    "Fix",
                                    if busy { Kind::Disabled } else { Kind::Plain },
                                )
                                .when(!busy, |b| {
                                    b.on_click(cx.listener(move |this, _, window, cx| {
                                        this.ask_commands(
                                            format!("Fix {}", line),
                                            "What hippo doctor suggests for this row.",
                                            vec![args.clone()],
                                            window,
                                            cx,
                                        );
                                    }))
                                })
                            }))
                            .when(show_memory, |d| {
                                d.child(
                                    kit::button(
                                        look,
                                        ("doctor-memory", i),
                                        "Show in Memory",
                                        Kind::Plain,
                                    )
                                    .on_click(cx.listener(
                                        |this, _, window, cx| {
                                            this.show_section(
                                                crate::state::Section::Memory,
                                                window,
                                                cx,
                                            );
                                        },
                                    )),
                                )
                            }),
                    );
                }
            }
            Some(Err(e)) => {
                card = card.child(div().text_color(look.c.error).child(e.clone()));
            }
            None => {
                card = card.child(self.render_health(look));
            }
        }
        // How often an incident was a false alarm, said by the
        // person; absent when the week had none.
        if let Some(line) = self.incidents_alarm_line() {
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(10.))
                    .py(look.px(4.))
                    .text_size(look.text(12.5))
                    .child(kit::dot(look, look.c.faint))
                    .child(div().flex_1().min_w_0().truncate().child(line)),
            );
        }
        card.into_any_element()
    }

    fn settings_card(&self, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.themes.committed().label();
        let answers = self.ask_answers;
        let setting = |title: &str, detail: String| {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(div().child(title.to_owned()))
                .child(
                    div()
                        .text_size(look.text(12.))
                        .text_color(kit::dim(look))
                        .child(detail),
                )
        };
        let act = self.act(cx, |this, _, cx| {
            this.set_ask_answers(!this.ask_answers, cx)
        });
        settings_section(look)
            .p(look.px(16.))
            .gap(look.px(12.))
            .child(self.card_head(look, "Settings", "this app only", None))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(12.))
                    .child(setting("Theme", theme))
                    .child(
                        kit::button(look, "manage-theme", "Select theme… ⇧⌘T", Kind::Plain)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_theme_picker(window, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(12.))
                    .child(setting(
                        "Written answers in Ask",
                        "one model call, citations checked; off: sources only".into(),
                    ))
                    .child(controls::switch(
                        look,
                        "answers-switch",
                        "Written answers in Ask",
                        answers,
                        true,
                        act,
                    )),
            )
            .child(setting(
                "Text size",
                format!(
                    "{}% · ⌘= larger, ⌘- smaller, ⌘0 default",
                    self.state.scale.percent()
                ),
            ))
            .child(self.reach_settings(look, cx))
            .into_any_element()
    }

    /// The reach settings: the menu-bar item, a switch per notification,
    /// open at login.
    fn reach_settings(&self, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        use crate::menubar::login::Status;
        use crate::reach::Kind as N;
        let row = |title: &str, detail: String, switch: gpui::Stateful<gpui::Div>| {
            div()
                .flex()
                .items_center()
                .gap(look.px(12.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(div().child(title.to_owned()))
                        .child(
                            div()
                                .text_size(look.text(12.))
                                .text_color(kit::dim(look))
                                .child(detail),
                        ),
                )
                .child(switch)
        };
        let menubar = self.reach.menubar.get();
        let sw = self.reach.switches;
        let notify = |id: &'static str, label: &'static str, on: bool, act: controls::Act| {
            controls::switch(look, id, label, on, true, act)
        };
        let act_waiting = self.act(cx, move |this, _, cx| {
            this.set_notify(N::Waiting, !sw.waiting, cx)
        });
        let act_stopped = self.act(cx, move |this, _, cx| {
            this.set_notify(N::Stopped, !sw.stopped, cx)
        });
        let act_reset = self.act(cx, move |this, _, cx| {
            this.set_notify(N::Reset, !sw.reset, cx)
        });
        let login = self.reach.login;
        let login_on = matches!(login, Some(Status::Enabled | Status::RequiresApproval));
        let login_detail = match (&self.reach.login_error, login) {
            (Some(e), _) => format!("macOS refused: {e}"),
            (None, Some(Status::RequiresApproval)) => {
                "waiting for your approval in System Settings › General › Login Items".into()
            }
            (None, None) => "not available on this macOS".into(),
            _ => "starts Hippo when you log in, for the menu-bar item".into(),
        };
        div()
            .flex()
            .flex_col()
            .gap(look.px(12.))
            .child(row(
                "Menu-bar item",
                "who is waiting and which account is next, in the menu bar; closing the window keeps Hippo running".into(),
                controls::switch(
                    look,
                    "menubar-switch",
                    "Menu-bar item",
                    menubar,
                    true,
                    self.act(cx, move |this, _, cx| this.set_menubar(!menubar, cx)),
                ),
            ))
            .child(row(
                "Notify when an agent waits on you",
                "once per question, at most once an hour per session; quiet while Hippo shows it".into(),
                notify("notify-waiting", "Notify when an agent waits on you", sw.waiting, act_waiting),
            ))
            .child(row(
                "Notify when an agent stops at its limit",
                "with Resume, on the account to use next when its own is out".into(),
                notify("notify-stopped", "Notify when an agent stops at its limit", sw.stopped, act_stopped),
            ))
            .child(row(
                "Notify when a limit resets",
                "while a session stopped at it is still to resume".into(),
                notify("notify-reset", "Notify when a limit resets", sw.reset, act_reset),
            ))
            .child(row(
                "Open at login",
                login_detail,
                controls::switch(
                    look,
                    "login-switch",
                    "Open at login",
                    login_on,
                    login.is_some(),
                    self.act(cx, move |this, _, cx| this.set_login(!login_on, cx)),
                ),
            ))
            .children(self.terminal_setting(look, cx))
            .into_any_element()
    }

    /// The terminal Resume and Brief's starts use, offered when
    /// Ghostty is installed.
    fn terminal_setting(&self, look: &Look, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        crate::launch::ghostty_app()?;
        let ghostty = matches!(self.term(), crate::launch::Term::Ghostty(_));
        Some(
            div()
                .flex()
                .items_center()
                .gap(look.px(12.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child("Terminal")
                        .child(
                            div()
                                .text_size(look.text(12.))
                                .text_color(kit::dim(look))
                                .child("where Resume, Start in Claude Code and Start in Codex run"),
                        ),
                )
                .child(
                    kit::chip(look, "term-ghostty", "Ghostty", ghostty)
                        .on_click(cx.listener(|this, _, _, cx| this.set_terminal("ghostty", cx))),
                )
                .child(
                    kit::chip(look, "term-terminal", "Terminal", !ghostty)
                        .on_click(cx.listener(|this, _, _, cx| this.set_terminal("terminal", cx))),
                )
                .into_any_element(),
        )
    }

    /// The confirmation sheet over the window.
    pub(super) fn render_manage_confirm(
        &self,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let c = self.manage_ui.confirm.as_ref()?;
        let can = c.error.is_none() && !self.manage_busy();
        // The path is named once above; the commands carry it short.
        let lines: Vec<String> = c
            .commands
            .iter()
            .map(|a| {
                let line = format!("$ {}", hippo_view::onboarding::shell_line(a));
                match &c.path {
                    Some(p) => {
                        let full = p.display().to_string();
                        line.replace(&full, &middle_ellipsis(&tilde(p), 40))
                    }
                    None => line,
                }
            })
            .collect();
        Some(
            div()
                .id("manage-confirm-overlay")
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
                        .id("manage-confirm")
                        .key_context("ManageConfirm")
                        .track_focus(&self.manage_ui.confirm_focus)
                        .role(Role::Dialog)
                        .aria_label(c.title.clone())
                        .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, window, cx| {
                            match e.keystroke.key.as_str() {
                                "escape" => {
                                    this.manage_ui.confirm = None;
                                    this.manage_ui.budget_drag = None;
                                    cx.notify();
                                }
                                "enter" if crate::keyguard::allow(cx) => {
                                    this.confirm_manage(window, cx)
                                }
                                "enter" => {}
                                _ => return,
                            }
                            cx.stop_propagation();
                        }))
                        .flex()
                        .flex_col()
                        .gap(look.px(10.))
                        .w(look.px(440.))
                        .max_w_full()
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
                                .child(c.title.clone()),
                        )
                        .child(div().text_color(look.c.text_muted).child(c.what.clone()))
                        .children(c.path.as_ref().map(|p| {
                            div()
                                .font_family(MONO)
                                .text_size(look.text(12.))
                                .child(middle_ellipsis(&tilde(p), 64))
                        }))
                        .children(c.diffs.iter().map(|d| {
                            let mut lines = d.lines();
                            let file = lines.next().unwrap_or("").to_owned();
                            div()
                                .flex()
                                .flex_col()
                                .gap(look.px(2.))
                                .p(look.px(10.))
                                .rounded(look.px(6.))
                                .bg(look.c.window)
                                .font_family(MONO)
                                .text_size(look.text(11.5))
                                .child(
                                    div()
                                        .text_color(kit::dim(look))
                                        .child(tilde(std::path::Path::new(&file))),
                                )
                                .children(lines.map(|l| {
                                    let color = if l.starts_with("- ") {
                                        look.c.deleted
                                    } else if l.starts_with("+ ") {
                                        look.c.created
                                    } else {
                                        look.c.text_muted
                                    };
                                    div().text_color(color).child(l.to_owned())
                                }))
                        }))
                        .child(
                            div()
                                .pt(look.px(4.))
                                .text_size(look.text(12.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(look.c.text_muted)
                                .child("Runs"),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(look.px(2.))
                                .px(look.px(12.))
                                .py(look.px(10.))
                                .rounded(look.px(8.))
                                .bg(look.c.inset)
                                .border(look.fixed(1.))
                                .border_color(look.c.border)
                                .font_family(MONO)
                                .text_size(look.text(12.))
                                .children(
                                    lines.into_iter().map(|l| {
                                        div().child(l.trim_start_matches("$ ").to_owned())
                                    }),
                                ),
                        )
                        // Only when it is not the usual home, on
                        // one line.
                        .when(!default_home(&self.installation.home), |d| {
                            d.child(
                                div()
                                    .truncate()
                                    .text_size(look.text(11.5))
                                    .text_color(kit::dim(look))
                                    .child(format!(
                                        "acts on $HIPPO_HOME {}",
                                        middle_ellipsis(&tilde(&self.installation.home), 48)
                                    )),
                            )
                        })
                        .children(
                            c.error
                                .clone()
                                .map(|e| div().text_color(look.c.error).child(e)),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap(look.px(8.))
                                .child(
                                    kit::button_key(
                                        look,
                                        "manage-cancel",
                                        "Cancel",
                                        "esc",
                                        Kind::Ghost,
                                    )
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.manage_ui.confirm = None;
                                            this.manage_ui.budget_drag = None;
                                            cx.notify();
                                        },
                                    )),
                                )
                                .child(
                                    kit::button_key(
                                        look,
                                        "manage-change",
                                        confirm_verb(&c.title),
                                        "↵",
                                        if can { Kind::Primary } else { Kind::Disabled },
                                    )
                                    .when(can, |b| {
                                        b.on_click(cx.listener(|this, _, window, cx| {
                                            this.confirm_manage(window, cx)
                                        }))
                                    }),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

/// `text` cut in the middle to at most `max` characters: `~/Code/…/hippo`.
pub fn middle_ellipsis(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max || max < 5 {
        return text.to_owned();
    }
    let keep = max - 1;
    let tail = keep / 2 + keep % 2;
    let head = keep - tail;
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(&chars[chars.len() - tail..]);
    out
}

/// A doctor row's detail in the app's words: RFC 3339 times as
/// `21:50` (or `Mon 21:50` on another day), `3675 bytes` as `3.6 KB`.
/// The confirm button's words: the title's verb and what it acts
/// on, `Start job`, `Add pack`, `Make relevant-only`, `Set budget`.
pub fn confirm_verb(title: &str) -> String {
    let t = title.trim_end_matches('…');
    let lower = t.to_lowercase();
    let first = t.split_whitespace().next().unwrap_or("Change");
    if lower.starts_with("make ") && lower.ends_with(" relevant-only") {
        return "Make relevant-only".into();
    }
    if lower.starts_with("turn off") || lower.starts_with("turn on") {
        let w: Vec<&str> = t.split_whitespace().take(2).collect();
        return format!("{} pass", w.join(" "));
    }
    if lower.starts_with("stop reading") {
        return "Stop reading".into();
    }
    // An incident's Never again.
    if lower.starts_with("draft a rule") {
        return "Draft rule".into();
    }
    let noun = [
        ("job", "job"),
        ("pack", "pack"),
        ("hooks", "hooks"),
        ("pass", "pass"),
    ]
    .iter()
    .find(|(k, _)| lower.split_whitespace().any(|w| w == *k))
    .map(|(_, n)| *n);
    match (first, noun) {
        ("Ignore", _) => "Ignore folder".into(),
        ("Require", _) => "Require note".into(),
        ("Read", _) => "Read profile".into(),
        (v, Some(n)) => format!("{v} {n}"),
        _ if lower.starts_with("context budget") => "Set budget".into(),
        _ if lower.starts_with("ingest every") => "Set interval".into(),
        _ if lower.starts_with("dream at") => "Set time".into(),
        _ if lower.starts_with("snapshots") => "Set snapshots".into(),
        _ if t.split_whitespace().count() <= 3 => t.to_owned(),
        (v, None) => v.to_owned(),
    }
}

/// A failing or warning doctor row in one plain sentence; the
/// engine's own text stays behind `▸`. Zero counts are left out.
pub fn doctor_sentence(name: &str, detail: &str) -> String {
    let d = detail.to_lowercase();
    let job = if d.contains("dev.hippo.ingest") && !d.contains("dev.hippo.dream") {
        "The ingest job"
    } else if d.contains("dev.hippo.dream") && !d.contains("dev.hippo.ingest") {
        "The dreaming job"
    } else {
        "The schedule"
    };
    match name {
        "launchd" if d.contains("not installed") || d.contains("missing") => {
            format!("{job} is not installed")
        }
        "launchd" => format!("{job} needs a look"),
        "last success" => "No successful dreaming run is recorded".into(),
        "last report" => "The last dreaming run failed".into(),
        "model" => "The model did not answer".into(),
        "lessons" => "The lessons pass failed".into(),
        "config" => "The configuration has a problem".into(),
        "index" => "The index needs a look".into(),
        "notes dir" | "notes" => "The notes folder cannot be read".into(),
        _ => {
            // `linter · 22 notes; 0 errors, 6 warnings (4 missing …, 0 bad
            // …)` reads `linter: 6 warnings (4 missing frontmatter)`.
            let first = detail.split(';').nth(1).unwrap_or(detail).trim();
            let kept: Vec<String> = first
                .split([',', '(', ')'])
                .map(str::trim)
                .filter(|p| !p.is_empty() && !p.starts_with("0 "))
                .map(str::to_owned)
                .collect();
            format!("{name}: {}", kept.join(", "))
        }
    }
}

/// Whether `home` is the usual `~/.hippo`.
fn default_home(home: &std::path::Path) -> bool {
    hippo_core::config::user_home().is_some_and(|h| h.join(".hippo") == home)
}

pub fn doctor_words(detail: &str) -> String {
    let off = super::status::local_offset();
    let today = time::OffsetDateTime::now_utc().to_offset(off).date();
    let words = doctor_words_at(detail, off, today);
    // Paths under the home directory read `~/…`, as everywhere in Manage.
    match hippo_core::config::user_home() {
        Some(h) => words.replace(&format!("{}/", h.display()), "~/"),
        None => words,
    }
}

fn doctor_words_at(detail: &str, off: time::UtcOffset, today: time::Date) -> String {
    let mut out: Vec<String> = Vec::new();
    let words: Vec<&str> = detail.split(' ').collect();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let (core, trail) = match w.find([':', ',', ';', ')']) {
            // A trailing punctuation mark after a timestamp's last digit.
            Some(k) if k + 1 == w.len() && !w.ends_with("Z:") => (&w[..k], &w[k..]),
            _ => (w, ""),
        };
        let core_ts = core.strip_suffix(':').unwrap_or(core);
        if let Ok(t) = time::OffsetDateTime::parse(
            core_ts.trim_end_matches(':'),
            &time::format_description::well_known::Rfc3339,
        ) {
            let l = t.to_offset(off);
            let hm = format!("{:02}:{:02}", l.hour(), l.minute());
            let at = if l.date() == today {
                hm
            } else {
                let d = crate::state::edition_title(l.date());
                format!("{} {hm}", &d[..3.min(d.len())])
            };
            let colon = if w.ends_with(':') { ":" } else { trail };
            out.push(format!("{at}{colon}"));
            i += 1;
            continue;
        }
        if let (Ok(n), Some(next)) = (w.parse::<u64>(), words.get(i + 1)) {
            let unit = next.trim_end_matches([';', ',', ')']);
            if unit == "bytes" || unit == "B" {
                let rest = &next[unit.len()..];
                out.push(format!("{}{rest}", hippo_view::memory::human(n)));
                i += 2;
                continue;
            }
        }
        out.push(w.to_owned());
        i += 1;
    }
    out.join(" ")
}

/// A path with the home directory written `~`.
pub fn tilde(p: &std::path::Path) -> String {
    match hippo_core::config::user_home()
        .and_then(|h| p.strip_prefix(&h).ok().map(|r| r.to_path_buf()))
    {
        Some(r) if r.as_os_str().is_empty() => "~".into(),
        Some(r) => format!("~/{}", r.display()),
        None => p.display().to_string(),
    }
}

/// A settings section: no surface or border, the column's
/// rhythm only; its head draws the hairline.
fn settings_section(look: &Look) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(look.px(2.))
        .text_size(look.text(13.))
}

#[cfg(test)]
mod step6_tests {
    use super::*;

    #[test]
    fn doctor_problems_read_as_one_sentence() {
        assert_eq!(
            doctor_sentence(
                "launchd",
                "schedule not installed: dev.hippo.ingest missing from /x/LaunchAgents; run hippo schedule install"
            ),
            "The ingest job is not installed"
        );
        assert_eq!(
            doctor_sentence(
                "linter",
                "22 notes; 0 errors, 6 warnings (4 missing frontmatter, 0 bad frontmatter)"
            ),
            "linter: 6 warnings, 4 missing frontmatter"
        );
    }

    #[test]
    fn confirm_buttons_repeat_the_titles_verb() {
        assert_eq!(confirm_verb("Start the ingest job"), "Start job");
        assert_eq!(confirm_verb("Stop the dreaming job"), "Stop job");
        assert_eq!(confirm_verb("Add a pack"), "Add pack");
        assert_eq!(confirm_verb("Remove the glossary pack"), "Remove pack");
        assert_eq!(
            confirm_verb("Install hippo's hooks in demo"),
            "Install hooks"
        );
        assert_eq!(confirm_verb("Ignore x"), "Ignore folder");
        assert_eq!(
            confirm_verb("Make Agent rules relevant-only"),
            "Make relevant-only"
        );
        assert_eq!(confirm_verb("Turn off the lessons pass"), "Turn off pass");
        assert_eq!(confirm_verb("Context budget 12 KB"), "Set budget");
    }

    #[test]
    fn doctor_rows_read_in_the_apps_words() {
        let off = time::UtcOffset::from_hms(9, 30, 0).unwrap();
        let today = time::macros::date!(2026 - 11 - 10);
        assert_eq!(
            doctor_words_at(
                "last ingest 2026-11-09T17:50:54.868565Z: 18 files seen",
                off,
                today
            ),
            "last ingest 03:20: 18 files seen"
        );
        assert_eq!(
            doctor_words_at("last ingest 2026-11-08T06:00:00Z", off, today),
            "last ingest Sun 15:30"
        );
        assert_eq!(
            doctor_words_at("3675 bytes; kept 7 days (snapshot_keep_days)", off, today),
            "3.6 KB; kept 7 days (snapshot_keep_days)"
        );
        assert_eq!(doctor_words_at("ok", off, today), "ok");
    }

    #[test]
    fn a_long_path_is_cut_in_the_middle() {
        assert_eq!(middle_ellipsis("~/short", 20), "~/short");
        let cut = middle_ellipsis("~/Code/a-very-long-folder-name/and/another/hippo", 20);
        assert_eq!(cut.chars().count(), 20);
        assert!(cut.starts_with("~/Code/a-") && cut.ends_with("hippo") && cut.contains('…'));
    }
}
