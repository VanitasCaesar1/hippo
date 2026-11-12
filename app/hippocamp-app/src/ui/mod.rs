//! The window: the shell, built on GPUI: the rail, the title bar, the main view and the status bar. [`Shell`] is the root view; the other modules draw its parts
//! and each adds an `impl Shell` block for its part.

pub mod about;
pub mod ask;
pub mod attention_tab;
pub mod brief;
pub mod context_tab;
pub mod controls;
pub mod dock;
pub mod fleet;
pub mod help_tab;
pub mod home;
pub mod home_panes;
pub mod inbox;
pub mod incidents_ui;
pub mod input;
pub mod kit;
pub mod lessons;
pub mod manage;
pub mod manage_page;
pub mod markdown;
pub mod memory;
pub mod menus;
pub mod note_tab;
pub mod onboarding;
pub mod overview;
pub mod palette;
pub mod pane;
pub mod picker;
pub mod proposal_tab;
pub mod rail;
pub mod reach_ui;
pub mod replay_ui;
pub mod run_tab;
pub mod scrollbar;
pub mod session_tab;
pub mod setup;
pub mod sidebar;
pub mod status;
pub mod subtask;
pub mod theme;
pub mod theme_picker;
pub mod titlebar;
pub mod tooltip;
pub mod xray;
mod xray_v2;

use std::collections::HashMap;
use std::path::PathBuf;

use gpui::AnimationExt as _;
use gpui::{
    App, Bounds, Context, DragMoveEvent, Entity, FocusHandle, Focusable, Render, Subscription,
    Window, WindowBounds, WindowOptions, div, point, prelude::*, px, size,
};
use time::OffsetDateTime;

use crate::actions::{self, Ctx, TABLE};
use crate::cli::{self, Installation};
use crate::state::{self, AppState, Loaded, Reloader, Section, Tab};
use crate::themes::ThemeState;
use dock::DragDock;
use hippocamp_view::brain::NoteView;
use input::{InputEvent, TextInput};
use picker::{Picker, PickerEvent};
use theme::{ActiveTheme, Look, UiScale};

struct OpenPicker {
    entity: Entity<Picker>,
    /// Where focus was when the picker opened; it goes back there.
    restore: Option<FocusHandle>,
    /// For the theme picker: the id of the theme in use when it opened, which
    /// closing it without a choice goes back to.
    theme_before: Option<String>,
    _subscription: Subscription,
}

/// The root view.
pub struct Shell {
    pub(crate) state: AppState,
    pub(crate) installation: Installation,
    /// `None` until the first reload ends.
    pub(crate) loaded: Option<Loaded>,
    reloader: Reloader,
    focus: FocusHandle,
    pub(crate) dock_focus: FocusHandle,
    pub(crate) pane_focus: FocusHandle,
    pub(crate) dock_scroll: gpui::ScrollHandle,
    /// The notes list row to scroll into view at the next draw, and whether a
    /// draw has already tried (see `Shell::apply_reveal`).
    pub(crate) dock_reveal: std::cell::Cell<Option<(usize, bool)>>,
    brain_filter: Entity<TextInput>,
    /// Fleet's `/` filter.
    pub(crate) fleet_filter: Entity<TextInput>,
    /// Its text, kept for the views that read rows without a context.
    pub(crate) fleet_filter_text: String,
    /// Below 900 points the filter field shows only after `/`.
    pub(crate) fleet_filter_shown: bool,
    /// Fleet's narrow profile menu is open.
    pub(crate) fleet_profiles_open: bool,
    picker: Option<OpenPicker>,
    pub(crate) revert_running: Option<String>,
    /// The command (or resume key) last copied, which its button marks
    /// "copied".
    pub(crate) copied: Option<String>,
    /// The width of the Inbox's and the Home's review rows at the last
    /// draw, which their quotes are fitted to.
    pub(crate) review_width: std::rc::Rc<std::cell::Cell<gpui::Pixels>>,
    pub(crate) home_review_width: std::rc::Rc<std::cell::Cell<gpui::Pixels>>,
    /// The title bar's health detail is open.
    pub(crate) health_open: bool,
    /// The Home's scroll, which its scrollbar reads.
    pub(crate) home_scroll: gpui::ScrollHandle,
    /// The sidebar's shelves folded by hand, and its scroll.
    pub(crate) sidebar: sidebar::SidebarUi,
    pub(crate) sidebar_scroll: gpui::ScrollHandle,
    /// Pane toggles that are off (`pane-headroom`, `pane-memory`).
    pub(crate) panes_off: std::collections::HashSet<&'static str>,
    /// The scroll of the other section views (Ask, Fleet, Inbox, Manage).
    pub(crate) page_scroll: gpui::ScrollHandle,
    /// The Ask bar's question.
    pub(crate) ask_field: Entity<TextInput>,
    pub(crate) ask: ask::AskState,
    /// Ask adds a written answer (the preference).
    pub(crate) ask_answers: bool,
    /// The last questions asked, newest first (the preferences').
    pub(crate) recent_questions: Vec<String>,
    pub(crate) fleet_ui: fleet::FleetUi,
    /// Fleet's Lanes view: the day shown, its zoom, the scrubber.
    pub(crate) replay: replay_ui::ReplayUi,
    /// Fleet's rows take ↑ ↓ ↩ when it shows and nothing else has focus.
    pub(crate) fleet_focus: FocusHandle,
    /// Incidents: the list's selection and filters, the marks
    /// kept in the preferences, the toast and the hook sheet.
    pub(crate) incidents_ui: incidents_ui::IncidentsUi,
    /// The Inbox: its filter, selection, the rule being edited, and the
    /// last `hippocamp lessons` command.
    pub(crate) inbox: lessons::InboxUi,
    pub(crate) xray: xray::XrayUi,
    /// Memory's map: its size toggle, list, selection and hover.
    pub(crate) memory_ui: memory::MemoryUi,
    pub(crate) manage_ui: manage_page::ManageUi,
    /// Brief: the Ask bar's mode and the brief it built.
    pub(crate) brief: brief::BriefUi,
    /// Note pages' stats (reads per day, profiles), read with the page.
    pub(crate) note_stats: HashMap<PathBuf, hippocamp_view::memory::NoteStats>,
    /// Note tabs' contents, read when a tab first shows and again after a
    /// reload.
    pub(crate) note_views: HashMap<PathBuf, Result<NoteView, String>>,
    /// Note pages (sections, links, dreaming, readers), read with the view.
    pub(crate) note_pages: HashMap<PathBuf, Option<hippocamp_view::notepage::NotePage>>,
    pub(crate) context_task: Entity<TextInput>,
    pub(crate) context: context_tab::ContextState,
    pub(crate) sessions_ui: session_tab::SessionsState,
    /// Focus inside a session's turns, where Space folds a tool turn.
    pub(crate) turns_focus: FocusHandle,
    pub(crate) dream_ui: proposal_tab::DreamUi,
    /// The revert confirmation's focus: Return and Escape cancel there.
    pub(crate) confirm_focus: FocusHandle,
    synced_ctx: Option<Ctx>,
    pub(crate) themes: ThemeState,
    /// First open: the onboarding pages, the first home's banner and the
    /// step-two sheet.
    pub(crate) flow: crate::flow::Flow,
    /// The pointer went down on the title bar: a move starts the window drag.
    pub(crate) title_drag: bool,
    /// The menu-bar item, notifications, Open and Resume, the
    /// digest.
    pub(crate) reach: reach_ui::ReachUi,
    _theme_poll: Option<gpui::Task<()>>,
    _fleet_poll: Option<gpui::Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Shell {
    /// `prefs_path` is the preferences file ([`crate::prefs::default_path`]
    /// in the app; tests point it at a temporary directory).
    pub fn new(
        installation: Installation,
        prefs_path: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Shell {
        let brain_filter =
            cx.new(|cx| TextInput::new("brain-filter", "Filter notes", "Filter notes", cx));
        let fleet_filter =
            cx.new(|cx| TextInput::new("fleet-filter", "Filter sessions", "Filter  /", cx));
        let context_task = cx.new(|cx| {
            TextInput::new(
                "context-task",
                "Task",
                "A task, as an agent would be given it",
                cx,
            )
        });
        let ask_field = cx.new(|cx| {
            TextInput::new(
                "ask-field",
                "Ask",
                "Has any agent already solved this? · or describe a task to brief a new agent",
                cx,
            )
            .bare(16.)
        });
        let mut subscriptions = Vec::new();
        subscriptions.push(
            cx.subscribe(&fleet_filter, |this, input, _: &InputEvent, cx| {
                this.fleet_filter_text = input.read(cx).text().to_owned();
                cx.notify();
            }),
        );
        subscriptions.push(cx.subscribe(&brain_filter, |this, _, _: &InputEvent, cx| {
            this.sync_menus(cx);
            cx.notify();
        }));
        // Reload when the window comes to the front (which lists the theme
        // directories again, too).
        subscriptions.push(cx.observe_window_activation(window, |this, window, cx| {
            this.reach.window_active = window.is_window_active();
            this.sync_key_guard(window.is_window_active(), cx);
            if window.is_window_active() {
                this.reload(cx);
            }
        }));
        // An arrow (or Tab) engages the view shown, for the key guard. An
        // interceptor sees bound keys before their actions run; element key
        // listeners never see them.
        // Esc closes the sidebar's overlay before anything else sees it.
        let shell = cx.entity().downgrade();
        subscriptions.push(cx.intercept_keystrokes(move |e, _, cx| {
            if e.keystroke.key == "escape" && !e.keystroke.modifiers.modified() {
                let closed = shell
                    .update(cx, |s, cx| {
                        let open = s.sidebar.overlay;
                        if open {
                            s.sidebar.overlay = false;
                            cx.notify();
                        }
                        open
                    })
                    .unwrap_or(false);
                if closed {
                    cx.stop_propagation();
                    return;
                }
            }
            let m = &e.keystroke.modifiers;
            if crate::keyguard::is_navigation(e.keystroke.key.as_str())
                && !(m.platform || m.control || m.alt)
            {
                cx.default_global::<crate::keyguard::KeyGuard>()
                    .navigated(std::time::Instant::now());
            }
        }));
        // Show a pending chord in the status bar.
        subscriptions.push(cx.observe_pending_input(window, |_, _, cx| cx.notify()));
        // System follows the light or dark appearance.
        let appearance = cx.observe_window_appearance(window, |this, window, cx| {
            this.themes.follow_appearance(window.appearance());
            if !this.themes.previewing && this.themes.committed().system {
                cx.set_global(this.themes.committed().clone());
                window.refresh();
            }
        });
        subscriptions.push(appearance);

        // The theme: the saved one, or System; Option held at launch means
        // System for this session.
        let workspace = hippocamp_core::config::Config::load_in(&installation.home)
            .ok()
            .and_then(|c| c.workspace);
        let fonts = cx.text_system().all_font_names().into_iter().collect();
        let mut themes = ThemeState::new(
            &installation.home,
            workspace.as_deref(),
            prefs_path,
            fonts,
            window.appearance(),
        );
        let active = themes.launch(window.modifiers().alt, window.appearance());
        cx.set_global(active);

        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let saved = crate::prefs::load(themes.prefs_path());
        let reach = reach_ui::ReachUi::new(&saved, cx.focus_handle());
        let ask_answers = saved.ask_answers;
        let recent_questions = saved.recent_questions.clone();
        let mut shell = Shell {
            state: AppState::default(),
            installation,
            loaded: None,
            reloader: Reloader::default(),
            focus,
            dock_focus: cx.focus_handle().tab_stop(true),
            pane_focus: cx.focus_handle().tab_stop(true),
            dock_scroll: dock::scroll_handle(),
            dock_reveal: std::cell::Cell::new(None),
            brain_filter,
            fleet_filter,
            fleet_filter_text: String::new(),
            fleet_filter_shown: false,
            fleet_profiles_open: false,
            picker: None,
            revert_running: None,
            copied: None,
            review_width: std::rc::Rc::new(std::cell::Cell::new(gpui::px(0.))),
            home_review_width: std::rc::Rc::new(std::cell::Cell::new(gpui::px(0.))),
            health_open: false,
            home_scroll: gpui::ScrollHandle::new(),
            sidebar: sidebar::SidebarUi {
                width: saved.sidebar_width.map(|w| w as f32),
                ..Default::default()
            },
            sidebar_scroll: gpui::ScrollHandle::new(),
            panes_off: std::collections::HashSet::new(),
            page_scroll: gpui::ScrollHandle::new(),
            ask_field,
            ask: ask::AskState::new(cx.focus_handle()),
            ask_answers,
            recent_questions,
            fleet_ui: Default::default(),
            replay: Default::default(),
            fleet_focus: cx.focus_handle(),
            incidents_ui: incidents_ui::IncidentsUi::new(&saved, cx),
            inbox: lessons::InboxUi::new(cx),
            xray: Default::default(),
            memory_ui: memory::MemoryUi::new(cx.focus_handle()),
            manage_ui: manage_page::ManageUi::new(cx),
            brief: brief::BriefUi::new(cx.focus_handle()),
            note_stats: HashMap::new(),
            note_views: HashMap::new(),
            note_pages: HashMap::new(),
            context_task,
            context: Default::default(),
            sessions_ui: Default::default(),
            turns_focus: cx.focus_handle(),
            dream_ui: Default::default(),
            confirm_focus: cx.focus_handle(),
            synced_ctx: None,
            themes,
            flow: crate::flow::Flow::default(),
            title_drag: false,
            reach,
            _theme_poll: None,
            _fleet_poll: None,
            _subscriptions: subscriptions,
        };
        shell.sync_menus(cx);
        shell.reload(cx);
        shell.start_theme_poll(cx);
        shell.start_fleet_poll(cx);
        shell.start_reach(window, cx);
        shell
    }

    /// Saves whether Ask writes answers.
    pub(crate) fn set_ask_answers(&mut self, on: bool, cx: &mut Context<Self>) {
        self.ask_answers = on;
        if let Err(e) = crate::prefs::update(self.themes.prefs_path(), |p| p.ask_answers = on) {
            self.state
                .notices
                .push(format!("The setting could not be saved: {e}"));
        }
        cx.notify();
    }

    /// Every 30 seconds today's fleet is read again (transcript tails and
    /// the caches, read-only): live is a turn in the last two minutes, so a
    /// fleet read at the last reload goes stale. While Home or Fleet shows,
    /// and always while the menu-bar item or a notification is on,
    /// whether or not the window is open.
    fn start_fleet_poll(&mut self, cx: &mut Context<Self>) {
        let task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(30))
                    .await;
                let Ok(Some(cfg)) = this.update(cx, |this, _| {
                    this.loaded
                        .as_ref()
                        .filter(|_| {
                            this.reach.watching()
                                || matches!(
                                    this.state.tabs.section(),
                                    Section::Home | Section::Fleet
                                )
                        })
                        .and_then(|l| l.setup.config().cloned())
                }) else {
                    if this.update(cx, |_, _| ()).is_err() {
                        return;
                    }
                    continue;
                };
                let fleet = cx
                    .background_executor()
                    .spawn(async move { state::load_fleet(&cfg) })
                    .await;
                if this
                    .update(cx, |this, cx| {
                        if let Some(l) = this.loaded.as_mut() {
                            match fleet {
                                Ok(f) => {
                                    l.fleet = Some(f);
                                    l.fleet_error = None;
                                }
                                Err(e) => l.fleet_error = Some(e),
                            }
                        }
                        this.sync_reach(cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
        });
        self._fleet_poll = Some(task);
    }

    /// Memory's notes filter.
    pub(crate) fn brain_filter(&self) -> &Entity<TextInput> {
        &self.brain_filter
    }

    fn ctx(&self, cx: &App) -> Ctx {
        let has_text = !self.brain_filter.read(cx).is_empty();
        let mut ctx = self.state.ctx(
            self.loaded.as_ref().is_some_and(Loaded::ready),
            has_text,
            self.picker.is_some(),
        );
        ctx.active_file = self.active_file().is_some();
        ctx.revert_ready = self.revert_ready();
        ctx.confirm_open = self.dream_ui.confirm.is_some();
        ctx
    }

    /// The workspace's root, once hippocamp is set up.
    pub(crate) fn workspace_root(&self) -> Option<PathBuf> {
        match self.loaded.as_ref().map(|l| &l.setup) {
            Some(hippocamp_view::setup::Setup::Ready { ws, .. }) => Some(ws.root.clone()),
            _ => None,
        }
    }

    /// The file the active tab shows: a note's file or a report.
    pub(crate) fn active_file(&self) -> Option<PathBuf> {
        let panels = self.loaded.as_ref()?.panels.as_ref()?;
        match self.state.tabs.active_tab()? {
            Tab::Note(path) => Some(self.workspace_root()?.join(path)),
            Tab::Memory => Some(self.workspace_root()?.join(self.memory_note()?)),
            Tab::Report(path) => {
                let _ = panels;
                Some(path.clone())
            }
            _ => None,
        }
    }

    /// A session's title: the fleet's, else the rollup's, else what its
    /// transcript head says.
    pub(crate) fn session_title(&self, id: &str) -> Option<String> {
        let loaded = self.loaded.as_ref()?;
        loaded
            .fleet
            .as_ref()
            .and_then(|f| f.row(id))
            .map(|r| r.title.clone())
            .or_else(|| loaded.titles.get(id).cloned())
            .or_else(|| loaded.about.get(id).and_then(|a| a.title.clone()))
    }

    /// A tab's title: a note's title, a session's task, `<note> · <pass>`
    /// for a proposal; the default otherwise.
    pub(crate) fn tab_title(&self, tab: &Tab) -> String {
        if let Tab::Incident(id) = tab
            && let Some(i) = self.incident(id)
        {
            return crate::incidents::short_evidence(&i, 48);
        }
        let panels = self.loaded.as_ref().and_then(|l| l.panels.as_ref());
        let found = panels.and_then(|p| match tab {
            Tab::Note(path) => p
                .brain
                .rows
                .iter()
                .find(|r| &r.path == path)
                .map(|r| r.title.clone()),
            Tab::Session { id, .. } | Tab::Xray(id) => self.session_title(id),
            Tab::Task { session, ord } => self.task_crumbs(session, *ord).map(|c| c.1),
            // A run by when it ran, in the reader's time, as its page and
            // the Dreaming list say it: the file's date is the UTC day its
            // window ends.
            Tab::Report(path) => p
                .dreaming
                .as_ref()
                .ok()?
                .runs
                .iter()
                .find(|r| &r.report.path == path)?
                .written
                .map(|t| {
                    let t = time::OffsetDateTime::from(t).to_offset(status::local_offset());
                    format!(
                        "Run {} {:02}:{:02}",
                        state::edition_title(t.date()),
                        t.hour(),
                        t.minute()
                    )
                }),
            Tab::Proposal(id) => p.history.as_ref().ok().and_then(|h| {
                let change = h.changes.iter().find(|c| c.proposal_id == *id)?;
                let (title, _) = crate::rows::change_text(change);
                let pass = change.proposal.record.as_ref().ok()?.pass.clone();
                Some(format!(
                    "{title} · {}",
                    hippocamp_view::text::visible(&pass)
                ))
            }),
            _ => None,
        });
        found.unwrap_or_else(|| tab.default_title())
    }

    /// Reads the active tab's contents if they have not been read since the
    /// last reload. Blocks briefly: one note.
    fn prepare_active_tab(&mut self) {
        let path = match self.state.tabs.active_tab().cloned() {
            Some(Tab::Note(path)) => path,
            _ => return,
        };
        if self.note_views.contains_key(&path) {
            return;
        }
        let Some(loaded) = self.loaded.as_ref() else {
            return;
        };
        let (Some(panels), hippocamp_view::setup::Setup::Ready { ws, .. }) =
            (loaded.panels.as_ref(), &loaded.setup)
        else {
            return;
        };
        let empty = hippocamp_core::dream::History {
            changes: Vec::new(),
            unreadable: Vec::new(),
        };
        let history = panels.history.as_ref().unwrap_or(&empty);
        let view = hippocamp_view::brain::view(ws, &panels.brain, history, &path)
            .map_err(|e| e.to_string());
        self.note_views.insert(path.clone(), view);
        let rollup = hippocamp_core::rollup::Rollup::open(&self.installation.home)
            .ok()
            .flatten();
        let titles = &loaded.titles;
        let runs: &[hippocamp_view::dreaming::Run] =
            panels.dreaming.as_ref().map_or(&[], |d| d.runs.as_slice());
        let page = hippocamp_view::notepage::page(
            &hippocamp_view::notepage::Inputs {
                root: &ws.root,
                notes_dir: &ws.notes_dir,
                brain: &panels.brain,
                runs,
                rollup: rollup.as_ref(),
                titles,
                today: self.today(),
                offset: status::local_offset(),
            },
            &path,
        );
        if let Some(r) = rollup.as_ref() {
            let stats = hippocamp_view::memory::note_stats(
                r,
                &path,
                time::OffsetDateTime::now_utc(),
                status::local_offset(),
            );
            self.note_stats.insert(path.clone(), stats);
        }
        self.note_pages.insert(path, page);
    }

    /// Back on a section's own view: the view that takes keys gets the
    /// focus (Memory's map, Fleet's rows, the Inbox's cards).
    pub(crate) fn focus_section_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.state.tabs.active_tab() {
            Some(Tab::Memory) => window.focus(&self.memory_ui.focus, cx),
            Some(Tab::Fleet) => window.focus(&self.fleet_focus, cx),
            Some(Tab::Incidents) => window.focus(&self.incidents_ui.focus, cx),
            Some(Tab::Inbox) => window.focus(&self.inbox.focus, cx),
            _ => {}
        }
    }

    /// Command-C outside a text field: copies the active tab's main text (a
    /// note's body, a change's new text, a report, the current turn).
    fn copy_text(&mut self, cx: &mut Context<Self>) {
        if self.state.tabs.active_tab() == Some(&Tab::Fleet) && self.copy_selected_resume(cx) {
            return;
        }
        if let Some(Tab::Xray(id)) = self.state.tabs.active_tab().cloned()
            && self.copy_xray_resume(&id, cx)
        {
            return;
        }
        let text = match self.state.tabs.active_tab() {
            Some(Tab::Ask) => self.selected_resume().inspect(|cmd| {
                self.copied = Some(cmd.clone());
            }),
            Some(Tab::Note(p)) => self
                .note_views
                .get(p)
                .and_then(|v| v.as_ref().ok())
                .map(|v| v.body.clone()),
            Some(Tab::Proposal(id)) => self
                .dream_ui
                .views
                .get(id)
                .and_then(|v| v.as_ref().ok())
                .and_then(|v| v.change.proposal.record.as_ref().ok())
                .map(|p| p.new.clone()),
            Some(Tab::Report(d)) => self
                .dream_ui
                .reports
                .get(d)
                .and_then(|r| r.as_ref().ok())
                .cloned(),
            Some(Tab::Session { id, .. }) => self.current_entry_text(id),
            Some(Tab::Context) => self
                .context
                .result
                .as_ref()
                .and_then(|r| r.as_ref().ok())
                .map(|o| o.text.clone()),
            _ => None,
        };
        if let Some(text) = text {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        }
    }

    /// Command-P: the finder over every note and every session.
    fn open_finder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panels) = self.loaded.as_ref().and_then(|l| l.panels.as_ref()) else {
            return;
        };
        let empty = hippocamp_view::sessions::Sessions { rows: Vec::new() };
        let list = panels.sessions.as_ref().unwrap_or(&empty);
        let titles = self
            .loaded
            .as_ref()
            .map(|l| l.titles.clone())
            .unwrap_or_default();
        let rows = hippocamp_view::finder::rows(&panels.brain, list, |id| titles.get(id).cloned());
        let items = rows
            .iter()
            .map(|r| {
                let mut item = picker::PickerItem::new(r.text.clone());
                item.keys = Some(
                    match r.kind {
                        hippocamp_view::finder::FinderKind::Note => "note",
                        hippocamp_view::finder::FinderKind::Session => "session",
                    }
                    .to_string(),
                );
                item
            })
            .collect();
        let restore = window.focused(cx);
        let entity = cx.new(|cx| {
            Picker::new(
                "finder",
                "Find a note or a session",
                "Note title or path, session id or task",
                items,
                window,
                cx,
            )
        });
        let subscription = cx.subscribe_in(
            &entity,
            window,
            move |this, _, event: &PickerEvent, window, cx| match event {
                PickerEvent::Confirmed(i) => {
                    this.close_picker(window, cx);
                    if let Some(row) = rows.get(*i) {
                        let tab = match &row.key {
                            hippocamp_view::finder::FinderKey::Note(p) => Tab::Note(p.clone()),
                            hippocamp_view::finder::FinderKey::Session(id) => Tab::Session {
                                id: id.clone(),
                                ordinal: None,
                            },
                        };
                        // A session opened from the finder takes the keys,
                        // as Return in a list does: arrows walk its rows.
                        let session = matches!(tab, Tab::Session { .. });
                        this.state.tabs.open(tab);
                        if session {
                            window.focus(&this.turns_focus, cx);
                        }
                        this.after_state_change(window, cx);
                    }
                }
                PickerEvent::Dismissed => this.close_picker(window, cx),
                PickerEvent::Selected(_) => {}
            },
        );
        self.picker = Some(OpenPicker {
            entity,
            restore,
            theme_before: None,
            _subscription: subscription,
        });
        self.after_state_change(window, cx);
    }

    /// Rebuilds the menu bar when what can run has changed.
    fn sync_menus(&mut self, cx: &mut Context<Self>) {
        let ctx = self.ctx(cx);
        if self.synced_ctx != Some(ctx) {
            self.synced_ctx = Some(ctx);
            cx.set_menus(menus::build(&ctx));
        }
    }

    pub(crate) fn after_state_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let _ = window;
        self.sync_menus(cx);
        cx.notify();
    }

    // ---- reloading --------------------------------------------------

    /// Reads everything again on the background executor while the old view
    /// stays up. One reload runs and at most one waits.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.rescan_themes(cx);
        if self.reloader.request() {
            self.start_reload(cx);
        }
    }

    fn start_reload(&mut self, cx: &mut Context<Self>) {
        let home = self.installation.home.clone();
        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_executor()
                .spawn(async move { state::load(&home, OffsetDateTime::now_utc()) })
                .await;
            this.update(cx, |this, cx| {
                let mut loaded = loaded;
                if let hippocamp_view::setup::Setup::Ready { ws, .. } = &loaded.setup {
                    let packs = ws.root.join(".hippocamp").join("packs");
                    if this.themes.packs_dir() != Some(packs.as_path()) {
                        this.themes.set_workspace(Some(&ws.root));
                        this.rescan_themes(cx);
                    }
                }
                // What was read for opened sessions stays: their turns are
                // read again when they show.
                if let Some(old) = this.loaded.take() {
                    loaded.turns = old.turns;
                    loaded.about = old.about;
                }
                // The note selected in Memory stays while it exists.
                let notes: std::collections::HashSet<String> = loaded
                    .panels
                    .as_ref()
                    .map(|p| {
                        p.brain
                            .rows
                            .iter()
                            .map(|r| r.path.display().to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                if loaded.panels.is_some()
                    && let Some(notice) = this
                        .state
                        .selections
                        .reconcile(Section::Memory, |id| notes.contains(id))
                {
                    this.state.notices.push(notice);
                }
                this.loaded = Some(loaded);
                this.decide_onboarding();
                this.note_views.clear();
                this.note_pages.clear();
                let kept: Vec<(String, gpui::ListOffset)> = this
                    .sessions_ui
                    .turns
                    .iter()
                    .filter_map(|(id, v)| {
                        Some((id.clone(), v.as_ref().ok()?.list.logical_scroll_top()))
                    })
                    .collect();
                this.sessions_ui.kept_scroll.extend(kept);
                this.sessions_ui.turns.clear();
                this.dream_ui.views.clear();
                this.dream_ui.reports.clear();
                this.sync_menus(cx);
                this.sync_reach(cx);
                if this.reloader.finish() {
                    this.start_reload(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Whether a reload is running (for tests).
    #[cfg(test)]
    pub fn reloading(&self) -> bool {
        self.reloader.running()
    }

    // ---- actions ----------------------------------------------------

    /// Keeps the sidebar's dragged width in the preferences.
    pub(crate) fn save_sidebar_width(&self) {
        let w = self.sidebar.width.map(|w| w.round().max(0.) as u32);
        let _ = crate::prefs::update(self.themes.prefs_path(), |p| p.sidebar_width = w);
    }

    /// Tells the key guard whether the window is active. Coming forward
    /// clears the Inbox's selection, so no card waits under a stray Return.
    pub(crate) fn sync_key_guard(&mut self, active: bool, cx: &mut Context<Self>) {
        let now = std::time::Instant::now();
        let g = cx.default_global::<crate::keyguard::KeyGuard>();
        match (active, g.is_active()) {
            (true, false) => g.activated(now),
            (false, true) => g.deactivated(),
            _ => return,
        }
        if self.inbox.editing.is_none() {
            self.inbox.selected = None;
        }
    }

    /// Runs the action `id` if it can run now.
    pub(crate) fn run(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(def) = TABLE.iter().find(|d| d.id == id) else {
            return;
        };
        if def.check(&self.ctx(cx)).is_err() {
            return;
        }
        // A key that writes acts only in a settled, engaged window.
        if KEYED_WRITES.contains(&id) && !crate::keyguard::allow(cx) {
            cx.notify();
            return;
        }
        if def.palette.is_some() {
            self.state.note_action(id);
        }
        match id {
            "About" => self.open_about(cx),
            "Quit" => cx.quit(),
            "CommandPalette" => self.toggle_palette(window, cx),
            "Find" => {
                if self.picker.is_some() {
                    self.close_picker(window, cx);
                }
                self.open_finder(window, cx);
            }
            "ContextPreview" => {
                self.state.tabs.open(Tab::Context);
                window.focus(&self.context_task.focus_handle(cx), cx);
            }
            "RunContext" => self.run_context(cx),
            "FocusAsk" => self.focus_ask(window, cx),
            "AskSubmit" if self.brief.mode => self.submit_brief(window, cx),
            "AskSubmit" => self.submit_ask(false, window, cx),
            "ToggleBrief" => {
                let on = !self.brief.mode;
                self.set_brief_mode(on, window, cx);
            }
            // In Brief, ⌘↵ is Send to T3 Code, behind the key
            // guard.
            "AskSourcesOnly" if self.brief.mode => self.brief_send_key(cx),
            "AskSourcesOnly" => self.submit_ask(true, window, cx),
            "SourceNext" => self.step_source(1),
            "SourcePrevious" => self.step_source(-1),
            "SourceOpen" => self.open_source(self.ask.selected, window, cx),
            "InboxNext" => self.step_inbox(1),
            "InboxPrevious" => self.step_inbox(-1),
            "InboxKeep" => self.act_on_selected('k', window, cx),
            "InboxEdit" => self.act_on_selected('e', window, cx),
            "InboxDismiss" => self.act_on_selected('d', window, cx),
            "LessonSave" => self.save_edit(window, cx),
            "LessonCancel" => self.cancel_edit(window, cx),
            // A sheet over the page takes Escape first (on an
            // incident's page, which is deeper than its section, Escape
            // would otherwise go back with the sheet left open).
            "GoBack" if self.incidents_ui.hook_sheet.is_some() => {
                self.incidents_ui.hook_sheet = None;
                window.focus(&self.focus, cx);
            }
            "GoBack" if self.manage_ui.confirm.is_some() => {
                self.manage_ui.confirm = None;
                self.manage_ui.budget_drag = None;
                window.focus(&self.focus, cx);
            }
            "GoBack" => {
                // Focus inside the view that goes would be left on nothing,
                // and no key would reach the window: give it to the window.
                let inside = self.turns_focus.is_focused(window)
                    || self.ask.focus.is_focused(window)
                    || self.ask_field.focus_handle(cx).is_focused(window)
                    || self.pane_focus.contains_focused(window, cx);
                self.state.tabs.back();
                if inside {
                    window.focus(&self.focus, cx);
                }
                self.focus_section_view(window, cx);
            }
            "FoldTurn" => self.fold_turn(),
            "ToggleChanges" => self.toggle_changes(),
            "TurnNext" => self.step_turn(1),
            "TurnPrevious" => self.step_turn(-1),
            "TurnFirst" => self.edge_turn(false),
            "TurnLast" => self.edge_turn(true),
            "AskRevert" => self.ask_revert(window, cx),
            "ConfirmRevert" => self.confirm_revert(window, cx),
            "CancelRevert" => self.cancel_revert(window, cx),
            "OpenNewestReport" => self.open_newest_report(),
            "NeedsAttention" => self.state.tabs.open(Tab::Attention),
            "CopyText" => self.copy_text(cx),
            "OpenInEditor" => {
                if let Some(file) = self.active_file() {
                    cx.open_with_system(&file);
                }
            }
            "RevealInFinder" => {
                if let Some(file) = self.active_file() {
                    cx.reveal_path(&file);
                }
            }
            "ToggleDock" => {
                // Focus inside a list that hides would be left on nothing.
                let inside = self.dock_focus.contains_focused(window, cx)
                    || self.brain_filter.focus_handle(cx).is_focused(window);
                self.state.dock.visible = !self.state.dock.visible;
                if !self.state.dock.visible && inside {
                    window.focus(&self.focus, cx);
                }
            }
            "ToggleSidebar" => self.toggle_sidebar(window),
            "ShowHome" => self.show_section(Section::Home, window, cx),
            "ShowInbox" => self.show_section(Section::Inbox, window, cx),
            "ShowFleet" => self.show_section(Section::Fleet, window, cx),
            "ShowIncidents" => self.show_section(Section::Incidents, window, cx),
            "ShowMemory" => self.show_section(Section::Memory, window, cx),
            "ShowManage" => self.show_section(Section::Manage, window, cx),
            "FocusPane" => window.focus(&self.pane_focus, cx),
            "FocusFilter" => {
                self.state.dock.visible = true;
                window.focus(&self.brain_filter.focus_handle(cx), cx);
            }
            "ClearFilter" => {
                let filter = self.brain_filter.clone();
                if filter.read(cx).is_empty() {
                    window.focus(&self.dock_focus, cx);
                } else {
                    filter.update(cx, |f, cx| f.set_text("", window, cx));
                }
            }
            // On the Empty page, ⌘R looks for history again.
            "Reload" if self.flow.page == Some(crate::flow::Page::Empty) => {
                self.look_for_history(cx)
            }
            "Reload" => self.reload(cx),
            "TextLarger" => self.change_scale(window, cx, |s| s.larger()),
            "TextSmaller" => self.change_scale(window, cx, |s| s.smaller()),
            "TextDefault" => self.change_scale(window, cx, |s| s.reset()),
            "SelectTheme" => self.open_theme_picker(window, cx),
            "UseSystemTheme" => self.use_system_theme(window, cx),
            "Help" => self.state.tabs.open(Tab::Help),
            "Minimize" => window.minimize_window(),
            "Zoom" => window.zoom_window(),
            "DockDown" => self.dock_step(1, window, cx),
            "DockUp" => self.dock_step(-1, window, cx),
            "DockOpen" => self.dock_open(cx),
            _ => {}
        }
        self.after_state_change(window, cx);
    }

    fn change_scale(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut state::TextScale),
    ) {
        f(&mut self.state.scale);
        cx.set_global(UiScale(self.state.scale.value()));
        window.refresh();
    }

    /// Shows a section: the top of its stack, or, when it is already
    /// shown, its own view. Memory opens its notes list, focused.
    pub(crate) fn show_section(
        &mut self,
        section: Section,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.health_open = false;
        self.sidebar.overlay = false;
        self.state.tabs.select(section);
        if section == Section::Memory {
            window.focus(&self.memory_ui.focus, cx);
        } else if section == Section::Manage {
            self.manage_ui.data = None;
            if !cfg!(test) {
                self.reach.login = crate::menubar::login::status();
            }
            window.focus(&self.focus, cx);
        } else if section == Section::Inbox {
            self.inbox.editing = None;
            window.focus(&self.inbox.focus, cx);
        } else if section == Section::Incidents {
            self.incidents_ui.menu = None;
            self.mark_incidents_seen();
            window.focus(&self.incidents_ui.focus, cx);
        } else {
            window.focus(&self.focus, cx);
        }
        self.after_state_change(window, cx);
    }

    fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picker.is_some() {
            self.close_picker(window, cx);
        } else {
            self.open_palette(window, cx);
        }
    }

    pub fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ctx = self.ctx(cx);
        let (defs, items) = palette::rows(&ctx, &self.state.ran);
        let restore = window.focused(cx);
        let entity = cx.new(|cx| {
            Picker::new(
                "palette",
                "Command palette",
                "Type a command",
                items,
                window,
                cx,
            )
        });
        let subscription = cx.subscribe_in(
            &entity,
            window,
            move |this, _, event: &PickerEvent, window, cx| match event {
                PickerEvent::Confirmed(i) => {
                    this.close_picker(window, cx);
                    if let Some(def) = defs.get(*i) {
                        window.dispatch_action(def.action(), cx);
                    }
                }
                PickerEvent::Dismissed => this.close_picker(window, cx),
                PickerEvent::Selected(_) => {}
            },
        );
        self.picker = Some(OpenPicker {
            entity,
            restore,
            theme_before: None,
            _subscription: subscription,
        });
        self.after_state_change(window, cx);
    }

    /// Closes the open picker and returns focus to where it was. A theme
    /// picker closed without a choice puts the theme in use back.
    pub fn close_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = self.picker.take() {
            if let Some(before) = &open.theme_before {
                self.restore_theme(before, window, cx);
            }
            match open.restore {
                Some(handle) => window.focus(&handle, cx),
                None => window.focus(&self.focus, cx),
            }
        }
        self.after_state_change(window, cx);
    }

    #[cfg(test)]
    pub fn picker_open(&self) -> bool {
        self.picker.is_some()
    }

    #[cfg(test)]
    pub fn state(&self) -> &AppState {
        &self.state
    }

    #[cfg(test)]
    pub fn loaded(&self) -> Option<&Loaded> {
        self.loaded.as_ref()
    }

    fn open_about(&mut self, cx: &mut Context<Self>) {
        let installation = self.installation.clone();
        let bounds = Bounds::centered(None, size(px(460.), px(280.)), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("About Hippocamp".into()),
                    ..Default::default()
                }),
                is_resizable: false,
                ..Default::default()
            },
            |window, cx| {
                cx.new(|cx| {
                    let focus = cx.focus_handle();
                    window.focus(&focus, cx);
                    about::About {
                        installation: installation.clone(),
                        cli_version: None,
                        focus,
                    }
                })
            },
        );
        let Ok(handle) = opened else { return };
        let installation = self.installation.clone();
        let workspace = match self.loaded.as_ref().map(|l| &l.setup) {
            Some(hippocamp_view::setup::Setup::Ready { ws, .. }) => ws.root.clone(),
            _ => installation.home.clone(),
        };
        cx.spawn(async move |_, cx| {
            let version = cx
                .background_executor()
                .spawn(async move {
                    cli::run(
                        &installation.binary,
                        &installation.home,
                        &workspace,
                        &cli::version_args(),
                        cli::Limits::version(),
                    )
                })
                .await;
            let text = match version {
                Ok(out) if out.success() => out.stdout.trim().to_string(),
                Ok(out) if out.timed_out => "did not answer in 10 seconds".to_string(),
                Ok(out) => format!("failed: {}", out.stderr.trim()),
                Err(e) => format!("not found: {e}"),
            };
            handle
                .update(cx, |about, _, cx| {
                    about.cli_version = Some(text);
                    cx.notify();
                })
                .ok();
        })
        .detach();
    }
}

impl Focusable for Shell {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// Actions bound only to keys that write: they pass the key guard first.
/// (Clicks on the same buttons call their methods directly.)
pub(crate) const KEYED_WRITES: &[&str] = &[
    "InboxKeep",
    "InboxDismiss",
    "InboxEdit",
    "LessonSave",
    "ConfirmRevert",
];

macro_rules! wire {
    ($el:expr, $cx:expr, [$($a:ident),* $(,)?]) => {{
        let el = $el;
        $( let el = el.on_action($cx.listener(|this, _: &actions::$a, window, cx| {
            this.run(stringify!($a), window, cx)
        })); )*
        el
    }};
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.prepare_active_tab();
        // Profile marks are unique among every profile the app knows
        // (Manage's never-used folders too).
        if let Some(f) = self.loaded.as_ref().and_then(|l| l.fleet.as_ref()) {
            let managed: Vec<String> = self
                .manage_ui
                .data
                .as_ref()
                .map(|m| m.agents.iter().map(|a| a.name()).collect())
                .unwrap_or_default();
            kit::sync_profiles_with(
                f.headroom
                    .iter()
                    .map(|h| h.profile.as_str())
                    .chain(f.rows.iter().filter_map(|r| r.profile.as_deref())),
                managed.iter().map(String::as_str),
            );
        }
        // The key guard: the view on screen, and whether the window is
        // active (the observer is the main path; this catches a launch).
        let shown = self.state.tabs.active_tab().cloned();
        cx.default_global::<crate::keyguard::KeyGuard>().show(shown);
        self.sync_key_guard(window.is_window_active(), cx);
        let look = Look::from_app(cx);
        window.set_rem_size(look.px(theme::BASE));
        let ready = self.loaded.as_ref().is_some_and(Loaded::ready);
        let message = self
            .loaded
            .as_ref()
            .and_then(|l| setup::message(&l.setup, &self.installation.home));

        let page = self.onboarding_page();
        let has_message = message.is_some();
        // The Inbox's keys need its cards focused: when the window's root
        // (or nothing) holds the focus there, the cards take it.
        if page.is_none()
            && self.state.tabs.active_tab() == Some(&Tab::Fleet)
            && self.picker.is_none()
            && (self.focus.is_focused(window) || window.focused(cx).is_none())
        {
            window.focus(&self.fleet_focus, cx);
        }
        if page.is_none()
            && self.state.tabs.active_tab() == Some(&Tab::Incidents)
            && self.picker.is_none()
            && self.incidents_ui.hook_sheet.is_none()
            && self.manage_ui.confirm.is_none()
            && (self.focus.is_focused(window) || window.focused(cx).is_none())
        {
            window.focus(&self.incidents_ui.focus, cx);
        }
        if page.is_none()
            && self.state.tabs.active_tab() == Some(&Tab::Memory)
            && self.picker.is_none()
            && (self.focus.is_focused(window) || window.focused(cx).is_none())
        {
            window.focus(&self.memory_ui.focus, cx);
        }
        if page.is_none()
            && self.state.tabs.active_tab() == Some(&Tab::Inbox)
            && self.inbox.editing.is_none()
            && self.picker.is_none()
            && (self.focus.is_focused(window) || window.focused(cx).is_none())
        {
            window.focus(&self.inbox.focus, cx);
        }
        let body = match message {
            _ if page.is_some() => {
                let page = page.expect("checked");
                self.flow.compact = window.viewport_size().height < gpui::px(680.);
                self.flow.compact_width = window.viewport_size().width < gpui::px(900.);
                // A page that replaced the focused control leaves nothing
                // focused; Return (the ↩ buttons) needs the window's root.
                if !self.focus.contains_focused(window, cx) {
                    window.focus(&self.focus, cx);
                }
                self.render_onboarding(page, &look, cx)
            }
            Some(m) => self.render_setup(&m, &look, cx).into_any_element(),
            None => {
                let pane = self.render_pane(&look, window, cx).into_any_element();
                // Memory's map and list replace the old notes list.
                let dock = (ready && self.state.dock.visible && false)
                    .then(|| self.render_dock(&look, window, cx).into_any_element());
                let _ = &dock;
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .child(pane)
                    .into_any_element()
            }
        };

        let picker = self.picker.as_ref().map(|p| p.entity.clone());
        let confirmation = self.render_confirmation(&look, cx);
        let step_two = self.render_step_two(&look, cx);
        let root = div()
            .id("workspace")
            .key_context("Workspace")
            .track_focus(&self.focus)
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(look.c.window)
            .text_color(look.c.text)
            .text_size(look.text(theme::BASE))
            .font_family(
                look.font_family
                    .clone()
                    .unwrap_or_else(|| ".SystemUIFont".to_string()),
            )
            // Tabular figures everywhere (ages
            // and counts tick without moving what is beside them).
            .font_features(gpui::FontFeatures(std::sync::Arc::new(vec![(
                "tnum".to_owned(),
                1,
            )])))
            // A real press, or an arrow once settled, engages the view shown.
            .capture_any_mouse_down(|_, _, cx| {
                cx.default_global::<crate::keyguard::KeyGuard>().clicked();
            })
            .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, window, cx| {
                let field = this.ask_field.read(cx).focused(window);
                if this.onboarding_key(e.keystroke.key.as_str(), field, cx) {
                    cx.stop_propagation();
                    return;
                }
                let m = &e.keystroke.modifiers;
                if !field
                    && !(m.platform || m.control || m.alt || m.shift)
                    && this.task_key(e.keystroke.key.as_str(), window, cx)
                {
                    cx.stop_propagation();
                }
            }))
            // The sidebar's edge, dragged.
            .on_mouse_move(cx.listener(|this, e: &gpui::MouseMoveEvent, window, cx| {
                if !this.sidebar.dragging {
                    return;
                }
                if e.pressed_button != Some(gpui::MouseButton::Left) {
                    this.sidebar.dragging = false;
                    this.save_sidebar_width();
                    return;
                }
                let w = f32::from(window.viewport_size().width);
                let x = f32::from(e.position.x) / f32::from(Look::from_app(cx).px(1.0));
                this.sidebar.width = Some(sidebar::clamp_width(x, w));
                cx.notify();
            }))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.sidebar.dragging {
                        this.sidebar.dragging = false;
                        this.save_sidebar_width();
                        cx.notify();
                    }
                }),
            )
            .on_drag_move(
                cx.listener(|this, e: &DragMoveEvent<DragDock>, window, cx| {
                    let look = Look::from_app(cx);
                    let x = f32::from(e.event.position.x - e.bounds.left());
                    this.state.dock.drag_to(x, look.px(1.0).into());
                    window.refresh();
                    cx.notify();
                }),
            );
        let root = wire!(
            root,
            cx,
            [
                About,
                Quit,
                CommandPalette,
                Find,
                SelectTheme,
                UseSystemTheme,
                ToggleDock,
                ToggleSidebar,
                ShowHome,
                ShowInbox,
                ShowFleet,
                ShowIncidents,
                ShowMemory,
                ShowManage,
                FocusAsk,
                GoBack,
                AskSubmit,
                AskSourcesOnly,
                ToggleBrief,
                SourceNext,
                SourcePrevious,
                SourceOpen,
                InboxNext,
                InboxPrevious,
                InboxKeep,
                InboxEdit,
                InboxDismiss,
                LessonSave,
                LessonCancel,
                FocusPane,
                FocusFilter,
                ClearFilter,
                ContextPreview,
                ToggleChanges,
                DockDown,
                DockUp,
                DockOpen,
                RunContext,
                FoldTurn,
                TurnNext,
                TurnPrevious,
                TurnFirst,
                TurnLast,
                CopyText,
                OpenInEditor,
                RevealInFinder,
                OpenNewestReport,
                NeedsAttention,
                AskRevert,
                ConfirmRevert,
                CancelRevert,
                Reload,
                TextLarger,
                TextSmaller,
                TextDefault,
                Help,
                Minimize,
                Zoom,
            ]
        );
        // The sidebar runs the window's height; the title bar and
        // the view share the column beside it. No status bar: health and
        // `indexed` are in the sidebar's foot, key hints in the buttons.
        let shell_ready = ready && page.is_none() && !has_message;
        let docked = self.sidebar_docked(window);
        if docked || !shell_ready {
            self.sidebar.overlay = false;
        }
        let sidebar = (shell_ready && docked)
            .then(|| self.render_sidebar(&look, window, cx).into_any_element());
        // Where it does not dock, the full sidebar opens over the
        // content; a click outside or Esc closes it.
        // It comes in as T3 Code's sheet does, 200 ms ease-in-out,
        // sliding 32 points from the left as it fades in, the scrim fading
        // with it.
        let scrim = if look.c.dark { 0.45 } else { 0.25 };
        let sheet_in = || {
            gpui::Animation::new(std::time::Duration::from_millis(200))
                .with_easing(gpui::ease_in_out)
        };
        let overlay = (shell_ready && !docked && self.sidebar.overlay).then(|| {
            div()
                .id("sidebar-overlay")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .occlude()
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.sidebar.overlay = false;
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .id("sidebar-overlay-panel")
                        .relative()
                        .h_full()
                        .w(look.px(self.sidebar_own_width(window)))
                        .shadow_lg()
                        .occlude()
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(self.render_sidebar(&look, window, cx))
                        .with_animation("sidebar-overlay-panel-in", sheet_in(), |d, t| {
                            d.left(gpui::px(-32. * (1. - t))).opacity(t)
                        }),
                )
                .with_animation("sidebar-overlay-in", sheet_in(), move |d, t| {
                    d.bg(gpui::black().opacity(scrim * t))
                })
                .into_any_element()
        });
        root.child(
            div()
                .flex()
                .flex_row()
                .flex_1()
                .min_h_0()
                .children(sidebar)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .bg(look.c.canvas)
                        .child(self.render_title_bar(&look, window, cx))
                        .child(body),
                ),
        )
        .children(overlay)
        .children(confirmation)
        .children(self.render_manage_confirm(&look, cx))
        .children(self.render_hook_sheet(&look, cx))
        .children(self.render_incident_toast(&look, cx))
        .children(self.render_resume_confirm(&look, cx))
        .children(step_two)
        .children(self.render_health_popover(&look, cx))
        .when_some(picker, |d, picker| {
            d.child(
                div()
                    .id("picker-overlay")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .justify_center()
                    .pt(look.px(32. + 72.))
                    .occlude()
                    .child(
                        div()
                            .on_mouse_down_out(
                                cx.listener(|this, _, window, cx| this.close_picker(window, cx)),
                            )
                            .child(picker),
                    ),
            )
        })
    }
}

/// The window's options: 1200 by 800 points, centred, at least 760 by 520, with the
/// title bar drawn by the app.
pub fn window_options(cx: &App) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(1200.), px(800.)),
            cx,
        ))),
        titlebar: Some(gpui::TitlebarOptions {
            title: Some("Hippocamp".into()),
            appears_transparent: true,
            // In the vertical middle of the 36-point title-bar row.
            traffic_light_position: Some(point(px(16.), px(15.))),
        }),
        // The app's title bar starts the drag itself, as Zed's does, which
        // avoids AppKit's click delay on the title bar.
        app_owns_titlebar_drag: true,
        window_min_size: Some(size(px(760.), px(520.))),
        ..Default::default()
    }
}

/// Sets the globals every view reads: the theme in use and the text scale.
pub fn init_globals(cx: &mut App) {
    cx.set_global(ActiveTheme::system(cx.window_appearance()));
    cx.set_global(UiScale(1.0));
    cx.set_global(crate::keyguard::KeyGuard::default());
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // tests build a temporary hippocamp home
mod tests {
    use super::picker::Picker;
    use super::*;
    use crate::actions::{Cancel, CommandPalette, Confirm, Help, Reload, ToggleDock};
    use crate::cli::Source;
    use gpui::Action;
    use gpui::{Entity, TestAppContext, VisualTestContext};
    use hippocamp_core::config::{InitOptions, init_in};
    use std::path::Path;

    fn installation(home: &Path) -> Installation {
        Installation {
            binary: home.join("no-hippocamp"),
            home: home.to_path_buf(),
            source: Source::Fallback("test".into()),
        }
    }

    fn setup_globals(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.bind_keys(actions::key_bindings());
            cx.bind_keys(input::key_bindings());
            init_globals(cx);
        });
    }

    fn open(
        cx: &mut TestAppContext,
        home: &Path,
    ) -> (gpui::WindowHandle<Shell>, VisualTestContext) {
        // No preferences file: System, whatever this machine has saved.
        open_with_prefs(cx, home, home.with_file_name("preferences.json"))
    }

    fn open_with_prefs(
        cx: &mut TestAppContext,
        home: &Path,
        prefs_path: std::path::PathBuf,
    ) -> (gpui::WindowHandle<Shell>, VisualTestContext) {
        let installation = installation(home);
        let window = cx.update(|cx| {
            cx.open_window(window_options(cx), |window, cx| {
                cx.new(|cx| Shell::new(installation, prefs_path, window, cx))
            })
            .unwrap()
        });
        let vcx = VisualTestContext::from_window(window.into(), cx);
        vcx.executor().allow_parking();
        vcx.run_until_parked();
        (window, vcx)
    }

    fn ready_home() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&ws).unwrap();
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
        (dir, home)
    }

    fn dispatch(vcx: &mut VisualTestContext, window: gpui::WindowHandle<Shell>, a: &dyn Action) {
        let handle = window
            .root(vcx)
            .unwrap()
            .read_with(vcx, |s, cx| s.focus_handle(cx));
        vcx.update(|w, cx| handle.dispatch_action(a, w, cx));
        vcx.run_until_parked();
    }

    /// Every action of the table is wired to `Shell::run` on the root, or
    /// its key binding does nothing (as Return in the context field once
    /// did). Picker actions are handled by the picker.
    #[test]
    fn every_action_is_wired_to_the_shell() {
        let src = include_str!("mod.rs");
        let start = src.find("let root = wire!(").unwrap();
        let block = &src[start..start + src[start..].find(']').unwrap()];
        for def in crate::actions::TABLE {
            if ["SelectNext", "SelectPrevious", "Confirm", "Cancel"].contains(&def.id) {
                continue;
            }
            assert!(
                block.contains(&format!("{},", def.id)),
                "{} is not wired",
                def.id
            );
        }
    }

    #[gpui::test]
    fn an_empty_home_shows_the_setup_message_and_opens_no_panels(cx: &mut TestAppContext) {
        setup_globals(cx);
        let dir = tempfile::tempdir().unwrap();
        let (window, mut vcx) = open(cx, &dir.path().join("none"));
        let root = window.root(&mut vcx).unwrap();
        root.read_with(&vcx, |shell, _| {
            let loaded = shell.loaded().expect("the first reload ended");
            assert!(!loaded.ready());
            assert!(setup::message(&loaded.setup, &shell.installation.home).is_some());
        });
    }

    #[gpui::test]
    fn a_ready_workspace_loads_and_the_window_draws(cx: &mut TestAppContext) {
        setup_globals(cx);
        let (_dir, home) = ready_home();
        let (window, mut vcx) = open(cx, &home);
        let root = window.root(&mut vcx).unwrap();
        root.read_with(&vcx, |shell, _| {
            let loaded = shell.loaded().unwrap();
            assert!(loaded.ready(), "{:?}", loaded.setup);
            assert!(loaded.status.is_some());
            assert!(!shell.reloading());
        });
    }

    #[gpui::test]
    fn the_palette_opens_runs_an_action_and_does_nothing_on_a_dimmed_row(cx: &mut TestAppContext) {
        setup_globals(cx);
        let (_dir, home) = ready_home();
        let (window, mut vcx) = open(cx, &home);
        let root = window.root(&mut vcx).unwrap();

        dispatch(&mut vcx, window, &CommandPalette);
        assert!(root.read_with(&vcx, |s, _| s.picker_open()));

        // Return on a dimmed row (Revert, with no change selected) does nothing.
        let picker = root.read_with(&vcx, |s, _| s.picker.as_ref().unwrap().entity.clone());
        vcx.update(|w, cx| picker.update(cx, |p, cx| p.set_query("Revert Change", w, cx)));
        let handle = picker.read_with(&vcx, |p, cx| p.focus_handle(cx));
        vcx.update(|w, cx| handle.dispatch_action(&Confirm, w, cx));
        vcx.run_until_parked();
        assert!(root.read_with(&vcx, |s, _| s.picker_open()), "still open");
        assert_eq!(
            root.read_with(&vcx, |s, _| s.state().ran.clone()),
            ["CommandPalette"]
        );

        // Return on Memory runs it and closes the palette.
        vcx.update(|w, cx| picker.update(cx, |p, cx| p.set_query("View: Memory", w, cx)));
        vcx.update(|w, cx| handle.dispatch_action(&Confirm, w, cx));
        vcx.run_until_parked();
        root.read_with(&vcx, |s, _| {
            assert!(!s.picker_open());
            assert_eq!(s.state().tabs.section(), Section::Memory, "Memory ran");
            assert_eq!(s.state().ran, ["ShowMemory", "CommandPalette"]);
        });

        // Escape closes the palette.
        dispatch(&mut vcx, window, &CommandPalette);
        assert!(root.read_with(&vcx, |s, _| s.picker_open()));
        let picker = root.read_with(&vcx, |s, _| s.picker.as_ref().unwrap().entity.clone());
        let handle = picker.read_with(&vcx, |p, cx| p.focus_handle(cx));
        vcx.update(|w, cx| handle.dispatch_action(&Cancel, w, cx));
        vcx.run_until_parked();
        assert!(!root.read_with(&vcx, |s, _| s.picker_open()));
    }

    #[gpui::test]
    fn actions_change_the_section_the_list_and_text_size(cx: &mut TestAppContext) {
        setup_globals(cx);
        let (_dir, home) = ready_home();
        let (window, mut vcx) = open(cx, &home);
        let root = window.root(&mut vcx).unwrap();

        dispatch(&mut vcx, window, &actions::ShowMemory);
        dispatch(&mut vcx, window, &ToggleDock);
        assert!(!root.read_with(&vcx, |s, _| s.state().dock.visible));
        dispatch(&mut vcx, window, &ToggleDock);
        assert!(root.read_with(&vcx, |s, _| s.state().dock.visible));

        dispatch(&mut vcx, window, &actions::ShowFleet);
        assert_eq!(
            root.read_with(&vcx, |s, _| s.state().tabs.active_tab().cloned()),
            Some(Tab::Fleet)
        );

        dispatch(&mut vcx, window, &Help);
        dispatch(&mut vcx, window, &Help);
        root.read_with(&vcx, |s, _| {
            assert_eq!(
                s.state().tabs.stack(),
                &[Tab::Fleet, Tab::Help],
                "one per item"
            );
        });
        dispatch(&mut vcx, window, &actions::GoBack);
        assert_eq!(root.read_with(&vcx, |s, _| s.state().tabs.depth()), 1);

        dispatch(&mut vcx, window, &actions::TextLarger);
        assert_eq!(root.read_with(&vcx, |s, _| s.state().scale.percent()), 110);
        dispatch(&mut vcx, window, &actions::TextDefault);
        assert_eq!(root.read_with(&vcx, |s, _| s.state().scale.percent()), 100);

        // A reload asked for twice while one runs still ends.
        dispatch(&mut vcx, window, &Reload);
        dispatch(&mut vcx, window, &Reload);
        assert!(!root.read_with(&vcx, |s, _| s.reloading()));
    }

    /// Choosing a section shows the top of its stack and keeps the others';
    /// choosing it again goes back to its own view.
    #[gpui::test]
    fn a_section_keeps_its_stack_and_memory_shows_a_note(cx: &mut TestAppContext) {
        setup_globals(cx);
        let (dir, home) = ready_home();
        let notes = dir.path().join("ws/notes");
        std::fs::create_dir_all(&notes).unwrap();
        std::fs::write(notes.join("index.md"), "# Index\n\nThe entry note.\n").unwrap();
        std::fs::write(notes.join("other.md"), "# Other\n\nA second note.\n").unwrap();
        let (window, mut vcx) = open(cx, &home);
        let root = window.root(&mut vcx).unwrap();
        let active = |vcx: &mut VisualTestContext| {
            root.read_with(vcx, |s, _| s.state().tabs.active_tab().cloned())
        };
        assert_eq!(active(&mut vcx), Some(Tab::Home), "Home at launch");
        dispatch(&mut vcx, window, &Help);
        dispatch(&mut vcx, window, &actions::ShowMemory);
        assert_eq!(active(&mut vcx), Some(Tab::Memory));
        // Moving down the notes list shows a note on top of Memory.
        dispatch(&mut vcx, window, &actions::DockDown);
        assert!(
            matches!(active(&mut vcx), Some(Tab::Note(_))),
            "{:?}",
            active(&mut vcx)
        );
        dispatch(&mut vcx, window, &actions::ShowHome);
        assert_eq!(active(&mut vcx), Some(Tab::Help), "Home kept its stack");
        dispatch(&mut vcx, window, &actions::ShowHome);
        assert_eq!(
            active(&mut vcx),
            Some(Tab::Home),
            "chosen again: its own view"
        );
        dispatch(&mut vcx, window, &actions::ShowMemory);
        assert!(matches!(active(&mut vcx), Some(Tab::Note(_))));
    }

    fn write_harbour(home: &Path, window: &str, pad: usize) {
        std::fs::create_dir_all(home.join("themes")).unwrap();
        let body = format!(
            r##"{{"name":"Fam","author":"t","themes":[{{"name":"Harbour","appearance":"dark",
            "style":{{"background":"{window}","text":"#f0f0f0ff","border.focused":"#80c0ffff"}}}}]}}{}"##,
            " ".repeat(pad)
        );
        std::fs::write(home.join("themes/harbour.json"), body).unwrap();
    }

    fn window_colour(vcx: &mut VisualTestContext) -> u32 {
        vcx.update(|_, cx| cx.global::<ActiveTheme>().theme.colors.window.0)
    }

    fn theme_name(vcx: &mut VisualTestContext) -> String {
        vcx.update(|_, cx| cx.global::<ActiveTheme>().theme.name.clone())
    }

    fn open_theme_picker(
        vcx: &mut VisualTestContext,
        window: gpui::WindowHandle<Shell>,
    ) -> Entity<Picker> {
        dispatch(vcx, window, &actions::SelectTheme);
        let root = window.root(vcx).unwrap();
        root.read_with(vcx, |s, _| {
            s.picker.as_ref().expect("the picker opens").entity.clone()
        })
    }

    /// A key that writes does nothing right after the window
    /// comes forward, nor in a view nobody clicked or moved in; the Inbox
    /// shows no card selected until the person moves.
    #[gpui::test]
    fn a_keyed_write_waits_for_a_settled_and_engaged_window(cx: &mut TestAppContext) {
        use crate::keyguard::{KeyGuard, Refusal};
        setup_globals(cx);
        let (_dir, home) = ready_home();
        let (window, mut vcx) = open(cx, &home);
        let root = window.root(&mut vcx).unwrap();
        dispatch(&mut vcx, window, &actions::ShowInbox);
        vcx.run_until_parked();
        // Coming forward selects nothing.
        assert_eq!(root.read_with(&vcx, |s, _| s.inbox.selected.clone()), None);
        // The window comes forward (the activation observer tells the guard).
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        // Coming forward reloads, and waiting for that can take longer than
        // the guard's second on a busy machine: the key below is pressed
        // as the window comes forward.
        vcx.update(|_, cx| cx.default_global::<KeyGuard>().came_forward_now());
        assert!(vcx.update(|_, cx| cx.default_global::<KeyGuard>().is_active()));
        dispatch(&mut vcx, window, &actions::InboxKeep);
        assert_eq!(
            vcx.update(|_, cx| cx.default_global::<KeyGuard>().refused),
            Some(Refusal::JustCameForward)
        );
        // A second later, still nothing was clicked or moved here.
        vcx.update(|_, cx| {
            cx.default_global::<KeyGuard>()
                .backdate(std::time::Duration::from_millis(1500))
        });
        dispatch(&mut vcx, window, &actions::InboxDismiss);
        assert_eq!(
            vcx.update(|_, cx| cx.default_global::<KeyGuard>().refused),
            Some(Refusal::NotEngaged)
        );
        // An arrow once settled engages it too (bound keys reach the guard).
        arrow(&mut vcx, false);
        vcx.update(|_, cx| {
            let g = cx.default_global::<KeyGuard>();
            assert_eq!(g.check(std::time::Instant::now()), Ok(()));
        });
        vcx.deactivate_window();
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            cx.default_global::<KeyGuard>()
                .backdate(std::time::Duration::from_millis(1500))
        });
        // A real press in the window engages the Inbox; then keys may act.
        vcx.simulate_mouse_down(
            gpui::point(gpui::px(600.), gpui::px(300.)),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        vcx.update(|_, cx| {
            let g = cx.default_global::<KeyGuard>();
            assert_eq!(g.refused, None, "a click clears the notice");
            assert_eq!(g.check(std::time::Instant::now()), Ok(()));
        });
        // Leaving the window ends it.
        vcx.deactivate_window();
        dispatch(&mut vcx, window, &actions::InboxKeep);
        assert_eq!(
            vcx.update(|_, cx| cx.default_global::<KeyGuard>().refused),
            Some(Refusal::NotActive)
        );
    }

    fn arrow(vcx: &mut VisualTestContext, held: bool) {
        vcx.simulate_event(gpui::KeyDownEvent {
            keystroke: gpui::Keystroke::parse("down").unwrap(),
            is_held: held,
            prefer_character_input: false,
        });
    }

    #[gpui::test]
    fn the_theme_picker_lists_themes_previews_saves_and_restores(cx: &mut TestAppContext) {
        setup_globals(cx);
        let (dir, home) = ready_home();
        write_harbour(&home, "#112233ff", 0);
        let prefs = dir.path().join("prefs/preferences.json");
        let (window, mut vcx) = open_with_prefs(cx, &home, prefs.clone());
        let root = window.root(&mut vcx).unwrap();
        assert!(vcx.update(|_, cx| cx.global::<ActiveTheme>().system));

        let picker = open_theme_picker(&mut vcx, window);
        picker.read_with(&vcx, |p, _| {
            let texts: Vec<&str> = p.items().iter().map(|i| i.text.as_str()).collect();
            assert_eq!(
                texts,
                ["System", "Hippocamp Light", "Hippocamp Dark", "Harbour"]
            );
            assert!(
                p.items()[3]
                    .detail
                    .as_deref()
                    .unwrap()
                    .starts_with("harbour.json")
            );
            assert_eq!(p.selected_item(), Some(0), "opens on the theme in use");
            assert!(p.items().iter().all(|i| !i.accessible_label().is_empty()));
        });

        // Each selection previews at once.
        vcx.simulate_keystrokes("down");
        assert_eq!(theme_name(&mut vcx), "Hippocamp Light");
        vcx.simulate_keystrokes("down down");
        assert_eq!(theme_name(&mut vcx), "Harbour");
        assert_eq!(window_colour(&mut vcx), 0x112233ff);
        assert!(!prefs.exists(), "a preview saves nothing");

        // Escape goes back to the theme in use before.
        vcx.simulate_keystrokes("escape");
        assert!(!root.read_with(&vcx, |s, _| s.picker_open()));
        assert!(vcx.update(|_, cx| cx.global::<ActiveTheme>().system));
        assert!(!prefs.exists());

        // Return keeps it and saves its id.
        open_theme_picker(&mut vcx, window);
        vcx.simulate_keystrokes("down down down enter");
        assert!(!root.read_with(&vcx, |s, _| s.picker_open()));
        assert_eq!(window_colour(&mut vcx), 0x112233ff);
        assert_eq!(
            crate::prefs::load(&prefs).theme,
            "user:harbour.json/Harbour"
        );

        // Opened again, it opens on that theme, and Escape keeps it.
        let picker = open_theme_picker(&mut vcx, window);
        assert_eq!(picker.read_with(&vcx, |p, _| p.selected_item()), Some(3));
        vcx.simulate_keystrokes("up escape");
        assert_eq!(theme_name(&mut vcx), "Harbour", "restored by its id");
    }

    #[gpui::test]
    fn a_held_arrow_key_previews_only_when_released(cx: &mut TestAppContext) {
        setup_globals(cx);
        let (dir, home) = ready_home();
        write_harbour(&home, "#112233ff", 0);
        let (window, mut vcx) = open_with_prefs(cx, &home, dir.path().join("p.json"));
        open_theme_picker(&mut vcx, window);

        arrow(&mut vcx, false);
        assert_eq!(
            theme_name(&mut vcx),
            "Hippocamp Light",
            "the first press previews"
        );
        // Two repeats: the selection moves to Harbour but the window does not.
        arrow(&mut vcx, true);
        arrow(&mut vcx, true);
        assert_eq!(theme_name(&mut vcx), "Hippocamp Light", "repeats wait");
        vcx.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse("down").unwrap(),
        });
        assert_eq!(theme_name(&mut vcx), "Harbour", "the release previews");
    }

    #[gpui::test]
    fn the_saved_theme_is_used_at_launch_and_a_gone_one_gives_system_and_a_notice(
        cx: &mut TestAppContext,
    ) {
        setup_globals(cx);
        let (dir, home) = ready_home();
        write_harbour(&home, "#112233ff", 0);
        let prefs = dir.path().join("p.json");
        crate::prefs::save(
            &prefs,
            &crate::prefs::Prefs {
                theme: "user:harbour.json/Harbour".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let (window, mut vcx) = open_with_prefs(cx, &home, prefs.clone());
        assert_eq!(theme_name(&mut vcx), "Harbour");
        let root = window.root(&mut vcx).unwrap();
        assert_eq!(
            root.read_with(&vcx, |s, _| s.themes.notice().cloned()),
            None
        );

        crate::prefs::save(
            &prefs,
            &crate::prefs::Prefs {
                theme: "user:gone.json/Gone".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let (window, mut vcx) = open_with_prefs(cx, &home, prefs);
        assert!(vcx.update(|_, cx| cx.global::<ActiveTheme>().system));
        let root = window.root(&mut vcx).unwrap();
        let notice = root
            .read_with(&vcx, |s, _| s.themes.notice().cloned())
            .unwrap();
        assert!(notice.problem && notice.text.contains("gone"), "{notice:?}");
    }

    #[gpui::test]
    fn a_changed_theme_file_is_applied_and_a_broken_one_keeps_the_last_good_version(
        cx: &mut TestAppContext,
    ) {
        setup_globals(cx);
        let (dir, home) = ready_home();
        write_harbour(&home, "#112233ff", 0);
        let prefs = dir.path().join("p.json");
        crate::prefs::save(
            &prefs,
            &crate::prefs::Prefs {
                theme: "user:harbour.json/Harbour".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let (window, mut vcx) = open_with_prefs(cx, &home, prefs);
        let root = window.root(&mut vcx).unwrap();
        assert_eq!(window_colour(&mut vcx), 0x112233ff);

        write_harbour(&home, "#445566ff", 1);
        root.update(&mut vcx, |s, cx| s.poll_theme_now(cx));
        assert_eq!(window_colour(&mut vcx), 0x445566ff, "the edit shows");

        std::fs::write(home.join("themes/harbour.json"), "{ broken").unwrap();
        root.update(&mut vcx, |s, cx| s.poll_theme_now(cx));
        assert_eq!(
            window_colour(&mut vcx),
            0x445566ff,
            "the last good version stays"
        );
        let notice = root
            .read_with(&vcx, |s, _| s.themes.notice().cloned())
            .unwrap();
        assert!(notice.text.contains("last good version"), "{notice:?}");

        // The once-a-second poll does the same on its own.
        write_harbour(&home, "#778899ff", 2);
        vcx.executor()
            .advance_clock(std::time::Duration::from_millis(1100));
        vcx.run_until_parked();
        assert_eq!(window_colour(&mut vcx), 0x778899ff);
    }

    #[gpui::test]
    fn use_system_switches_and_saves_without_the_picker(cx: &mut TestAppContext) {
        setup_globals(cx);
        let (dir, home) = ready_home();
        write_harbour(&home, "#112233ff", 0);
        let prefs = dir.path().join("p.json");
        crate::prefs::save(
            &prefs,
            &crate::prefs::Prefs {
                theme: "user:harbour.json/Harbour".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let (window, mut vcx) = open_with_prefs(cx, &home, prefs.clone());
        dispatch(&mut vcx, window, &actions::UseSystemTheme);
        assert!(vcx.update(|_, cx| cx.global::<ActiveTheme>().system));
        assert_eq!(crate::prefs::load(&prefs).theme, "system");
        let root = window.root(&mut vcx).unwrap();
        assert!(!root.read_with(&vcx, |s, _| s.picker_open()));
    }
}
