//! The app's state, with no window code: the section and each section's
//! stack of views, Memory's notes list, selections by id, the reload's
//! one-running-one-waiting rule, and the actions run this session.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use hippocamp_core::config::{Config, Workspace};
use hippocamp_core::dream::{self, History};
use hippocamp_view::setup::{self, Setup};
use hippocamp_view::status::{self, Status};
use hippocamp_view::{brain, dreaming, sessions};
use time::OffsetDateTime;
use ulid::Ulid;

use crate::actions::Ctx;
use crate::rows::Panels;

/// A section of the rail: Home, Inbox, Fleet, Memory and Manage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Section {
    Home,
    Inbox,
    Fleet,
    /// What went wrong, which agent did it and whether it had been told.
    Incidents,
    Memory,
    Manage,
}

impl Section {
    pub const ALL: [Section; 6] = [
        Section::Home,
        Section::Inbox,
        Section::Fleet,
        Section::Incidents,
        Section::Memory,
        Section::Manage,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Section::Home => "Home",
            Section::Inbox => "Inbox",
            Section::Fleet => "Fleet",
            Section::Incidents => "Incidents",
            Section::Memory => "Memory",
            Section::Manage => "Manage",
        }
    }

    /// The view the section opens on.
    pub fn root(self) -> Tab {
        match self {
            Section::Home => Tab::Home,
            Section::Inbox => Tab::Inbox,
            Section::Fleet => Tab::Fleet,
            Section::Incidents => Tab::Incidents,
            Section::Memory => Tab::Memory,
            Section::Manage => Tab::Manage,
        }
    }
}

/// What the main view shows: a section's own view, or an item opened from
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tab {
    /// The Ask bar, headroom, the inbox and the fleet at a glance.
    Home,
    /// The answer to a question asked in the Ask bar.
    Ask,
    /// Dreaming changes to look at, and the recent runs.
    Inbox,
    /// Today's sessions and each profile's headroom.
    Fleet,
    /// What went wrong this week.
    Incidents,
    /// The notes list beside the selected (else the entry) note.
    Memory,
    /// Settings, and what later steps bring.
    Manage,
    /// One incident's page, by its id.
    Incident(String),
    Note(PathBuf),
    Session {
        id: String,
        ordinal: Option<u32>,
    },
    Proposal(Ulid),
    /// A session's context x-ray: what it was given, what it used, what it
    /// searched for, and its claims' evidence.
    Xray(String),
    /// A sub-task of a session as its own page: task `ord` of
    /// session `session`'s tasks, in call order.
    Task {
        session: String,
        ord: usize,
    },
    /// The start block for a new agent on the Ask bar's task (Brief mode).
    Brief,
    /// A dreaming run, by its report file.
    Report(PathBuf),
    /// The notes that need attention, from the linter.
    Attention,
    Context,
    Help,
}

impl Tab {
    /// Whether two tabs show the same item. A session's tab is one per
    /// session whatever turn it is scrolled to.
    pub fn same_item(&self, other: &Tab) -> bool {
        match (self, other) {
            (Tab::Session { id: a, .. }, Tab::Session { id: b, .. }) => a == b,
            _ => self == other,
        }
    }

    /// A section's own view, not an item opened from it.
    pub fn is_root(&self) -> bool {
        matches!(
            self,
            Tab::Home | Tab::Inbox | Tab::Fleet | Tab::Incidents | Tab::Memory | Tab::Manage
        )
    }

    /// The title when nothing better is known.
    pub fn default_title(&self) -> String {
        match self {
            Tab::Home => "Home".to_owned(),
            Tab::Ask => "Ask".to_owned(),
            Tab::Brief => "Brief a new agent".to_owned(),
            Tab::Inbox => "Inbox".to_owned(),
            Tab::Fleet => "Fleet".to_owned(),
            Tab::Incidents => "Incidents".to_owned(),
            Tab::Incident(_) => "Incident".to_owned(),
            Tab::Memory => "Memory".to_owned(),
            Tab::Manage => "Manage".to_owned(),
            Tab::Note(path) => path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            Tab::Session { id, .. } => crate::rows::id_title(id),
            Tab::Xray(id) => crate::rows::id_title(id),
            Tab::Task { .. } => "Sub-task".to_owned(),
            Tab::Proposal(id) => format!("Proposal {id}"),
            Tab::Report(path) => report_title(path),
            Tab::Attention => "Needs attention".into(),
            Tab::Context => "Context".into(),
            Tab::Help => "Help".into(),
        }
    }
}

/// A day's short title: `Sun 8 Nov`.
pub fn edition_title(date: time::Date) -> String {
    date.format(time::macros::format_description!(
        "[weekday repr:short] [day padding:none] [month repr:short]"
    ))
    .unwrap_or_else(|_| date.to_string())
}

/// A run's title from its report's file name: `Run 2026-11-08`, and
/// `Run 2026-11-08 #2` for a later run that day.
pub fn report_title(path: &std::path::Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = name.strip_suffix("-dream.md").unwrap_or(&name);
    // `<YYYY-MM-DD>` or `<YYYY-MM-DD>-<n>`.
    match (stem.get(..10), stem.get(10..)) {
        (Some(date), Some(rest)) if rest.starts_with('-') => format!("Run {date} #{}", &rest[1..]),
        (Some(date), _) => format!("Run {date}"),
        _ => format!("Run {stem}"),
    }
}

/// Where the person is: a section, and in each section a stack of its own
/// view with the items opened from it. There is no tab bar: an item opened
/// is pushed, the back link pops it, and choosing a section shows the top
/// of its stack (or, when it is already shown, its own view).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nav {
    current: Section,
    stacks: BTreeMap<Section, Vec<Tab>>,
}

impl Default for Nav {
    fn default() -> Nav {
        Nav {
            current: Section::Home,
            stacks: Section::ALL.iter().map(|s| (*s, vec![s.root()])).collect(),
        }
    }
}

impl Nav {
    pub fn section(&self) -> Section {
        self.current
    }

    fn stack_mut(&mut self) -> &mut Vec<Tab> {
        self.stacks
            .entry(self.current)
            .or_insert_with(|| vec![self.current.root()])
    }

    /// The current section's stack, its own view first.
    pub fn stack(&self) -> &[Tab] {
        self.stacks.get(&self.current).map_or(&[], Vec::as_slice)
    }

    /// How deep the current section's stack is (1: its own view).
    pub fn depth(&self) -> usize {
        self.stack().len()
    }

    /// What the main view shows.
    pub fn active_tab(&self) -> Option<&Tab> {
        self.stack().last()
    }

    /// Chooses `section`. Chosen again while shown, it goes back to its
    /// own view.
    pub fn select(&mut self, section: Section) {
        if self.current == section {
            self.stack_mut().truncate(1);
        } else {
            self.current = section;
        }
    }

    /// Opens `tab` in the current section: the item on top is updated in
    /// place when it is the same item; otherwise `tab` is pushed.
    pub fn open(&mut self, tab: Tab) {
        let stack = self.stack_mut();
        if stack.last().is_some_and(|t| t.same_item(&tab)) {
            *stack.last_mut().expect("checked") = tab;
            return;
        }
        if tab.is_root() {
            stack.truncate(1);
            return;
        }
        stack.push(tab);
    }

    /// Shows `tab` in place of the item on top (a selection moving through
    /// a list), or pushes it when the section's own view is on top.
    pub fn show_preview(&mut self, tab: Tab) {
        let stack = self.stack_mut();
        if stack.len() > 1 {
            stack.pop();
        }
        if !tab.is_root() {
            stack.push(tab);
        }
    }

    /// A link followed: the item opens on top.
    pub fn follow(&mut self, tab: Tab, _double: bool) {
        self.open(tab);
    }

    /// Back to `tab` when it is below on the stack (a crumb followed);
    /// otherwise `tab` opens on top.
    pub fn back_to(&mut self, tab: Tab) {
        let stack = self.stack_mut();
        if let Some(i) = stack.iter().rposition(|t| t.same_item(&tab)) {
            stack.truncate(i + 1);
            return;
        }
        self.open(tab);
    }

    /// Back to what was shown before; `false` at the section's own view.
    pub fn back(&mut self) -> bool {
        let stack = self.stack_mut();
        if stack.len() > 1 {
            stack.pop();
            true
        } else {
            false
        }
    }
}

/// The notes list's smallest and largest width, in points.
pub const DOCK_MIN: f32 = 200.0;
pub const DOCK_MAX: f32 = 480.0;
/// Its width at launch, in points.
pub const DOCK_DEFAULT: f32 = 280.0;

/// Memory's notes list: how wide, shown or hidden.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dock {
    /// The width the user dragged to, in pixels; `None` is the default, which
    /// scales with the text size.
    pub dragged_width: Option<f32>,
    pub visible: bool,
}

impl Default for Dock {
    fn default() -> Self {
        Dock {
            dragged_width: None,
            visible: true,
        }
    }
}

impl Dock {
    /// A width in pixels dragged to, kept between the bounds (which scale
    /// with the text like every length, `scale_factor` being points to pixels).
    pub fn drag_to(&mut self, pixels: f32, scale_factor: f32) {
        self.dragged_width = Some(pixels.clamp(DOCK_MIN * scale_factor, DOCK_MAX * scale_factor));
    }
}

/// The text size steps: 80% to 200% of the theme's size, in 10% steps.
pub const SCALE_MIN: f32 = 0.8;
pub const SCALE_MAX: f32 = 2.0;

/// The session's text scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextScale(u8);

impl Default for TextScale {
    fn default() -> Self {
        TextScale(10)
    }
}

impl TextScale {
    pub fn value(self) -> f32 {
        f32::from(self.0) / 10.0
    }

    pub fn larger(&mut self) {
        self.0 = (self.0 + 1).min((SCALE_MAX * 10.0) as u8);
    }

    pub fn smaller(&mut self) {
        self.0 = (self.0 - 1).max((SCALE_MIN * 10.0) as u8);
    }

    pub fn reset(&mut self) {
        *self = TextScale::default();
    }

    pub fn percent(self) -> u32 {
        u32::from(self.0) * 10
    }
}

/// One reload runs at a time; one asked for while another runs waits, and
/// further requests join the waiting one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reloader {
    running: bool,
    waiting: bool,
}

impl Reloader {
    /// A reload is asked for. `true` means start it now.
    pub fn request(&mut self) -> bool {
        if self.running {
            self.waiting = true;
            false
        } else {
            self.running = true;
            true
        }
    }

    /// The running reload ended. `true` means the waiting one starts now.
    pub fn finish(&mut self) -> bool {
        self.running = false;
        if self.waiting {
            self.waiting = false;
            self.running = true;
            true
        } else {
            false
        }
    }

    #[cfg(test)]
    pub fn running(&self) -> bool {
        self.running
    }
}

/// The selected item of each section, by id (a note's path), kept across
/// reloads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selections(BTreeMap<Section, String>);

impl Selections {
    pub fn select(&mut self, section: Section, id: impl Into<String>) {
        self.0.insert(section, id.into());
    }

    pub fn get(&self, section: Section) -> Option<&str> {
        self.0.get(&section).map(String::as_str)
    }

    /// After a reload: keeps the selection when its item still exists, and
    /// otherwise clears it and returns the notice saying so.
    pub fn reconcile(&mut self, section: Section, exists: impl Fn(&str) -> bool) -> Option<String> {
        let id = self.0.get(&section)?;
        if exists(id) {
            return None;
        }
        let id = self.0.remove(&section)?;
        Some(format!(
            "The selected {} item ({id}) is gone after the reload.",
            section.name()
        ))
    }
}

/// What a reload reads: whether hippocamp is set up and, when it is, the status
/// bar's data, the notes and dreaming lists, today's fleet and what needs
/// the person.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub setup: Setup,
    pub status: Option<Status>,
    pub panels: Option<Panels>,
    /// Today's sessions and each profile's headroom.
    pub fleet: Option<hippocamp_view::fleet::Fleet>,
    /// Why there is no fleet, when there is none.
    pub fleet_error: Option<String>,
    /// What needs the person (the dreaming review) and hippocamp's health.
    pub home: Option<hippocamp_view::home::Home>,
    /// Every session's title from the rollup, for items opened by id.
    pub titles: HashMap<String, String>,
    /// The Ask bar's suggestions: recent sessions' topics.
    pub suggestions: Vec<String>,
    /// Each opened session's indexed turn count.
    pub turns: HashMap<String, u64>,
    /// Each opened session's (and its sub-agents') title, project and
    /// parent.
    pub about: HashMap<String, hippocamp_view::sessions::About>,
    /// The lessons waiting in the Inbox, newest first.
    pub lessons: Vec<hippocamp_core::lessons::Lesson>,
    /// The last lessons scan.
    pub lessons_state: Option<hippocamp_core::lessons::State>,
    /// This week's incidents and session counts.
    pub incidents: Option<crate::incidents::Data>,
    /// Memory's map: every note with its reads, size, edits and freshness.
    pub memory: Option<hippocamp_view::memory::Map>,
    /// What keeps hippocamp from working, as Manage shows it: `ingest
    /// is off`, `hook not installed`.
    pub gaps: Vec<String>,
}

impl Loaded {
    pub fn ready(&self) -> bool {
        matches!(self.setup, Setup::Ready { .. } | Setup::HistoryOnly { .. })
    }
}

/// Reads everything a reload reads, at `now`. Blocks; the app calls it on the
/// background executor. Writes nothing.
pub fn load(home: &Path, now: OffsetDateTime) -> Loaded {
    let setup = setup::load(home);
    let (status, panels) = match &setup {
        Setup::Ready { cfg, ws } => (Some(status::load(cfg, ws, now)), Some(load_panels(cfg, ws))),
        Setup::HistoryOnly { cfg } => (Some(status::load_history(cfg, now)), None),
        _ => (None, None),
    };
    let mut loaded = Loaded {
        setup,
        status,
        panels,
        fleet: None,
        fleet_error: None,
        home: None,
        titles: HashMap::new(),
        suggestions: Vec::new(),
        turns: HashMap::new(),
        about: HashMap::new(),
        lessons: Vec::new(),
        lessons_state: None,
        incidents: None,
        memory: None,
        gaps: Vec::new(),
    };
    if let Setup::Ready { cfg, .. } = &loaded.setup {
        loaded.lessons = hippocamp_core::lessons::pending(&cfg.home).unwrap_or_default();
        loaded.lessons_state = hippocamp_core::lessons::State::load(&cfg.home);
    }
    let offset = crate::ui::status::local_offset();
    if loaded.ready() {
        let user = hippocamp_core::config::user_home();
        loaded.gaps = hippocamp_view::manage::setup_gaps(loaded.setup.workspace(), user.as_deref());
    }
    if let Some(cfg) = loaded.setup.config().cloned() {
        let rollup = hippocamp_core::rollup::Rollup::open(&cfg.home)
            .ok()
            .flatten();
        loaded.incidents = crate::incidents::load(&cfg.home, now);
        match fleet_at(&cfg, rollup.as_ref(), now) {
            Ok(f) => loaded.fleet = Some(f),
            Err(e) => loaded.fleet_error = Some(e),
        }
        if let Some(r) = rollup.as_ref() {
            loaded.titles = r.titles().unwrap_or_default();
            loaded.suggestions = suggestions(r, now);
        }
        loaded.home = load_home(&loaded, rollup.as_ref(), now, offset);
        if let (Some(panels), Some(ws)) = (loaded.panels.as_ref(), loaded.setup.workspace()) {
            // The x-ray's fleet week: what start blocks cost.
            let fleet = rollup
                .as_ref()
                .and_then(|r| hippocamp_view::memory::fleet(ws, r, now));
            loaded.memory = Some(hippocamp_view::memory::load(
                &hippocamp_view::memory::Inputs {
                    ws,
                    brain: &panels.brain,
                    history: panels.history.as_ref().ok(),
                    rollup: rollup.as_ref(),
                    fleet: fleet.as_ref(),
                    now,
                    offset,
                },
            ));
        }
    }
    loaded
}

/// Today's fleet at `now`: the transcripts under the roots, the usage
/// cache and the rollup, read-only.
fn fleet_at(
    cfg: &Config,
    rollup: Option<&hippocamp_core::rollup::Rollup>,
    now: OffsetDateTime,
) -> Result<hippocamp_view::fleet::Fleet, String> {
    let offset = crate::ui::status::local_offset();
    let roots = hippocamp_core::adapters::transcript_roots(cfg);
    let snap = hippocamp_core::usage::Snapshot::read(&cfg.home, &roots, now, offset)
        .map_err(|e| e.to_string())?;
    Ok(hippocamp_view::fleet::build(
        &hippocamp_view::fleet::Inputs {
            now,
            offset,
            roots: &roots,
            rollup,
            usage: &snap,
        },
    ))
}

/// Today's fleet, now: what the fleet's 30-second refresh reads.
pub fn load_fleet(cfg: &Config) -> Result<hippocamp_view::fleet::Fleet, String> {
    let rollup = hippocamp_core::rollup::Rollup::open(&cfg.home)
        .ok()
        .flatten();
    fleet_at(cfg, rollup.as_ref(), OffsetDateTime::now_utc())
}

/// Up to six topics of the newest top-level sessions of the last week,
/// each a few words of its title.
fn suggestions(rollup: &hippocamp_core::rollup::Rollup, now: OffsetDateTime) -> Vec<String> {
    let Ok(mut recent) = rollup.sessions_in(now - time::Duration::days(7), now) else {
        return Vec::new();
    };
    recent.retain(|s| s.facts.parent.is_none() && !s.facts.automated);
    recent.sort_by_key(|s| std::cmp::Reverse(s.facts.last_turn));
    let mut out: Vec<String> = Vec::new();
    for s in recent {
        let Some(title) = s.facts.title.as_deref() else {
            continue;
        };
        let topic = topic_of(title);
        if topic.split_whitespace().count() < 2 {
            continue;
        }
        if !out.iter().any(|t| t.eq_ignore_ascii_case(&topic)) {
            out.push(topic);
        }
        if out.len() == 6 {
            break;
        }
    }
    out
}

/// A title's topic, as a chip shows it: the part after a `project:`
/// prefix, up to its first comma, bracket or dash, at most four words.
pub fn topic_of(title: &str) -> String {
    let title = title.split_once(": ").map_or(title, |(_, t)| t);
    let cut = [",", "(", " — ", " – ", " - ", ";", ":", ". "]
        .iter()
        .filter_map(|m| title.find(m))
        .min()
        .unwrap_or(title.len());
    title[..cut]
        .split_whitespace()
        .take(4)
        .collect::<Vec<_>>()
        .join(" ")
}

/// What needs the person (the dreaming review) and hippocamp's health, from the
/// dreaming history and its review marks and the rollup. Writes nothing.
pub fn load_home(
    loaded: &Loaded,
    rollup: Option<&hippocamp_core::rollup::Rollup>,
    now: OffsetDateTime,
    offset: time::UtcOffset,
) -> Option<hippocamp_view::home::Home> {
    let cfg = loaded.setup.config()?;
    let ws = loaded.setup.workspace();
    let marked = ws
        .and_then(|ws| hippocamp_core::dream::seen::marked(ws).ok())
        .unwrap_or_default();
    let panels = loaded.panels.as_ref();
    let history = panels.and_then(|p| p.history.as_ref().ok());
    let runs: &[dreaming::Run] = panels
        .and_then(|p| p.dreaming.as_ref().ok())
        .map_or(&[], |d| d.runs.as_slice());
    let notes: &[brain::NoteRow] = panels.map_or(&[], |p| p.brain.rows.as_slice());
    let profiles: Vec<String> = rollup
        .and_then(|r| r.summary().ok())
        .map(|s| s.profiles)
        .unwrap_or_default();
    Some(hippocamp_view::home::build(&hippocamp_view::home::Inputs {
        now,
        offset,
        today: None,
        yesterday: None,
        rollup,
        ws,
        history,
        marked: &marked,
        notes,
        runs,
        status: loaded.status.as_ref(),
        profiles: &profiles,
        ingest_interval_minutes: cfg.ingest_interval_minutes,
        dream_at: &cfg.dream_at,
    }))
}

/// Reads the three panels' lists. Writes nothing.
fn load_panels(cfg: &Config, ws: &Workspace) -> Panels {
    let history = dream::history(ws).map_err(|e| e.to_string());
    let empty = History {
        changes: Vec::new(),
        unreadable: Vec::new(),
    };
    let brain = brain::load(ws, history.as_ref().unwrap_or(&empty));
    let sessions = sessions::load(ws).map_err(|e| e.to_string());
    let dreaming = match &history {
        Ok(h) => dreaming::load(ws, h).map_err(|e| e.to_string()),
        Err(e) => Err(e.clone()),
    };
    let mut attention = brain::attention(&brain, &ws.notes_dir);
    // The notes with no frontmatter that changed last come first: they are
    // the ones in use.
    let modified: HashMap<&Path, SystemTime> = brain
        .rows
        .iter()
        .filter_map(|r| {
            let t = std::fs::metadata(ws.root.join(&r.path))
                .and_then(|m| m.modified())
                .ok()?;
            Some((r.path.as_path(), t))
        })
        .collect();
    attention
        .no_frontmatter
        .sort_by_key(|(p, _)| std::cmp::Reverse(modified.get(p.as_path()).copied()));
    Panels {
        brain,
        sessions,
        history,
        dreaming,
        attention,
        notes_dir: ws.notes_dir.clone(),
        dream_at: cfg.dream_at.clone(),
    }
}

/// The app's state.
#[derive(Debug, Clone, Default)]
pub struct AppState {
    pub tabs: Nav,
    pub dock: Dock,
    pub scale: TextScale,
    pub selections: Selections,
    /// Ids of the actions run this session, newest first.
    pub ran: Vec<&'static str>,
    pub notices: Vec<String>,
}

impl AppState {
    /// Notes that an action ran, for the palette's order.
    pub fn note_action(&mut self, id: &'static str) {
        self.ran.retain(|r| *r != id);
        self.ran.insert(0, id);
    }

    pub fn ctx(&self, setup_ready: bool, filter_has_text: bool, picker_open: bool) -> Ctx {
        Ctx {
            setup_ready,
            depth: self.tabs.depth(),
            memory: self.tabs.section() == Section::Memory,
            filter_has_text,
            picker_open,
            active_file: false,
            revert_ready: false,
            confirm_open: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(name: &str) -> Tab {
        Tab::Note(PathBuf::from(name))
    }

    #[test]
    fn each_section_keeps_its_own_stack_and_opens_on_its_top() {
        let mut n = Nav::default();
        assert_eq!(n.active_tab(), Some(&Tab::Home));
        n.open(Tab::Ask);
        n.open(Tab::Session {
            id: "s1".into(),
            ordinal: None,
        });
        assert_eq!(n.depth(), 3);
        n.select(Section::Fleet);
        assert_eq!(n.active_tab(), Some(&Tab::Fleet));
        n.open(note("a.md"));
        n.select(Section::Home);
        assert!(
            matches!(n.active_tab(), Some(Tab::Session { .. })),
            "Home kept its stack"
        );
        // Chosen again while shown: back to its own view.
        n.select(Section::Home);
        assert_eq!(n.active_tab(), Some(&Tab::Home));
        n.select(Section::Fleet);
        assert_eq!(n.active_tab(), Some(&note("a.md")));
    }

    #[test]
    fn back_pops_to_the_sections_own_view_and_no_further() {
        let mut n = Nav::default();
        n.open(Tab::Ask);
        assert!(n.back());
        assert_eq!(n.active_tab(), Some(&Tab::Home));
        assert!(!n.back());
        assert_eq!(n.depth(), 1);
    }

    #[test]
    fn the_same_item_updates_in_place_and_a_preview_replaces_the_top() {
        let mut n = Nav::default();
        let s = |ordinal| Tab::Session {
            id: "s1".into(),
            ordinal,
        };
        n.open(s(None));
        n.open(s(Some(7)));
        assert_eq!(n.depth(), 2);
        assert_eq!(n.active_tab(), Some(&s(Some(7))));
        n.select(Section::Memory);
        n.show_preview(note("a.md"));
        n.show_preview(note("b.md"));
        assert_eq!(n.stack(), &[Tab::Memory, note("b.md")]);
        // A section's own view opened clears what was on top.
        n.open(Tab::Memory);
        assert_eq!(n.stack(), &[Tab::Memory]);
    }

    #[test]
    fn the_dock_width_stays_between_its_bounds() {
        let mut d = Dock::default();
        d.drag_to(10.0, 1.0);
        assert_eq!(d.dragged_width, Some(DOCK_MIN));
        d.drag_to(10_000.0, 2.0);
        assert_eq!(d.dragged_width, Some(DOCK_MAX * 2.0));
        d.drag_to(300.0, 1.0);
        assert_eq!(d.dragged_width, Some(300.0));
    }

    #[test]
    fn the_text_scale_runs_from_80_to_200_percent_in_steps_of_ten() {
        let mut s = TextScale::default();
        assert_eq!(s.percent(), 100);
        for _ in 0..20 {
            s.larger();
        }
        assert_eq!(s.percent(), 200);
        for _ in 0..20 {
            s.smaller();
        }
        assert_eq!(s.percent(), 80);
        s.reset();
        assert_eq!(s.value(), 1.0);
    }

    #[test]
    fn one_reload_runs_and_at_most_one_waits() {
        let mut r = Reloader::default();
        assert!(r.request());
        assert!(!r.request());
        assert!(!r.request());
        assert!(r.finish(), "the waiting reload starts");
        assert!(!r.finish());
        assert!(!r.running());
    }

    #[test]
    fn a_selection_is_kept_by_id_and_cleared_with_a_notice_when_its_item_is_gone() {
        let mut s = Selections::default();
        s.select(Section::Memory, "notes/a.md");
        assert_eq!(s.reconcile(Section::Memory, |id| id == "notes/a.md"), None);
        let notice = s.reconcile(Section::Memory, |_| false).unwrap();
        assert!(notice.contains("notes/a.md"), "{notice}");
        assert_eq!(s.get(Section::Memory), None);
    }

    #[test]
    fn the_actions_run_are_kept_newest_first_without_repeats() {
        let mut s = AppState::default();
        s.note_action("Reload");
        s.note_action("ToggleDock");
        s.note_action("Reload");
        assert_eq!(s.ran, ["Reload", "ToggleDock"]);
    }

    #[test]
    fn a_topic_is_a_few_words_of_a_title() {
        assert_eq!(
            topic_of("hippocamp: make the Edition honest"),
            "make the Edition honest"
        );
        assert_eq!(
            topic_of("hippocamp: Fleet & memory app, part 1 (shell, Home)"),
            "Fleet & memory app"
        );
        assert_eq!(
            topic_of("Album desktop update rollback script"),
            "Album desktop update rollback"
        );
    }

    /// A reload writes nothing under the home or the workspace.
    #[test]
    #[allow(clippy::disallowed_methods)] // the test builds a temporary hippocamp home
    fn a_reload_changes_no_file() {
        use hippocamp_core::config::{InitOptions, init_in};
        use std::fs;
        use walk::snapshot;

        mod walk {
            use std::path::{Path, PathBuf};
            use std::time::SystemTime;
            pub fn snapshot(root: &Path) -> Vec<(PathBuf, u64, SystemTime)> {
                let mut out = Vec::new();
                let mut stack = vec![root.to_path_buf()];
                while let Some(dir) = stack.pop() {
                    for entry in std::fs::read_dir(&dir).unwrap() {
                        let path = entry.unwrap().path();
                        let meta = std::fs::symlink_metadata(&path).unwrap();
                        if meta.is_dir() {
                            stack.push(path.clone());
                        }
                        out.push((path, meta.len(), meta.modified().unwrap()));
                    }
                }
                out.sort();
                out
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let ws = dir.path().join("ws");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&ws).unwrap();
        init_in(
            &home,
            InitOptions {
                workspace: Some(ws),
                notes_dir: Some("notes".into()),
                entry_note: Some("index.md".into()),
                add_roots: Vec::new(),
                force: false,
            },
        )
        .unwrap();
        let before = snapshot(dir.path());
        let loaded = load(&home, OffsetDateTime::now_utc());
        assert!(loaded.ready());
        assert_eq!(snapshot(dir.path()), before);
    }
}
