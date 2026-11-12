//! The context x-ray: what
//! context hippocamp's hooks gave a session and what it used, what it searched
//! the notes for, and whether its claims have evidence. The data is
//! `hippocamp_core::xray`, read from the index when the x-ray opens; the tune
//! tip is the fleet's last 7 days, from the rollup. Writes go through the
//! CLI, after the person confirms: `hippocamp xray unrequire --yes` and
//! `hippocamp lessons propose --miss`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use gpui::{Context, Hsla, Window, div, prelude::*};
use hippocamp_core::xray::{self, Claim, Search, Source, Status, Tip, Use, Xray, human_bytes};

use super::Shell;
use super::kit::{self, Kind};
use super::theme::Look;
use crate::flow::Purpose;
use crate::state::Tab;

/// One session's x-ray and what its header shows.
#[derive(Debug, Clone)]
pub struct XrayData {
    pub x: Xray,
    pub title: String,
    /// The resume command, and what ⌘C copies (the command and a handoff).
    pub resume: Option<(String, String)>,
    /// What the panes show, from the rollup and the transcript.
    pub profile: Option<String>,
    pub project: Option<String>,
    /// `T3 Code`, `Codex`, `Terminal`, `Claude Code`.
    pub client: String,
    pub facts: Option<hippocamp_core::rollup::SessionFacts>,
    pub changes: Vec<hippocamp_core::rollup::Changed>,
    pub tasks: Vec<hippocamp_view::fleet::SubTask>,
    /// Each pull request once, merged over opened: `(url, done)`.
    pub pulls: Vec<(String, &'static str)>,
    /// The current turn, as the transcript's tail had it when opened.
    pub turn: hippocamp_view::fleet::tail::Turn,
    /// What the sessions below it did, and each one's title.
    pub sub: hippocamp_view::fleet::SubWork,
    pub sub_titles: HashMap<String, String>,
}

/// The x-ray's state in the shell.
#[derive(Debug, Default)]
pub struct XrayUi {
    pub data: HashMap<String, Result<XrayData, String>>,
    pub loading: HashSet<String>,
    /// The fleet's last 7 days: tips and the claims line.
    pub fleet: Option<Result<xray::Fleet, String>>,
    pub fleet_loading: bool,
    /// The tip whose change is being confirmed, with the line before and
    /// after.
    pub confirm: Option<(Tip, String, String)>,
    pub tune_result: Option<Result<String, String>>,
    pub miss_result: Option<Result<String, String>>,
    /// The claim whose "Ask it to show" request was just copied.
    pub asked: Option<u32>,
    pub copied_resume: bool,
    /// Panels showing every row, not the first six.
    pub show_all: HashSet<&'static str>,
    /// Verbose: every call of the turn, every claim with its
    /// evidence, every task and file; Normal keeps the first few.
    pub verbose: bool,
    /// Sessions too new for the index, since when they wait.
    pub waiting: HashMap<String, std::time::Instant>,
    /// Sub-task pages as read, with when.
    pub task_pages:
        HashMap<(String, usize), (Result<super::subtask::TaskPage, String>, std::time::Instant)>,
    pub task_loading: HashSet<(String, usize)>,
    /// The Tasks row ↑ ↓ selected, by its place in [`XrayUi::task_order`].
    pub task_sel: Option<usize>,
    /// The Tasks pane's rows as drawn: `(session, ord)`.
    pub task_order: Vec<(String, usize)>,
}

/// A calm line in place of a page not read yet.
pub fn quiet_note(look: &Look, text: &str, error: bool) -> gpui::AnyElement {
    div()
        .p(look.px(20.))
        .text_color(if error {
            look.c.error
        } else {
            look.c.text_muted
        })
        .child(text.to_owned())
        .into_any_element()
}

/// From this window width the x-ray shows its pane column.
pub const PANES_FROM: f32 = super::home_panes::PANES_FROM;

/// What an x-ray says while its session is newer than the index.
pub const NOT_YET: &str = "not in the index yet";

/// Rows a panel shows before `show all n`.
const ROWS: usize = 6;

impl Shell {
    /// Reads session `id`'s x-ray (and the fleet's stats once) on the
    /// background executor.
    fn load_xray(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(loaded) = self.loaded.as_ref() else {
            return;
        };
        let Some(cfg) = loaded.setup.config().cloned() else {
            return;
        };
        let ws = loaded.setup.workspace().cloned();
        if !self.xray.fleet_loading && self.xray.fleet.is_none() {
            self.xray.fleet_loading = true;
            let (cfg, ws) = (cfg.clone(), ws.clone());
            cx.spawn(async move |this, cx| {
                let f = cx
                    .background_executor()
                    .spawn(async move {
                        let rollup = hippocamp_core::rollup::Rollup::open(&cfg.home)
                            .map_err(|e| e.to_string())?
                            .ok_or_else(|| "no rollup yet".to_owned())?;
                        xray::fleet(ws.as_ref(), &rollup, 7, time::OffsetDateTime::now_utc())
                            .map_err(|e| e.to_string())
                    })
                    .await;
                this.update(cx, |this, cx| {
                    this.xray.fleet = Some(f);
                    this.xray.fleet_loading = false;
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        if self.xray.data.contains_key(id) || self.xray.loading.contains(id) {
            return;
        }
        self.xray.loading.insert(id.to_owned());
        let id = id.to_owned();
        let fallback_title = self.session_title(&id);
        cx.spawn(async move |this, cx| {
            let key = id.clone();
            let read = cx
                .background_executor()
                .spawn(async move { read_xray(&cfg, ws.as_ref(), &id, fallback_title) })
                .await;
            this.update(cx, |this, cx| {
                this.xray.loading.remove(&key);
                this.xray.data.insert(key, read);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Opens the x-ray of session `id` in the current section.
    pub(crate) fn open_xray(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.mark_opened(id);
        self.sidebar.overlay = false;
        // Read it afresh each time it opens: the index may have moved on.
        self.xray.data.remove(id);
        self.xray.miss_result = None;
        self.xray.tune_result = None;
        self.xray.asked = None;
        self.xray.copied_resume = false;
        self.xray.show_all.clear();
        self.page_scroll.set_offset(gpui::Point::default());
        self.state.tabs.open(Tab::Xray(id.to_owned()));
        // Fleet's rows leave the screen with their focus: keys go to the
        // window.
        window.focus(&self.focus, cx);
        self.after_state_change(window, cx);
    }

    /// The confirmation of a tune tip: the `required_notes` line before
    /// and after, read from `.hippocamp/hippocamp.toml` now.
    pub(super) fn ask_tune(&mut self, tip: Tip, cx: &mut Context<Self>) {
        let Some(ws) = self.loaded.as_ref().and_then(|l| l.setup.workspace()) else {
            return;
        };
        let path = ws.hippocamp_dir().join("hippocamp.toml");
        let plan = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|text| xray::unrequire(&text, &tip.note));
        match plan {
            Ok(u) => self.xray.confirm = Some((tip, u.before, u.after)),
            Err(e) => self.xray.tune_result = Some(Err(e)),
        }
        cx.notify();
    }

    /// ⌘C on an x-ray: the session's resume command and handoff.
    pub(crate) fn copy_xray_resume(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        let Some(Ok(d)) = self.xray.data.get(id) else {
            return false;
        };
        let Some((_, clip)) = d.resume.clone() else {
            return false;
        };
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(clip));
        self.xray.copied_resume = true;
        cx.notify();
        true
    }

    /// `show all n` under a capped panel.
    pub(super) fn show_all_row(
        &self,
        panel: &'static str,
        total: usize,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if total <= ROWS || self.xray.show_all.contains(panel) {
            return None;
        }
        Some(
            div()
                .id(gpui::SharedString::from(format!("show-all-{panel}")))
                .role(gpui::Role::Button)
                .pt(look.px(8.))
                .text_size(look.text(12.))
                .text_color(look.c.text)
                .cursor_pointer()
                .hover(|h| h.underline())
                .child(format!("show all {total}"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.xray.show_all.insert(panel);
                    cx.notify();
                }))
                .into_any_element(),
        )
    }

    pub(super) fn render_xray_tab(
        &mut self,
        id: &str,
        look: &Look,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        self.load_xray(id, cx);
        let data = match self.xray.data.get(id) {
            Some(Ok(d)) => d.clone(),
            Some(Err(e)) if e == NOT_YET => {
                // Read again every 20 seconds until the index holds it:
                // one timer per wait, then a fresh read.
                let first = !self.xray.waiting.contains_key(id);
                let since = *self
                    .xray
                    .waiting
                    .entry(id.to_owned())
                    .or_insert_with(std::time::Instant::now);
                if since.elapsed().as_secs() >= 20 {
                    self.xray.waiting.remove(id);
                    self.xray.data.remove(id);
                    cx.notify();
                } else if first {
                    cx.spawn(async move |this, cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_secs(21))
                            .await;
                        this.update(cx, |_, cx| cx.notify()).ok();
                    })
                    .detach();
                }
                return quiet_note(
                    look,
                    "This session started after the last ingest. Its x-ray fills in after the next one, within ten minutes.",
                    false,
                );
            }
            other => {
                let (text, error) = match other {
                    Some(Err(e)) => (e.clone(), true),
                    _ => ("Reading the transcript…".to_owned(), false),
                };
                return div()
                    .p(look.px(20.))
                    .text_color(if error {
                        look.c.error
                    } else {
                        look.c.text_muted
                    })
                    .child(text)
                    .into_any_element();
            }
        };
        self.render_xray_v2(id, &data, look, window, cx)
    }

    pub(super) fn render_injected(
        &self,
        x: &Xray,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        // The legend lives in the `ⓘ` only.
        let head = card_head(look, "Injected at start", "").child(kit::info(
            look,
            "xray-info",
            "read = opened or quoted in a tool call · cited = named or quoted in a reply. Both come from the transcript; what the model attended to inside its context cannot be known.",
        ));
        if x.block == Source::NotRecorded {
            return kit::card(look)
                .px(look.px(16.))
                .py(look.px(14.))
                .child(head)
                .child(
                    div()
                        .pt(look.px(8.))
                        .text_color(look.c.text_muted)
                        .child("Not recorded: this session's context block was not kept (a Codex session, a session started outside the workspace's hooks, or one from before its client kept hook output). Nothing is reconstructed."),
                )
                .into_any_element();
        }
        let cap = if self.xray.show_all.contains("parts") {
            usize::MAX
        } else {
            ROWS
        };
        let rows = x.parts.iter().take(cap).enumerate().map(|(i, p)| {
            let (text, color) = match p.used {
                Use::Read => (
                    format!("read · turn {}", p.read_at.unwrap_or(0)),
                    kit::ok(look),
                ),
                Use::Cited => (
                    match p.cited {
                        0 | 1 => format!("cited · turn {}", p.cited_at.unwrap_or(0)),
                        2 => "cited twice".to_owned(),
                        n => format!("cited {n} times"),
                    },
                    // Cited is the info colour, not a warning.
                    look.c.focus,
                ),
                Use::Ignored if p.note.is_some() => ("never opened".to_owned(), kit::dim(look)),
                Use::Ignored => ("ignored".to_owned(), kit::dim(look)),
            };
            let size = match p.note_bytes {
                Some(n) => format!(
                    "{} · note {}",
                    human_bytes(p.bytes),
                    human_bytes(n as usize)
                ),
                None => human_bytes(p.bytes),
            };
            let note = p.note.clone();
            let ignored = p.used == Use::Ignored;
            div()
                .id(("xray-part", i))
                .flex()
                .items_center()
                .gap(look.px(12.))
                .py(look.px(8.))
                .border_b(look.fixed(1.))
                .border_color(look.c.border_variant.opacity(0.6))
                .when(note.is_some(), |d| {
                    d.cursor_pointer().hover(|h| h.bg(look.c.hover))
                })
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .min_w_0()
                        .gap(look.px(8.))
                        .items_baseline()
                        .child(
                            div()
                                .truncate()
                                .min_w_0()
                                .text_color(if ignored {
                                    look.c.text_muted
                                } else {
                                    look.c.text
                                })
                                .child(p.label.clone()),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(look.text(12.))
                                .text_color(kit::dim(look))
                                .child(p.kind.as_str()),
                        ),
                )
                // Sizes right-aligned in their own column.
                .child(
                    div()
                        .flex_shrink_0()
                        .min_w(look.px(118.))
                        .flex()
                        .justify_end()
                        .font_family("Menlo")
                        .text_size(look.text(11.5))
                        .text_color(look.c.text_muted)
                        .child(size),
                )
                .child(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .w(look.px(150.))
                        .items_center()
                        .gap(look.px(6.))
                        .child(kit::dot(look, color))
                        .child(div().truncate().text_color(look.c.text_muted).child(text)),
                )
                .when_some(note, |d, note| {
                    d.on_click(cx.listener(move |this, _, window, cx| {
                        this.state.tabs.open(Tab::Note(note.clone()));
                        this.after_state_change(window, cx);
                    }))
                })
        });
        kit::card(look)
            .px(look.px(16.))
            .py(look.px(14.))
            .child(head)
            .children(rows)
            .children(self.show_all_row("parts", x.parts.len(), look, cx))
            .children(self.render_tune(x, look, cx))
            .when(x.parts.is_empty(), |d| {
                d.child(
                    div()
                        .pt(look.px(8.))
                        .text_color(look.c.text_muted)
                        .child("The block was empty."),
                )
            })
            .into_any_element()
    }

    /// The tune tip for a note this session was given, else the fleet's
    /// first.
    fn render_tune(
        &self,
        x: &Xray,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let fleet = self.xray.fleet.as_ref()?.as_ref().ok()?;
        // Notes this session was given and ignored; a note it used is no
        // case for dropping it here.
        let here: Vec<&PathBuf> = x
            .parts
            .iter()
            .filter(|p| p.used == Use::Ignored)
            .filter_map(|p| p.note.as_ref())
            .collect();
        let used_here: Vec<&PathBuf> = x
            .parts
            .iter()
            .filter(|p| p.used != Use::Ignored)
            .filter_map(|p| p.note.as_ref())
            .collect();
        // A note made relevant-only since the stats were read has no tip.
        let required: Vec<PathBuf> = self
            .loaded
            .as_ref()
            .and_then(|l| l.setup.workspace())
            .map(|ws| {
                ws.required_notes
                    .iter()
                    .map(|p| hippocamp_core::notes::normalise(p))
                    .collect()
            })
            .unwrap_or_default();
        let tips: Vec<&Tip> = fleet
            .tips
            .iter()
            .filter(|t| t.action.is_none() || required.contains(&t.note))
            .filter(|t| !used_here.contains(&&t.note))
            .collect();
        // Only a tip for a note this session ignored, as one muted
        // line under the list; the fleet-wide tip lives in Memory.
        let tip = tips
            .iter()
            .copied()
            .find(|t| here.contains(&&t.note))?
            .clone();
        let mut body = div().flex().flex_col().gap(look.px(8.)).child(
            div()
                .id("xray-tune-line")
                .flex()
                .flex_wrap()
                .items_center()
                .gap(look.px(6.))
                .text_size(look.text(12.))
                .text_color(kit::dim(look))
                .child(kit::icon(look, "bulb", 13., kit::dim(look)))
                .child(format!(
                    "{} is ignored in {} of {} sessions",
                    tip.label, tip.ignored, tip.sessions
                ))
                .child("·")
                .child(
                    kit::link(look, "xray-tune-memory", "tune in Memory ›")
                        .text_color(look.c.text_muted)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.show_section(crate::state::Section::Memory, window, cx);
                        })),
                ),
        );
        match (&self.xray.confirm, tip.action.as_deref()) {
            (Some((t, before, after)), _) if t.note == tip.note => {
                let note = t.note.clone();
                body = body
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(look.px(2.))
                            .p(look.px(8.))
                            .rounded(look.px(6.))
                            .bg(look.c.window.opacity(0.5))
                            .font_family("Menlo")
                            .text_size(look.text(11.5))
                            .child(
                                div()
                                    .text_color(look.c.deleted)
                                    .child(format!("- {before}")),
                            )
                            .child(div().text_color(look.c.created).child(format!("+ {after}"))),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(look.px(8.))
                            .child(
                                kit::button(look, "tune-yes", "Make relevant-only", Kind::Primary)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let args = vec![
                                            "xray".to_owned(),
                                            "unrequire".to_owned(),
                                            note.display().to_string(),
                                            "--yes".to_owned(),
                                        ];
                                        this.flow.log.clear();
                                        this.start_job(Purpose::Tune, vec![args], cx);
                                    })),
                            )
                            .child(
                                kit::button(look, "tune-no", "Cancel", Kind::Plain).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.xray.confirm = None;
                                        cx.notify();
                                    }),
                                ),
                            ),
                    );
            }
            _ => {}
        }
        if let Some(r) = &self.xray.tune_result {
            let (text, err) = match r {
                Ok(t) => (t.clone(), false),
                Err(e) => (e.clone(), true),
            };
            body = body.child(
                div()
                    .text_size(look.text(12.))
                    .text_color(if err { look.c.error } else { kit::dim(look) })
                    .child(text),
            );
        }
        Some(div().pt(look.px(10.)).child(body).into_any_element())
    }

    pub(super) fn render_searches(
        &self,
        id: &str,
        searches: &[Search],
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let head = card_head(
            look,
            "What it searched for",
            "hippocamp recall, grep and Glob over the notes",
        );
        // `used n` shows only when some search in the session has a known,
        // non-zero count: never a column of zeros.
        let show_used = searches.iter().any(|s| s.used.is_some_and(|u| u > 0));
        // Only searches with a known result; identical queries merged
        //The rest fold into one line.
        let (known, folded) = merge_searches(searches);
        let total = known.len();
        let cap = if self.xray.show_all.contains("searches") {
            usize::MAX
        } else {
            ROWS
        };
        let rows = known
            .into_iter()
            .take(cap)
            .enumerate()
            .map(move |(i, (s, n))| {
                let miss = s.is_miss();
                let right = match s.hits {
                    Some(0) => match &s.follow {
                        Some(f) => format!("0 hits · {f}"),
                        None => "0 hits".to_owned(),
                    },
                    Some(n) => match s.used {
                        Some(u) if show_used => format!("{n} hits · opened {u}"),
                        _ => format!("{n} hits"),
                    },
                    None => "hits mixed with other output".to_owned(),
                };
                div()
                    .id(("xray-search", i))
                    .flex()
                    .items_center()
                    .gap(look.px(10.))
                    .py(look.px(8.))
                    .border_b(look.fixed(1.))
                    .border_color(look.c.border_variant.opacity(0.6))
                    .child(kit::icon(look, "search", 13., kit::dim(look)).flex_shrink_0())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family("Menlo")
                            .text_size(look.text(12.))
                            .text_color(look.c.text)
                            .child(if n > 1 {
                                format!("{} ×{n}", s.query)
                            } else {
                                s.query.clone()
                            }),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .max_w(look.px(260.))
                            .truncate()
                            .text_size(look.text(12.))
                            .text_color(if miss {
                                look.c.error
                            } else {
                                look.c.text_muted
                            })
                            .child(right),
                    )
            });
        let misses: Vec<&Search> = searches.iter().filter(|s| s.is_miss()).collect();
        let found = misses.iter().find(|s| {
            s.follow
                .as_deref()
                .is_some_and(|f| f.starts_with("found") || f.starts_with("then asked"))
        });
        let footer = (!misses.is_empty()).then(|| {
            let n = misses.len();
            let mut row = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(look.px(10.))
                .pt(look.px(10.))
                .text_size(look.text(12.5))
                .text_color(look.c.text_muted)
                .child(match found {
                    Some(s) => format!(
                        "{n} miss{} → propose a note on where \"{}\" lives",
                        if n == 1 { "" } else { "es" },
                        short_command(&s.query, 40)
                    ),
                    None => format!(
                        "{n} miss{}; nothing after shows where the answer was",
                        if n == 1 { "" } else { "es" }
                    ),
                });
            if let Some(s) = found {
                let arg = format!("{id}:{}", s.ordinal);
                let busy = self.lesson_job_running();
                row = row.child(
                    kit::button(
                        look,
                        "xray-miss",
                        "Add to inbox",
                        if busy { Kind::Disabled } else { Kind::Plain },
                    )
                    .tooltip(super::tooltip::Tooltip::text(
                        "Drafts a rule with the lessons pass, checked by its fresh reviewer; it waits in the Inbox for Keep.",
                    ))
                    .when(!busy, |b| {
                        b.on_click(cx.listener(move |this, _, _, cx| {
                            let args = vec![
                                "lessons".to_owned(),
                                "propose".to_owned(),
                                "--miss".to_owned(),
                                arg.clone(),
                            ];
                            this.xray.miss_result = None;
                            this.flow.log.clear();
                            this.start_job(Purpose::Miss, vec![args], cx);
                        }))
                    }),
                );
            }
            row
        });
        let result = self.xray.miss_result.as_ref().map(|r| {
            let (text, err) = match r {
                Ok(t) => (t.clone(), false),
                Err(e) => (e.clone(), true),
            };
            div()
                .pt(look.px(6.))
                .text_size(look.text(12.))
                .text_color(if err { look.c.error } else { kit::dim(look) })
                .child(text)
        });
        let drafting = self.flow.job.as_ref().is_some_and(|j| {
            j.purpose == Purpose::Miss && j.result.lock().map(|r| r.is_none()).unwrap_or(false)
        });
        kit::card(look)
            .px(look.px(16.))
            .py(look.px(14.))
            .child(head)
            .children(rows)
            .children(self.show_all_row("searches", total, look, cx))
            .when(folded > 0, |d| {
                d.child(
                    div()
                        .pt(look.px(8.))
                        .text_size(look.text(12.))
                        .text_color(kit::dim(look))
                        .child(format!(
                            "{folded} {} inside compound commands; {} hits cannot be read",
                            if folded == 1 {
                                "more search ran"
                            } else {
                                "more searches ran"
                            },
                            if folded == 1 { "its" } else { "their" }
                        )),
                )
            })
            .when(searches.is_empty(), |d| {
                d.child(
                    div()
                        .pt(look.px(8.))
                        .text_color(look.c.text_muted)
                        .child("No search of the notes in this session."),
                )
            })
            .children(footer)
            .when(drafting, |d| {
                d.child(
                    div()
                        .pt(look.px(6.))
                        .text_size(look.text(12.))
                        .text_color(kit::dim(look))
                        .child("drafting…"),
                )
            })
            .children(result)
            .into_any_element()
    }

    pub(super) fn render_claims(
        &self,
        claims: &[Claim],
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let count = |s: Status| claims.iter().filter(|c| c.status == s).count();
        let (v, u, q) = (
            count(Status::Verified),
            count(Status::Unverified),
            count(Status::Unclear),
        );
        let head = div()
            .flex()
            .items_center()
            .gap(look.px(8.))
            .pb(look.px(4.))
            .child(kit::label(look, "Claims vs evidence", look.c.text_muted))
            .when(v > 0, |d| {
                d.child(kit::pill(look, format!("{v} verified"), kit::ok(look)))
            })
            .when(u > 0, |d| {
                d.child(kit::pill(look, format!("{u} unverified"), look.c.warning))
            })
            .when(q > 0, |d| {
                d.child(kit::pill(look, format!("{q} unclear"), look.c.text_muted))
            });
        let cap = if self.xray.show_all.contains("claims") {
            usize::MAX
        } else {
            ROWS
        };
        let rows = claims.iter().take(cap).enumerate().map(|(i, c)| {
            let color = match c.status {
                Status::Verified => kit::ok(look),
                Status::Unverified => look.c.warning,
                Status::Unclear => kit::dim(look),
            };
            let right: gpui::AnyElement = match c.status {
                Status::Unverified => {
                    let ord = c.ordinal;
                    let request = format!(
                        "Show me the output of the command that shows this: \"{}\". If you did not run it after your last edit, run it now and show its result.",
                        c.claim
                    );
                    let copied = self.xray.asked == Some(ord);
                    kit::button(
                        look,
                        ("claim-ask", i),
                        if copied { "Copied" } else { "Ask it to show" },
                        Kind::Plain,
                    )
                    .tooltip(super::tooltip::Tooltip::text(
                        "Copies a request to paste into the session. hippocamp never writes into an agent's session.",
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(request.clone()));
                        this.xray.asked = Some(ord);
                        cx.notify();
                    }))
                    .into_any_element()
                }
                s => div()
                    .flex_shrink_0()
                    .text_size(look.text(12.))
                    .text_color(kit::dim(look))
                    .child(s.as_str())
                    .into_any_element(),
            };
            div()
                .id(("xray-claim", i))
                .flex()
                .items_center()
                .gap(look.px(10.))
                .py(look.px(8.))
                .border_b(look.fixed(1.))
                .border_color(look.c.border_variant.opacity(0.6))
                .child(kit::dot(look, color).flex_shrink_0())
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(look.px(2.))
                        .child(
                            div()
                                .italic()
                                .text_color(if c.status == Status::Unclear {
                                    look.c.text_muted
                                } else {
                                    look.c.text
                                })
                                .child(format!("\u{201c}{}\u{201d}", plain_claim(&c.claim))),
                        )
                        .child(
                            div()
                                .text_size(look.text(11.5))
                                .text_color(look.c.text_muted)
                                .child(c.evidence.clone()),
                        ),
                )
                .child(right)
        });
        let week = self
            .xray
            .fleet
            .as_ref()
            .and_then(|f| f.as_ref().ok())
            .filter(|f| f.claims.claims > 0)
            .map(|f| {
                format!(
                    "This week: {} · unverified claims show on the Fleet row",
                    f.claims.line()
                )
            });
        kit::card(look)
            .px(look.px(16.))
            .py(look.px(14.))
            .child(head)
            .children(rows)
            .children(self.show_all_row("claims", claims.len(), look, cx))
            .when(claims.is_empty(), |d| {
                d.child(div().pt(look.px(8.)).text_color(look.c.text_muted).child(
                    "No claims of tests, builds, merges, pushes or deploys in the agent's replies.",
                ))
            })
            .children(week.map(|w| {
                div()
                    .pt(look.px(10.))
                    .text_size(look.text(12.))
                    .text_color(kit::dim(look))
                    .child(w)
            }))
            .into_any_element()
    }
}

/// A quoted claim without Markdown marks: `**Checks:** clean`
/// reads `Checks: clean`.
pub(super) fn plain_claim(text: &str) -> String {
    text.replace("**", "")
        .replace("__", "")
        .replace('`', "")
        .trim()
        .to_owned()
}

/// A card's heading: a small capital label and a muted gloss.
fn card_head(look: &Look, label: &str, gloss: &str) -> gpui::Div {
    div()
        .flex()
        .items_baseline()
        .gap(look.px(10.))
        .pb(look.px(4.))
        .child(kit::label(look, label, look.c.text_muted).flex_shrink_0())
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(look.text(12.))
                .text_color(kit::dim(look))
                .child(gloss.to_owned()),
        )
}

/// The context-use bar: read, cited and ignored bytes of the block.
pub(super) fn use_bar(x: &Xray, look: &Look) -> gpui::AnyElement {
    let t = x.totals;
    let row = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(look.px(14.))
        .child(kit::label(look, "Context use", look.c.text_muted).flex_shrink_0());
    if x.block == Source::NotRecorded || t.total == 0 {
        return row
            .child(
                div()
                    .text_color(look.c.text_muted)
                    .child("not recorded for this session"),
            )
            .into_any_element();
    }
    let frac = |n: usize| n as f32 / t.total as f32;
    let seg = |n: usize, c: Hsla| {
        div()
            .h_full()
            .flex_basis(gpui::relative(frac(n)))
            .flex_shrink_0()
            .bg(c)
    };
    let bar = div()
        .flex()
        .flex_1()
        .min_w(look.px(160.))
        .h(look.px(8.))
        .rounded(look.px(4.))
        .overflow_hidden()
        .bg(look.c.border_variant)
        .child(seg(t.read, kit::ok(look)))
        .child(seg(t.cited, look.c.focus));
    let legend = |c: Hsla, text: String| {
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(look.px(6.))
            .child(kit::dot(look, c))
            .child(text)
    };
    row.child(bar)
        .child(
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(look.px(12.))
                .text_color(look.c.text_muted)
                .child(legend(
                    kit::ok(look),
                    format!("read {}", human_bytes(t.read)),
                ))
                .child(legend(
                    look.c.focus,
                    format!("cited {}", human_bytes(t.cited)),
                ))
                .child(legend(
                    kit::dim(look),
                    format!("ignored {}", human_bytes(t.ignored)),
                ))
                .child(
                    div()
                        .text_color(kit::dim(look))
                        .child(format!("of {} injected", human_bytes(t.total))),
                ),
        )
        .into_any_element()
}

/// The searches with a known result (hits or a miss), identical queries
/// merged with their count, newest result kept; and how many had no known
/// result (they ran inside compound commands).
fn merge_searches(searches: &[Search]) -> (Vec<(&Search, usize)>, usize) {
    let mut out: Vec<(&Search, usize)> = Vec::new();
    let mut folded = 0;
    for s in searches {
        if s.hits.is_none() {
            folded += 1;
            continue;
        }
        match out.iter_mut().find(|(o, _)| o.query == s.query) {
            Some(slot) => {
                slot.1 += 1;
                // A later hit beats an earlier miss.
                if slot.0.is_miss() && !s.is_miss() {
                    slot.0 = s;
                }
            }
            None => out.push((s, 1)),
        }
    }
    (out, folded)
}

/// `Mon 03:40 → Tue 03:19 · 23 h 39 m`; the day after the arrow only when
/// it differs.
pub(super) fn span_words(
    a: time::OffsetDateTime,
    b: time::OffsetDateTime,
    offset: time::UtcOffset,
) -> String {
    let (la, lb) = (a.to_offset(offset), b.to_offset(offset));
    let day = |t: time::OffsetDateTime| {
        let d = crate::state::edition_title(t.date());
        d[..3.min(d.len())].to_owned()
    };
    let hm = |t: time::OffsetDateTime| format!("{:02}:{:02}", t.hour(), t.minute());
    let end = if la.date() == lb.date() {
        hm(lb)
    } else {
        format!("{} {}", day(lb), hm(lb))
    };
    // The app's one duration form: `33m`, `1h 4m`, `2d 3h`.
    let dur = hippocamp_view::fleet::span((b - a).whole_seconds());
    format!("{} {} → {end} · {dur}", day(la), hm(la))
}

fn short_command(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut s: String = text.chars().take(max - 1).collect();
    s.push('…');
    s
}

/// Reads a session's x-ray and its header. Blocks; writes nothing.
fn read_xray(
    cfg: &hippocamp_core::config::Config,
    ws: Option<&hippocamp_core::config::Workspace>,
    id: &str,
    fallback_title: Option<String>,
) -> Result<XrayData, String> {
    use hippocamp_core::index::Index;
    let index = Index::open(&cfg.home, false).map_err(|e| match e {
        hippocamp_core::error::Error::MissingIndex => crate::rows::NO_INDEX.to_owned(),
        e => e.to_string(),
    })?;
    // A session newer than the index waits for the next ingest.
    let x = xray::of(ws, &index, id).map_err(|e| match e {
        hippocamp_core::error::Error::InvalidSessionId(_) => NOT_YET.to_owned(),
        e => e.to_string(),
    })?;
    let rollup = hippocamp_core::rollup::Rollup::open(&cfg.home)
        .ok()
        .flatten();
    let facts = rollup.as_ref().and_then(|r| r.session(id).ok().flatten());
    let (tool, path) = index
        .source(id)
        .ok()
        .flatten()
        .unwrap_or_else(|| (x.tool.clone(), PathBuf::new()));
    let origin = hippocamp_core::session_meta::origin(id, &tool, &path);
    let title = fallback_title
        .or_else(|| facts.as_ref().and_then(|f| f.title.clone()))
        .or_else(|| origin.named.clone())
        .unwrap_or_else(|| crate::rows::id_title(id));
    let target = hippocamp_core::resume::Target {
        id,
        tool: &tool,
        transcript: &path,
        cwd: origin.cwd.as_deref(),
    };
    let resume = hippocamp_core::resume::command(&target).map(|command| {
        let turns = index.turns_for_session(id).unwrap_or_default();
        let open = hippocamp_core::resume::open_items(&turns);
        let handoff = hippocamp_core::resume::handoff(
            &title,
            facts.as_ref().and_then(|f| f.asked.as_deref()),
            facts.as_ref().and_then(|f| f.outcome.as_deref()),
            &open,
        );
        let clip = format!("{command}\n\n{handoff}");
        (command, clip)
    });
    let mut pulls: Vec<(String, &'static str)> = Vec::new();
    for p in rollup
        .as_ref()
        .and_then(|r| r.pulls_of(id).ok())
        .unwrap_or_default()
    {
        match pulls.iter_mut().find(|(u, _)| *u == p.url) {
            Some(slot) if p.done.as_str() == "merged" => slot.1 = "merged",
            Some(_) => {}
            None => pulls.push((p.url.clone(), p.done.as_str())),
        }
    }
    let mut changes = rollup
        .as_ref()
        .and_then(|r| r.changes_of(id).ok())
        .unwrap_or_default();
    changes.sort_by_key(|c| std::cmp::Reverse(c.lines.added + c.lines.removed));
    let mut tasks: Vec<hippocamp_view::fleet::SubTask> = rollup
        .as_ref()
        .and_then(|r| r.tasks_of(id).ok())
        .unwrap_or_default()
        .iter()
        .map(hippocamp_view::fleet::SubTask::of)
        .collect();
    if let Some(r) = rollup.as_ref() {
        hippocamp_view::fleet::settle_all(&mut tasks, r);
    }
    let sub = rollup
        .as_ref()
        .map(|r| hippocamp_view::fleet::SubWork::of(r, id, &tasks))
        .unwrap_or_default();
    let sub_titles = rollup
        .as_ref()
        .map(|r| sub_titles(r, id))
        .unwrap_or_default();
    let client = match (origin.client(), tool.as_str()) {
        (Some(c), _) if c.eq_ignore_ascii_case("terminal") => "Terminal".to_owned(),
        (Some(c), _) => c.to_owned(),
        (None, "codex") => "Codex".to_owned(),
        (None, _) => "Claude Code".to_owned(),
    };
    let turn = if path.as_os_str().is_empty() {
        Default::default()
    } else {
        hippocamp_view::fleet::tail::read_turn(&path)
    };
    Ok(XrayData {
        x,
        title,
        resume,
        profile: hippocamp_core::rollup::profile_of(&tool, &path),
        project: facts
            .as_ref()
            .and_then(|f| f.project.clone())
            .or_else(|| origin.project()),
        client,
        facts,
        changes,
        tasks,
        pulls,
        turn,
        sub,
        sub_titles,
    })
}

/// The title of each session below `id`: the task that started it, else
/// its own.
fn sub_titles(rollup: &hippocamp_core::rollup::Rollup, id: &str) -> HashMap<String, String> {
    let below = rollup.descendants_of(id).unwrap_or_default();
    let mut out: HashMap<String, String> = HashMap::new();
    for s in std::iter::once(id.to_owned()).chain(below.iter().cloned()) {
        for t in rollup.tasks_of(&s).unwrap_or_default() {
            if let Some(c) = t.child {
                out.entry(c).or_insert(t.title);
            }
        }
    }
    for d in below {
        if let std::collections::hash_map::Entry::Vacant(e) = out.entry(d.clone()) {
            e.insert(
                rollup
                    .session(&d)
                    .ok()
                    .flatten()
                    .and_then(|f| f.title)
                    .unwrap_or_else(|| crate::rows::id_title(&d)),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_span_names_the_second_day_only_when_it_differs() {
        use time::macros::{datetime, offset};
        let off = offset!(+9:30);
        assert_eq!(
            span_words(
                datetime!(2026-11-08 18:10 UTC),
                datetime!(2026-11-09 17:49 UTC),
                off
            ),
            "Mon 03:40 → Tue 03:19 · 23h 39m"
        );
        assert_eq!(
            span_words(
                datetime!(2026-11-09 00:00 UTC),
                datetime!(2026-11-09 00:45 UTC),
                off
            ),
            "Mon 09:30 → 10:15 · 45m"
        );
    }

    #[test]
    fn a_long_command_is_cut_with_an_ellipsis() {
        assert_eq!(
            short_command("claude --resume 4c1e0000", 12),
            "claude --re…"
        );
    }
}
