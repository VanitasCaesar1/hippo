//! A sub-task as its own page:
//! a sub-agent, a background command or a T3 Code delegated task, opened
//! from its parent's Tasks pane. What it was asked, its steps, its report,
//! what it changed, how long it ran and what it cost, each from the rollup
//! and the index; a run still writing reads its own
//! transcript's tail for its newest steps. Reads only.

use std::path::{Path, PathBuf};
use std::time::Instant;

use gpui::{Context, FontWeight, Role, Window, div, prelude::*};
use hippo_core::index::Index;
use hippo_core::rollup::{Changed, Rollup, SessionFacts};
use hippo_view::fleet::SubTask;
use hippo_view::fleet::tail::{self, Turn};
use time::{Duration, OffsetDateTime};

use super::Shell;
use super::kit;
use super::theme::Look;
use crate::state::Tab;

/// What the page says while the rollup does not hold the task yet.
pub const WAITING: &str = "waiting for the index";

/// A sub-task's page, as read.
#[derive(Debug, Clone)]
pub struct TaskPage {
    pub task: SubTask,
    /// The pages above it, its root session first: `(title, tab)`.
    pub crumbs: Vec<(String, Tab)>,
    /// Its own run, when its transcript is known.
    pub run: Option<Run>,
    pub profile: Option<String>,
}

/// A sub-agent's or delegated session's own run.
#[derive(Debug, Clone)]
pub struct Run {
    pub id: String,
    pub facts: Option<SessionFacts>,
    /// Its calls, newest first.
    pub steps: Turn,
    pub changes: Vec<Changed>,
    /// Its own sub-tasks (a delegated session's, or nested ones).
    pub tasks: Vec<SubTask>,
    pub model: Option<String>,
    /// The context its newest reply was given, in tokens.
    pub context: Option<u64>,
    /// Its transcript's newest turn.
    pub last: Option<OffsetDateTime>,
}

impl Run {
    /// Written in the last two minutes.
    pub fn live(&self, now: OffsetDateTime) -> bool {
        self.last
            .is_some_and(|l| now - l <= hippo_view::fleet::LIVE_FOR)
    }
}

/// Reads task `ord` of session `session` from the home at `home`. Blocks;
/// writes nothing.
pub fn read_task(home: &Path, session: &str, ord: usize) -> Result<TaskPage, String> {
    let rollup = Rollup::open(home)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| WAITING.to_owned())?;
    let tasks = rollup.tasks_of(session).map_err(|e| e.to_string())?;
    let t = tasks.get(ord).cloned().ok_or_else(|| WAITING.to_owned())?;
    let index = Index::open(home, false).ok();
    let source = |id: &str| -> Option<(String, PathBuf)> { index.as_ref()?.source(id).ok()? };
    let crumbs = crumbs_of(&rollup, session);
    let side = t.kind == hippo_core::rollup::TaskKind::Agent;
    // A sub-agent the index does not hold yet still has its transcript
    // beside its parent's: `<parent>/subagents/agent-<id>.jsonl`.
    let (child, path) = match (&t.child, &t.id) {
        (Some(c), _) => (Some(c.clone()), source(c).map(|s| s.1)),
        (None, Some(agent)) if side && agent.chars().all(|c| c.is_ascii_alphanumeric()) => {
            let p = source(session).map(|(_, p)| {
                p.with_extension("")
                    .join("subagents")
                    .join(format!("agent-{agent}.jsonl"))
            });
            match p.filter(|p| p.is_file()) {
                Some(p) => (Some(format!("{session}-agent-{agent}")), Some(p)),
                None => (None, None),
            }
        }
        _ => (None, None),
    };
    let run_path = path.clone();
    let run = match (child, path) {
        (Some(id), Some(path)) => Some(run_of(&rollup, index.as_ref(), &id, &path, side)),
        _ => None,
    };
    let profile = run
        .as_ref()
        .and_then(|r| r.facts.as_ref())
        .and_then(|f| f.profile())
        .or_else(|| {
            source(session).and_then(|(tool, p)| hippo_core::rollup::profile_of(&tool, &p))
        });
    let mut task = SubTask::of(&t);
    if let Some(p) = run_path.as_deref() {
        task.settle(p);
    }
    Ok(TaskPage {
        task,
        crumbs,
        run,
        profile,
    })
}

fn run_of(rollup: &Rollup, index: Option<&Index>, id: &str, path: &Path, side: bool) -> Run {
    let facts = rollup.session(id).ok().flatten();
    let stored = index
        .and_then(|i| i.turns_for_session(id).ok())
        .unwrap_or_default();
    let mut steps = tail::turn_of_stored(&stored);
    let tail = if side {
        tail::read_sub_turn(path)
    } else {
        tail::read_turn(path)
    };
    let last = hippo_view::fleet::last_turn_at(path);
    let indexed = facts.as_ref().and_then(|f| f.last_turn);
    // Written since the index read it: the tail has the newest steps.
    let behind = match (last, indexed) {
        (Some(l), Some(i)) => l - i > Duration::seconds(30),
        (Some(_), None) => true,
        _ => false,
    };
    if (steps.calls.is_empty() || behind) && !tail.calls.is_empty() {
        steps = tail.clone();
    }
    steps.asked = None;
    let tool = facts.as_ref().map_or("claude-code", |f| f.tool.as_str());
    let model = if side {
        hippo_core::session_meta::sub_agent_meta(path).model
    } else {
        hippo_core::session_meta::run_facts(tool, path).model
    };
    Run {
        id: id.to_owned(),
        changes: rollup.changes_of(id).unwrap_or_default(),
        tasks: {
            let mut t: Vec<SubTask> = rollup
                .tasks_of(id)
                .unwrap_or_default()
                .iter()
                .map(SubTask::of)
                .collect();
            hippo_view::fleet::settle_all(&mut t, rollup);
            t
        },
        facts,
        steps,
        model,
        context: tail.tokens,
        last,
    }
}

/// The pages above session `session`'s tasks, root first: a sub-agent's
/// or delegated session's own task page, then its parent's, up to the
/// session nobody started.
fn crumbs_of(rollup: &Rollup, session: &str) -> Vec<(String, Tab)> {
    let mut out: Vec<(String, Tab)> = Vec::new();
    let mut cur = session.to_owned();
    for _ in 0..8 {
        let facts = rollup.session(&cur).ok().flatten();
        let up = facts
            .as_ref()
            .and_then(|f| f.parent.clone())
            .or_else(|| rollup.delegated_by(&cur).ok().flatten().map(|(p, _)| p));
        let own_title = || {
            facts
                .as_ref()
                .and_then(|f| f.title.clone())
                .unwrap_or_else(|| crate::rows::id_title(&cur))
        };
        let Some(parent) = up else {
            out.push((own_title(), Tab::Xray(cur.clone())));
            break;
        };
        let theirs = rollup.tasks_of(&parent).unwrap_or_default();
        match theirs
            .iter()
            .position(|t| t.child.as_deref() == Some(cur.as_str()))
        {
            Some(k) => out.push((
                theirs[k].title.clone(),
                Tab::Task {
                    session: parent.clone(),
                    ord: k,
                },
            )),
            None => out.push((own_title(), Tab::Xray(cur.clone()))),
        }
        cur = parent;
    }
    out.reverse();
    out
}

/// A task's state: its glyph, its word and the word's colour.
pub fn state_of(
    t: &SubTask,
    running: bool,
    look: &Look,
) -> (&'static str, &'static str, gpui::Hsla) {
    match (running, t.status.as_deref()) {
        (true, _) => ("circle-dashed", "Running", look.c.info),
        (false, Some("failed")) => ("circle-x", "Failed", look.c.err),
        (false, Some("killed" | "stopped")) => ("circle-pause", "Stopped", look.c.text_muted),
        (false, Some(_)) => ("circle-check", "Completed", look.c.text_muted),
        (false, None) => ("circle-pause", "Stopped", look.c.text_muted),
    }
}

/// `Agent`, `Command`, `Delegated`.
pub fn kind_word(kind: &str) -> &'static str {
    match kind {
        "bash" => "Command",
        "delegate" => "Delegated",
        _ => "Agent",
    }
}

impl Shell {
    /// Reads a task's page on the background executor; again every few
    /// seconds while its run is writing.
    fn load_task(&mut self, session: &str, ord: usize, cx: &mut Context<Self>) {
        let key = (session.to_owned(), ord);
        if self.xray.task_loading.contains(&key) {
            return;
        }
        let now = OffsetDateTime::now_utc();
        let fresh = match self.xray.task_pages.get(&key) {
            None => false,
            Some((Ok(p), at)) => {
                !(p.run.as_ref().is_some_and(|r| r.live(now)) && p.task.ended.is_none())
                    || at.elapsed().as_secs() < 4
            }
            // The rollup will hold it after the next ingest.
            Some((Err(_), at)) => at.elapsed().as_secs() < 20,
        };
        if fresh {
            return;
        }
        let Some(home) = self
            .loaded
            .as_ref()
            .and_then(|l| l.setup.config())
            .map(|c| c.home.clone())
        else {
            return;
        };
        self.xray.task_loading.insert(key.clone());
        cx.spawn(async move |this, cx| {
            let (s, o) = key.clone();
            let read = cx
                .background_executor()
                .spawn(async move { read_task(&home, &s, o) })
                .await;
            this.update(cx, |this, cx| {
                this.xray.task_loading.remove(&key);
                this.xray.task_pages.insert(key, (read, Instant::now()));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Opens task `ord` of session `session` as its own page.
    pub(crate) fn open_task(
        &mut self,
        session: &str,
        ord: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.xray.task_pages.remove(&(session.to_owned(), ord));
        self.xray.show_all.clear();
        self.xray.task_sel = None;
        self.page_scroll.set_offset(gpui::Point::default());
        self.state.tabs.open(Tab::Task {
            session: session.to_owned(),
            ord,
        });
        window.focus(&self.focus, cx);
        self.after_state_change(window, cx);
    }

    /// ↑ ↓ (J K) select a row of the Tasks pane on an x-ray or a sub-task's
    /// page, ↩ opens it. `false` when the key is not one of these or there
    /// is no row.
    pub(crate) fn task_key(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !matches!(
            self.state.tabs.active_tab(),
            Some(Tab::Xray(_) | Tab::Task { .. })
        ) || self.xray.task_order.is_empty()
        {
            return false;
        }
        let n = self.xray.task_order.len();
        let at = self.xray.task_sel.filter(|&k| k < n);
        match key {
            "down" | "j" => self.xray.task_sel = Some(at.map_or(0, |k| (k + 1).min(n - 1))),
            "up" | "k" => self.xray.task_sel = Some(at.map_or(0, |k| k.saturating_sub(1))),
            "enter" => {
                let Some(k) = at else { return false };
                let (s, o) = self.xray.task_order[k].clone();
                self.open_task(&s, o, window, cx);
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    /// The session whose T3 Code delegated task session `id` is, when
    /// today's fleet knows.
    pub(crate) fn delegated_parent(&self, id: &str) -> Option<String> {
        self.loaded
            .as_ref()?
            .fleet
            .as_ref()?
            .row(id)?
            .delegated_by
            .clone()
    }

    /// The page's crumbs above it and its own title, once read.
    pub(crate) fn task_crumbs(
        &self,
        session: &str,
        ord: usize,
    ) -> Option<(Vec<(String, Tab)>, String)> {
        let (Ok(p), _) = self.xray.task_pages.get(&(session.to_owned(), ord))? else {
            return None;
        };
        let mut crumbs = p.crumbs.clone();
        // The root's title as the sidebar shows it.
        if let Some((title, Tab::Xray(id))) = crumbs.first_mut()
            && let Some(t) = self.session_title(id)
        {
            *title = t;
        }
        Some((crumbs, p.task.title.clone()))
    }

    pub(super) fn render_task_tab(
        &mut self,
        session: &str,
        ord: usize,
        look: &Look,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        self.load_task(session, ord, cx);
        let page = match self.xray.task_pages.get(&(session.to_owned(), ord)) {
            Some((Ok(p), _)) => p.clone(),
            Some((Err(e), _)) => {
                let text = if e == WAITING {
                    "This sub-task is newer than the index. It fills in after the next ingest, within ten minutes.".to_owned()
                } else {
                    e.clone()
                };
                return super::xray::quiet_note(look, &text, e != WAITING);
            }
            None => return super::xray::quiet_note(look, "Reading…", false),
        };
        let now = OffsetDateTime::now_utc();
        let width = f32::from(window.viewport_size().width);
        let panes = width >= super::xray::PANES_FROM;
        let verbose = self.xray.verbose;
        let t = &page.task;
        let run = page.run.as_ref();
        let parent_row = self
            .loaded
            .as_ref()
            .and_then(|l| l.fleet.as_ref())
            .and_then(|f| f.row(session).cloned());
        let live = run.is_some_and(|r| r.live(now));
        let running =
            t.ended.is_none() && (live || parent_row.as_ref().is_some_and(|r| t.running(&r.state)));

        let mut centre = div()
            .flex()
            .flex_col()
            .gap(look.px(22.))
            .flex_1()
            .min_w_0()
            .child(task_header(&page, running, now, look));
        let asked = t.input.clone().or_else(|| {
            run.and_then(|r| r.facts.as_ref())
                .and_then(|f| f.asked.clone())
        });
        let report = t.report.clone().or_else(|| {
            (!running)
                .then(|| {
                    run.and_then(|r| r.facts.as_ref())
                        .and_then(|f| f.outcome.clone())
                })
                .flatten()
        });
        centre = centre.child(self.task_thread(t, asked.as_deref(), report.as_deref(), look, cx));
        if let Some(r) = run.filter(|r| !r.steps.calls.is_empty()) {
            centre = centre.child(super::xray_v2::xray_activity(
                &r.steps,
                live && running,
                verbose,
                now,
                look,
                if verbose { usize::MAX } else { 12 },
            ));
        }
        let changes = run.map(|r| r.changes.as_slice()).unwrap_or_default();
        let nested = run.map(|r| r.tasks.as_slice()).unwrap_or_default();
        let mut side: Vec<gpui::AnyElement> = Vec::new();
        if !changes.is_empty() {
            side.push(changes_card(changes, verbose, look).into_any_element());
        }
        self.xray.task_order.clear();
        if let Some(r) = run.filter(|r| !r.tasks.is_empty()) {
            let state = live.then_some(hippo_view::fleet::State::Live);
            let (a, b, _) = super::xray_v2::task_order(nested, state.as_ref(), verbose);
            self.xray.task_order = a.into_iter().chain(b).map(|o| (r.id.clone(), o)).collect();
            side.push(
                self.xray_tasks_pane(&r.id, nested, state.as_ref(), verbose, now, look, cx)
                    .into_any_element(),
            );
        }
        let body = if panes && !side.is_empty() {
            div()
                .flex()
                .items_start()
                .gap(look.px(20.))
                .child(centre)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(look.px(12.))
                        .w(look.px(super::xray_v2::PANE_W))
                        .flex_shrink_0()
                        .children(side),
                )
        } else {
            // Under the pane column's width, the panes follow the steps.
            div()
                .flex()
                .flex_col()
                .gap(look.px(16.))
                .child(centre)
                .children(side)
        };
        div()
            .relative()
            .size_full()
            .child(
                div()
                    .id("task-page")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.page_scroll)
                    .child(
                        div()
                            .w_full()
                            .px(look.px(20.))
                            .pt(look.px(8.))
                            .pb(look.px(24.))
                            .text_size(look.text(13.))
                            .child(body),
                    ),
            )
            .children(super::scrollbar::vertical(&self.page_scroll, look))
            .into_any_element()
    }

    /// What it was asked (the person's side, at the right) and what it
    /// gave back (at the left), each three lines until clicked; a command
    /// shows as its command.
    fn task_thread(
        &self,
        t: &SubTask,
        asked: Option<&str>,
        report: Option<&str>,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let open = |k: &str| self.xray.show_all.contains(k);
        let toggle = |k: &'static str| {
            cx.listener(move |this, _, _, cx| {
                if !this.xray.show_all.remove(k) {
                    this.xray.show_all.insert(k);
                }
                cx.notify();
            })
        };
        let label = |text: &str| {
            div()
                .text_size(look.text(12.))
                .text_color(look.c.faint)
                .child(text.to_owned())
        };
        let command = t.kind == "bash";
        let asked_el = asked.map(|q| {
            let on = open("task-asked");
            let text = div()
                .id("task-asked")
                .role(Role::Button)
                .aria_label(format!(
                    "{}: {q}",
                    if command {
                        "Command"
                    } else {
                        "What it was asked"
                    }
                ))
                .aria_expanded(on)
                .px(look.px(14.))
                .py(look.px(10.))
                .rounded(look.px(if command { 8. } else { 14. }))
                .bg(look.c.raised)
                .cursor_pointer()
                .hover(|h| h.bg(look.c.hover))
                .active(|a| a.bg(look.c.raised))
                .when(command, |d| {
                    d.font_family(kit::MONO)
                        .text_size(look.text(12.))
                        .line_height(look.px(19.))
                })
                .when(!command, |d| {
                    d.text_size(look.text(13.5)).line_height(look.px(21.))
                })
                .text_color(look.c.text)
                .when(!on, |d| d.line_clamp(3).text_ellipsis())
                .child(if on || command {
                    q.trim().to_owned()
                } else {
                    kit::plain(q)
                })
                .tooltip(super::tooltip::Tooltip::text(if on {
                    "Click to fold"
                } else {
                    "Click for all of it"
                }))
                .on_click(toggle("task-asked"));
            if command {
                div()
                    .flex()
                    .flex_col()
                    .gap(look.px(4.))
                    .child(label("Command"))
                    .child(text)
            } else {
                div()
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap(look.px(4.))
                    .child(label("What it was asked"))
                    .child(div().max_w(gpui::relative(0.8)).child(text))
            }
        });
        let body = report.map(kit::report_body).filter(|r| !r.is_empty());
        let report_el = body.as_deref().map(|r| {
            let on = open("task-report");
            div()
                .id("task-report")
                .role(Role::Button)
                .aria_label(format!("Its report: {r}"))
                .aria_expanded(on)
                .flex()
                .flex_col()
                .gap(look.px(4.))
                .cursor_pointer()
                .child(label(match t.kind {
                    "delegate" => "Its report to the thread that delegated it",
                    _ => "Its report to the parent",
                }))
                .child(
                    div()
                        .text_size(look.text(13.5))
                        .line_height(look.px(21.))
                        .text_color(look.c.text)
                        .when(!on, |d| d.line_clamp(3).text_ellipsis())
                        .child(if on {
                            r.trim().to_owned()
                        } else {
                            kit::plain(r)
                        }),
                )
                .tooltip(super::tooltip::Tooltip::text(if on {
                    "Click to fold"
                } else {
                    "Click for all of it"
                }))
                .on_click(toggle("task-report"))
        });
        div()
            .flex()
            .flex_col()
            .gap(look.px(16.))
            .children(asked_el)
            .children(report_el)
    }
}

/// The title once, then the facts: its kind, its state, how long it ran,
/// what it cost and its model; a command's exit code.
fn task_header(page: &TaskPage, running: bool, now: OffsetDateTime, look: &Look) -> gpui::Div {
    let t = &page.task;
    let run = page.run.as_ref();
    let (glyph, word, color) = state_of(t, running, look);
    let mut facts = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_x(look.px(16.))
        .gap_y(look.px(4.))
        .text_size(look.text(12.5))
        .text_color(look.c.text_muted)
        .child(kit::kind_chip(look, kind_word(t.kind)))
        .child(
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(look.px(5.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(color)
                .child(kit::icon(
                    look,
                    glyph,
                    14.,
                    if word == "Completed" {
                        look.c.ok
                    } else {
                        color
                    },
                ))
                .child(word),
        );
    let mut more: Vec<String> = Vec::new();
    let mut add = |text: String| more.push(text);
    if let Some(a) = t.started {
        add(format!("started {}", kit::hm(a)));
    }
    match (t.started, t.ended) {
        (Some(a), Some(b)) => add(format!("ran {}", kit::span(b - a))),
        (Some(a), None) if running => add(format!("running {}", kit::span(now - a))),
        _ => {}
    }
    if let Some(code) = t.exit {
        add(format!("exit code {code}"));
    }
    if let Some(n) = t.tokens {
        add(format!("{} tokens", hippo_view::fleet::compact(n as f64)));
    } else if let Some(n) = run.and_then(|r| r.context).filter(|_| running) {
        add(format!(
            "{} tokens in context",
            hippo_view::fleet::compact(n as f64)
        ));
    }
    let calls = t
        .tools
        .or_else(|| run.map(|r| r.steps.count as u64).filter(|n| *n > 0));
    if let Some(n) = calls {
        add(format!("{n} tool call{}", if n == 1 { "" } else { "s" }));
    }
    if let Some(m) = run.and_then(|r| r.model.clone()) {
        add(m);
    }
    facts = facts.children(
        more.into_iter()
            .map(|text| div().flex_shrink_0().child(text)),
    );
    div()
        .flex()
        .flex_col()
        .gap(look.px(6.))
        .min_w_0()
        .child(
            div()
                .flex()
                .items_center()
                .gap(look.px(10.))
                .min_w_0()
                .child(kit::mark(look, page.profile.as_deref(), 24.))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(look.text(20.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(look.c.text)
                        .child(kit::title(&t.title)),
                ),
        )
        .child(facts)
}

/// Changes: its files with `+a −r`, the totals at the right.
fn changes_card(changes: &[Changed], verbose: bool, look: &Look) -> gpui::Div {
    let (mut a, mut r) = (0u64, 0u64);
    for c in changes {
        a += c.lines.added;
        r += c.lines.removed;
    }
    let mut sorted: Vec<&Changed> = changes.iter().collect();
    sorted.sort_by_key(|c| std::cmp::Reverse(c.lines.added + c.lines.removed));
    let shown = if verbose { sorted.len() } else { 8 };
    let more = sorted.len().saturating_sub(shown);
    kit::card(look)
        .overflow_hidden()
        .child(
            div()
                .flex()
                .items_center()
                .gap(look.px(8.))
                .h(look.px(38.))
                .px(look.px(14.))
                .child(
                    div()
                        .flex_1()
                        .text_size(look.text(12.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Changes"),
                )
                .child(
                    div()
                        .text_size(look.text(12.))
                        .text_color(look.c.faint)
                        .child(format!(
                            "{} file{}",
                            changes.len(),
                            if changes.len() == 1 { "" } else { "s" }
                        )),
                )
                .children(kit::diff_counts(look, a, r)),
        )
        .children(sorted.into_iter().take(shown).enumerate().map(|(i, c)| {
            div()
                .id(("task-file", i))
                .flex()
                .items_center()
                .gap(look.px(8.))
                .h(look.px(24.))
                .px(look.px(14.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(kit::MONO)
                        .text_size(look.text(11.5))
                        .text_color(look.c.text_muted)
                        .child(kit::middle(&kit::short_path(&c.path), 34)),
                )
                .children(if c.uncounted > 0 && c.lines.added + c.lines.removed == 0 {
                    Some(
                        div()
                            .text_size(look.text(11.5))
                            .text_color(look.c.faint)
                            .child("—")
                            .into_any_element(),
                    )
                } else {
                    kit::diff_counts(look, c.lines.added, c.lines.removed)
                        .map(|d| d.into_any_element())
                })
                .tooltip(super::tooltip::Tooltip::text(c.path.clone()))
        }))
        .when(more > 0, |d| {
            d.child(
                div()
                    .px(look.px(14.))
                    .text_size(look.text(12.))
                    .text_color(look.c.faint)
                    .child(format!("{more} more in Verbose")),
            )
        })
        .child(div().h(look.px(8.)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;

    #[test]
    fn a_crumb_goes_back_to_its_page_and_a_new_one_opens_on_top() {
        let mut s = AppState::default();
        s.tabs.open(Tab::Xray("p".into()));
        let task = |ord| Tab::Task {
            session: "p".into(),
            ord,
        };
        s.tabs.open(task(0));
        s.tabs.open(Tab::Task {
            session: "c".into(),
            ord: 1,
        });
        s.tabs.back_to(Tab::Xray("p".into()));
        assert_eq!(s.tabs.active_tab(), Some(&Tab::Xray("p".into())));
        s.tabs.back_to(task(2));
        assert_eq!(s.tabs.active_tab(), Some(&task(2)));
        assert!(s.tabs.back());
        assert_eq!(s.tabs.active_tab(), Some(&Tab::Xray("p".into())));
    }

    #[test]
    fn the_tasks_pane_lists_running_ones_first_then_the_newest_finished() {
        let t = |title: &str, start: OffsetDateTime, end: Option<OffsetDateTime>| SubTask {
            kind: "agent",
            title: title.into(),
            started: Some(start),
            ended: end,
            status: end.map(|_| "completed".into()),
            child: None,
            input: None,
            report: None,
            tokens: None,
            tools: None,
            exit: None,
        };
        let a = OffsetDateTime::now_utc() - Duration::hours(1);
        let tasks = [
            t("old", a, Some(a + Duration::minutes(1))),
            t("open", a, None),
            t("new", a, Some(a + Duration::minutes(9))),
        ];
        let live = hippo_view::fleet::State::Live;
        let (running, done, total) = super::super::xray_v2::task_order(&tasks, Some(&live), false);
        assert_eq!((running, done, total), (vec![1], vec![2, 0], 2));
        assert_eq!(kind_word("bash"), "Command");
        assert_eq!(kind_word("delegate"), "Delegated");
    }
}
