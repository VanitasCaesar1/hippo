//! The Inbox: lessons hippo proposed (corrections, repeated failures,
//! rules from Ask) and the overnight dreaming changes, as cards under filter
//! chips, with a detail panel showing what hippo saw and the proposed rule.
//! Keep, Edit and Dismiss run `hippo lessons keep|dismiss` through the CLI.

use std::path::{Path, PathBuf};

use gpui::{Context, Entity, FocusHandle, Focusable, FontWeight, Hsla, Window, div, prelude::*};
use hippo_core::lessons::{Kind as LessonKind, Lesson};
use hippo_view::home::ReviewItem;
use time::OffsetDateTime;
use ulid::Ulid;

use super::Shell;
use super::input::TextInput;
use super::kit::{self, Kind};
use super::theme::Look;
use crate::flow::Purpose;
use crate::state::{Section, Tab};

/// The Inbox's detail pane width.
const DETAIL_W: f32 = 400.;

/// The Inbox's filters (the title bar's segments).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    #[default]
    All,
    Corrections,
    Failures,
    Overnight,
}

impl Filter {
    pub const ALL: [Filter; 4] = [
        Filter::All,
        Filter::Corrections,
        Filter::Failures,
        Filter::Overnight,
    ];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Corrections => "Corrections",
            Filter::Failures => "Repeated failures",
            Filter::Overnight => "Note changes",
        }
    }
}

/// One card of the Inbox.
#[derive(Debug, Clone)]
pub enum Item {
    Lesson(Box<Lesson>),
    /// Two or more lessons of one kind whose rules go to the same note
    /// section: one card, a row each, and Keep all.
    Group(Vec<Lesson>),
    Change(Box<ReviewItem>),
    /// The overnight changes as one row, under All.
    Overnight(Vec<ReviewItem>),
}

impl Item {
    fn key(&self) -> String {
        match self {
            Item::Lesson(l) => l.id.to_string(),
            Item::Group(g) => g.first().map(|l| l.id.to_string()).unwrap_or_default(),
            Item::Change(c) => c.receipt.to_string(),
            Item::Overnight(_) => "overnight".to_owned(),
        }
    }

    /// Whether the card is, or holds, the one keyed `key`.
    fn holds(&self, key: &str) -> bool {
        match self {
            Item::Group(g) => g.iter().any(|l| l.id.to_string() == key),
            other => other.key() == key,
        }
    }
}

/// Lessons in the Inbox's order: corrections, then repeated
/// failures, then the rest; within a kind, seen in the most sessions first,
/// then the newest. Two or more of one kind whose rules append to the same
/// note section become one [`Item::Group`], where the first of them was.
pub fn group_lessons(mut lessons: Vec<Lesson>) -> Vec<Item> {
    let rank = |k: LessonKind| match k {
        LessonKind::Correction => 0,
        LessonKind::Failure => 1,
        _ => 2,
    };
    lessons.sort_by(|a, b| {
        rank(a.kind)
            .cmp(&rank(b.kind))
            .then_with(|| b.seen.sessions.cmp(&a.seen.sessions))
            .then_with(|| b.seen.last.cmp(&a.seen.last))
    });
    let place = |l: &Lesson| {
        l.target
            .as_ref()
            .filter(|t| !t.new)
            .map(|t| (rank(l.kind), t.note.clone(), t.section.clone()))
    };
    let mut out: Vec<Item> = Vec::new();
    let mut taken = vec![false; lessons.len()];
    for i in 0..lessons.len() {
        if taken[i] {
            continue;
        }
        taken[i] = true;
        let mut group = vec![lessons[i].clone()];
        if let Some(p) = place(&lessons[i]) {
            for j in i + 1..lessons.len() {
                if !taken[j] && place(&lessons[j]).as_ref() == Some(&p) {
                    taken[j] = true;
                    group.push(lessons[j].clone());
                }
            }
        }
        out.push(if group.len() > 1 {
            Item::Group(group)
        } else {
            Item::Lesson(Box::new(group.remove(0)))
        });
    }
    out
}

/// What an agent did before a lesson, in words: a tool call
/// (`Name: input`) reads `ran html_render`, `ran cargo test`, `ran Edit
/// on memory.rs`, with the call itself as the second value; a sentence
/// stays as it is.
pub fn action_words(action: &str) -> (String, Option<String>) {
    let a = action.trim_start_matches("agent: ").trim();
    let Some((name, rest)) = a.split_once(": ") else {
        return (a.to_owned(), None);
    };
    let tool_like = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !tool_like {
        return (a.to_owned(), None);
    }
    let short = name.rsplit("__").next().unwrap_or(name);
    let rest = rest.trim();
    let raw = Some(rest.to_owned());
    if rest.starts_with('{') {
        let v: Option<serde_json::Value> = serde_json::from_str(rest).ok();
        let get = |k: &str| {
            v.as_ref()
                .and_then(|v| v.get(k))
                .and_then(|x| x.as_str())
                .map(str::to_owned)
        };
        if let Some(d) = get("description") {
            return (format!("ran {short}: {}", d.trim_end_matches('.')), raw);
        }
        if let Some(f) = get("file_path").or_else(|| get("path")) {
            let file = f
                .rsplit('/')
                .find(|p| !p.is_empty())
                .unwrap_or(&f)
                .to_owned();
            return (format!("ran {short} on {file}"), raw);
        }
        return (format!("ran {short}"), raw);
    }
    if matches!(name, "Bash" | "shell" | "exec_command" | "local_shell") {
        let label = hippo_view::fleet::tail::command_label(rest);
        return (format!("ran {label}"), raw);
    }
    if rest.starts_with('/') || rest.starts_with('~') {
        let file = rest.rsplit('/').find(|p| !p.is_empty()).unwrap_or(rest);
        return (format!("ran {short} on {file}"), raw);
    }
    let shown: String = rest.chars().take(60).collect();
    (
        format!("ran {short}: {shown}"),
        (rest.chars().count() > 60).then(|| rest.to_owned()),
    )
}

/// A lesson's count as a group row shows it: `32 sessions`, `seen 2×`.
pub fn count_words(l: &Lesson) -> String {
    match l.kind {
        LessonKind::Correction if l.seen.sessions > 1 => format!("seen {}×", l.seen.sessions),
        LessonKind::Correction => "seen once".to_owned(),
        _ => format!(
            "{} session{}",
            l.seen.sessions,
            if l.seen.sessions == 1 { "" } else { "s" }
        ),
    }
}

/// The Inbox's own state.
pub struct InboxUi {
    pub filter: Filter,
    /// The selected card, by its key (a lesson or receipt id).
    pub selected: Option<String>,
    /// The lesson whose rule is being edited.
    pub editing: Option<Ulid>,
    pub edit_field: Entity<TextInput>,
    /// The cards' focus, where ↑ ↓ ↩ E ⌫ work.
    pub focus: FocusHandle,
    /// The lesson a running `hippo lessons` command is about.
    pub busy: Option<Ulid>,
    /// The lesson whose evidence shows its raw tool call.
    pub show_call: Option<Ulid>,
    /// The last command's line and result.
    pub result: Option<(String, Result<(), String>)>,
    /// Ask's "Make it a rule" or "Save as note": running, or what came back.
    pub proposing: bool,
    pub proposed: Option<Result<String, String>>,
}

impl InboxUi {
    pub fn new(cx: &mut Context<Shell>) -> InboxUi {
        let edit_field = cx.new(|cx| {
            TextInput::new(
                "lesson-rule",
                "Rule",
                "The rule, as agents will read it",
                cx,
            )
        });
        InboxUi {
            filter: Filter::All,
            selected: None,
            editing: None,
            edit_field,
            focus: cx.focus_handle(),
            busy: None,
            show_call: None,
            result: None,
            proposing: false,
            proposed: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Words

/// `22:41` today, else `Sun 22:41`, or `Fri 31 Oct` beyond a week.
pub fn when(t: OffsetDateTime, now: OffsetDateTime) -> String {
    let offset = super::status::local_offset();
    let (t, now) = (t.to_offset(offset), now.to_offset(offset));
    let hm = format!("{:02}:{:02}", t.hour(), t.minute());
    let day = |t: OffsetDateTime| {
        let w = t.weekday().to_string();
        w[..3].to_owned()
    };
    if t.date() == now.date() {
        hm
    } else if (now.date() - t.date()).whole_days() < 7 {
        format!("{} {hm}", day(t))
    } else {
        format!("{} {} {}", day(t), t.day(), &t.month().to_string()[..3])
    }
}

/// The card's kind line, in sentence case without the date:
/// `Correction · seen 2× · 2 profiles`, `Repeated failure · 32 sessions`.
/// (9b's rows show the parts apart; the tests keep its wording.)
#[cfg_attr(not(test), allow(dead_code))]
pub fn kind_line(l: &Lesson, _now: OffsetDateTime) -> String {
    match l.kind {
        LessonKind::Correction => {
            let mut parts = vec!["Correction".to_owned()];
            // The profile is the mark only.
            if l.seen.sessions > 1 {
                parts.push(format!("seen {}×", l.seen.sessions));
            }
            parts.join(" · ")
        }
        LessonKind::Failure => format!("Repeated failure · {}", count_words(l)),
        LessonKind::AskRule => "From Ask · a rule".to_owned(),
        LessonKind::AskNote => "From Ask · a new note".to_owned(),
        LessonKind::Miss => "From a missed search".to_owned(),
    }
}

/// A note's path as cards show it: below the notes folder, without `.md`,
/// folders joined by ` / `: `Team / Agent rules`.
pub fn note_label(path: &Path, notes_dir: Option<&Path>) -> String {
    let rel = notes_dir
        .and_then(|d| path.strip_prefix(d).ok())
        .unwrap_or(path);
    let rel: PathBuf = rel.with_extension("");
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" / ")
}

/// Where a kept lesson goes: `(verb, place)`, such as `("appends to",
/// "Agent rules › Tools")` or `("creates", "Build limits")`.
pub fn target_words(l: &Lesson, notes_dir: Option<&Path>) -> Option<(&'static str, String)> {
    let t = l.target.as_ref()?;
    let note = note_label(&t.note, notes_dir);
    if t.new {
        return Some(("new note", note));
    }
    Some((
        "appends to",
        match &t.section {
            Some(s) => format!("{note} › {s}"),
            None => note,
        },
    ))
}

/// A kind line's colour (a category is a muted label, never red).
fn kind_color(look: &Look, kind: LessonKind) -> Hsla {
    match kind {
        LessonKind::Correction => look.c.text,
        LessonKind::Failure => look.c.text_muted,
        LessonKind::AskRule | LessonKind::AskNote | LessonKind::Miss => look.c.text_muted,
    }
}

/// A kind line in sentence case.
fn kind_text(look: &Look, text: &str, color: Hsla) -> gpui::Div {
    // Sentence case, never small capitals.
    div()
        .min_w_0()
        .truncate()
        .text_size(look.text(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(color)
        .child(kit::sentence(text))
}

/// A group label row: `Repeated failures 6`, muted, the count
/// faint.
fn group_label(look: &Look, label: &str, n: usize) -> gpui::Div {
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(look.px(6.))
        .text_size(look.text(13.))
        // A section's name reads above its rows' facts (a fresh
        // reader found Home's labels and facts one flat grey).
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(look.c.text)
                .child(label.to_owned()),
        )
        .child(
            div()
                .text_size(look.text(12.))
                .text_color(look.c.faint)
                .child(n.to_string()),
        )
}

/// A profile as a small letter square and its name.
fn profile_tag(look: &Look, profile: Option<&str>, extra: Option<String>) -> gpui::Div {
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(look.px(6.))
        .child(
            div()
                .flex_shrink_0()
                .size(look.px(18.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(look.px(4.))
                .border(look.fixed(1.))
                .border_color(look.c.border)
                .text_size(look.text(10.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if profile == Some("codex") {
                    look.c.focus
                } else {
                    look.c.text
                })
                .child(kit::profile_letter(profile)),
        )
        .child(
            div()
                .text_color(look.c.text_muted)
                .whitespace_nowrap()
                .child(format!(
                    "{}{}",
                    profile.map_or_else(|| "?".to_owned(), kit::profile_name),
                    extra.unwrap_or_default()
                )),
        )
}

// ---------------------------------------------------------------------------
// The Shell's side

impl Shell {
    /// The lessons waiting.
    pub(crate) fn lessons(&self) -> &[Lesson] {
        self.loaded.as_ref().map_or(&[], |l| l.lessons.as_slice())
    }

    fn notes_dir(&self) -> Option<PathBuf> {
        match self.loaded.as_ref().map(|l| &l.setup) {
            Some(hippo_view::setup::Setup::Ready { ws, .. }) => Some(ws.notes_dir.clone()),
            _ => None,
        }
    }

    /// The cards under `filter`: lessons first, newest first, then the
    /// overnight changes.
    pub(crate) fn inbox_items(&self, filter: Filter) -> Vec<Item> {
        let lessons: Vec<Lesson> = self
            .lessons()
            .iter()
            .filter(|l| match filter {
                Filter::All => true,
                Filter::Corrections => l.kind == LessonKind::Correction,
                Filter::Failures => l.kind == LessonKind::Failure,
                Filter::Overnight => false,
            })
            .cloned()
            .collect();
        let mut out = group_lessons(lessons);
        let changes = self.review_items();
        match filter {
            // Under All, the overnight changes are one row.
            Filter::All if changes.len() > 1 => out.push(Item::Overnight(changes)),
            Filter::All | Filter::Overnight => {
                out.extend(changes.into_iter().map(|c| Item::Change(Box::new(c))))
            }
            _ => {}
        }
        out
    }

    /// The lessons and changes under `filter`, each counted once.
    pub(crate) fn count(&self, filter: Filter) -> usize {
        self.inbox_items(filter)
            .iter()
            .map(|i| match i {
                Item::Group(g) => g.len(),
                Item::Overnight(c) => c.len(),
                _ => 1,
            })
            .sum()
    }

    /// Runs `hippo lessons keep <id>` once per lesson, in order:
    /// each kept rule is its own receipt, revertible alone. The run stops
    /// at the first failure, which its result names.
    pub(crate) fn keep_all(&mut self, ids: Vec<Ulid>, cx: &mut Context<Self>) {
        if self.lesson_job_running() || ids.is_empty() {
            return;
        }
        if let Some(last) = ids.last() {
            self.select_after(*last);
        }
        let commands = ids
            .iter()
            .map(|id| vec!["lessons".to_owned(), "keep".to_owned(), id.to_string()])
            .collect();
        self.inbox.busy = ids.first().copied();
        self.inbox.result = None;
        self.flow.log.clear();
        self.start_job(Purpose::Lesson, commands, cx);
    }

    /// The selected card's index in `items`; none when nothing is chosen or
    /// the chosen one has gone.
    fn inbox_index(&self, items: &[Item]) -> Option<usize> {
        if items.is_empty() {
            return None;
        }
        // Nothing is selected until the person picks or moves:
        // a card selected by default would let a stray Return keep a lesson.
        self.inbox
            .selected
            .as_ref()
            .and_then(|k| items.iter().position(|i| i.holds(k)))
    }

    pub(crate) fn lesson_job_running(&self) -> bool {
        self.flow.job.as_ref().is_some_and(|j| {
            matches!(
                j.purpose,
                Purpose::Lesson
                    | Purpose::Propose
                    | Purpose::Review
                    | Purpose::Miss
                    | Purpose::Tune
            ) && j.result.lock().map(|r| r.is_none()).unwrap_or(false)
        })
    }

    /// Selects the card after lesson `id` (else the one before), which
    /// stays selected when `id` leaves the Inbox.
    fn select_after(&mut self, id: Ulid) {
        let items = self.inbox_items(self.inbox.filter);
        let key = id.to_string();
        if let Some(at) = items.iter().position(|i| i.holds(&key)) {
            let next = items
                .get(at + 1)
                .or_else(|| at.checked_sub(1).and_then(|p| items.get(p)));
            if self.inbox.selected.as_deref().is_none_or(|s| s == key) {
                self.inbox.selected = next.map(Item::key);
            }
        }
    }

    /// Runs `hippo lessons keep <id> [--text …]`.
    pub(crate) fn keep_lesson(&mut self, id: Ulid, text: Option<String>, cx: &mut Context<Self>) {
        if self.lesson_job_running() {
            return;
        }
        self.select_after(id);
        let mut args = vec!["lessons".to_owned(), "keep".to_owned(), id.to_string()];
        if let Some(t) = text {
            args.push("--text".to_owned());
            args.push(t);
        }
        self.inbox.busy = Some(id);
        self.inbox.result = None;
        self.flow.log.clear();
        self.start_job(Purpose::Lesson, vec![args], cx);
    }

    /// Runs `hippo lessons dismiss <id>`.
    pub(crate) fn dismiss_lesson(&mut self, id: Ulid, cx: &mut Context<Self>) {
        if self.lesson_job_running() {
            return;
        }
        self.select_after(id);
        self.inbox.busy = Some(id);
        self.inbox.result = None;
        self.flow.log.clear();
        self.start_job(
            Purpose::Lesson,
            vec![vec![
                "lessons".to_owned(),
                "dismiss".to_owned(),
                id.to_string(),
            ]],
            cx,
        );
    }

    /// Opens the rule of lesson `id` for editing, in the Inbox.
    pub(crate) fn edit_lesson(&mut self, id: Ulid, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rule) = self
            .lessons()
            .iter()
            .find(|l| l.id == id)
            .map(|l| l.rule.clone())
        else {
            return;
        };
        if self.state.tabs.section() != Section::Inbox {
            self.state.tabs.select(Section::Inbox);
        }
        self.inbox.selected = Some(id.to_string());
        self.inbox.editing = Some(id);
        let field = self.inbox.edit_field.clone();
        field.update(cx, |f, cx| f.set_text(&rule, window, cx));
        window.focus(&field.focus_handle(cx), cx);
        cx.notify();
    }

    /// Saves the edited rule and keeps the lesson (⌘↩).
    pub(crate) fn save_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.inbox.editing else {
            return;
        };
        let text = self.inbox.edit_field.read(cx).text().trim().to_owned();
        if text.is_empty() {
            return;
        }
        self.inbox.editing = None;
        window.focus(&self.inbox.focus, cx);
        self.keep_lesson(id, Some(text), cx);
    }

    /// Leaves the edit without keeping (Escape).
    pub(crate) fn cancel_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.inbox.editing = None;
        window.focus(&self.inbox.focus, cx);
        cx.notify();
    }

    /// ↑ ↓ in the Inbox; inside a group, through its rules first.
    pub(crate) fn step_inbox(&mut self, by: i32) {
        let items = self.inbox_items(self.inbox.filter);
        if items.is_empty() {
            return;
        }
        let Some(at) = self.inbox_index(&items) else {
            // The first move selects the first card (↓) or the last (↑).
            let first = if by < 0 { items.len() - 1 } else { 0 };
            self.inbox.selected = Some(items[first].key());
            if let (true, Item::Group(g)) = (by < 0, &items[first]) {
                self.inbox.selected = g.last().map(|l| l.id.to_string());
            }
            return;
        };
        self.inbox.editing = None;
        if let Item::Group(g) = &items[at] {
            let pos = self
                .inbox
                .selected
                .as_ref()
                .and_then(|k| g.iter().position(|l| &l.id.to_string() == k))
                .unwrap_or(0);
            let q = pos as i32 + by;
            if q >= 0 && (q as usize) < g.len() {
                self.inbox.selected = Some(g[q as usize].id.to_string());
                return;
            }
        }
        let next = (at as i32 + by).clamp(0, items.len() as i32 - 1) as usize;
        self.inbox.selected = Some(items[next].key());
        if let (true, Item::Group(g)) = (by < 0, &items[next]) {
            self.inbox.selected = g.last().map(|l| l.id.to_string());
        }
    }

    /// ↩, E and ⌫ on the selected card.
    pub(crate) fn act_on_selected(
        &mut self,
        act: char,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items = self.inbox_items(self.inbox.filter);
        let Some(at) = self.inbox_index(&items) else {
            return;
        };
        // In a group, the selected rule (else its first).
        let member = |g: &Vec<Lesson>| {
            self.inbox
                .selected
                .as_ref()
                .and_then(|k| g.iter().find(|l| &l.id.to_string() == k))
                .or_else(|| g.first())
                .map(|l| l.id)
        };
        match (&items[at], act) {
            (Item::Group(g), 'k') => {
                if let Some(id) = member(g) {
                    self.keep_lesson(id, None, cx)
                }
            }
            (Item::Group(g), 'e') => {
                if let Some(id) = member(g) {
                    self.edit_lesson(id, window, cx)
                }
            }
            (Item::Group(g), 'd') => {
                if let Some(id) = member(g) {
                    self.dismiss_lesson(id, cx)
                }
            }
            (Item::Overnight(_), 'k' | 'd') => {
                self.inbox.filter = Filter::Overnight;
                self.inbox.selected = None;
            }
            (Item::Lesson(l), 'k') => self.keep_lesson(l.id, None, cx),
            (Item::Lesson(l), 'e') => self.edit_lesson(l.id, window, cx),
            (Item::Lesson(l), 'd') => self.dismiss_lesson(l.id, cx),
            (Item::Change(c), 'k') => self.mark_right(vec![c.receipt.to_string()], cx),
            (Item::Change(c), 'd') if c.undo.is_some() => {
                self.state.tabs.open(Tab::Proposal(c.proposal));
            }
            _ => {}
        }
    }

    /// Ask's "Make it a rule" (`note` false) or "Save as note".
    pub(crate) fn propose_from_ask(&mut self, note: bool, cx: &mut Context<Self>) {
        if self.lesson_job_running() {
            return;
        }
        let Some(Ok((Some(answer), _))) = &self.ask.answer else {
            return;
        };
        let mut args = vec![
            "lessons".to_owned(),
            "propose".to_owned(),
            "--question".to_owned(),
            self.ask.question.clone(),
            "--answer".to_owned(),
            answer.text.clone(),
        ];
        if note {
            args.push("--note".to_owned());
        }
        self.inbox.proposing = true;
        self.inbox.proposed = None;
        self.flow.log.clear();
        self.start_job(Purpose::Propose, vec![args], cx);
    }

    /// What a lesson job printed last (not its command line).
    pub(crate) fn last_output(&self) -> Option<String> {
        self.flow
            .log
            .iter()
            .rev()
            .find(|l| !l.starts_with('$') && !l.trim().is_empty())
            .cloned()
    }

    /// When the lessons pass last ran, for the Inbox's `ⓘ`.
    pub(crate) fn lessons_caveat(&self) -> String {
        let scan = self.loaded.as_ref().and_then(|l| l.lessons_state.as_ref());
        match scan {
            Some(s) if s.error.is_some() => format!(
                "The last lessons pass failed: {}",
                s.error.clone().unwrap_or_default()
            ),
            Some(s) => match s.at {
                Some(at) => format!(
                    "The lessons pass ran {} and read {} sessions. A kept rule reaches every agent at its next session start.",
                    kit::hm(at),
                    s.sessions
                ),
                None => "The lessons pass runs after dreaming each night.".to_owned(),
            },
            None => "The lessons pass runs after dreaming each night.".to_owned(),
        }
    }

    // ---- rendering ----------------------------------------------------

    /// What the last Keep or Dismiss did.
    pub(super) fn lesson_result(&self, look: &Look) -> Option<gpui::Div> {
        let (line, r) = self.inbox.result.as_ref()?;
        let (text, err) = match r {
            Ok(()) => (line.clone(), false),
            Err(e) => (e.clone(), true),
        };
        Some(
            div()
                .pt(look.px(4.))
                .font_family("Menlo")
                .text_size(look.text(11.))
                .text_color(if err { look.c.error } else { kit::dim(look) })
                .child(text),
        )
    }

    /// The Inbox.
    pub(super) fn render_lessons_inbox(
        &mut self,
        look: &Look,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let now = OffsetDateTime::now_utc();
        let items = self.inbox_items(self.inbox.filter);
        let selected = self.inbox_index(&items);
        let pane = f32::from(window.viewport_size().width) - self.sidebar_width(window);
        // The list and a 400-point detail pane side by side from
        // 900 points of pane; below that the detail opens under its row.
        let wide = pane >= 900.;
        let notes_dir = self.notes_dir();

        let mut rows: Vec<gpui::AnyElement> = Vec::new();
        let mut singles_head: Option<LessonKind> = None;
        for (k, item) in items.iter().enumerate() {
            let on = Some(k) == selected;
            let row = match item {
                Item::Lesson(l) => {
                    if singles_head != Some(l.kind) {
                        singles_head = Some(l.kind);
                        let n = items
                            .iter()
                            .filter(|i| matches!(i, Item::Lesson(x) if x.kind == l.kind))
                            .count();
                        let (label, gloss) = match l.kind {
                            LessonKind::Correction => {
                                ("Corrections", "you corrected an agent; keep it as a rule")
                            }
                            LessonKind::Failure => {
                                ("Repeated failures", "the same error in several sessions")
                            }
                            _ => ("Rules", "from Ask"),
                        };
                        rows.push(
                            div()
                                .flex()
                                .items_center()
                                .gap(look.px(8.))
                                .h(look.px(30.))
                                .px(look.px(8.))
                                .child(group_label(look, label, n))
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(look.text(12.))
                                        .text_color(look.c.faint)
                                        .child(gloss),
                                )
                                .into_any_element(),
                        );
                    }
                    self.lesson_card(k, l, on, now, notes_dir.as_deref(), look, cx)
                }
                Item::Group(g) => {
                    singles_head = None;
                    self.group_card(k, g, on, notes_dir.as_deref(), true, look, cx)
                }
                Item::Change(c) => self.change_card(k, c, on, look, cx),
                Item::Overnight(c) => self.overnight_card(k, c, on, true, look, cx),
            };
            rows.push(div().pt(look.px(4.)).child(row).into_any_element());
            if on && !wide {
                rows.push(self.render_detail(
                    &items[k],
                    now,
                    notes_dir.as_deref(),
                    false,
                    look,
                    cx,
                ));
            }
        }
        if items.is_empty() {
            rows.push(
                div()
                    .px(look.px(8.))
                    .py(look.px(12.))
                    .text_color(look.c.text_muted)
                    .child(match self.inbox.filter {
                        Filter::Overnight => "No note change waiting: every change dreaming made has been marked.".to_owned(),
                        _ => {
                            let scan = self.loaded.as_ref().and_then(|l| l.lessons_state.as_ref());
                            match scan.and_then(|s| s.at) {
                                Some(at) => format!(
                                    "Nothing waiting. The lessons pass last ran {} over {} sessions.",
                                    when(at, now),
                                    scan.map_or(0, |s| s.sessions)
                                ),
                                None => "Nothing waiting. The lessons pass runs after dreaming each night (hippo lessons scan runs it now).".to_owned(),
                            }
                        }
                    })
                    .into_any_element(),
            );
        }
        let result = self.lesson_result(look);
        let focus = self.inbox.focus.clone();
        let list = div()
            .id("inbox-cards")
            .key_context("Inbox")
            .track_focus(&focus)
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .track_scroll(&self.page_scroll)
            .pb(look.px(16.))
            .children(rows)
            .children(result)
            .child(div().pt(look.px(16.)).child(self.runs_card(look, cx)));

        let detail = wide.then(|| {
            let body = match selected {
                Some(k) => self.render_detail(&items[k], now, notes_dir.as_deref(), true, look, cx),
                None => kit::card(look)
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(look.px(8.))
                    .p(look.px(24.))
                    .text_color(look.c.text_muted)
                    .child(kit::icon(look, "inbox", 20., look.c.faint))
                    .child(div().text_color(look.c.text).child(if items.is_empty() {
                        "Nothing to review"
                    } else {
                        "Pick a card"
                    }))
                    .when(!items.is_empty(), |d| {
                        d.child(
                            div()
                                .text_size(look.text(12.5))
                                .text_align(gpui::TextAlign::Center)
                                .child("Click one, or press ↓ for the first. What hippo saw, the proposed rule and Keep show here."),
                        )
                    })
                    .child(
                        div()
                            .pt(look.px(8.))
                            .text_size(look.text(11.5))
                            .text_color(look.c.faint)
                            .text_align(gpui::TextAlign::Center)
                            .child(self.lessons_caveat()),
                    )
                    .into_any_element(),
            };
            div()
                .flex_shrink_0()
                .w(look.px(DETAIL_W))
                .h_full()
                .pb(look.px(16.))
                .child(body)
        });
        div()
            .id("inbox")
            .size_full()
            .flex()
            .gap(look.px(16.))
            .pt(look.px(6.))
            .px(look.px(16.))
            .text_size(look.text(13.))
            .child(list)
            .children(detail)
            .into_any_element()
    }

    /// One correction or rule as a three-line row: the account
    /// mark, the title and when; the quote; where the rule goes. A click
    /// selects it (its detail shows at the right).
    #[allow(clippy::too_many_arguments)]
    fn lesson_card(
        &self,
        k: usize,
        l: &Lesson,
        on: bool,
        now: OffsetDateTime,
        notes_dir: Option<&Path>,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let id = l.id;
        let newest = l.evidence.first();
        let failure = l.kind == LessonKind::Failure;
        let profile = newest
            .and_then(|e| e.profile.clone())
            .or_else(|| l.seen.profiles.first().cloned());
        let target = target_words(l, notes_dir).map(|(verb, place)| {
            div()
                .flex()
                .items_center()
                .gap(look.px(5.))
                .min_w_0()
                .text_size(look.text(12.))
                .text_color(look.c.faint)
                .child(kit::icon(
                    look,
                    if verb == "new note" {
                        "file-plus"
                    } else {
                        "arrow-right"
                    },
                    12.,
                    look.c.faint,
                ))
                .child(div().min_w_0().truncate().child(if verb == "new note" {
                    format!("new note · {place}")
                } else {
                    place
                }))
        });
        let key = id.to_string();
        let row = div()
            .id(("lesson-card", k))
            .flex()
            .items_start()
            .gap(look.px(10.))
            .px(look.px(8.))
            .py(look.px(8.))
            .rounded(look.px(8.))
            .cursor_pointer()
            .child(kit::mark(look, profile.as_deref(), 22.).mt(look.px(1.)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(look.px(2.))
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(look.px(8.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(look.text(13.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(look.c.text)
                                    .child(kit::title(&l.title)),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(look.text(12.))
                                    .text_color(look.c.faint)
                                    .child(match newest {
                                        Some(e) => when(e.at, now),
                                        None => count_words(l),
                                    }),
                            ),
                    )
                    .children(newest.map(|e| {
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(look.text(12.5))
                            .text_color(look.c.text_muted)
                            .when(failure, |d| {
                                d.font_family(kit::MONO).text_size(look.text(11.5))
                            })
                            .child(if failure {
                                e.quote.clone()
                            } else {
                                format!("“{}”", e.quote)
                            })
                    }))
                    .children(target),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.inbox.selected = Some(key.clone());
                if this.inbox.editing.is_some_and(|e| e.to_string() != key) {
                    this.inbox.editing = None;
                }
                window.focus(&this.inbox.focus, cx);
                cx.notify();
            }));
        kit::row_fill(row, look, on).into_any_element()
    }

    /// A group of lessons whose rules go to one note section:
    /// the kind and target once, a row per rule with ✓ ✎ ✕, and Keep all.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn group_card(
        &self,
        k: usize,
        g: &[Lesson],
        on: bool,
        notes_dir: Option<&Path>,
        home: bool,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(first) = g.first() else {
            return div().into_any_element();
        };
        let ids: Vec<Ulid> = g.iter().map(|l| l.id).collect();
        let busy = self.lesson_job_running() && self.inbox.busy.is_some_and(|b| ids.contains(&b));
        let kind = match first.kind {
            LessonKind::Correction => "Corrections",
            LessonKind::Failure => "Repeated failures",
            _ => "Rules",
        };
        let place = target_words(first, notes_dir)
            .map(|(_, p)| p)
            .unwrap_or_default();
        let n = g.len();
        let all = ids.clone();
        let head = div()
            .flex()
            .items_center()
            .gap(look.px(10.))
            .min_w_0()
            .h(look.px(34.))
            .px(look.px(8.))
            .child(group_label(look, kind, n))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(4.))
                    .flex_1()
                    .min_w_0()
                    .text_size(look.text(12.))
                    .text_color(look.c.faint)
                    .child(kit::icon(look, "arrow-right", 12., look.c.faint))
                    .child(div().min_w_0().truncate().child(place)),
            )
            .child(
                kit::button(
                    look,
                    ("keep-all", k + if home { 500 } else { 0 }),
                    if busy { "…".to_owned() } else { format!("Keep all {n}") },
                    if on { Kind::Primary } else { Kind::Plain },
                )
                .tooltip(super::tooltip::Tooltip::text(
                    "Keeps each rule in turn (hippo lessons keep, once per rule): each is its own change, revertible alone.",
                ))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.keep_all(all.clone(), cx);
                })),
            );
        let selected = self.inbox.selected.clone();
        let rows: Vec<gpui::AnyElement> = g
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let sel = on && !home && selected.as_deref() == Some(l.id.to_string().as_str());
                let base = (k * 100 + i) + if home { 50_000 } else { 0 };
                self.lesson_row(l, base, true, sel, look, cx)
            })
            .collect();
        let body = div()
            .flex()
            .flex_col()
            .gap(look.px(if home { 0. } else { 8. }))
            .child(head)
            .child(div().flex().flex_col().children(rows));
        if home {
            return div()
                .id(("home-group", k))
                .pt(look.px(6.))
                .child(body)
                .into_any_element();
        }
        kit::card(look)
            .id(("group-card", k))
            .p(look.px(14.))
            .when(on, |d| d.border_color(look.c.text_accent.opacity(0.7)))
            .child(body)
            .into_any_element()
    }

    /// The overnight changes as one row: how many, that a fresh
    /// reviewer confirmed each, All look right and Review.
    pub(super) fn overnight_card(
        &self,
        k: usize,
        changes: &[ReviewItem],
        on: bool,
        home: bool,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let n = changes.len();
        let receipts: Vec<String> = changes.iter().map(|c| c.receipt.to_string()).collect();
        let notes: Vec<String> = changes.iter().map(|c| c.label.clone()).collect();
        // The head and its buttons on one row, the notes under
        // them across the whole width (an explanation wraps, never cut).
        let head = div()
            .flex()
            .items_center()
            .gap(look.px(10.))
            .min_w_0()
            .child(
                div().flex_1().min_w_0().child(kind_text(
                    look,
                    &format!(
                        "{n} note change{} · fresh reviewer confirmed",
                        if n == 1 { "" } else { "s" }
                    ),
                    look.c.text_muted,
                )),
            )
            .child(
                kit::button(
                    look,
                    ("all-right", k + if home { 500 } else { 0 }),
                    if self.reviewing_now() { "…" } else { "All look right" },
                    Kind::Plain,
                )
                .tooltip(super::tooltip::Tooltip::text(
                    "Marks every overnight change as looked at (hippo review <receipts> --ok). Undo stays in each change's page.",
                ))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.mark_right(receipts.clone(), cx);
                })),
            )
            .child(
                kit::button(look, ("review-all", k + if home { 500 } else { 0 }), "Review", Kind::Plain)
                    .on_click(cx.listener(|this, _, window, cx| {
                        cx.stop_propagation();
                        this.inbox.filter = Filter::Overnight;
                        this.inbox.selected = None;
                        this.show_section(Section::Inbox, window, cx);
                    })),
            );
        let row = div()
            .flex()
            .flex_col()
            .gap(look.px(4.))
            .min_w_0()
            .child(head)
            .child(
                div()
                    .min_w_0()
                    .text_size(look.text(12.5))
                    .text_color(look.c.text_muted)
                    .child(notes.join(" · ")),
            );
        if home {
            // `Note changes 4`, the reviewer chip, the two buttons,
            // then the notes as outline chips on one line.
            let receipts = changes
                .iter()
                .map(|c| c.receipt.to_string())
                .collect::<Vec<_>>();
            // The chips wrap onto a hidden second line, so one
            // that does not fit drops whole instead of being cut mid-word;
            // a single chip too long for the line ends in an ellipsis.
            let mut chips = div()
                .flex()
                .flex_wrap()
                .gap_x(look.px(6.))
                .gap_y(look.px(8.))
                .h(look.px(24.))
                .mx(look.px(8.))
                .overflow_hidden()
                .min_w_0();
            for (i, note) in notes.iter().enumerate() {
                chips = chips.child(
                    div()
                        .flex()
                        .max_w_full()
                        .items_center()
                        .gap(look.px(5.))
                        .h(look.px(24.))
                        .px(look.px(8.))
                        .rounded_full()
                        .border(look.fixed(1.))
                        .border_color(look.c.border_strong)
                        .text_size(look.text(12.5))
                        .text_color(look.c.text)
                        .id(("home-note-chip", i))
                        .child(kit::icon(look, "file-text", 12., look.c.text_muted))
                        .child(div().min_w_0().truncate().child(note.clone())),
                );
            }
            return div()
                .id(("home-overnight", k))
                .flex()
                .flex_col()
                .gap(look.px(4.))
                .pt(look.px(4.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(look.px(8.))
                        .h(look.px(34.))
                        .px(look.px(8.))
                        .child(group_label(look, "Note changes", n))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(look.px(4.))
                                .h(look.px(20.))
                                .px(look.px(7.))
                                .rounded_full()
                                .bg(look.c.ok.opacity(if look.c.dark { 0.12 } else { 0.09 }))
                                .text_color(look.c.ok)
                                .text_size(look.text(11.5))
                                .font_weight(FontWeight::MEDIUM)
                                .child(kit::icon(look, "shield-check", 12., look.c.ok))
                                .child("fresh reviewer confirmed"),
                        )
                        .child(div().flex_1())
                        .child(
                            kit::button(
                                look,
                                ("all-right", k + 500),
                                if self.reviewing_now() { "…" } else { "All look right" },
                                Kind::Plain,
                            )
                            .h(look.px(26.))
                            .tooltip(super::tooltip::Tooltip::text(
                                "Marks every overnight change as looked at (hippo review <receipts> --ok). Undo stays in each change's page.",
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.mark_right(receipts.clone(), cx);
                            })),
                        )
                        .child(
                            kit::button(look, ("review-all", k + 500), "Review", Kind::Ghost)
                                .h(look.px(26.))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.inbox.filter = Filter::Overnight;
                                    this.inbox.selected = None;
                                    this.show_section(Section::Inbox, window, cx);
                                })),
                        ),
                )
                .child(chips)
                .into_any_element();
        }
        kit::card(look)
            .id(("overnight-card", k))
            .p(look.px(14.))
            .when(on, |d| d.border_color(look.c.text_accent.opacity(0.7)))
            .child(row)
            .into_any_element()
    }

    fn change_card(
        &self,
        k: usize,
        c: &ReviewItem,
        on: bool,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let key = c.receipt.to_string();
        let width = self.review_width.clone();
        kit::card(look)
            .id(("change-card", k))
            .p(look.px(14.))
            .gap(look.px(4.))
            .cursor_pointer()
            .when(on, |d| d.border_color(look.c.text_accent.opacity(0.7)))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.inbox.selected = Some(key.clone());
                window.focus(&this.inbox.focus, cx);
                cx.notify();
            }))
            .child(kit::label(
                look,
                "Overnight · dreaming · confirmed by a fresh reviewer",
                kit::ok(look),
            ))
            .child(self.render_review_rows(std::slice::from_ref(c), &width, false, look, cx))
            .into_any_element()
    }

    /// The selected card's detail: what hippo saw, and the proposed rule.
    pub(super) fn render_detail(
        &self,
        item: &Item,
        now: OffsetDateTime,
        notes_dir: Option<&Path>,
        panel: bool,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let body = match item {
            Item::Lesson(l) => self.lesson_detail(l, now, notes_dir, look, cx),
            Item::Group(g) => {
                // The selected rule's detail, else the first's.
                let l = self
                    .inbox
                    .selected
                    .as_ref()
                    .and_then(|k| g.iter().find(|l| &l.id.to_string() == k))
                    .or_else(|| g.first());
                match l {
                    Some(l) => self.lesson_detail(l, now, notes_dir, look, cx),
                    None => div().into_any_element(),
                }
            }
            Item::Change(c) => self.change_detail(c, look, cx),
            Item::Overnight(c) => match c.first() {
                Some(first) => self.change_detail(first, look, cx),
                None => div().into_any_element(),
            },
        };
        // Keep, Edit and Dismiss sit at the pane's foot.
        let lesson: Option<&Lesson> = match item {
            Item::Lesson(l) => Some(&**l),
            Item::Group(g) => self
                .inbox
                .selected
                .as_ref()
                .and_then(|k| g.iter().find(|l| &l.id.to_string() == k))
                .or_else(|| g.first()),
            _ => None,
        };
        let foot = lesson
            .filter(|l| self.inbox.editing != Some(l.id))
            .map(|l| self.lesson_actions(l, look, cx));
        kit::card(look)
            .id("inbox-detail")
            .flex()
            .flex_col()
            .when(panel, |d| d.size_full())
            .child(
                div()
                    .id("inbox-detail-body")
                    .flex_1()
                    .min_h_0()
                    .p(look.px(16.))
                    .when(panel, |d| d.overflow_y_scroll())
                    .child(body),
            )
            .children(foot.map(|f| {
                div()
                    .flex_shrink_0()
                    .px(look.px(14.))
                    .py(look.px(12.))
                    .border_t(look.fixed(1.))
                    .border_color(look.c.border)
                    .child(f)
            }))
            .into_any_element()
    }

    fn lesson_detail(
        &self,
        l: &Lesson,
        now: OffsetDateTime,
        notes_dir: Option<&Path>,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let id = l.id;
        let color = kind_color(look, l.kind);
        let failure = l.kind == LessonKind::Failure;
        let what = match l.kind {
            LessonKind::Correction => "Correction · what hippo saw",
            LessonKind::Failure => "Repeated failure · what hippo saw",
            _ => "From Ask · what it drew on",
        };
        let titles = self.loaded.as_ref().map(|x| &x.titles);
        let newest = l.evidence.first();
        let saw = newest.map(|e| {
            let root = e
                .session
                .split("-agent-")
                .next()
                .unwrap_or(&e.session)
                .to_owned();
            let title = titles.and_then(|t| t.get(&root)).cloned();
            let meta = [
                title,
                Some(when(e.at, now)),
                Some(format!("turn {}", e.ordinal)),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
            div()
                .flex()
                .flex_col()
                .gap(look.px(8.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(look.px(8.))
                        .min_w_0()
                        .child(profile_tag(look, e.profile.as_deref(), None))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(kit::dim(look))
                                .child(meta),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(look.px(6.))
                        .pl(look.px(12.))
                        .border_l(look.fixed(2.))
                        .border_color(look.c.border)
                        // A tool call in words, the call itself
                        // behind `show call ▸`.
                        .children(e.action.as_ref().map(|a| {
                            let (words, raw) = action_words(a);
                            let shown = self.inbox.show_call == Some(id);
                            div()
                                .flex()
                                .flex_col()
                                .gap(look.px(4.))
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .gap(look.px(6.))
                                        .child(
                                            div()
                                                .flex_shrink_0()
                                                .text_color(look.c.text_muted)
                                                .child("agent"),
                                        )
                                        .child(div().min_w_0().text_color(look.c.text).child(words))
                                        .when(raw.is_some(), |d| {
                                            d.child(
                                                kit::link(
                                                    look,
                                                    "show-call",
                                                    if shown {
                                                        "hide call ▾"
                                                    } else {
                                                        "show call ▸"
                                                    },
                                                )
                                                .text_color(look.c.text_muted)
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.inbox.show_call =
                                                        if this.inbox.show_call == Some(id) {
                                                            None
                                                        } else {
                                                            Some(id)
                                                        };
                                                    cx.notify();
                                                })),
                                            )
                                        }),
                                )
                                .when_some(raw.filter(|_| shown), |d, raw| {
                                    d.child(
                                        div()
                                            .min_w_0()
                                            .font_family("Menlo")
                                            .text_size(look.text(12.))
                                            .text_color(look.c.text_muted)
                                            .child(raw),
                                    )
                                })
                        }))
                        .child(
                            div()
                                .flex()
                                .gap(look.px(6.))
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .text_color(look.c.text_muted)
                                        .child(if failure { "error" } else { "you" }),
                                )
                                .child(
                                    div()
                                        .min_w_0()
                                        .text_color(look.c.text)
                                        .when(failure, |d| {
                                            d.font_family("Menlo").text_size(look.text(12.))
                                        })
                                        .when(!failure, |d| d.italic())
                                        .child(if failure {
                                            e.quote.clone()
                                        } else {
                                            format!("“{}”", e.quote)
                                        }),
                                ),
                        ),
                )
        });
        let earlier = l.evidence.iter().skip(1).take(5).map(|e| {
            div()
                .min_w_0()
                .truncate()
                .text_color(kit::dim(look))
                .child(format!(
                    "also {} · {} · {}",
                    e.profile
                        .as_deref()
                        .map_or_else(|| "?".to_owned(), kit::profile_name),
                    when(e.at, now),
                    if failure {
                        e.action.clone().unwrap_or_default()
                    } else {
                        format!("“{}”", e.quote)
                    }
                ))
        });
        let editing = self.inbox.editing == Some(id);
        let rule_box: gpui::AnyElement = if editing {
            let text = self.inbox.edit_field.read(cx).text().to_owned();
            div()
                .flex()
                .flex_col()
                .gap(look.px(8.))
                .child(
                    div()
                        .key_context("LessonEdit")
                        .overflow_hidden()
                        .p(look.px(8.))
                        .rounded(look.px(8.))
                        .border(look.fixed(1.))
                        .border_color(look.c.text_accent)
                        .child(self.inbox.edit_field.clone()),
                )
                .child(
                    div()
                        .p(look.px(12.))
                        .rounded(look.px(8.))
                        .bg(look.c.window.opacity(0.5))
                        .text_color(look.c.text_muted)
                        .child(text),
                )
                .child(
                    div()
                        .flex()
                        .gap(look.px(6.))
                        .child(
                            kit::button(look, "lesson-save", "Save and keep  ⌘↩", Kind::Primary)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.save_edit(window, cx)),
                                ),
                        )
                        .child(
                            kit::button(look, "lesson-cancel", "Cancel  esc", Kind::Plain)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.cancel_edit(window, cx)),
                                ),
                        ),
                )
                .into_any_element()
        } else {
            div()
                .p(look.px(14.))
                .rounded(look.px(8.))
                .border(look.fixed(1.))
                .border_color(look.c.border)
                .text_size(look.text(14.))
                .text_color(look.c.text)
                .child(l.rule.clone())
                .into_any_element()
        };
        let target = target_words(l, notes_dir).map(|(verb, place)| {
            let verb = if verb == "new note" {
                "creates the note"
            } else {
                verb
            };
            div()
                .flex()
                .items_center()
                .gap(look.px(6.))
                .min_w_0()
                .child(kit::icon(look, "note", 14., look.c.text_muted))
                .child(
                    div()
                        .flex_shrink_0()
                        .text_color(look.c.text_muted)
                        .child(verb),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(look.c.text)
                        .child(place),
                )
        });
        let review = l.review.as_ref().map(|r| {
            div()
                .text_size(look.text(12.))
                .text_color(kit::dim(look))
                .child(format!("A fresh reviewer confirmed it: {}", r.notes))
        });
        let session = newest.map(|e| {
            e.session
                .split("-agent-")
                .next()
                .unwrap_or(&e.session)
                .to_owned()
        });
        let ordinal = newest.map(|e| e.ordinal);
        let _ = (what, color);
        let kind = match l.kind {
            LessonKind::Correction => "Correction",
            LessonKind::Failure => "Repeated failure",
            _ => "From Ask",
        };
        let head = div()
            .flex()
            .items_center()
            .gap(look.px(8.))
            .child(kit::kind_chip(look, kind))
            .child(
                div()
                    .text_size(look.text(12.))
                    .text_color(look.c.faint)
                    .child(count_words(l)),
            )
            .child(div().flex_1())
            .when_some(session, |d, s| {
                d.child(
                    kit::button_icon(
                        look,
                        "detail-open",
                        Some("arrow-up-right"),
                        "Open session",
                        "",
                        Kind::Ghost,
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.state.tabs.open(Tab::Session {
                            id: s.clone(),
                            ordinal,
                        });
                        this.after_state_change(window, cx);
                    })),
                )
            });
        div()
            .flex()
            .flex_col()
            .gap(look.px(10.))
            .child(head)
            .child(
                div()
                    .text_size(look.text(17.))
                    .line_height(look.px(24.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(look.c.text)
                    .child(l.title.clone()),
            )
            .children(saw)
            .children(earlier)
            .child(
                div()
                    .h(look.fixed(1.))
                    .bg(look.c.border_variant)
                    .my(look.px(6.)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(kit::label(look, "Proposed rule", look.c.text_muted))
                    .child(div().flex_1())
                    .when(!editing, |d| {
                        d.child(div().text_color(kit::dim(look)).child("E to edit"))
                    }),
            )
            .child(rule_box)
            .children(target)
            .children(review)
            .into_any_element()
    }

    /// The detail's foot: Keep ↵ (the view's primary), Edit E,
    /// and Dismiss ⌫ at the right.
    fn lesson_actions(&self, l: &Lesson, look: &Look, cx: &mut Context<Self>) -> gpui::Div {
        let id = l.id;
        let busy = self.inbox.busy == Some(id) && self.lesson_job_running();
        div()
            .flex()
            .items_center()
            .gap(look.px(8.))
            .child(
                kit::button_icon(
                    look,
                    "detail-keep",
                    Some("check"),
                    if busy { "Keeping…" } else { "Keep" },
                    "↵",
                    Kind::Primary,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.keep_lesson(id, None, cx))),
            )
            .child(
                kit::button_key(look, "detail-edit", "Edit", "E", Kind::Plain).on_click(
                    cx.listener(move |this, _, window, cx| this.edit_lesson(id, window, cx)),
                ),
            )
            .child(div().flex_1())
            .child(
                kit::button_key(look, "detail-dismiss", "Dismiss", "⌫", Kind::Ghost)
                    .on_click(cx.listener(move |this, _, _, cx| this.dismiss_lesson(id, cx))),
            )
    }

    fn change_detail(
        &self,
        c: &ReviewItem,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let receipt = c.receipt.to_string();
        let proposal = c.proposal;
        let undo = c.undo.is_some();
        let note = c.note.clone();
        div()
            .flex()
            .flex_col()
            .gap(look.px(10.))
            .child(kit::label(
                look,
                "Overnight · what dreaming changed",
                kit::ok(look),
            ))
            .child(
                div()
                    .text_size(look.text(20.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(c.label.clone()),
            )
            .children(c.old.as_ref().map(|o| {
                div()
                    .p(look.px(12.))
                    .rounded(look.px(8.))
                    .bg(look.c.deleted_background)
                    .text_color(look.c.deleted)
                    .child(o.clone())
            }))
            .child(
                div()
                    .p(look.px(12.))
                    .rounded(look.px(8.))
                    .bg(look.c.created.opacity(0.12))
                    .text_color(look.c.text)
                    .child(c.new.clone()),
            )
            .children(
                c.edited_since
                    .as_ref()
                    .map(|e| div().text_color(look.c.warning).child(e.clone())),
            )
            .child(
                div()
                    .flex()
                    .gap(look.px(8.))
                    .pt(look.px(6.))
                    .child(
                        kit::button(look, "detail-right", "Looks right  ↩", Kind::Plain)
                            .text_color(kit::ok(look))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.mark_right(vec![receipt.clone()], cx)
                            })),
                    )
                    .child(
                        kit::button(
                            look,
                            "detail-undo",
                            "Undo",
                            if undo { Kind::Plain } else { Kind::Disabled },
                        )
                        .when(undo, |b| {
                            b.on_click(cx.listener(move |this, _, window, cx| {
                                this.state.tabs.open(Tab::Proposal(proposal));
                                this.after_state_change(window, cx);
                            }))
                        }),
                    )
                    .child(div().flex_1())
                    .child(
                        kit::button(look, "detail-note", "Open note", Kind::Plain).on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.state.tabs.open(Tab::Note(note.clone()));
                                this.after_state_change(window, cx);
                            }),
                        ),
                    ),
            )
            .into_any_element()
    }

    /// The Home's lesson rows: at most `n`, each with Keep, Edit and
    /// Dismiss.
    /// Home's lesson rows: every lesson a 28-point row (mark,
    /// title, `seen 2×` or `32 sessions`, ✓ ✎ ✕); corrections first, then
    /// each group with its Keep all; at most `n` rows, then how many more.
    pub(super) fn render_home_lessons(
        &self,
        n: usize,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let notes_dir = self.notes_dir();
        let items: Vec<Item> = self
            .inbox_items(Filter::All)
            .into_iter()
            .filter(|i| matches!(i, Item::Lesson(_) | Item::Group(_)))
            .collect();
        let mut out: Vec<gpui::AnyElement> = Vec::new();
        let mut rows = 0usize;
        let mut hidden = 0usize;
        let mut singles_head = false;
        for (k, item) in items.iter().enumerate() {
            match item {
                Item::Lesson(l) => {
                    if rows >= n {
                        hidden += 1;
                        continue;
                    }
                    if !singles_head {
                        singles_head = true;
                        let singles = items
                            .iter()
                            .filter(|i| matches!(i, Item::Lesson(x) if x.kind == l.kind))
                            .count();
                        out.push(
                            div()
                                .flex()
                                .items_center()
                                .h(look.px(34.))
                                .px(look.px(8.))
                                .pt(look.px(6.))
                                .child(group_label(
                                    look,
                                    if l.kind == LessonKind::Correction {
                                        "Corrections"
                                    } else {
                                        "Rules"
                                    },
                                    singles,
                                ))
                                .into_any_element(),
                        );
                    }
                    out.push(self.lesson_row(l, 60_000 + k, true, false, look, cx));
                    rows += 1;
                }
                Item::Group(g) => {
                    if rows >= n {
                        hidden += g.len();
                        continue;
                    }
                    out.push(self.group_card(k, g, false, notes_dir.as_deref(), true, look, cx));
                    rows += g.len();
                }
                _ => {}
            }
        }
        if hidden > 0 {
            out.push(
                div()
                    .id("home-lessons-more")
                    .pt(look.px(6.))
                    .pl(look.px(36.))
                    .text_size(look.text(12.5))
                    .text_color(look.c.text_muted)
                    .cursor_pointer()
                    .hover(|d| d.text_color(look.c.text))
                    .child(format!("{hidden} more"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.show_section(Section::Inbox, window, cx)
                    }))
                    .into_any_element(),
            );
        }
        out
    }

    /// One lesson as a 28-point row (the grouped card's rows and
    /// Home's): the profile mark (`mark`), the title, how often, ✓ ✎ ✕. A
    /// click selects it in the Inbox.
    pub(super) fn lesson_row(
        &self,
        l: &Lesson,
        base: usize,
        mark: bool,
        sel: bool,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let id = l.id;
        let key = id.to_string();
        let busy = self.inbox.busy == Some(id) && self.lesson_job_running();
        let small = |id: (&'static str, usize), label: &'static str, tip: &'static str| {
            kit::icon_button(look, id, label, tip).tooltip(super::tooltip::Tooltip::text(tip))
        };
        let profile = l
            .evidence
            .first()
            .and_then(|e| e.profile.clone())
            .or_else(|| l.seen.profiles.first().cloned());
        // ✓ ✎ ✕ show under the pointer (T3 Code's row controls),
        // or on the selected row; at rest a row is its title and count.
        let actions = div()
            .flex()
            .items_center()
            .gap(look.px(2.))
            .when(!sel && !busy, |d| {
                d.invisible().group_hover("lesson-row", |s| s.visible())
            });
        div()
            .id(("lesson-row", base))
            .group("lesson-row")
            .flex()
            .items_center()
            .gap(look.px(8.))
            .h(look.px(34.))
            .gap(look.px(10.))
            .px(look.px(8.))
            .rounded(look.px(8.))
            .when(sel, |d| d.bg(look.c.raised))
            .cursor_pointer()
            .hover(|d| d.bg(look.c.hover))
            .when(mark, |d| d.child(kit::mark(look, profile.as_deref(), 18.)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(look.c.text)
                    .child(kit::title(&l.title)),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(look.text(12.))
                    .text_color(kit::dim(look))
                    .child(count_words(l)),
            )
            .child(
                actions
                    .child(
                        small(
                            ("r-keep", base),
                            if busy { "…" } else { "✓" },
                            "Keep: write this rule into its note",
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.keep_lesson(id, None, cx)
                        })),
                    )
                    .child(
                        small(("r-edit", base), "✎", "Edit the rule").on_click(cx.listener(
                            move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.edit_lesson(id, window, cx);
                                this.after_state_change(window, cx);
                            },
                        )),
                    )
                    .child(
                        small(
                            ("r-dismiss", base),
                            "✕",
                            "Dismiss: proposed again only if it recurs in 3 more sessions",
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.dismiss_lesson(id, cx)
                        })),
                    ),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.state.tabs.section() != Section::Inbox {
                    this.state.tabs.select(Section::Inbox);
                }
                this.inbox.selected = Some(key.clone());
                window.focus(&this.inbox.focus, cx);
                this.after_state_change(window, cx);
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lesson(kind: LessonKind, sessions: usize, profiles: &[&str], new: bool) -> Lesson {
        let at = OffsetDateTime::now_utc();
        Lesson {
            id: Ulid::nil(),
            kind,
            key: "k".into(),
            terms: vec![],
            title: "Use tabs".into(),
            rule: "Use tabs.".into(),
            target: Some(hippo_core::lessons::Target {
                note: "brain/Team/Agent rules.md".into(),
                section: (!new).then(|| "Tools".to_owned()),
                new,
            }),
            evidence: vec![],
            seen: hippo_core::lessons::Seen {
                sessions,
                profiles: profiles.iter().map(|p| p.to_string()).collect(),
                first: at,
                last: at,
                most_in_a_day: sessions,
            },
            status: hippo_core::lessons::Status::Pending,
            review: None,
            created: at,
            updated: at,
            closed_sessions: None,
            kept: None,
            ask: None,
        }
    }

    #[test]
    fn tool_calls_read_as_words() {
        assert_eq!(
            action_words(
                r#"agent: mcp__t3-code__html_render: {"height":540,"html":"<!doctype html>"}"#
            )
            .0,
            "ran html_render"
        );
        assert_eq!(
            action_words(r#"agent: Bash: {"command":"x","description":"Run the tests."}"#).0,
            "ran Bash: Run the tests"
        );
        let (w, raw) = action_words("agent: Bash: cd /w && cargo test -p core");
        assert_eq!(w, "ran cargo test");
        assert_eq!(raw.as_deref(), Some("cd /w && cargo test -p core"));
        assert_eq!(
            action_words("agent: Edit: /w/src/memory.rs").0,
            "ran Edit on memory.rs"
        );
        // A sentence stays a sentence.
        let (w, raw) = action_words("agent: Shall I go ahead? It is ready.");
        assert_eq!(w, "Shall I go ahead? It is ready.");
        assert_eq!(raw, None);
    }

    #[test]
    fn cards_say_how_often_and_where() {
        let now = OffsetDateTime::now_utc();
        // Sentence case, no date.
        let line = kind_line(
            &lesson(LessonKind::Correction, 3, &["default", "work"], false),
            now,
        );
        assert_eq!(line, "Correction · seen 3×");
        let line = kind_line(&lesson(LessonKind::Correction, 1, &["work"], false), now);
        assert_eq!(line, "Correction");
        let line = kind_line(&lesson(LessonKind::Failure, 4, &["default"], false), now);
        assert_eq!(line, "Repeated failure · 4 sessions");
        let brain = Some(Path::new("brain"));
        assert_eq!(
            target_words(&lesson(LessonKind::Correction, 1, &[], false), brain),
            Some(("appends to", "Team / Agent rules › Tools".to_owned()))
        );
        assert_eq!(
            target_words(&lesson(LessonKind::Correction, 1, &[], true), brain).map(|t| t.0),
            Some("new note")
        );
    }

    #[test]
    fn lessons_for_one_section_group_and_corrections_come_first() {
        let with = |kind, sessions, id: u128, section: &str| {
            let mut l = lesson(kind, sessions, &["work"], false);
            l.id = Ulid(id);
            if let Some(t) = l.target.as_mut() {
                t.section = Some(section.to_owned());
            }
            l
        };
        let items = group_lessons(vec![
            with(LessonKind::Failure, 32, 1, "During work"),
            with(LessonKind::Correction, 1, 2, "Tools"),
            with(LessonKind::Failure, 43, 3, "During work"),
            with(LessonKind::Failure, 5, 4, "Shell"),
            with(LessonKind::Correction, 2, 5, "Tools"),
        ]);
        let shape: Vec<String> = items
            .iter()
            .map(|i| match i {
                Item::Lesson(l) => format!("{}", l.id.0),
                Item::Group(g) => format!(
                    "[{}]",
                    g.iter()
                        .map(|l| l.id.0.to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                ),
                _ => String::new(),
            })
            .collect();
        // Corrections first (most sessions first), grouped by section; then
        // failures; a failure alone in its section stays a card.
        assert_eq!(shape, ["[5,2]", "[3,1]", "4"]);
        assert!(items[1].holds(&Ulid(1).to_string()));
        assert_eq!(
            count_words(&with(LessonKind::Failure, 32, 9, "x")),
            "32 sessions"
        );
        assert_eq!(
            count_words(&with(LessonKind::Correction, 2, 9, "x")),
            "seen 2×"
        );
        // A rule that makes a new note never groups.
        let a = lesson(LessonKind::Correction, 1, &[], true);
        let mut b = lesson(LessonKind::Correction, 1, &[], true);
        b.id = Ulid(7);
        assert_eq!(group_lessons(vec![a, b]).len(), 2);
    }

    #[test]
    fn notes_read_as_folders_and_a_title() {
        assert_eq!(
            note_label(
                Path::new("brain/Team/Agent rules.md"),
                Some(Path::new("brain"))
            ),
            "Team / Agent rules"
        );
        assert_eq!(
            note_label(Path::new("notes/Topic.md"), None),
            "notes / Topic"
        );
    }
}
