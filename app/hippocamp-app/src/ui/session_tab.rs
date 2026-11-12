//! A session's tab, as the conversation it was: a header that says what was
//! asked and how it ended, then the dialogue. Requests and replies render
//! their Markdown; each run of tool calls between two messages is one quiet
//! line that opens to its steps, the harness's own steps hidden until asked
//! for; handoffs and summaries are folded. Only the rows in view are laid
//! out.
//! opened from Fleet rows and Ask sources.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::{
    ClipboardItem, Context, FontWeight, ListAlignment, ListState, Role, SharedString, Window, div,
    list, prelude::*, px,
};
use hippocamp_core::index::Index;
use hippocamp_core::sessions::{Injection, RunFacts};
use hippocamp_view::changes::{self, Changes, DiffLine, FileChange, LineKind};
use hippocamp_view::conversation::{self, Digest, Entry, Item, Kind, Steps};
use hippocamp_view::markdown;
use hippocamp_view::sessions;
use hippocamp_view::text::visible;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use time::macros::format_description;

use super::Shell;
use super::markdown::{code_block, markdown_element};
use super::status::local_offset;
use super::theme::Look;
use crate::rows;
use crate::state::Tab;

/// A long request, reply or tool output shows this many lines, or
/// characters, until opened.
pub const CLIP_LINES: usize = 40;
pub const CLIP_CHARS: usize = 4000;
/// The header's request and outcome show this much until "more".
const ASKED_LINES: usize = 4;
const ASKED_CHARS: usize = 480;
const OUTCOME_LINES: usize = 8;
const OUTCOME_CHARS: usize = 900;
/// The most sub-agents the header names.
const HEADER_AGENTS: usize = 12;
/// How many sub-agents the header names before "all agents".
const HEADER_AGENTS_FOLDED: usize = 3;
/// The most pull requests the header's facts line names.
const HEADER_PULLS: usize = 4;
/// An inline diff shows this many lines until opened.
const INLINE_DIFF_LINES: usize = 12;
/// A hunk in the Changes pane shows this many lines until "show all".
const PANE_HUNK_LINES: usize = 80;
/// The avatar column left of a message.
const GUTTER: f32 = 28.;
/// The Changes pane's width.
const PANE_WIDTH: f32 = 460.;

/// One row of a session's list after the header: a message, a group of
/// steps, one step of an opened group, or an opened group's line that shows
/// or hides its harness steps. A row names its item and entry, so it stays
/// the same row while groups open and close.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Row {
    Message { item: usize },
    Edit { item: usize },
    Group { item: usize },
    Step { item: usize, entry: usize },
    Harness { item: usize },
}

/// A session's entries, its dialogue, what happened, what it changed, and
/// the lists that draw them (list item 0 is the header, item `r + 1` is
/// `rows[r]`; the Changes pane's list draws `pane`).
#[derive(Clone)]
pub struct SessionView {
    pub entries: Rc<Vec<Entry>>,
    pub items: Rc<Vec<Item>>,
    pub digest: Rc<Digest>,
    pub changes: Rc<Changes>,
    /// Branch, model and effort, from the transcript.
    pub run: Rc<RunFacts>,
    pub rows: Rc<Vec<Row>>,
    pub list: ListState,
    pub pane: Rc<Vec<PaneRow>>,
    pub pane_list: ListState,
}

/// One row of the Changes pane: the list of files, a file's header, or one
/// of its hunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneRow {
    Index,
    File(usize),
    Hunk(usize, usize),
}

/// The Changes pane's rows: the list of files, then each file and its
/// hunks.
pub fn pane_rows(changes: &Changes) -> Vec<PaneRow> {
    let mut out = vec![PaneRow::Index];
    for (f, file) in changes.files.iter().enumerate() {
        out.push(PaneRow::File(f));
        out.extend((0..file.hunks.len()).map(|h| PaneRow::Hunk(f, h)));
    }
    out
}

/// The rows of `items` with the groups in `open` opened, and the harness
/// steps of those in `harness` shown.
pub fn rows_of(items: &[Item], open: &HashSet<usize>, harness: &HashSet<usize>) -> Vec<Row> {
    let mut out = Vec::with_capacity(items.len());
    for (k, item) in items.iter().enumerate() {
        match item {
            Item::Message { .. } => out.push(Row::Message { item: k }),
            Item::Edit { .. } => out.push(Row::Edit { item: k }),
            Item::Steps(steps) => {
                out.push(Row::Group { item: k });
                if open.contains(&k) {
                    let shown = if harness.contains(&k) {
                        &steps.entries
                    } else {
                        &steps.work
                    };
                    out.extend(shown.iter().map(|&entry| Row::Step { item: k, entry }));
                    if steps.entries.len() > steps.work.len() {
                        out.push(Row::Harness { item: k });
                    }
                }
            }
        }
    }
    out
}

/// The item that holds entry `e`, and whether `e` is a step there. An entry
/// in a run of harness steps alone, which has no item, gives the next item.
pub fn item_of(items: &[Item], e: usize) -> Option<(usize, bool)> {
    let mut next = None;
    for (k, item) in items.iter().enumerate() {
        match item {
            Item::Message { entry, images } if (*entry..=*entry + *images).contains(&e) => {
                return Some((k, false));
            }
            Item::Steps(s) if s.entries.contains(&e) => return Some((k, true)),
            Item::Edit { entry } if *entry == e => return Some((k, false)),
            Item::Edit { entry } if *entry > e && next.is_none() => next = Some(k),
            Item::Message { entry, .. } if *entry > e && next.is_none() => next = Some(k),
            Item::Steps(s) if s.entries.first().is_some_and(|f| *f > e) && next.is_none() => {
                next = Some(k)
            }
            _ => {}
        }
    }
    next.map(|k| (k, false))
}

/// The session views' state.
#[derive(Default)]
pub struct SessionsState {
    /// Each session's entries, read once per reload.
    pub turns: HashMap<String, Result<SessionView, String>>,
    /// Where each session's list was scrolled when a reload dropped its
    /// view, so the view read again opens there (a step revealed by Open
    /// the step stays in view).
    pub kept_scroll: HashMap<String, gpui::ListOffset>,
    pub loading: HashSet<String>,
    /// Entries opened, by entry index: a folded one unfolded, or a long one
    /// shown whole.
    pub open: HashSet<(String, usize)>,
    /// Entries whose tool output or injected text is shown in full.
    pub full: HashSet<(String, usize)>,
    /// Groups of steps opened, by item index.
    pub groups: HashSet<(String, usize)>,
    /// Opened groups whose harness steps are shown.
    pub harness: HashSet<(String, usize)>,
    /// The header's parts opened: `asked`, `outcome`, `details`.
    pub header: HashSet<(String, &'static str)>,
    /// Hunks of the Changes pane shown in full: (session, file, hunk).
    pub pane_full: HashSet<(String, usize, usize)>,
    /// The Changes pane is hidden (it shows by default once a session has
    /// edits).
    pub changes_hidden: bool,
    /// The row the arrow keys, Space and Return act on.
    pub current: Option<(String, Row)>,
    /// A turn to scroll to once its session's entries are read.
    pub scroll_to: Option<(String, u32)>,
    /// The entry an Ask source named, drawn in the selected colour.
    pub highlight: Option<(String, usize)>,
}

impl SessionsState {
    fn set_of(&self, id: &str, which: &HashSet<(String, usize)>) -> HashSet<usize> {
        which
            .iter()
            .filter(|(s, _)| s == id)
            .map(|(_, k)| *k)
            .collect()
    }

    /// Recomputes a session's rows after a group opened or closed, keeping
    /// the list's measurements of the rows that stayed.
    fn relayout(&mut self, id: &str, group: usize) {
        let open = self.set_of(id, &self.groups);
        let harness = self.set_of(id, &self.harness);
        let Some(Ok(view)) = self.turns.get_mut(id) else {
            return;
        };
        let old = view.rows.clone();
        let new = rows_of(&view.items, &open, &harness);
        let Some(at) = old.iter().position(|r| *r == Row::Group { item: group }) else {
            view.rows = Rc::new(new);
            view.list.reset(view.rows.len() + 1);
            return;
        };
        let children = |rows: &[Row]| {
            rows[at + 1..]
                .iter()
                .take_while(|r| matches!(r, Row::Step { item, .. } | Row::Harness { item } if *item == group))
                .count()
        };
        let (was, now) = (children(&old), children(&new));
        view.list.splice(at + 2..at + 2 + was, now);
        view.list.remeasure_items(at + 1..at + 2);
        view.rows = Rc::new(new);
        // A current step that is no longer shown moves to its group.
        if let Some((s, row)) = &self.current
            && s == id
            && !view.rows.contains(row)
        {
            self.current = Some((id.to_string(), Row::Group { item: group }));
        }
    }
}

/// `2026-11-08T10:05:33Z` as the local `10:05`.
pub fn clock(ts: &str) -> String {
    OffsetDateTime::parse(ts, &Rfc3339)
        .ok()
        .and_then(|t| {
            t.to_offset(local_offset())
                .format(format_description!("[hour]:[minute]"))
                .ok()
        })
        .unwrap_or_else(|| ts.get(11..16).unwrap_or(ts).to_string())
}

/// A tool's name as a reader wants it: an MCP tool's
/// `mcp__<server>__<tool>` as `<server> <tool>`.
pub fn tool_label(name: &str) -> String {
    match name.strip_prefix("mcp__").and_then(|r| r.rsplit_once("__")) {
        Some((server, tool)) => format!("{} {tool}", server.trim_start_matches("claude_ai_")),
        None => name.to_string(),
    }
}

/// An entry's text for Command-C.
pub fn entry_text(entry: &Entry) -> String {
    match &entry.kind {
        Kind::Tool { input, result, .. } => [input.as_deref(), result.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("\n\n"),
        _ => entry.text.clone(),
    }
}

/// A tool's input as a reader wants it: a JSON object as `key: value`
/// lines, a string value with its line breaks; anything else as it is.
fn pretty_input(input: &str) -> String {
    let value = serde_json::from_str::<serde_json::Value>(input)
        .ok()
        .and_then(|v| match v {
            // Codex: a JSON string of JSON.
            serde_json::Value::String(s) => serde_json::from_str::<serde_json::Value>(&s).ok(),
            v => Some(v),
        });
    match value {
        Some(serde_json::Value::Object(map)) => map
            .iter()
            .map(|(k, v)| match v {
                serde_json::Value::String(s) if s.contains('\n') => format!("{k}:\n{s}"),
                serde_json::Value::String(s) => format!("{k}: {s}"),
                v => format!("{k}: {v}"),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(v) => serde_json::to_string_pretty(&v).unwrap_or_else(|_| input.to_string()),
        None => input.to_string(),
    }
}

/// What the harness steps of a group are, for the line that shows them:
/// `3 harness steps: ToolSearch, Skill, reminders`.
fn harness_label(entries: &[Entry], steps: &Steps) -> String {
    let mut kinds: Vec<String> = Vec::new();
    for &e in &steps.entries {
        let Some(entry) = entries.get(e) else {
            continue;
        };
        if !entry.is_harness() {
            continue;
        }
        let kind = match &entry.kind {
            Kind::Tool { name, .. } => name.clone(),
            Kind::Injection(Injection::SystemReminder) => "reminders".into(),
            Kind::Injection(Injection::TaskNotification) => "notifications".into(),
            Kind::Injection(Injection::Image) => "images".into(),
            Kind::Injection(i) => i.label().to_lowercase(),
            _ => continue,
        };
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    let n = steps.entries.len() - steps.work.len();
    format!(
        "{n} harness {}: {}",
        if n == 1 { "step" } else { "steps" },
        kinds.join(", ")
    )
}

/// The text a request shows: without an orchestrator's preamble, citation
/// links and attachment lines when anything is left.
fn request_body(text: &str) -> String {
    conversation::clean_request(text).unwrap_or_else(|| text.to_string())
}

impl Shell {
    fn index_home(&self) -> Option<std::path::PathBuf> {
        self.loaded
            .as_ref()
            .and_then(|l| l.setup.config())
            .map(|c| c.home.clone())
    }

    /// The session that started sub-agent `id`.
    fn parent_of(&self, id: &str) -> Option<String> {
        self.loaded
            .as_ref()
            .and_then(|l| l.about.get(id))
            .and_then(|a| a.parent.clone())
            .or_else(|| id.split_once("-agent-").map(|(p, _)| p.to_owned()))
    }

    /// Session `id`'s sub-agents, by id, with their titles, as read when
    /// the session opened.
    fn agents_of(&self, id: &str) -> Vec<(String, String)> {
        let Some(about) = self.loaded.as_ref().map(|l| &l.about) else {
            return Vec::new();
        };
        let mut out: Vec<(String, String)> = about
            .iter()
            .filter(|(_, a)| a.parent.as_deref() == Some(id))
            .map(|(aid, a)| {
                (
                    aid.clone(),
                    a.title.clone().unwrap_or_else(|| rows::id_title(aid)),
                )
            })
            .collect();
        out.sort();
        out
    }

    /// A session's record, when it has one.
    pub(crate) fn session_row(&self, id: &str) -> Option<&hippocamp_core::sessions::LogRow> {
        self.loaded
            .as_ref()?
            .panels
            .as_ref()?
            .sessions
            .as_ref()
            .ok()?
            .rows
            .iter()
            .filter_map(|l| l.record.as_ref().ok())
            .find(|r| r.id == id)
    }

    /// Reads a session's turns on the background executor, once per reload,
    /// and makes its entries, dialogue and digest.
    pub(crate) fn load_turns(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.sessions_ui.turns.contains_key(id) || self.sessions_ui.loading.contains(id) {
            return;
        }
        let Some(home) = self.index_home() else {
            return;
        };
        self.sessions_ui.loading.insert(id.to_string());
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            let key = id.clone();
            let read = cx
                .background_executor()
                .spawn(async move {
                    let index = Index::open(&home, false).map_err(|e| match e {
                        hippocamp_core::error::Error::MissingIndex => rows::NO_INDEX.to_string(),
                        e => e.to_string(),
                    })?;
                    let turns = sessions::turns(&index, &id).map_err(|e| e.to_string())?;
                    let entries = conversation::entries(&turns);
                    let items = conversation::items(&entries);
                    let digest = conversation::digest(&entries);
                    let changes = changes::changes(&entries);
                    let run = match index.source(&id) {
                        Ok(Some((tool, path))) => hippocamp_core::sessions::run_facts(&tool, &path),
                        _ => RunFacts::default(),
                    };
                    // Its title, project and parent, and its sub-agents':
                    // their transcript heads.
                    let prefix = format!("{id}-agent-");
                    let about: Vec<(String, hippocamp_view::sessions::About)> = index
                        .sources()
                        .map(|all| {
                            all.into_iter()
                                .filter(|(sid, _)| *sid == id || sid.starts_with(&prefix))
                                .map(|(sid, (tool, path))| {
                                    let o =
                                        hippocamp_core::session_meta::origin(&sid, &tool, &path);
                                    let a = hippocamp_view::sessions::About {
                                        title: o.named.clone().or(o.description.clone()),
                                        project: o.project(),
                                        cwd: o.cwd.clone(),
                                        parent: o.parent.clone(),
                                        automated: o.automated,
                                    };
                                    (sid, a)
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let count = turns.len() as u64;
                    Ok::<_, String>((entries, items, digest, changes, run, about, count))
                })
                .await;
            this.update(cx, |this, cx| {
                this.sessions_ui.loading.remove(&key);
                let open = this.sessions_ui.set_of(&key, &this.sessions_ui.groups);
                let harness = this.sessions_ui.set_of(&key, &this.sessions_ui.harness);
                let view = read.map(|(entries, items, digest, changes, run, about, count)| {
                    if let Some(l) = this.loaded.as_mut() {
                        l.turns.insert(key.clone(), count);
                        l.about.extend(about);
                    }
                    let rows = rows_of(&items, &open, &harness);
                    let pane = pane_rows(&changes);
                    let list = ListState::new(rows.len() + 1, ListAlignment::Top, px(1200.));
                    if let Some(at) = this.sessions_ui.kept_scroll.remove(&key) {
                        list.scroll_to(at);
                    }
                    SessionView {
                        list,
                        pane_list: ListState::new(pane.len(), ListAlignment::Top, px(1200.)),
                        entries: Rc::new(entries),
                        items: Rc::new(items),
                        digest: Rc::new(digest),
                        changes: Rc::new(changes),
                        run: Rc::new(run),
                        rows: Rc::new(rows),
                        pane: Rc::new(pane),
                    }
                });
                this.sessions_ui.turns.insert(key, view);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The active tab's session, and its view once read.
    fn active_session(&self) -> Option<(String, SessionView)> {
        let Some(Tab::Session { id, .. }) = self.state.tabs.active_tab() else {
            return None;
        };
        let view = self.sessions_ui.turns.get(id)?.as_ref().ok()?.clone();
        Some((id.clone(), view))
    }

    /// The current row's position in the active session's rows.
    fn current_pos(&self, id: &str, view: &SessionView) -> Option<usize> {
        match self.sessions_ui.current.as_ref() {
            Some((s, row)) if s == id => view.rows.iter().position(|r| r == row),
            _ => None,
        }
    }

    /// Makes row `r` current and scrolls it into view.
    fn set_current(&mut self, id: &str, view: &SessionView, r: usize) {
        if view.rows.is_empty() {
            return;
        }
        let r = r.min(view.rows.len() - 1);
        self.sessions_ui.current = Some((id.to_string(), view.rows[r]));
        view.list.scroll_to_reveal_item(r + 1);
    }

    /// Up or Down in a session tab: the previous or next row.
    pub(crate) fn step_turn(&mut self, step: isize) {
        let Some((id, view)) = self.active_session() else {
            return;
        };
        let next = match self.current_pos(&id, &view) {
            Some(r) => (r as isize + step).max(0) as usize,
            None => 0,
        };
        self.set_current(&id, &view, next);
    }

    /// Makes row `r` current and puts it at the top of the view: for a jump
    /// past rows not yet measured, where revealing would guess.
    fn jump_to(&mut self, id: &str, view: &SessionView, r: usize) {
        if view.rows.is_empty() {
            return;
        }
        let r = r.min(view.rows.len() - 1);
        self.sessions_ui.current = Some((id.to_string(), view.rows[r]));
        view.list.scroll_to(gpui::ListOffset {
            item_ix: r + 1,
            offset_in_item: px(0.),
        });
    }

    /// Home or End in a session tab.
    pub(crate) fn edge_turn(&mut self, last: bool) {
        let Some((id, view)) = self.active_session() else {
            return;
        };
        if last {
            self.jump_to(&id, &view, view.rows.len().saturating_sub(1));
        } else if let Some(first) = view.rows.first() {
            self.sessions_ui.current = Some((id, *first));
            view.list.scroll_to(gpui::ListOffset::default());
        }
    }

    /// Space or Return: opens or folds the current row (a group, a step, a
    /// folded message or a long one cut short), the first row when none is
    /// current.
    pub(crate) fn fold_turn(&mut self) {
        let Some((id, view)) = self.active_session() else {
            return;
        };
        let r = self.current_pos(&id, &view).unwrap_or(0);
        let Some(row) = view.rows.get(r).copied() else {
            return;
        };
        self.sessions_ui.current = Some((id.clone(), row));
        self.toggle_row(&id, row);
    }

    /// Opens or folds a row.
    fn toggle_row(&mut self, id: &str, row: Row) {
        match row {
            Row::Group { item } => {
                let key = (id.to_string(), item);
                if !self.sessions_ui.groups.remove(&key) {
                    self.sessions_ui.groups.insert(key);
                }
                self.sessions_ui.relayout(id, item);
            }
            Row::Harness { item } => {
                let key = (id.to_string(), item);
                if !self.sessions_ui.harness.remove(&key) {
                    self.sessions_ui.harness.insert(key);
                }
                self.sessions_ui.relayout(id, item);
            }
            Row::Message { item } => {
                let entry = match self.session_view(id).map(|v| v.items.get(item).cloned()) {
                    Some(Some(Item::Message { entry, .. })) => entry,
                    _ => return,
                };
                self.toggle_entry(id, row, entry);
            }
            Row::Step { entry, .. } => self.toggle_entry(id, row, entry),
            Row::Edit { item } => {
                let entry = match self.session_view(id).map(|v| v.items.get(item).cloned()) {
                    Some(Some(Item::Edit { entry })) => entry,
                    _ => return,
                };
                self.toggle_entry(id, row, entry);
            }
        }
    }

    /// Shows or hides the Changes pane.
    pub(crate) fn toggle_changes(&mut self) {
        self.sessions_ui.changes_hidden = !self.sessions_ui.changes_hidden;
    }

    fn session_view(&self, id: &str) -> Option<&SessionView> {
        self.sessions_ui.turns.get(id)?.as_ref().ok()
    }

    fn remeasure_row(&self, id: &str, row: Row) {
        if let Some(view) = self.session_view(id)
            && let Some(r) = view.rows.iter().position(|x| *x == row)
        {
            view.list.remeasure_items(r + 1..r + 2);
        }
    }

    fn toggle_entry(&mut self, id: &str, row: Row, entry: usize) {
        let key = (id.to_string(), entry);
        if !self.sessions_ui.open.remove(&key) {
            self.sessions_ui.open.insert(key.clone());
        }
        self.sessions_ui.full.remove(&key);
        self.remeasure_row(id, row);
    }

    /// The text Command-C copies in a session tab: the current row's.
    pub(crate) fn current_entry_text(&self, id: &str) -> Option<String> {
        let (s, row) = self.sessions_ui.current.as_ref()?;
        if s != id {
            return None;
        }
        let view = self.session_view(id)?;
        match *row {
            Row::Message { item } | Row::Edit { item } => match view.items.get(item)? {
                Item::Message { entry, .. } | Item::Edit { entry } => {
                    view.entries.get(*entry).map(entry_text)
                }
                Item::Steps(_) => None,
            },
            Row::Step { entry, .. } => view.entries.get(entry).map(entry_text),
            Row::Group { item } | Row::Harness { item } => match view.items.get(item)? {
                Item::Steps(steps) => Some(steps.label()),
                Item::Message { .. } | Item::Edit { .. } => None,
            },
        }
    }

    /// Scrolls once to the turn an Ask source named: its message, or its
    /// step with the step's group opened.
    fn reveal_ordinal(&mut self, id: &str, ordinal: u32) {
        let Some(view) = self.session_view(id).cloned() else {
            return;
        };
        let Some(e) = conversation::position(&view.entries, ordinal) else {
            return;
        };
        self.sessions_ui.highlight = Some((id.to_string(), e));
        let Some((item, step)) = item_of(&view.items, e) else {
            return;
        };
        let row = if let Some(Item::Edit { .. }) = view.items.get(item) {
            Row::Edit { item }
        } else if step {
            if view.entries.get(e).is_some_and(Entry::is_harness) {
                self.sessions_ui.harness.insert((id.to_string(), item));
            }
            self.sessions_ui.groups.insert((id.to_string(), item));
            self.sessions_ui.relayout(id, item);
            Row::Step { item, entry: e }
        } else {
            Row::Message { item }
        };
        let Some(view) = self.session_view(id).cloned() else {
            return;
        };
        if let Some(r) = view.rows.iter().position(|x| *x == row) {
            self.jump_to(id, &view, r);
        }
    }

    pub(super) fn render_session_tab(
        &mut self,
        id: &str,
        ordinal: Option<u32>,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        self.load_turns(id, cx);
        let title = self.session_title(id).unwrap_or_else(|| rows::id_title(id));
        if let Some(o) = ordinal
            && self.sessions_ui.scroll_to.as_ref() == Some(&(id.to_string(), o))
            && self.session_view(id).is_some()
        {
            self.sessions_ui.scroll_to = None;
            self.reveal_ordinal(id, o);
        }
        let view = self.sessions_ui.turns.get(id).cloned();
        let body = match view {
            Some(Ok(view)) => {
                let key = id.to_string();
                let conversation = div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(
                        div().flex_1().min_h_0().child(
                            list(
                                view.list.clone(),
                                cx.processor(move |this, ix: usize, window, cx| {
                                    this.render_session_item(&key, ix, window, cx)
                                }),
                            )
                            .size_full(),
                        ),
                    )
                    .child(self.render_session_footer(id, &view, look));
                let show_pane = !view.changes.files.is_empty() && !self.sessions_ui.changes_hidden;
                let key = id.to_string();
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(self.render_thread_bar(id, &title, &view, look, cx))
                    .child(div().flex_1().min_h_0().flex().child(conversation).when(
                        show_pane,
                        |d| {
                            d.child(
                                div()
                                    .id("changes-pane")
                                    .role(Role::Region)
                                    .aria_label("Changes")
                                    .w(look.px(PANE_WIDTH))
                                    .flex_shrink_0()
                                    .h_full()
                                    .border_l(look.fixed(1.))
                                    .border_color(look.c.border)
                                    .flex()
                                    .flex_col()
                                    .child(self.render_pane_header(&view, look))
                                    .child(
                                        div().flex_1().min_h_0().child(
                                            list(
                                                view.pane_list.clone(),
                                                cx.processor(
                                                    move |this, ix: usize, _window, cx| {
                                                        this.render_pane_item(&key, ix, cx)
                                                    },
                                                ),
                                            )
                                            .size_full(),
                                        ),
                                    ),
                            )
                        },
                    ))
                    .into_any_element()
            }
            other => {
                let message = match other {
                    None => ("Reading the turns…".to_string(), false),
                    Some(Err(e)) => {
                        let error = e != rows::NO_INDEX;
                        (e, error)
                    }
                    Some(Ok(_)) => unreachable!(),
                };
                div()
                    .p(look.px(16.))
                    .flex()
                    .flex_col()
                    .gap(look.px(8.))
                    .child(self.render_session_header(id, look, cx))
                    .child(
                        div()
                            .text_color(if message.1 {
                                look.c.error
                            } else {
                                look.c.text_muted
                            })
                            .child(message.0),
                    )
                    .into_any_element()
            }
        };
        div()
            .id(SharedString::from(format!("session-tab-{id}")))
            .key_context("SessionTurns")
            .track_focus(&self.turns_focus)
            .role(Role::Article)
            .aria_label(format!("Session: {title}"))
            .size_full()
            .flex()
            .flex_col()
            .text_size(look.text(13.))
            .child(body)
            .into_any_element()
    }

    /// Item `ix` of a session's list: the header, or row `ix - 1`.
    fn render_session_item(
        &mut self,
        id: &str,
        ix: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let look = Look::from_app(cx);
        let inner = if ix == 0 {
            self.render_session_header(id, &look, cx).into_any_element()
        } else {
            let view = self.session_view(id).cloned();
            match view.as_ref().and_then(|v| v.rows.get(ix - 1).copied()) {
                Some(row) => self.render_thread_row(id, row, view.as_ref().unwrap(), &look, cx),
                None => div().into_any_element(),
            }
        };
        div()
            .w_full()
            .px(look.px(16.))
            .when(ix == 0, |d| d.pt(look.px(16.)))
            .child(div().max_w(look.px(720.)).child(inner))
            .into_any_element()
    }

    /// A link-coloured button that toggles one of the header's parts.
    fn header_toggle(
        &self,
        id: &str,
        part: &'static str,
        text: &'static str,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = id.to_string();
        div()
            .id(SharedString::from(format!("session-header-{part}")))
            .role(Role::Button)
            .aria_label(text)
            .cursor_pointer()
            .text_size(look.text(12.))
            .text_color(look.c.text_accent)
            .hover(|h| h.underline())
            .child(text)
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                let key = (id.clone(), part);
                if !this.sessions_ui.header.remove(&key) {
                    this.sessions_ui.header.insert(key);
                }
                if let Some(view) = this.session_view(&id) {
                    view.list.remeasure_items(0..1);
                }
                window.focus(&this.turns_focus, cx);
                cx.notify();
            }))
    }

    /// The tab's header: the title and its facts, then what was asked, how
    /// it ended and what came of it, then the notes, parent and agents.
    fn render_session_header(
        &self,
        id: &str,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let row = self.session_row(id).cloned();
        let about = self.loaded.as_ref().and_then(|l| l.about.get(id)).cloned();
        let title = self.session_title(id).unwrap_or_else(|| rows::id_title(id));
        let turn_count = self.loaded.as_ref().and_then(|l| l.turns.get(id).copied());
        let digest = self.session_view(id).map(|v| v.digest.clone());
        let opened = |part: &'static str| self.sessions_ui.header.contains(&(id.to_string(), part));

        let mut line = Vec::new();
        if let Some(p) = about.as_ref().and_then(|a| a.project.clone()) {
            line.push(visible(&p));
        }
        if let Some(r) = &row {
            line.push(
                r.started
                    .to_offset(local_offset())
                    .format(format_description!(
                        "[weekday repr:short] [day] [month repr:short] [year] [hour]:[minute]"
                    ))
                    .unwrap_or_default(),
            );
            line.push(rows::session_meta(r, turn_count));
        } else {
            line.push("no session record".into());
        }
        let ended = row.as_ref().is_some_and(|r| r.ended.is_some());

        let link = |key: String, text: String, tab: Tab, cx: &mut Context<Self>| {
            div()
                .id(SharedString::from(key))
                .role(Role::Link)
                .aria_label(text.clone())
                .cursor_pointer()
                .text_color(look.c.text_accent)
                .hover(|h| h.underline())
                .child(text)
                .on_click(cx.listener(move |this, e: &gpui::ClickEvent, window, cx| {
                    this.state.tabs.follow(tab.clone(), e.click_count() >= 2);
                    this.after_state_change(window, cx);
                }))
        };
        let label = |text: &str| {
            div()
                .pt(look.px(8.))
                .text_size(look.text(11.))
                .text_color(look.c.text_muted)
                .child(text.to_string())
        };

        let mut column = div()
            .flex()
            .flex_col()
            .gap(look.px(4.))
            .pb(look.px(12.))
            .when(digest.is_none(), |d| {
                d.child(
                    div()
                        .text_size(look.text(15.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .line_clamp(2)
                        .text_ellipsis()
                        .child(title.clone()),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_x(look.px(8.))
                    .text_size(look.text(11.))
                    .text_color(look.c.text_muted)
                    .child(line.join(" · "))
                    .child(self.header_toggle(
                        id,
                        "details",
                        if opened("details") {
                            "hide details"
                        } else {
                            "details"
                        },
                        look,
                        cx,
                    )),
            );
        if opened("details") {
            let copy = id.to_string();
            column = column.child(
                div()
                    .flex()
                    .gap_x(look.px(8.))
                    .text_size(look.text(11.))
                    .text_color(look.c.text_muted)
                    .child(format!("id {}", visible(id)))
                    .child(
                        div()
                            .id("session-copy-id")
                            .role(Role::Button)
                            .aria_label("Copy id")
                            .cursor_pointer()
                            .text_color(look.c.text_accent)
                            .hover(|h| h.underline())
                            .child("Copy id")
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                            })),
                    ),
            );
        }

        // What was asked, how it ended, what came of it.
        if let Some(d) = digest.as_deref() {
            if let Some(asked) = &d.asked {
                let open = opened("asked");
                let (shown, cut) = if open {
                    (asked.as_str(), false)
                } else {
                    markdown::clip(asked, ASKED_LINES, ASKED_CHARS)
                };
                column = column
                    .child(label(if d.asked_earlier {
                        "Asked, in the thread this one continues"
                    } else {
                        "Asked"
                    }))
                    .child(markdown_element(&format!("{id}-asked"), shown, look, None));
                if cut || open {
                    column = column.child(self.header_toggle(
                        id,
                        "asked",
                        if open { "less" } else { "more" },
                        look,
                        cx,
                    ));
                }
            }
            if let Some(outcome) = &d.outcome {
                let open = opened("outcome");
                let (shown, cut) = if open {
                    (outcome.as_str(), false)
                } else {
                    markdown::clip(outcome, OUTCOME_LINES, OUTCOME_CHARS)
                };
                column = column
                    .child(label(if ended {
                        "How it ended"
                    } else {
                        "Latest reply"
                    }))
                    .child(markdown_element(
                        &format!("{id}-outcome"),
                        shown,
                        look,
                        None,
                    ));
                if cut || open {
                    column = column.child(self.header_toggle(
                        id,
                        "outcome",
                        if open { "less" } else { "more" },
                        look,
                        cx,
                    ));
                }
            }
            let sub_agents = self.agents_of(id).len();
            let mut fact_line = div()
                .flex()
                .flex_wrap()
                .gap_x(look.px(8.))
                .pt(look.px(4.))
                .text_size(look.text(12.))
                .text_color(look.c.text_muted);
            let mut any = false;
            if let Some(s) = d.seconds.filter(|s| *s > 0) {
                fact_line = fact_line.child(format!("ran {}", conversation::duration(s)));
                any = true;
            }
            match d.agents.max(sub_agents) {
                0 => {}
                1 => {
                    fact_line = fact_line.child("1 sub-agent");
                    any = true;
                }
                n => {
                    fact_line = fact_line.child(format!("{n} sub-agents"));
                    any = true;
                }
            }
            match d.files.len() {
                0 => {}
                1 => {
                    fact_line = fact_line.child("1 file edited");
                    any = true;
                }
                n => {
                    fact_line = fact_line.child(format!("{n} files edited"));
                    any = true;
                }
            }
            if !d.pulls.is_empty() {
                let repos: HashSet<&str> =
                    d.pulls.iter().filter_map(|u| u.split('/').nth(4)).collect();
                fact_line = fact_line.child(if d.pulls.len() == 1 { "PR" } else { "PRs" });
                for (i, url) in d.pulls.iter().take(HEADER_PULLS).enumerate() {
                    let url = url.clone();
                    let text = conversation::pull_label(&url, repos.len() > 1);
                    fact_line = fact_line.child(
                        div()
                            .id(("session-pull", i))
                            .role(Role::Link)
                            .aria_label(format!("Pull request {text}"))
                            .cursor_pointer()
                            .text_color(look.c.text_accent)
                            .hover(|h| h.underline())
                            .tooltip(super::tooltip::Tooltip::text(url.clone()))
                            .child(text)
                            .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&url))),
                    );
                }
                if d.pulls.len() > HEADER_PULLS {
                    fact_line =
                        fact_line.child(format!("and {} more", d.pulls.len() - HEADER_PULLS));
                }
                any = true;
            }
            if any {
                column = column.child(fact_line);
            }
        }

        if let Some(r) = row.as_ref().filter(|r| !r.notes.is_empty()) {
            let mut notes = div()
                .flex()
                .flex_wrap()
                .gap_x(look.px(8.))
                .text_size(look.text(12.))
                .child(div().text_color(look.c.text_muted).child("notes"));
            for (i, n) in r.notes.iter().enumerate() {
                let label = visible(&n.display().to_string());
                notes = notes.child(link(
                    format!("session-note-{i}"),
                    label,
                    Tab::Note(n.clone()),
                    cx,
                ));
            }
            column = column.child(notes);
        }
        // "unspecified" is a derived record's placeholder, not a reason.
        if let Some(why) = row
            .as_ref()
            .and_then(|r| r.no_change_reason.clone())
            .filter(|w| w != "unspecified")
        {
            column = column.child(
                div()
                    .text_size(look.text(12.))
                    .text_color(look.c.text_muted)
                    .child(format!("no change: {}", visible(&why))),
            );
        }
        {
            if let Some(parent) = self.parent_of(id) {
                let label = self
                    .session_title(&parent)
                    .unwrap_or_else(|| rows::id_title(&parent));
                column = column.child(
                    div()
                        .flex()
                        .gap_x(look.px(8.))
                        .text_size(look.text(12.))
                        .child(div().text_color(look.c.text_muted).child("started by"))
                        .child(link(
                            "session-parent".into(),
                            label,
                            Tab::Session {
                                id: parent,
                                ordinal: None,
                            },
                            cx,
                        )),
                );
            }
            let agents = self.agents_of(id);
            if !agents.is_empty() {
                let mut line = div()
                    .flex()
                    .flex_wrap()
                    .gap_x(look.px(8.))
                    .text_size(look.text(12.))
                    .child(
                        div()
                            .text_color(look.c.text_muted)
                            .child(match agents.len() {
                                1 => "1 sub-agent".to_string(),
                                n => format!("{n} sub-agents"),
                            }),
                    );
                let all = opened("agents");
                let shown = if all {
                    HEADER_AGENTS
                } else {
                    HEADER_AGENTS_FOLDED
                };
                for (i, (aid, atitle)) in agents.iter().take(shown).enumerate() {
                    line = line.child(link(
                        format!("session-agent-{i}"),
                        atitle.clone(),
                        Tab::Session {
                            id: aid.clone(),
                            ordinal: None,
                        },
                        cx,
                    ));
                }
                if agents.len() > HEADER_AGENTS_FOLDED {
                    line = line.child(self.header_toggle(
                        id,
                        "agents",
                        if all { "fewer" } else { "all agents" },
                        look,
                        cx,
                    ));
                }
                if all && agents.len() > HEADER_AGENTS {
                    line = line.child(
                        div()
                            .text_color(look.c.text_muted)
                            .child(format!("and {} more", agents.len() - HEADER_AGENTS)),
                    );
                }
                column = column.child(line);
            }
        }
        column.child(div().mt(look.px(4.)).h(look.fixed(1.)).bg(look.c.border))
    }

    /// One row: a message, a group's line, a step, or the harness line.
    fn render_thread_row(
        &mut self,
        id: &str,
        row: Row,
        view: &SessionView,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let current = self.sessions_ui.current.as_ref() == Some(&(id.to_string(), row));
        match row {
            Row::Message { item } => match view.items.get(item) {
                Some(Item::Message { entry, images }) => match view.entries.get(*entry) {
                    Some(e) => {
                        self.render_entry(id, row, *entry, e, *images, false, current, look, cx)
                    }
                    None => div().into_any_element(),
                },
                _ => div().into_any_element(),
            },
            Row::Step { entry, .. } => match view.entries.get(entry) {
                Some(e) => self.render_entry(id, row, entry, e, 0, true, current, look, cx),
                None => div().into_any_element(),
            },
            Row::Edit { item } => match view.items.get(item) {
                Some(Item::Edit { entry }) => {
                    self.render_edit(id, row, *entry, view, current, look, cx)
                }
                _ => div().into_any_element(),
            },
            Row::Group { item } | Row::Harness { item } => {
                let Some(Item::Steps(steps)) = view.items.get(item) else {
                    return div().into_any_element();
                };
                let group = matches!(row, Row::Group { .. });
                let open = if group {
                    self.sessions_ui.groups.contains(&(id.to_string(), item))
                } else {
                    self.sessions_ui.harness.contains(&(id.to_string(), item))
                };
                let text = if group {
                    format!("{} {}", steps.label(), if open { "▾" } else { "▸" })
                } else if open {
                    "hide harness steps".to_string()
                } else {
                    format!("show {}", harness_label(&view.entries, steps))
                };
                let id_owned = id.to_string();
                div()
                    .id(SharedString::from(format!("{id}-row-{item}-{group}")))
                    .role(Role::Button)
                    .aria_label(text.clone())
                    .aria_expanded(open)
                    .flex()
                    .my(look.px(2.))
                    .py(look.px(3.))
                    .px(look.px(8.))
                    .ml(look.px(GUTTER - 8.))
                    .mr(look.px(-8.))
                    .when(!group, |d| d.ml(look.px(GUTTER)))
                    .rounded(look.px(4.))
                    .border_l(look.fixed(super::theme::RING))
                    .border_color(if current {
                        look.c.focus
                    } else {
                        gpui::transparent_black()
                    })
                    .cursor_pointer()
                    .hover(|h| h.bg(look.c.hover))
                    .text_size(look.text(12.))
                    .text_color(if group {
                        look.c.text_muted
                    } else {
                        look.c.text_accent
                    })
                    .child(div().truncate().child(text))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.sessions_ui.current = Some((id_owned.clone(), row));
                        this.toggle_row(&id_owned, row);
                        window.focus(&this.turns_focus, cx);
                        cx.notify();
                    }))
                    .into_any_element()
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_entry(
        &mut self,
        id: &str,
        row: Row,
        i: usize,
        entry: &Entry,
        images: usize,
        step: bool,
        current: bool,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let key = (id.to_string(), i);
        let open = self.sessions_ui.open.contains(&key);
        let full = self.sessions_ui.full.contains(&key);
        let highlighted = self.sessions_ui.highlight.as_ref() == Some(&key);
        let folds = entry.folds();
        let el_key = format!("{id}-{i}");

        // The label line. A message has its time; a step and a folded line
        // do not.
        let (label, label_color, mono_label): (String, gpui::Hsla, bool) = match &entry.kind {
            Kind::Request => (
                match images {
                    0 => "You".to_string(),
                    1 => "You · 1 image".to_string(),
                    n => format!("You · {n} images"),
                },
                look.c.text,
                false,
            ),
            Kind::Assistant => ("Assistant".into(), look.c.text_muted, false),
            Kind::Tool { name, summary, .. } => (
                format!(
                    "{} {}  {}",
                    if open { "▾" } else { "▸" },
                    visible(&tool_label(name)),
                    visible(summary)
                ),
                look.c.text_muted,
                true,
            ),
            Kind::Injection(kind) => (
                format!(
                    "{} {} · {}",
                    if open { "▾" } else { "▸" },
                    kind.label(),
                    conversation::size(entry.text.len())
                ),
                look.c.text_muted,
                false,
            ),
        };
        let timed = matches!(entry.kind, Kind::Request | Kind::Assistant);
        let label_line = div()
            .flex()
            .items_center()
            .gap(look.px(8.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(look.text(if folds { 12. } else { 11. }))
                    .text_color(label_color)
                    .when(matches!(entry.kind, Kind::Request), |d| {
                        d.font_weight(FontWeight::SEMIBOLD)
                    })
                    .when(mono_label, |d| d.font_family("Menlo"))
                    .child(label),
            )
            .when(timed, |d| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .text_size(look.text(11.))
                        .text_color(look.c.text_muted)
                        .child(clock(&entry.ts)),
                )
            });

        // The body.
        let mut body: Option<gpui::AnyElement> = None;
        let mut more: Option<(&'static str, bool)> = None; // (label, toggles `full`)
        match &entry.kind {
            Kind::Request | Kind::Assistant => {
                let text = if entry.kind == Kind::Request {
                    request_body(&entry.text)
                } else {
                    entry.text.clone()
                };
                let (shown, cut) = if open {
                    (text.as_str(), false)
                } else {
                    markdown::clip(&text, CLIP_LINES, CLIP_CHARS)
                };
                body = Some(markdown_element(&el_key, shown, look, None));
                if cut {
                    more = Some(("show more", false));
                } else if open {
                    more = Some(("show less", false));
                }
            }
            Kind::Tool { input, result, .. } if open => {
                let mut col = div().flex().flex_col().gap(look.px(4.));
                col = col.child(
                    div()
                        .text_size(look.text(11.))
                        .text_color(look.c.text_muted)
                        .child(clock(&entry.ts)),
                );
                if let Some(input) = input {
                    let pretty = pretty_input(input);
                    let (shown, _) = markdown::clip(&pretty, CLIP_LINES, CLIP_CHARS);
                    col = col
                        .child(
                            div()
                                .text_size(look.text(11.))
                                .text_color(look.c.text_muted)
                                .child("input"),
                        )
                        .child(code_block(shown, look));
                }
                if let Some(result) = result {
                    let (shown, cut) = if full {
                        (result.as_str(), false)
                    } else {
                        markdown::clip(result, CLIP_LINES, CLIP_CHARS)
                    };
                    col = col
                        .child(
                            div()
                                .text_size(look.text(11.))
                                .text_color(look.c.text_muted)
                                .child(format!("output · {}", conversation::size(result.len()))),
                        )
                        .child(code_block(
                            if shown.is_empty() { "(empty)" } else { shown },
                            look,
                        ));
                    if cut {
                        more = Some(("show all", true));
                    }
                }
                body = Some(col.into_any_element());
            }
            Kind::Injection(Injection::ContextHandoff)
                if open && !conversation::handoff(&entry.text).1.is_empty() =>
            {
                let (note, items) = conversation::handoff(&entry.text);
                let mut col = div().flex().flex_col().gap(look.px(8.)).pl(look.px(12.));
                if let Some(note) = note {
                    col = col.child(
                        div()
                            .text_size(look.text(11.))
                            .text_color(look.c.text_muted)
                            .child(format!("From an earlier thread: {}", visible(&note))),
                    );
                }
                let mut any_cut = false;
                for (n, item) in items.iter().enumerate() {
                    let (shown, cut) = if full {
                        (item.text.as_str(), false)
                    } else {
                        markdown::clip(&item.text, CLIP_LINES / 2, CLIP_CHARS / 2)
                    };
                    any_cut |= cut;
                    col = col.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(look.px(2.))
                            .child(
                                div()
                                    .text_size(look.text(11.))
                                    .text_color(look.c.text_muted)
                                    .child(if item.role == "user" {
                                        "You"
                                    } else {
                                        "Assistant"
                                    }),
                            )
                            .child(markdown_element(
                                &format!("{el_key}-h{n}"),
                                shown,
                                look,
                                None,
                            )),
                    );
                }
                body = Some(col.into_any_element());
                if any_cut {
                    more = Some(("show all", true));
                }
            }
            Kind::Injection(Injection::Summary) if open => {
                let (shown, cut) = if full {
                    (entry.text.as_str(), false)
                } else {
                    markdown::clip(&entry.text, CLIP_LINES, CLIP_CHARS)
                };
                body = Some(
                    div()
                        .pl(look.px(12.))
                        .child(markdown_element(&format!("{el_key}-s"), shown, look, None))
                        .into_any_element(),
                );
                if cut {
                    more = Some(("show all", true));
                }
            }
            Kind::Injection(_) if open => {
                let (shown, cut) = if full {
                    (entry.text.as_str(), false)
                } else {
                    markdown::clip(&entry.text, CLIP_LINES, CLIP_CHARS)
                };
                body = Some(code_block(shown, look));
                if cut {
                    more = Some(("show all", true));
                }
            }
            _ => {}
        }

        let id_owned = id.to_string();
        let more_button = more.map(|(text, toggles_full)| {
            let id = id_owned.clone();
            div()
                .id(SharedString::from(format!("more-{el_key}")))
                .role(Role::Button)
                .aria_label(text)
                .cursor_pointer()
                .text_size(look.text(12.))
                .text_color(look.c.text_accent)
                .hover(|h| h.underline())
                .child(text)
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    let key = (id.clone(), i);
                    this.sessions_ui.current = Some((id.clone(), row));
                    if toggles_full {
                        if !this.sessions_ui.full.remove(&key) {
                            this.sessions_ui.full.insert(key);
                        }
                    } else if !this.sessions_ui.open.remove(&key) {
                        this.sessions_ui.open.insert(key);
                    }
                    this.remeasure_row(&id, row);
                    window.focus(&this.turns_focus, cx);
                    cx.notify();
                }))
        });

        let aria = match &entry.kind {
            Kind::Request => "You".to_string(),
            Kind::Assistant => "Assistant".to_string(),
            Kind::Tool { name, .. } => format!("Tool {}", visible(name)),
            Kind::Injection(k) => k.label().to_string(),
        };
        let message = matches!(entry.kind, Kind::Request | Kind::Assistant);
        let base = div()
            .id(SharedString::from(format!("entry-{el_key}")))
            .role(Role::Group)
            .aria_label(aria)
            .my(look.px(if folds { 1. } else { 2. }))
            .py(look.px(if folds { 2. } else { 8. }))
            .px(look.px(8.))
            .mr(look.px(-8.))
            .rounded(look.px(4.))
            .border_l(look.fixed(super::theme::RING))
            .border_color(if current {
                look.c.focus
            } else {
                gpui::transparent_black()
            })
            .when(highlighted, |d| d.bg(look.c.selected))
            .when(folds, |d| d.cursor_pointer().hover(|h| h.bg(look.c.hover)))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.sessions_ui.current = Some((id_owned.clone(), row));
                if folds {
                    this.toggle_row(&id_owned, row);
                }
                window.focus(&this.turns_focus, cx);
                cx.notify();
            }));
        if message {
            // A message: its avatar, its time in the avatar's tooltip, and
            // its text with no label line.
            let images_line = (images > 0).then(|| {
                div()
                    .text_size(look.text(11.))
                    .text_color(look.c.text_muted)
                    .child(if images == 1 {
                        "1 image attached".to_string()
                    } else {
                        format!("{images} images attached")
                    })
            });
            return base
                .ml(look.px(-8.))
                .flex()
                .gap(look.px(GUTTER - 20.))
                .child(self.avatar(entry, &el_key, look))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(look.px(4.))
                        .line_height(gpui::relative(1.55))
                        .children(body)
                        .children(images_line)
                        .children(more_button),
                )
                .into_any_element();
        }
        base.ml(look.px(if step { GUTTER } else { GUTTER - 8. }))
            .flex()
            .flex_col()
            .gap(look.px(4.))
            .child(label_line)
            .children(body)
            .children(more_button)
            .into_any_element()
    }

    /// A message's mark: the person's initial in a circle, or the model's
    /// in a rounded square. Its tooltip is the message's local time.
    fn avatar(&self, entry: &Entry, key: &str, look: &Look) -> impl IntoElement {
        let person = entry.kind == Kind::Request;
        let letter = if person {
            user_initial()
        } else {
            "A".to_string()
        };
        div()
            .id(SharedString::from(format!("avatar-{key}")))
            .flex_shrink_0()
            .mt(look.px(1.))
            .size(look.px(20.))
            .flex()
            .items_center()
            .justify_center()
            .text_size(look.text(11.))
            .font_weight(FontWeight::SEMIBOLD)
            .when(person, |d| {
                d.rounded_full()
                    .bg(look.c.text_accent)
                    .text_color(look.c.centre)
            })
            .when(!person, |d| {
                d.rounded(look.px(5.))
                    .border_1()
                    .border_color(look.c.border)
                    .bg(look.c.elevated)
                    .text_color(look.c.text_muted)
            })
            .tooltip(super::tooltip::Tooltip::text(clock(&entry.ts)))
            .child(letter)
    }

    /// An edit: one compact line (`Edit …/src/lib.rs  +3 −1`) over its diff,
    /// cut to [`INLINE_DIFF_LINES`] lines until opened. A failed edit is
    /// one line that says so.
    #[allow(clippy::too_many_arguments)]
    fn render_edit(
        &mut self,
        id: &str,
        row: Row,
        entry: usize,
        view: &SessionView,
        current: bool,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(e) = view.entries.get(entry) else {
            return div().into_any_element();
        };
        let Kind::Tool { name, summary, .. } = &e.kind else {
            return div().into_any_element();
        };
        let open = self.sessions_ui.open.contains(&(id.to_string(), entry));
        let failed = view.changes.failed.contains(&entry);
        let hunks = view
            .changes
            .by_entry
            .get(&entry)
            .cloned()
            .unwrap_or_default();
        let verb = match name.as_str() {
            "Write" => "Write",
            "apply_patch" => "Patch",
            _ => "Edit",
        };
        let path = hunks
            .first()
            .map(|(p, _)| conversation::short_path(p))
            .unwrap_or_else(|| summary.clone());
        let (added, removed) = hunks
            .iter()
            .fold((0, 0), |(a, r), (_, h)| (a + h.added(), r + h.removed()));
        let total: usize = hunks.iter().map(|(_, h)| h.lines.len()).sum();
        let cut = !open && total > INLINE_DIFF_LINES;
        let id_owned = id.to_string();

        let mut header = div()
            .flex()
            .items_center()
            .gap(look.px(8.))
            .text_size(look.text(12.))
            .child(div().text_color(look.c.text_muted).child(verb))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_family("Menlo")
                    .text_color(look.c.text_muted)
                    .child(visible(&path)),
            );
        if failed {
            header = header.child(
                div()
                    .text_color(look.c.text_muted)
                    .child("· failed, nothing changed"),
            );
        } else {
            header = header.child(counts(added, removed, look));
            if total > INLINE_DIFF_LINES {
                header = header.child(div().text_color(look.c.text_muted).child(if open {
                    "⌃"
                } else {
                    "⌄"
                }));
            }
        }
        let mut block = None;
        if !failed && total > 0 {
            let mut lines: Vec<DiffLine> = Vec::new();
            for (k, (p, h)) in hunks.iter().enumerate() {
                if k > 0 && hunks.len() > 1 {
                    lines.push(DiffLine {
                        kind: LineKind::Context,
                        number: None,
                        text: format!("… {}", conversation::short_path(p)),
                    });
                }
                lines.extend(h.lines.iter().cloned());
            }
            let shown = if cut {
                &lines[..INLINE_DIFF_LINES]
            } else {
                &lines[..]
            };
            let numbered = shown.iter().any(|l| l.number.is_some());
            let mut b = diff_block(shown, numbered, look);
            if cut {
                b = b.child(
                    div()
                        .px(look.px(8.))
                        .py(look.px(2.))
                        .text_size(look.text(11.))
                        .text_color(look.c.text_muted)
                        .child(format!("⌄ {} more lines", lines.len() - INLINE_DIFF_LINES)),
                );
            }
            block = Some(b);
        }
        div()
            .id(SharedString::from(format!("edit-{id}-{entry}")))
            .role(Role::Group)
            .aria_label(format!("{verb} {path}, {added} added, {removed} removed"))
            .ml(look.px(GUTTER - 8.))
            .mr(look.px(-8.))
            .my(look.px(4.))
            .px(look.px(8.))
            .py(look.px(4.))
            .rounded(look.px(4.))
            .border_l(look.fixed(super::theme::RING))
            .border_color(if current {
                look.c.focus
            } else {
                gpui::transparent_black()
            })
            .flex()
            .flex_col()
            .gap(look.px(4.))
            .cursor_pointer()
            .child(header)
            .children(block)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.sessions_ui.current = Some((id_owned.clone(), row));
                this.toggle_row(&id_owned, row);
                window.focus(&this.turns_focus, cx);
                cx.notify();
            }))
            .into_any_element()
    }

    /// The bar above the conversation: the thread's title, and the button
    /// that shows or hides the Changes pane with the session's totals.
    fn render_thread_bar(
        &self,
        id: &str,
        title: &str,
        view: &SessionView,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let c = &view.changes;
        let shown = !c.files.is_empty() && !self.sessions_ui.changes_hidden;
        div()
            .flex_shrink_0()
            .h(look.px(32.))
            .px(look.px(16.))
            .flex()
            .items_center()
            .gap(look.px(12.))
            .border_b(look.fixed(1.))
            .border_color(look.c.border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title.to_string()),
            )
            .when(!id.contains("-agent-"), |d| {
                let id = id.to_string();
                d.child(
                    super::kit::button(look, "open-xray", "X-ray", super::kit::Kind::Plain)
                        .h(look.px(22.))
                        .tooltip(super::tooltip::Tooltip::text(
                            "What context this session was given and used, what it searched for, and whether its claims have evidence",
                        ))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_xray(&id, window, cx);
                        })),
                )
            })
            .when(!c.files.is_empty(), |d| {
                d.child(
                    div()
                        .id("toggle-changes")
                        .role(Role::Button)
                        .aria_label(format!(
                            "{} Changes, {} added, {} removed",
                            if shown { "Hide" } else { "Show" },
                            c.added,
                            c.removed
                        ))
                        .flex()
                        .gap(look.px(6.))
                        .px(look.px(8.))
                        .py(look.px(2.))
                        .rounded(look.px(4.))
                        .cursor_pointer()
                        .text_size(look.text(12.))
                        .when(shown, |d| d.bg(look.c.selected))
                        .hover(|h| h.bg(look.c.hover))
                        .tooltip(super::tooltip::Tooltip::text("Changes (⌘⌥C)"))
                        .child(div().text_color(look.c.text).child("Changes"))
                        .child(counts(c.added, c.removed, look))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_changes();
                            cx.notify();
                        })),
                )
            })
    }

    /// The bar under the conversation: project, branch, the totals, the
    /// model and effort, each when known.
    fn render_session_footer(&self, id: &str, view: &SessionView, look: &Look) -> impl IntoElement {
        let project = self
            .loaded
            .as_ref()
            .and_then(|l| l.about.get(id))
            .and_then(|a| a.project.clone());
        let run = &view.run;
        let mut bar = div()
            .flex_shrink_0()
            .h(look.px(28.))
            .px(look.px(16.))
            .flex()
            .items_center()
            .gap(look.px(12.))
            .border_t(look.fixed(1.))
            .border_color(look.c.border)
            .text_size(look.text(11.))
            .text_color(look.c.text_muted);
        let place = [project.as_deref(), run.branch.as_deref()]
            .into_iter()
            .flatten()
            .map(visible)
            .collect::<Vec<_>>()
            .join(" / ");
        if !place.is_empty() {
            bar = bar.child(div().min_w_0().truncate().child(place));
        }
        if !view.changes.files.is_empty() {
            bar = bar.child(counts(view.changes.added, view.changes.removed, look));
        }
        bar = bar.child(div().flex_1());
        if let Some(effort) = &run.effort {
            bar = bar.child(visible(effort));
        }
        if let Some(model) = &run.model {
            bar = bar.child(visible(model));
        }
        bar
    }

    /// The Changes pane's top: its name, the totals, and where they come
    /// from.
    fn render_pane_header(&self, view: &SessionView, look: &Look) -> impl IntoElement {
        let c = &view.changes;
        div()
            .flex_shrink_0()
            .px(look.px(12.))
            .py(look.px(8.))
            .flex()
            .flex_col()
            .gap(look.px(2.))
            .border_b(look.fixed(1.))
            .border_color(look.c.border)
            .child(
                div()
                    .flex()
                    .gap(look.px(8.))
                    .text_size(look.text(12.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Changes"))
                    .child(counts(c.added, c.removed, look))
                    .child(
                        div()
                            .text_color(look.c.text_muted)
                            .child(match c.files.len() {
                                1 => "1 file".to_string(),
                                n => format!("{n} files"),
                            }),
                    ),
            )
            .child(
                div()
                    .text_size(look.text(11.))
                    .text_color(look.c.text_muted)
                    .child("From this session's edits, not git"),
            )
    }

    /// Row `ix` of the Changes pane: a file's header or one hunk.
    fn render_pane_item(
        &mut self,
        id: &str,
        ix: usize,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let look = Look::from_app(cx);
        let Some(view) = self.session_view(id).cloned() else {
            return div().into_any_element();
        };
        let Some(row) = view.pane.get(ix).copied() else {
            return div().into_any_element();
        };
        match row {
            PaneRow::Index => {
                let mut col = div()
                    .w_full()
                    .overflow_hidden()
                    .px(look.px(12.))
                    .py(look.px(8.))
                    .flex()
                    .flex_col()
                    .gap(look.px(2.))
                    .border_b(look.fixed(1.))
                    .border_color(look.c.border_variant);
                for (f, file) in view.changes.files.iter().enumerate() {
                    let (name, dir) = file.name_and_dir();
                    let target = view.pane.iter().position(|r| *r == PaneRow::File(f));
                    let list = view.pane_list.clone();
                    col = col.child(
                        div()
                            .id(("pane-file", f))
                            .w_full()
                            .role(Role::Link)
                            .aria_label(format!(
                                "{name}, {} added, {} removed",
                                file.added, file.removed
                            ))
                            .flex()
                            .items_center()
                            .gap(look.px(6.))
                            .px(look.px(4.))
                            .mx(look.px(-4.))
                            .rounded(look.px(4.))
                            .cursor_pointer()
                            .hover(|h| h.bg(look.c.hover))
                            .text_size(look.text(12.))
                            .child(div().flex_shrink_0().child(visible(&name)))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(look.c.text_muted)
                                    .child(visible(&dir)),
                            )
                            .child(div().flex_1())
                            .child(counts(file.added, file.removed, &look))
                            .on_click(move |_, _, _| {
                                if let Some(t) = target {
                                    list.scroll_to(gpui::ListOffset {
                                        item_ix: t,
                                        offset_in_item: px(0.),
                                    });
                                }
                            }),
                    );
                }
                col.into_any_element()
            }
            PaneRow::File(f) => {
                let Some(file) = view.changes.files.get(f) else {
                    return div().into_any_element();
                };
                file_header(file, &look).into_any_element()
            }
            PaneRow::Hunk(f, h) => {
                let Some(hunk) = view.changes.files.get(f).and_then(|x| x.hunks.get(h)) else {
                    return div().into_any_element();
                };
                let key = (id.to_string(), f, h);
                let full = self.sessions_ui.pane_full.contains(&key);
                let cut = !full && hunk.lines.len() > PANE_HUNK_LINES;
                let shown = if cut {
                    &hunk.lines[..PANE_HUNK_LINES]
                } else {
                    &hunk.lines[..]
                };
                let mut block = diff_block(shown, true, &look);
                if hunk.lines.len() > PANE_HUNK_LINES {
                    let text = if full {
                        "show less".to_string()
                    } else {
                        format!("show all {} lines", hunk.lines.len())
                    };
                    let id_owned = id.to_string();
                    block = block.child(
                        div()
                            .id(SharedString::from(format!("pane-more-{f}-{h}")))
                            .role(Role::Button)
                            .aria_label(text.clone())
                            .px(look.px(8.))
                            .py(look.px(2.))
                            .cursor_pointer()
                            .text_size(look.text(11.))
                            .text_color(look.c.text_accent)
                            .hover(|h| h.underline())
                            .child(text)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let key = (id_owned.clone(), f, h);
                                if !this.sessions_ui.pane_full.remove(&key) {
                                    this.sessions_ui.pane_full.insert(key);
                                }
                                if let Some(v) = this.session_view(&id_owned) {
                                    v.pane_list.remeasure_items(ix..ix + 1);
                                }
                                cx.notify();
                            })),
                    );
                }
                let when = view
                    .entries
                    .get(hunk.entry)
                    .map(|e| clock(&e.ts))
                    .unwrap_or_default();
                div()
                    .w_full()
                    .overflow_hidden()
                    .px(look.px(12.))
                    .pb(look.px(8.))
                    .flex()
                    .flex_col()
                    .gap(look.px(2.))
                    .child(
                        div()
                            .text_size(look.text(10.))
                            .text_color(look.c.text_muted)
                            .child(if hunk.whole {
                                format!("{when} · whole file written")
                            } else {
                                when
                            }),
                    )
                    .child(block)
                    .into_any_element()
            }
        }
    }
}

/// The person's initial, for their avatar: the login name's first letter.
fn user_initial() -> String {
    std::env::var("USER")
        .ok()
        .and_then(|u| u.chars().next())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "Y".to_string())
}

/// `+6 −4` in the added and removed colours; a side that is zero is left
/// out.
fn counts(added: usize, removed: usize, look: &Look) -> impl IntoElement {
    div()
        .flex()
        .gap(look.px(4.))
        .flex_shrink_0()
        .when(added > 0 || removed == 0, |d| {
            d.child(div().text_color(look.c.created).child(format!("+{added}")))
        })
        .when(removed > 0, |d| {
            d.child(
                div()
                    .text_color(look.c.deleted)
                    .child(format!("−{removed}")),
            )
        })
}

/// A file's header in the Changes pane: its name, its folder, its totals.
fn file_header(file: &FileChange, look: &Look) -> impl IntoElement {
    let (name, dir) = file.name_and_dir();
    div()
        .w_full()
        .overflow_hidden()
        .pt(look.px(12.))
        .pb(look.px(4.))
        .px(look.px(12.))
        .flex()
        .items_center()
        .gap(look.px(6.))
        .text_size(look.text(12.))
        .child(div().flex_shrink_0().child(visible(&name)))
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_color(look.c.text_muted)
                .child(visible(&dir)),
        )
        .child(div().flex_1())
        .child(counts(file.added, file.removed, look))
}

/// Diff lines in the monospace font, added and removed lines on their
/// colours, with a column of line numbers when `numbers`.
fn diff_block(lines: &[DiffLine], numbers: bool, look: &Look) -> gpui::Div {
    let mut block = div()
        .w_full()
        .py(look.px(4.))
        .rounded(look.px(4.))
        .border_1()
        .border_color(look.c.border)
        .bg(look.c.elevated)
        .overflow_hidden()
        .font_family("Menlo")
        .text_size(look.text(12.))
        .flex()
        .flex_col();
    for line in lines {
        let (bg, mark) = match line.kind {
            LineKind::Added => (Some(look.c.created_background), "+"),
            LineKind::Removed => (Some(look.c.deleted_background), "−"),
            LineKind::Context => (None, " "),
        };
        let text = visible(&line.text.replace('\t', "    "));
        block = block.child(
            div()
                .flex()
                .w_full()
                .when_some(bg, |d, bg| d.bg(bg))
                .when(numbers, |d| {
                    d.child(
                        div()
                            .flex_shrink_0()
                            .w(look.px(36.))
                            .pr(look.px(8.))
                            .flex()
                            .justify_end()
                            .text_color(look.c.text_muted)
                            .child(line.number.map(|n| n.to_string()).unwrap_or_default()),
                    )
                })
                .child(
                    div()
                        .flex_shrink_0()
                        .w(look.px(14.))
                        .text_color(look.c.text_muted)
                        .child(mark),
                )
                .child(div().min_w_0().truncate().pr(look.px(8.)).child(text)),
        );
    }
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tool_input_reads_as_keys_and_values() {
        assert_eq!(
            pretty_input(r#"{"a":1,"cmd":"ls\nwc"}"#),
            "a: 1\ncmd:\nls\nwc"
        );
        assert_eq!(pretty_input(r#""{\"b\":\"x\"}""#), "b: x");
        assert_eq!(pretty_input("[1,2]"), "[\n  1,\n  2\n]");
        assert_eq!(pretty_input("plain text"), "plain text");
    }

    #[test]
    fn copying_a_tool_entry_gives_its_input_and_output() {
        let e = Entry {
            ordinal: 0,
            ordinals: vec![0, 1],
            ts: "2026-11-08T10:05:33Z".into(),
            kind: Kind::Tool {
                name: "Bash".into(),
                summary: "ls".into(),
                input: Some("{\"command\":\"ls\"}".into()),
                result: Some("a.rs".into()),
            },
            text: String::new(),
        };
        assert_eq!(entry_text(&e), "{\"command\":\"ls\"}\n\na.rs");
    }

    #[test]
    fn mcp_tools_read_as_server_and_tool() {
        assert_eq!(
            tool_label("mcp__claude_ai_Claude_Docs__update"),
            "Claude_Docs update"
        );
        assert_eq!(
            tool_label("mcp__t3-code__html_render"),
            "t3-code html_render"
        );
        assert_eq!(tool_label("Bash"), "Bash");
    }

    #[test]
    fn the_clock_falls_back_to_the_stored_time() {
        assert_eq!(clock("short"), "short");
        assert_eq!(clock("2026-11-08T10:05:33Zjunk"), "10:05");
    }

    fn steps(entries: Vec<usize>, work: Vec<usize>) -> Item {
        Item::Steps(Steps {
            entries,
            work,
            tools: vec![("Bash".into(), 2)],
            seconds: 5,
        })
    }

    #[test]
    fn groups_open_to_their_work_and_then_their_harness_steps() {
        // A request with an image, a group of three entries (one harness),
        // a reply.
        let items = vec![
            Item::Message {
                entry: 0,
                images: 1,
            },
            steps(vec![2, 3, 4], vec![2, 4]),
            Item::Message {
                entry: 5,
                images: 0,
            },
        ];
        let none = HashSet::new();
        assert_eq!(
            rows_of(&items, &none, &none),
            vec![
                Row::Message { item: 0 },
                Row::Group { item: 1 },
                Row::Message { item: 2 }
            ]
        );
        let open: HashSet<usize> = [1].into();
        assert_eq!(
            rows_of(&items, &open, &none),
            vec![
                Row::Message { item: 0 },
                Row::Group { item: 1 },
                Row::Step { item: 1, entry: 2 },
                Row::Step { item: 1, entry: 4 },
                Row::Harness { item: 1 },
                Row::Message { item: 2 }
            ]
        );
        assert_eq!(rows_of(&items, &open, &open).len(), 7);
        // The image belongs to its request; a step to its group; an entry
        // no item holds goes to the next item.
        assert_eq!(item_of(&items, 1), Some((0, false)));
        assert_eq!(item_of(&items, 3), Some((1, true)));
        assert_eq!(item_of(&items, 5), Some((2, false)));
        assert_eq!(item_of(&items, 9), None);
        let gap = vec![
            Item::Message {
                entry: 0,
                images: 0,
            },
            Item::Message {
                entry: 3,
                images: 0,
            },
        ];
        assert_eq!(item_of(&gap, 1), Some((1, false)));
    }

    #[test]
    fn a_request_reads_without_its_preamble() {
        assert_eq!(
            request_body("Act as the implementation sub-agent for this task.\n\nFix the build."),
            "Fix the build."
        );
        assert_eq!(request_body("<system-reminder>x"), "<system-reminder>x");
    }
}
