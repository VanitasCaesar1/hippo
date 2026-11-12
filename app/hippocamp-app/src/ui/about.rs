//! The About window: versions, the binary in use, the home, and what the app
//! does not support.

use gpui::{Context, FocusHandle, KeyDownEvent, Role, Window, div, prelude::*};

use super::theme::Look;
use crate::cli::Installation;

/// What the window says about screen readers:
/// until Zed fixes the two VoiceOver bugs and the
/// walk-through passes, the app does not support them.
pub const SCREEN_READER_NOTE: &str = "Hippocamp does not support screen readers yet. Everything it shows is also in the hippocamp command line and in hippocamp's files.";

pub struct About {
    pub installation: Installation,
    /// `hippocamp --version`, once it has been read.
    pub cli_version: Option<String>,
    /// Focused when the window opens, so Escape and Command-W close it.
    pub focus: FocusHandle,
}

/// Whether a key press closes the About window: Escape or Command-W.
pub fn closes(key: &str, command: bool) -> bool {
    key == "escape" || (command && key == "w")
}

/// The lines of the window, as text.
pub fn lines(installation: &Installation, cli_version: Option<&str>) -> Vec<(String, String)> {
    vec![
        ("Hippocamp.app".into(), env!("CARGO_PKG_VERSION").into()),
        (
            "hippocamp".into(),
            cli_version.unwrap_or("reading the version").into(),
        ),
        (
            "Binary".into(),
            if installation.binary_exists() {
                installation.binary.display().to_string()
            } else {
                format!("{} (not found)", installation.binary.display())
            },
        ),
        (
            "HIPPOCAMP_HOME".into(),
            installation.home.display().to_string(),
        ),
    ]
}

impl Render for About {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::from_app(cx);
        div()
            .id("about")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|_, e: &KeyDownEvent, window, _| {
                if closes(&e.keystroke.key, e.keystroke.modifiers.platform) {
                    window.remove_window();
                }
            }))
            .role(Role::Dialog)
            .aria_label("About Hippocamp")
            .flex()
            .flex_col()
            .size_full()
            .gap(look.px(8.))
            .p(look.px(24.))
            .bg(look.c.centre)
            .text_size(look.text(13.))
            .text_color(look.c.text)
            .child(div().text_size(look.text(20.)).child("Hippocamp"))
            .children(
                lines(&self.installation, self.cli_version.as_deref())
                    .into_iter()
                    .map(|(k, v)| {
                        div()
                            .flex()
                            .gap(look.px(12.))
                            .child(
                                div()
                                    .w(look.px(90.))
                                    .flex_shrink_0()
                                    .text_color(look.c.text_muted)
                                    .child(k),
                            )
                            .child(div().child(v))
                    }),
            )
            .child(
                div()
                    .mt(look.px(8.))
                    .text_color(look.c.text_muted)
                    .child(SCREEN_READER_NOTE),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Source;
    use std::path::PathBuf;

    #[test]
    fn the_window_shows_both_versions_the_binary_and_the_home() {
        let inst = Installation {
            binary: PathBuf::from("/no/such/hippocamp"),
            home: PathBuf::from("/h/.hippocamp"),
            source: Source::LaunchAgent,
        };
        let l = lines(&inst, Some("hippocamp 0.0.1"));
        assert_eq!(l[0].1, env!("CARGO_PKG_VERSION"));
        assert_eq!(l[1].1, "hippocamp 0.0.1");
        assert!(l[2].1.ends_with("(not found)"));
        assert_eq!(l[3], ("HIPPOCAMP_HOME".into(), "/h/.hippocamp".into()));
        assert!(SCREEN_READER_NOTE.contains("screen readers"));
    }

    #[test]
    fn escape_and_command_w_close_the_window() {
        assert!(closes("escape", false));
        assert!(closes("w", true));
        assert!(!closes("w", false) && !closes("q", true));
    }
}
