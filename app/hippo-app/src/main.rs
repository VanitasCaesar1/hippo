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

/// A Lucide icon's file by its name. The names of the app's earlier icons
/// are kept as aliases so every caller draws a Lucide glyph.
fn icon(name: &str) -> Option<&'static [u8]> {
    let name = match name {
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
        other => other,
    };
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
        "n-morning" => include_bytes!("../icons/n-morning.svg").as_slice(),
        "n-brain" => include_bytes!("../icons/n-brain.svg").as_slice(),
        "n-agents" => include_bytes!("../icons/n-agents.svg").as_slice(),
        "n-guard" => include_bytes!("../icons/n-guard.svg").as_slice(),
        "n-note" => include_bytes!("../icons/n-note.svg").as_slice(),
        "n-folder" => include_bytes!("../icons/n-folder.svg").as_slice(),
        "n-check" => include_bytes!("../icons/n-check.svg").as_slice(),
        "n-x" => include_bytes!("../icons/n-x.svg").as_slice(),
        "n-undo" => include_bytes!("../icons/n-undo.svg").as_slice(),
        "n-said" => include_bytes!("../icons/n-said.svg").as_slice(),
        "n-rule" => include_bytes!("../icons/n-rule.svg").as_slice(),
        "n-receipt" => include_bytes!("../icons/n-receipt.svg").as_slice(),
        "n-clock" => include_bytes!("../icons/n-clock.svg").as_slice(),
        "n-up" => include_bytes!("../icons/n-up.svg").as_slice(),
        "n-search" => include_bytes!("../icons/n-search.svg").as_slice(),
        "n-sun" => include_bytes!("../icons/n-sun.svg").as_slice(),
        "n-moon" => include_bytes!("../icons/n-moon.svg").as_slice(),
        "n-terminal" => include_bytes!("../icons/n-terminal.svg").as_slice(),
        "n-right" => include_bytes!("../icons/n-right.svg").as_slice(),
        "n-script" => include_bytes!("../icons/n-script.svg").as_slice(),
        "n-key" => include_bytes!("../icons/n-key.svg").as_slice(),
        "n-tomodel" => include_bytes!("../icons/n-tomodel.svg").as_slice(),
        "n-clone" => include_bytes!("../icons/n-clone.svg").as_slice(),
        "n-fix" => include_bytes!("../icons/n-fix.svg").as_slice(),
        "n-eye" => include_bytes!("../icons/n-eye.svg").as_slice(),
        "n-down" => include_bytes!("../icons/n-down.svg").as_slice(),
        "n-open" => include_bytes!("../icons/n-open.svg").as_slice(),
        "n-thread" => include_bytes!("../icons/n-thread.svg").as_slice(),
        "n-wait" => include_bytes!("../icons/n-wait.svg").as_slice(),
        "n-start" => include_bytes!("../icons/n-start.svg").as_slice(),
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
        cx.open_window(options, |window, cx| {
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
