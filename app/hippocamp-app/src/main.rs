//! Hippocamp.app: a macOS window over hippocamp's files, on GPUI.

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

/// The assets: the hippocamp mark and Lucide's icons (ISC, `icons/LICENSE-lucide.txt`),
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
        _ => return None,
    })
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        if path == "hippocamp-mark.svg" {
            return Ok(Some(Cow::Borrowed(
                include_bytes!("../../../brand/hippocamp-mark.svg").as_slice(),
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

/// When the process started, for `HIPPOCAMP_APP_TIMING`.
pub static STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

fn main() {
    STARTED.get_or_init(std::time::Instant::now);
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
        let options = ui::window_options(cx);
        // Read once: `HIPPOCAMP_APP_PREFS` moves the preferences file.
        let prefs_path = prefs::default_path();
        cx.open_window(options, |window, cx| {
            cx.new(|cx| ui::Shell::new(installation, prefs_path, window, cx))
        })
        .expect("the window opens");
        cx.activate(true);
    });
}
