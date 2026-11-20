//! Hippo.app: a macOS window over hippo's files, on GPUI.

mod actions;
mod cli;
mod flow;
mod incidents;
mod keyguard;
mod launch;
mod menubar;
mod prefs;
mod reach;
mod rows;
mod shelving;
mod state;
mod themes;
mod ui;

use std::borrow::Cow;

use gpui::{App, AppContext as _, AssetSource, SharedString};
use gpui_platform::application;

/// The assets: the hippo mark and Lucide's icons (ISC, `icons/LICENSE-lucide.txt`),
/// drawn in the text colour.
struct Assets;

/// A Lucide icon's file by its name, or an agent's mark (`agent-<name>`,
/// brand/agents/README.md). The names of the app's earlier icons
/// are kept as aliases so every caller draws a Lucide glyph.
fn icon(name: &str) -> Option<&'static [u8]> {
    // A night-map name with no alias of its own (`n-minus`) is the Lucide
    // icon of the same name.
    icon_file(alias(name)).or_else(|| icon_file(alias(name.strip_prefix("n-")?)))
}

fn alias(name: &str) -> &str {
    match name {
        "home" => "house",
        "fleet" => "bot",
        "memory" => "brain",
        "manage" | "sliders" => "sliders-horizontal",
        "note" => "file-text",
        "gear" => "settings",
        "heart" => "heart-pulse",
        "box" => "package",
        "bulb" => "lightbulb",
        "sparkle" => "zap",
        "stack" => "layers",
        // The night map's names, each a Lucide icon.
        "n-morning" => "sunrise",
        "n-brain" => "brain",
        "n-agents" => "bot",
        "n-guard" => "shield",
        "n-note" => "file-text",
        "n-folder" => "folder",
        "n-check" => "check",
        "n-x" => "x",
        "n-undo" => "undo-2",
        "n-said" => "message-square-quote",
        "n-rule" => "scroll-text",
        "n-receipt" => "receipt",
        "n-clock" => "clock",
        "n-up" => "arrow-up",
        "n-search" => "search",
        "n-sun" => "sun",
        "n-moon" => "moon",
        "n-terminal" => "terminal",
        "n-right" => "arrow-right",
        "n-script" => "file-terminal",
        "n-key" => "key-round",
        "n-tomodel" => "send",
        "n-clone" => "folder-git-2",
        "n-fix" => "wrench",
        "n-eye" => "eye",
        "n-down" => "chevron-down",
        "n-open" => "external-link",
        "n-thread" => "messages-square",
        "n-wait" => "hourglass",
        "n-start" => "flag",
        other => other,
    }
}

fn icon_file(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "alarm-clock" => include_bytes!("../icons/alarm-clock.svg").as_slice(),
        "alarm-clock-off" => include_bytes!("../icons/alarm-clock-off.svg").as_slice(),
        "arrow-right" => include_bytes!("../icons/arrow-right.svg").as_slice(),
        "arrow-up" => include_bytes!("../icons/arrow-up.svg").as_slice(),
        "arrow-up-right" => include_bytes!("../icons/arrow-up-right.svg").as_slice(),
        "bot" => include_bytes!("../icons/bot.svg").as_slice(),
        "brain" => include_bytes!("../icons/brain.svg").as_slice(),
        "calendar-clock" => include_bytes!("../icons/calendar-clock.svg").as_slice(),
        "check" => include_bytes!("../icons/check.svg").as_slice(),
        "chevron-down" => include_bytes!("../icons/chevron-down.svg").as_slice(),
        "chevron-left" => include_bytes!("../icons/chevron-left.svg").as_slice(),
        "chevron-right" => include_bytes!("../icons/chevron-right.svg").as_slice(),
        "chevrons-up-down" => include_bytes!("../icons/chevrons-up-down.svg").as_slice(),
        "circle-check" => include_bytes!("../icons/circle-check.svg").as_slice(),
        "circle-alert" => include_bytes!("../icons/circle-alert.svg").as_slice(),
        "circle-dashed" => include_bytes!("../icons/circle-dashed.svg").as_slice(),
        "circle-dot-dashed" => include_bytes!("../icons/circle-dot-dashed.svg").as_slice(),
        "circle-pause" => include_bytes!("../icons/circle-pause.svg").as_slice(),
        "circle-x" => include_bytes!("../icons/circle-x.svg").as_slice(),
        "clock" => include_bytes!("../icons/clock.svg").as_slice(),
        "copy" => include_bytes!("../icons/copy.svg").as_slice(),
        "eye-off" => include_bytes!("../icons/eye-off.svg").as_slice(),
        "file-plus" => include_bytes!("../icons/file-plus.svg").as_slice(),
        "file-text" => include_bytes!("../icons/file-text.svg").as_slice(),
        "folder" => include_bytes!("../icons/folder.svg").as_slice(),
        "gauge" => include_bytes!("../icons/gauge.svg").as_slice(),
        "git-branch" => include_bytes!("../icons/git-branch.svg").as_slice(),
        "git-merge" => include_bytes!("../icons/git-merge.svg").as_slice(),
        "git-pull-request" => include_bytes!("../icons/git-pull-request.svg").as_slice(),
        "git-pull-request-arrow" => {
            include_bytes!("../icons/git-pull-request-arrow.svg").as_slice()
        }
        "hand" => include_bytes!("../icons/hand.svg").as_slice(),
        "heart-pulse" => include_bytes!("../icons/heart-pulse.svg").as_slice(),
        "house" => include_bytes!("../icons/house.svg").as_slice(),
        "inbox" => include_bytes!("../icons/inbox.svg").as_slice(),
        "info" => include_bytes!("../icons/info.svg").as_slice(),
        "layers" => include_bytes!("../icons/layers.svg").as_slice(),
        "layout-grid" => include_bytes!("../icons/layout-grid.svg").as_slice(),
        "lightbulb" => include_bytes!("../icons/lightbulb.svg").as_slice(),
        "link" => include_bytes!("../icons/link.svg").as_slice(),
        "list" => include_bytes!("../icons/list.svg").as_slice(),
        "list-checks" => include_bytes!("../icons/list-checks.svg").as_slice(),
        "loader-circle" => include_bytes!("../icons/loader-circle.svg").as_slice(),
        "message-circle-question" => {
            include_bytes!("../icons/message-circle-question.svg").as_slice()
        }
        "moon" => include_bytes!("../icons/moon.svg").as_slice(),
        "package" => include_bytes!("../icons/package.svg").as_slice(),
        "panel-left" => include_bytes!("../icons/panel-left.svg").as_slice(),
        "panel-right" => include_bytes!("../icons/panel-right.svg").as_slice(),
        "pencil" => include_bytes!("../icons/pencil.svg").as_slice(),
        "play" => include_bytes!("../icons/play.svg").as_slice(),
        "refresh-cw" => include_bytes!("../icons/refresh-cw.svg").as_slice(),
        "search" => include_bytes!("../icons/search.svg").as_slice(),
        "settings" => include_bytes!("../icons/settings.svg").as_slice(),
        "shield-check" => include_bytes!("../icons/shield-check.svg").as_slice(),
        "shield-question" => include_bytes!("../icons/shield-question.svg").as_slice(),
        "sliders-horizontal" => include_bytes!("../icons/sliders-horizontal.svg").as_slice(),
        "siren" => include_bytes!("../icons/siren.svg").as_slice(),
        "square-terminal" => include_bytes!("../icons/square-terminal.svg").as_slice(),
        "terminal" => include_bytes!("../icons/terminal.svg").as_slice(),
        "text-search" => include_bytes!("../icons/text-search.svg").as_slice(),
        "triangle-alert" => include_bytes!("../icons/triangle-alert.svg").as_slice(),
        "undo-2" => include_bytes!("../icons/undo-2.svg").as_slice(),
        "webhook" => include_bytes!("../icons/webhook.svg").as_slice(),
        "wrench" => include_bytes!("../icons/wrench.svg").as_slice(),
        "x" => include_bytes!("../icons/x.svg").as_slice(),
        "zap" => include_bytes!("../icons/zap.svg").as_slice(),
        "sunrise" => include_bytes!("../icons/sunrise.svg").as_slice(),
        "eye" => include_bytes!("../icons/eye.svg").as_slice(),
        "shield" => include_bytes!("../icons/shield.svg").as_slice(),
        "key-round" => include_bytes!("../icons/key-round.svg").as_slice(),
        "receipt" => include_bytes!("../icons/receipt.svg").as_slice(),
        "scroll-text" => include_bytes!("../icons/scroll-text.svg").as_slice(),
        "message-square-quote" => include_bytes!("../icons/message-square-quote.svg").as_slice(),
        "file-terminal" => include_bytes!("../icons/file-terminal.svg").as_slice(),
        "flag" => include_bytes!("../icons/flag.svg").as_slice(),
        "sun" => include_bytes!("../icons/sun.svg").as_slice(),
        "messages-square" => include_bytes!("../icons/messages-square.svg").as_slice(),
        "send" => include_bytes!("../icons/send.svg").as_slice(),
        "hourglass" => include_bytes!("../icons/hourglass.svg").as_slice(),
        "folder-git-2" => include_bytes!("../icons/folder-git-2.svg").as_slice(),
        "external-link" => include_bytes!("../icons/external-link.svg").as_slice(),
        "command" => include_bytes!("../icons/command.svg").as_slice(),
        "corner-down-left" => include_bytes!("../icons/corner-down-left.svg").as_slice(),
        "arrow-left" => include_bytes!("../icons/arrow-left.svg").as_slice(),
        "arrow-down" => include_bytes!("../icons/arrow-down.svg").as_slice(),
        "option" => include_bytes!("../icons/option.svg").as_slice(),
        "delete" => include_bytes!("../icons/delete.svg").as_slice(),
        "chevron-up" => include_bytes!("../icons/chevron-up.svg").as_slice(),
        "arrow-big-up" => include_bytes!("../icons/arrow-big-up.svg").as_slice(),
        "minus" => include_bytes!("../icons/minus.svg").as_slice(),
        "plus" => include_bytes!("../icons/plus.svg").as_slice(),
        "agent-claude-code" => include_bytes!("../icons/agent-claude-code.svg").as_slice(),
        "agent-codex" => include_bytes!("../icons/agent-codex.svg").as_slice(),
        "agent-cursor" => include_bytes!("../icons/agent-cursor.svg").as_slice(),
        "agent-gemini-cli" => include_bytes!("../icons/agent-gemini-cli.svg").as_slice(),
        "agent-opencode" => include_bytes!("../icons/agent-opencode.svg").as_slice(),
        "agent-deepseek" => include_bytes!("../icons/agent-deepseek.svg").as_slice(),
        "agent-muse" => include_bytes!("../icons/agent-muse.svg").as_slice(),
        "agent-command-code" => include_bytes!("../icons/agent-command-code.svg").as_slice(),
        _ => return None,
    })
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        if path == "hippo-mark.svg" {
            return Ok(Some(Cow::Borrowed(
                include_bytes!("../../../brand/hippo-mark.svg").as_slice(),
            )));
        }
        Ok(path
            .strip_prefix("icons/")
            .and_then(|p| p.strip_suffix(".svg"))
            .and_then(icon)
            .map(Cow::Borrowed))
    }

    fn list(&self, _path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

/// When the process started, for `HIPPO_APP_TIMING`.
pub static STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

fn main() {
    STARTED.get_or_init(std::time::Instant::now);
    // What the app writes under $HIPPO_HOME is the person's alone.
    hippo_core::perms::private_umask();
    // Before any thread starts: the time crate refuses to read the zone later.
    ui::status::init_local_offset();
    application().with_assets(Assets).run(|cx: &mut App| {
        cx.bind_keys(actions::key_bindings());
        cx.bind_keys(ui::input::key_bindings());
        cx.on_action(|_: &actions::Quit, cx| cx.quit());
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        ui::init_globals(cx);
        let installation = cli::find(&cli::Environment::current());
        let mut options = ui::window_options(cx);
        // Dev-only, for screenshots of a synthetic home: HIPPO_APP_DEV_SIZE
        // (`1200x800`) sizes the window and HIPPO_APP_DEV_VIEW (`inbox`)
        // opens a section. With either set, the app does not bring itself
        // to the front, so a capture never takes the person's focus.
        let dev_size = std::env::var("HIPPO_APP_DEV_SIZE").ok().and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some((w.parse::<f32>().ok()?, h.parse::<f32>().ok()?))
        });
        let dev_view = std::env::var("HIPPO_APP_DEV_VIEW").ok();
        if let Some((w, h)) = dev_size {
            options.window_bounds = Some(gpui::WindowBounds::Windowed(gpui::Bounds::centered(
                None,
                gpui::size(gpui::px(w), gpui::px(h)),
                cx,
            )));
        }
        // Read once: `HIPPO_APP_PREFS` moves the preferences file.
        let prefs_path = prefs::default_path();
        let view = dev_view.clone();
        // Dev-only: for a screenshot, read everything before the window
        // opens, so its first frame has the data. A window opened in the
        // background may be covered, and macOS draws a covered window once.
        let preloaded = (dev_size.is_some() || dev_view.is_some())
            .then(|| state::load(&installation.home, time::OffsetDateTime::now_utc()));
        let handle = cx
            .open_window(options, |window, cx| {
                cx.new(|cx| {
                    let mut shell = ui::Shell::new(installation, prefs_path, window, cx);
                    if let Some(loaded) = preloaded {
                        shell.preload(loaded);
                    }
                    if let Some(v) = view.as_deref() {
                        shell.open_section_named(v);
                    }
                    // Dev-only: a card to show selected, and the Share dialog.
                    shell.inbox.selected = std::env::var("HIPPO_APP_DEV_SELECT").ok();
                    shell.inbox.dev_share = std::env::var_os("HIPPO_APP_DEV_SHARE").is_some();
                    // Dev-only, the night map: what to select (`rule:0`,
                    // `fix:1`, `note:<path>`, `session:0`, `incident:0`), a
                    // dialog drawn as a view, and Brain's zoom.
                    shell.night.dev_select = std::env::var("HIPPO_APP_DEV_SELECT").ok();
                    shell.night.dev_dialog = std::env::var("HIPPO_APP_DEV_DIALOG").ok();
                    shell.night.dev_scroll_end =
                        std::env::var("HIPPO_APP_DEV_SCROLL").is_ok_and(|v| v == "end");
                    shell.night.dev_zoom = std::env::var("HIPPO_APP_DEV_ZOOM")
                        .ok()
                        .and_then(|z| z.parse().ok());
                    shell
                })
            })
            .expect("the window opens");
        // Dev-only: HIPPO_APP_DEV_THEN (`brain:4`) shows that section after
        // that many seconds, as a click on its tab does: for a screenshot of
        // what a change of view closes.
        if let Some((name, secs)) = std::env::var("HIPPO_APP_DEV_THEN").ok().and_then(|v| {
            let (n, s) = v.split_once(':')?;
            Some((n.to_owned(), s.parse::<f32>().ok()?))
        }) {
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs_f32(secs))
                    .await;
                let _ = handle.update(cx, |shell, window, cx| {
                    shell.show_section_named(&name, window, cx);
                });
            })
            .detach();
        }
        // Dev-only: HIPPO_APP_DEV_THEME (`dark`, `light`) draws Hippo's
        // theme of that appearance, saving nothing.
        if let Ok(which) = std::env::var("HIPPO_APP_DEV_THEME") {
            let [light, dark] = hippo_core::theme::builtin();
            let mut theme = if which == "light" { light } else { dark };
            hippo_core::theme::apply_focus_floor(&mut theme);
            cx.set_global(ui::theme::ActiveTheme {
                theme,
                system: false,
            });
        }
        if dev_size.is_none() && dev_view.is_none() {
            cx.activate(true);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use hippo_view::night::{KNOWN, agent};

    /// Every agent hippo knows is drawn with its own mark, the same file as
    /// brand/agents/; the terminal icon is only for an agent it does not know.
    #[test]
    fn every_known_agent_draws_its_own_mark() {
        let brand = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../brand/agents");
        for (code, tool, name, stem) in KNOWN {
            let a = agent(tool);
            assert_eq!(a.code, code);
            let path = ui::night::kit::mark_path(&a);
            assert_eq!(path, format!("icons/agent-{stem}.svg"), "{name}");
            let drawn = Assets
                .load(path)
                .unwrap()
                .unwrap_or_else(|| panic!("{name}: no asset"));
            let file = std::fs::read(brand.join(format!("{stem}.svg"))).unwrap();
            assert_eq!(
                drawn.as_ref(),
                file.as_slice(),
                "{name}: not brand/agents/{stem}.svg"
            );
        }
        assert_eq!(
            ui::night::kit::mark_path(&agent("zed")),
            "icons/terminal.svg"
        );
        assert!(Assets.load("icons/terminal.svg").unwrap().is_some());
    }

    /// Every icon the night views name is drawn: the names in their source
    /// (`icon(look, "…"`, `icon_button(look, t, "<id>", "…"`, `tile(look, t,
    /// "…"`, and the two names of a `tile(look, t, if … { "…" } else { "…" }`)
    /// and the guard's incident kinds. In 0.7.1 Brain's zoom buttons asked
    /// for `n-minus` and `n-plus`, which had no file, and drew nothing.
    #[test]
    fn every_icon_the_night_views_name_is_drawn() {
        let sources = [
            include_str!("ui/night/mod.rs"),
            include_str!("ui/night/kit.rs"),
            include_str!("ui/night/morning.rs"),
            include_str!("ui/night/brain.rs"),
            include_str!("ui/night/agents.rs"),
            include_str!("ui/night/guard.rs"),
        ];
        let literal = |s: &str| -> Option<String> {
            let s = s.trim_start().strip_prefix('"')?;
            Some(s[..s.find('"')?].to_owned())
        };
        let mut names = std::collections::BTreeSet::new();
        for src in sources {
            let src = src.split_whitespace().collect::<Vec<_>>().join(" ");
            for (needle, skip_id) in [
                ("icon(look, ", false),
                ("icon( look, ", false),
                ("icon_button( look, t, ", true),
                ("icon_button(look, t, ", true),
                ("tile(look, t, ", false),
                ("tile( look, t, ", false),
            ] {
                for (at, _) in src.match_indices(needle) {
                    let mut rest = &src[at + needle.len()..];
                    if skip_id {
                        let Some(id) = literal(rest) else { continue };
                        rest = &rest.trim_start()[id.len() + 2..];
                        rest = rest.trim_start().strip_prefix(',').unwrap_or(rest);
                    }
                    if let Some(name) = literal(rest) {
                        names.insert(name);
                    } else if let Some(cond) = rest.trim_start().strip_prefix("if ") {
                        for part in cond.split('{').skip(1).take(2) {
                            if let Some(name) = literal(part) {
                                names.insert(name);
                            }
                        }
                    }
                }
            }
        }
        for kind in ["script", "key", "tomodel", "clone", "guard"] {
            names.insert(kind.to_owned());
        }
        assert!(names.len() > 12, "{names:?}");
        for name in &names {
            assert!(
                icon(&format!("n-{name}")).is_some(),
                "icons/n-{name}.svg draws nothing"
            );
        }
    }
}
