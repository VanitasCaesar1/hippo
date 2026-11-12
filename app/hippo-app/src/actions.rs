//! The one table of actions: their palette names, menus, key bindings and the
//! condition under which each can run. The menus, the key bindings and the
//! command palette are all built from [`TABLE`].

#![allow(dead_code)] // the panels of items 7 and 8 use what this one does not yet

use gpui::{Action, KeyBinding, MenuItem, SharedString, actions};

actions!(
    hippo,
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
        SelectNext,
        SelectPrevious,
        DockDown,
        DockUp,
        DockOpen,
        RunContext,
        Confirm,
        Cancel,
        Help,
        Minimize,
        Zoom,
    ]
);

/// The menus of the menu bar, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuName {
    Hippo,
    File,
    Edit,
    View,
    Dreaming,
    Window,
    Help,
}

impl MenuName {
    pub const ALL: [MenuName; 7] = [
        MenuName::Hippo,
        MenuName::File,
        MenuName::Edit,
        MenuName::View,
        MenuName::Dreaming,
        MenuName::Window,
        MenuName::Help,
    ];

    pub fn title(self) -> &'static str {
        match self {
            MenuName::Hippo => "Hippo",
            MenuName::File => "File",
            MenuName::Edit => "Edit",
            MenuName::View => "View",
            MenuName::Dreaming => "Dreaming",
            MenuName::Window => "Window",
            MenuName::Help => "Help",
        }
    }
}

/// What the app knows that decides whether an action can run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Ctx {
    /// `Setup::Ready`.
    pub setup_ready: bool,
    /// How deep the current section's stack is: 1 is its own view.
    pub depth: usize,
    /// Memory is shown, with its notes list and filter.
    pub memory: bool,
    /// The notes filter holds text.
    pub filter_has_text: bool,
    /// A picker is open.
    pub picker_open: bool,
    /// The main view shows a file (a note or a report).
    pub active_file: bool,
    /// The main view is a change the revert rules allow reverting now.
    pub revert_ready: bool,
    /// The revert confirmation is open.
    pub confirm_open: bool,
}

/// When an action can run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Can {
    Always,
    SetupReady,
    /// An item is open on top of a section's own view.
    Deeper,
    Memory,
    FilterHasText,
    PickerOpen,
    ActiveFile,
    RevertReady,
    ConfirmOpen,
}

impl Can {
    /// `Ok` when the action can run, else the reason, in words.
    pub fn check(self, ctx: &Ctx) -> Result<(), String> {
        let (ok, why): (bool, &str) = match self {
            Can::Always => (true, ""),
            Can::SetupReady => (ctx.setup_ready, "hippo is not set up"),
            Can::Deeper => (ctx.depth > 1, "nothing is open to go back from"),
            Can::Memory => (ctx.memory, "the notes list is in Memory"),
            Can::FilterHasText => (ctx.filter_has_text, "the filter is empty"),
            Can::PickerOpen => (ctx.picker_open, "no picker is open"),
            Can::ActiveFile => (ctx.active_file, "the view shows no file"),
            Can::RevertReady => (
                ctx.revert_ready,
                "the view is not a change that can be reverted",
            ),
            Can::ConfirmOpen => (ctx.confirm_open, "no confirmation is open"),
        };
        if ok { Ok(()) } else { Err(why.to_string()) }
    }
}

/// One row of the table.
pub struct Def {
    /// The action's name without its namespace, for tests and the session's
    /// list of actions run.
    pub id: &'static str,
    /// The name in the palette, `Menu: Name`; `None` keeps the row out of it.
    pub palette: Option<&'static str>,
    pub menu: Option<MenuName>,
    /// GPUI key strings; a space separates the keystrokes of a chord.
    pub keys: &'static [&'static str],
    /// The key context the bindings apply in.
    pub context: Option<&'static str>,
    pub can: Can,
    make: fn() -> Box<dyn Action>,
    menu_item: fn(SharedString) -> MenuItem,
    bind: fn(&'static str, Option<&str>) -> KeyBinding,
}

impl Def {
    pub fn action(&self) -> Box<dyn Action> {
        (self.make)()
    }

    /// The name in the menu: the palette name without its `Menu: ` prefix.
    pub fn menu_label(&self) -> Option<&'static str> {
        let name = self.palette?;
        Some(name.split_once(": ").map_or(name, |(_, rest)| rest))
    }

    /// `Ok` when the action can run now.
    pub fn check(&self, ctx: &Ctx) -> Result<(), String> {
        self.can.check(ctx)
    }

    /// The bindings of the first key string, shown beside the row.
    pub fn key_label(&self) -> String {
        self.keys
            .first()
            .map(|k| display_keys(k))
            .unwrap_or_default()
    }

    pub fn bindings(&self) -> impl Iterator<Item = KeyBinding> + '_ {
        self.keys.iter().map(|keys| (self.bind)(keys, self.context))
    }

    pub fn menu_item(&self, ctx: &Ctx) -> Option<MenuItem> {
        self.menu?;
        let label = self.menu_label()?;
        Some((self.menu_item)(label.into()).disabled(self.check(ctx).is_err()))
    }
}

macro_rules! def {
    ($ty:ident, $palette:expr, $menu:expr, $keys:expr, $context:expr, $can:expr) => {
        Def {
            id: stringify!($ty),
            palette: $palette,
            menu: $menu,
            keys: $keys,
            context: $context,
            can: $can,
            make: || Box::new($ty),
            menu_item: |name| MenuItem::action(name, $ty),
            bind: |keys, context| KeyBinding::new(keys, $ty, context),
        }
    };
}

use Can::*;
use MenuName as M;

const WS: Option<&str> = Some("Workspace");

/// Every action. Order within a menu is the order of the menu.
pub static TABLE: &[Def] = &[
    def!(About, Some("Hippo: About"), Some(M::Hippo), &[], WS, Always),
    def!(
        ShowManage,
        Some("Hippo: Manage"),
        Some(M::Hippo),
        &["cmd-,", "cmd-5"],
        WS,
        Always
    ),
    def!(
        Quit,
        Some("Hippo: Quit"),
        Some(M::Hippo),
        &["cmd-q"],
        WS,
        Always
    ),
    def!(
        OpenInEditor,
        Some("File: Open in Editor"),
        Some(M::File),
        &["cmd-o"],
        WS,
        ActiveFile
    ),
    def!(
        RevealInFinder,
        Some("File: Reveal in Finder"),
        Some(M::File),
        &["cmd-shift-r"],
        WS,
        ActiveFile
    ),
    def!(
        OpenNewestReport,
        Some("Dreaming: Open Newest Run"),
        Some(M::File),
        &[],
        WS,
        SetupReady
    ),
    def!(
        CopyText,
        Some("Edit: Copy"),
        Some(M::Edit),
        &["cmd-c"],
        WS,
        Always
    ),
    def!(
        ToggleSidebar,
        Some("View: Fold Sidebar"),
        Some(M::View),
        &["cmd-ctrl-s"],
        WS,
        Always
    ),
    def!(
        ShowHome,
        Some("View: Home"),
        Some(M::View),
        &["cmd-1"],
        WS,
        Always
    ),
    def!(
        ShowInbox,
        Some("View: Inbox"),
        Some(M::View),
        &["cmd-i", "cmd-2"],
        WS,
        Always
    ),
    def!(
        ShowFleet,
        Some("View: Fleet"),
        Some(M::View),
        &["cmd-f", "cmd-3"],
        WS,
        Always
    ),
    def!(
        ShowIncidents,
        Some("View: Incidents"),
        Some(M::View),
        &["cmd-6"],
        WS,
        Always
    ),
    def!(
        ShowMemory,
        Some("View: Memory"),
        Some(M::View),
        &["cmd-4"],
        WS,
        Always
    ),
    def!(
        FocusAsk,
        Some("View: Ask"),
        Some(M::View),
        &["cmd-k"],
        WS,
        SetupReady
    ),
    def!(
        GoBack,
        Some("View: Back"),
        Some(M::View),
        &["cmd-[", "escape"],
        WS,
        Deeper
    ),
    def!(
        CommandPalette,
        Some("View: Command Palette"),
        Some(M::View),
        &["cmd-shift-p"],
        WS,
        Always
    ),
    def!(
        Find,
        Some("View: Find Note or Session"),
        Some(M::View),
        &["cmd-p"],
        WS,
        SetupReady
    ),
    def!(
        SelectTheme,
        Some("Theme: Select Theme"),
        Some(M::View),
        &["cmd-shift-t"],
        WS,
        Always
    ),
    def!(
        UseSystemTheme,
        Some("Theme: Use System"),
        Some(M::View),
        &[],
        WS,
        Always
    ),
    def!(
        ToggleDock,
        Some("Memory: Toggle Notes List"),
        Some(M::View),
        &["cmd-b"],
        WS,
        Memory
    ),
    def!(
        FocusFilter,
        Some("Memory: Filter Notes"),
        Some(M::View),
        &["cmd-shift-f"],
        WS,
        Memory
    ),
    def!(
        NeedsAttention,
        Some("Memory: Needs Attention"),
        Some(M::View),
        &[],
        WS,
        SetupReady
    ),
    def!(
        ContextPreview,
        Some("Memory: Context Preview"),
        Some(M::View),
        &["cmd-l"],
        WS,
        SetupReady
    ),
    def!(
        FocusPane,
        Some("View: Focus Pane"),
        None,
        &[],
        Some("Dock"),
        Always
    ),
    def!(ClearFilter, None, None, &["escape"], Some("Dock"), Always),
    def!(
        Reload,
        Some("View: Reload"),
        Some(M::View),
        &["cmd-r"],
        WS,
        Always
    ),
    def!(
        TextLarger,
        Some("View: Larger Text"),
        Some(M::View),
        &["cmd-="],
        WS,
        Always
    ),
    def!(
        TextSmaller,
        Some("View: Smaller Text"),
        Some(M::View),
        &["cmd--"],
        WS,
        Always
    ),
    def!(
        TextDefault,
        Some("View: Default Text Size"),
        Some(M::View),
        &["cmd-0"],
        WS,
        Always
    ),
    def!(
        AskRevert,
        Some("Dreaming: Revert Change"),
        Some(M::Dreaming),
        &["cmd-alt-z"],
        WS,
        RevertReady
    ),
    def!(
        ToggleChanges,
        Some("Session: Show Changes"),
        Some(M::View),
        &["cmd-alt-c"],
        WS,
        SetupReady
    ),
    def!(
        DockDown,
        None,
        None,
        &["down", "ctrl-n"],
        Some("Dock"),
        Always
    ),
    def!(DockUp, None, None, &["up", "ctrl-p"], Some("Dock"), Always),
    def!(DockOpen, None, None, &["enter"], Some("Dock"), Always),
    def!(
        RunContext,
        None,
        None,
        &["enter"],
        Some("ContextTask"),
        Always
    ),
    def!(AskSubmit, None, None, &["enter"], Some("AskField"), Always),
    def!(
        ToggleBrief,
        None,
        None,
        &["cmd-b"],
        Some("AskField"),
        Always
    ),
    def!(
        AskSourcesOnly,
        None,
        None,
        &["cmd-enter"],
        Some("AskField"),
        Always
    ),
    def!(
        SourceNext,
        None,
        None,
        &["down", "j"],
        Some("AskSources"),
        Always
    ),
    def!(
        SourcePrevious,
        None,
        None,
        &["up", "k"],
        Some("AskSources"),
        Always
    ),
    def!(
        SourceOpen,
        None,
        None,
        &["enter"],
        Some("AskSources"),
        Always
    ),
    def!(InboxNext, None, None, &["down", "j"], Some("Inbox"), Always),
    def!(
        InboxPrevious,
        None,
        None,
        &["up", "k"],
        Some("Inbox"),
        Always
    ),
    def!(InboxKeep, None, None, &["enter"], Some("Inbox"), Always),
    def!(InboxEdit, None, None, &["e"], Some("Inbox"), Always),
    def!(
        InboxDismiss,
        None,
        None,
        &["backspace", "delete"],
        Some("Inbox"),
        Always
    ),
    def!(
        LessonSave,
        None,
        None,
        &["cmd-enter"],
        Some("LessonEdit"),
        Always
    ),
    def!(
        LessonCancel,
        None,
        None,
        &["escape"],
        Some("LessonEdit"),
        Always
    ),
    def!(
        FoldTurn,
        None,
        None,
        &["space", "enter"],
        Some("SessionTurns"),
        Always
    ),
    def!(
        TurnNext,
        None,
        None,
        &["down", "j"],
        Some("SessionTurns"),
        Always
    ),
    def!(
        TurnPrevious,
        None,
        None,
        &["up", "k"],
        Some("SessionTurns"),
        Always
    ),
    def!(
        TurnFirst,
        None,
        None,
        &["home", "cmd-up"],
        Some("SessionTurns"),
        Always
    ),
    def!(
        TurnLast,
        None,
        None,
        &["end", "cmd-down"],
        Some("SessionTurns"),
        Always
    ),
    def!(
        ConfirmRevert,
        None,
        None,
        &["cmd-enter"],
        Some("Confirm"),
        ConfirmOpen
    ),
    def!(
        CancelRevert,
        None,
        None,
        &["enter", "escape"],
        Some("Confirm"),
        ConfirmOpen
    ),
    def!(
        SelectNext,
        None,
        None,
        &["down", "ctrl-n"],
        Some("Picker"),
        PickerOpen
    ),
    def!(
        SelectPrevious,
        None,
        None,
        &["up", "ctrl-p"],
        Some("Picker"),
        PickerOpen
    ),
    def!(Confirm, None, None, &["enter"], Some("Picker"), PickerOpen),
    def!(Cancel, None, None, &["escape"], Some("Picker"), PickerOpen),
    def!(
        Help,
        Some("Help: Hippo Help"),
        Some(M::Help),
        &["cmd-?"],
        WS,
        Always
    ),
    def!(
        Minimize,
        Some("Window: Minimize"),
        Some(M::Window),
        &["cmd-m"],
        WS,
        Always
    ),
    def!(Zoom, Some("Window: Zoom"), Some(M::Window), &[], WS, Always),
];

/// Every key binding of the table.
pub fn key_bindings() -> Vec<KeyBinding> {
    TABLE.iter().flat_map(|d| d.bindings()).collect()
}

/// The row that makes `action`.
pub fn find(action: &dyn Action) -> Option<&'static Def> {
    TABLE.iter().find(|d| d.action().partial_eq(action))
}

/// A GPUI key string as the palette shows it: `cmd-shift-p` as `⌘⇧P`, a chord
/// `cmd-k cmd-t` as `⌘K ⌘T`.
pub fn display_keys(keys: &str) -> String {
    keys.split_whitespace()
        .map(display_stroke)
        .collect::<Vec<_>>()
        .join(" ")
}

fn display_stroke(stroke: &str) -> String {
    // The key is what follows the last `-`, except for the key `-` itself.
    let (mods, key) = match stroke.strip_suffix("--") {
        Some(rest) => (rest, "-"),
        None => stroke.rsplit_once('-').unwrap_or(("", stroke)),
    };
    let mut out = String::new();
    for m in mods.split('-').filter(|m| !m.is_empty()) {
        out.push_str(match m {
            "ctrl" => "⌃",
            "alt" => "⌥",
            "shift" => "⇧",
            "cmd" => "⌘",
            other => other,
        });
    }
    out.push_str(&match key {
        "enter" => "↩".to_string(),
        "escape" => "⎋".to_string(),
        "tab" => "⇥".to_string(),
        "space" => "Space".to_string(),
        "up" => "↑".to_string(),
        "down" => "↓".to_string(),
        "left" => "←".to_string(),
        "right" => "→".to_string(),
        other => other.to_uppercase(),
    });
    out
}

/// The palette's rows in order: the actions run this session first, newest
/// first, then the rest in menu order. `ran` holds ids, newest first.
pub fn palette_order(ran: &[&str]) -> Vec<&'static Def> {
    let listed: Vec<&'static Def> = TABLE.iter().filter(|d| d.palette.is_some()).collect();
    let mut out: Vec<&'static Def> = Vec::new();
    for id in ran {
        if let Some(def) = listed.iter().find(|d| d.id == *id)
            && !out.iter().any(|d| d.id == def.id)
        {
            out.push(def);
        }
    }
    let mut rest: Vec<&'static Def> = listed
        .into_iter()
        .filter(|d| !out.iter().any(|o| o.id == d.id))
        .collect();
    // Menu order: by menu, then the table's order. Rows with no menu follow.
    rest.sort_by_key(|d| d.menu.map_or(usize::MAX, |m| m as usize));
    out.extend(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_row_builds_its_bindings_and_menu_item() {
        // KeyBinding::new panics on a key string it cannot parse.
        let bindings = key_bindings();
        let expected: usize = TABLE.iter().map(|d| d.keys.len()).sum();
        assert_eq!(bindings.len(), expected);
        let ctx = Ctx::default();
        for def in TABLE {
            assert_eq!(
                def.menu_item(&ctx).is_some(),
                def.menu.is_some(),
                "{}",
                def.id
            );
        }
    }

    #[test]
    fn ids_are_unique_and_match_the_action() {
        let mut seen = HashSet::new();
        for def in TABLE {
            assert!(seen.insert(def.id), "{} twice", def.id);
            assert!(
                def.action().name().ends_with(def.id),
                "{} vs {}",
                def.id,
                def.action().name()
            );
        }
    }

    #[test]
    fn the_design_table_is_all_here() {
        let ids: HashSet<_> = TABLE.iter().map(|d| d.id).collect();
        for id in [
            "About",
            "Quit",
            "CommandPalette",
            "Find",
            "SelectTheme",
            "UseSystemTheme",
            "ToggleDock",
            "ShowHome",
            "ShowInbox",
            "ShowFleet",
            "ShowIncidents",
            "ShowMemory",
            "ShowManage",
            "FocusAsk",
            "GoBack",
            "FocusPane",
            "FocusFilter",
            "ClearFilter",
            "ContextPreview",
            "FoldTurn",
            "CopyText",
            "OpenInEditor",
            "RevealInFinder",
            "OpenNewestReport",
            "AskRevert",
            "ConfirmRevert",
            "CancelRevert",
            "Reload",
            "TextLarger",
            "TextSmaller",
            "TextDefault",
            "SelectNext",
            "SelectPrevious",
            "DockDown",
            "DockUp",
            "DockOpen",
            "RunContext",
            "AskSubmit",
            "Confirm",
            "Cancel",
            "Help",
        ] {
            assert!(ids.contains(id), "{id} is missing from the table");
        }
    }

    #[test]
    fn every_menu_item_is_in_the_palette_and_every_menu_has_items() {
        for def in TABLE.iter().filter(|d| d.menu.is_some()) {
            assert!(
                def.palette.is_some(),
                "{} is in a menu but not the palette",
                def.id
            );
        }
        for menu in MenuName::ALL {
            assert!(
                TABLE.iter().any(|d| d.menu == Some(menu)),
                "{} is empty",
                menu.title()
            );
        }
    }

    #[test]
    fn the_palette_lists_rows_with_a_name_and_none_without() {
        let listed = palette_order(&[]);
        assert!(listed.iter().all(|d| d.palette.is_some()));
        assert_eq!(
            listed.len(),
            TABLE.iter().filter(|d| d.palette.is_some()).count()
        );
        assert!(!listed.iter().any(|d| d.id == "ClearFilter"));
    }

    #[test]
    fn palette_order_puts_actions_run_this_session_first_then_menu_order() {
        let order = palette_order(&["Reload", "ToggleDock", "Reload"]);
        assert_eq!(order[0].id, "Reload");
        assert_eq!(order[1].id, "ToggleDock");
        assert_eq!(order.iter().filter(|d| d.id == "Reload").count(), 1);
        // The rest follow in menu order: Hippo's rows, then File's, and so on.
        let rest: Vec<_> = order[2..].iter().map(|d| d.menu).collect();
        let ranks: Vec<usize> = rest
            .iter()
            .map(|m| m.map_or(usize::MAX, |m| m as usize))
            .collect();
        assert!(ranks.windows(2).all(|w| w[0] <= w[1]), "{ranks:?}");
        assert_eq!(order[2].id, "About");
    }

    #[test]
    fn an_action_that_cannot_run_gives_its_reason() {
        let ctx = Ctx::default();
        let find = TABLE.iter().find(|d| d.id == "Find").unwrap();
        assert_eq!(find.check(&ctx), Err("hippo is not set up".to_string()));
        let open = TABLE.iter().find(|d| d.id == "OpenInEditor").unwrap();
        assert_eq!(open.check(&ctx), Err("the view shows no file".to_string()));
        let back = TABLE.iter().find(|d| d.id == "GoBack").unwrap();
        assert!(back.check(&Ctx { depth: 1, ..ctx }).is_err());
        assert!(back.check(&Ctx { depth: 2, ..ctx }).is_ok());
        let focus = TABLE.iter().find(|d| d.id == "FocusFilter").unwrap();
        assert!(focus.check(&ctx).is_err());
        assert!(
            focus
                .check(&Ctx {
                    memory: true,
                    ..ctx
                })
                .is_ok()
        );
    }

    #[test]
    fn no_binding_starts_with_the_ask_key() {
        // ⌘K alone asks; a chord starting with it would hold every ⌘K for
        // a second.
        for def in TABLE {
            for keys in def.keys {
                assert!(!keys.starts_with("cmd-k "), "{} binds {keys}", def.id);
            }
        }
    }

    #[test]
    fn keys_are_shown_as_symbols() {
        assert_eq!(display_keys("cmd-shift-p"), "⌘⇧P");
        assert_eq!(display_keys("cmd-k cmd-t"), "⌘K ⌘T");
        assert_eq!(display_keys("cmd--"), "⌘-");
        assert_eq!(display_keys("ctrl-tab"), "⌃⇥");
        assert_eq!(display_keys("alt-cmd-right"), "⌥⌘→");
        assert_eq!(display_keys("escape"), "⎋");
    }
}
