//! The full-window messages for a hippocamp that is not set up, and their text.

use std::path::Path;

use gpui::{Context, Role, SharedString, div, prelude::*, svg};
use hippocamp_view::setup::Setup;

use super::Shell;
use super::theme::Look;

/// What the window says instead of the panels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupMessage {
    pub headline: String,
    /// The error or detail under it.
    pub detail: Option<String>,
    /// The commands to run, as `hippocamp` arguments, each shown with Copy.
    pub commands: Vec<&'static str>,
}

const NEXT_STEP: [&str; 2] = ["init", "schedule install"];

/// The message for a setup that is not ready; `None` for `Ready`. `home` is
/// the `HIPPOCAMP_HOME` the app looked in.
pub fn message(setup: &Setup, home: &Path) -> Option<SetupMessage> {
    match setup {
        Setup::Ready { .. } => None,
        // History alone is a normal state: the shell shows it.
        Setup::HistoryOnly { .. } => None,
        Setup::NotSetUp { home, error } => Some(SetupMessage {
            headline: format!("hippocamp is not set up for {}. Run:", home.display()),
            detail: Some(error.clone()),
            commands: NEXT_STEP.to_vec(),
        }),
        Setup::WorkspaceMissing { path } => Some(SetupMessage {
            headline: format!(
                "hippocamp is not set up for {}: {} is missing. Run:",
                home.display(),
                path.display()
            ),
            detail: None,
            commands: NEXT_STEP.to_vec(),
        }),
        Setup::AccessDenied { path } => Some(SetupMessage {
            headline: format!(
                "macOS has not allowed Hippocamp to read {}. Allow it in System Settings › Privacy & Security › Files and Folders.",
                path.display()
            ),
            detail: None,
            commands: Vec::new(),
        }),
    }
}

impl Shell {
    /// The message drawn over the whole window, top-left, with each command
    /// copyable and Command-R to retry.
    pub(super) fn render_setup(
        &self,
        message: &SetupMessage,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut rows = Vec::new();
        for (i, args) in message.commands.iter().enumerate() {
            let c = super::overview::command(&self.installation, args);
            rows.push(self.render_command(
                SharedString::from(format!("copy-setup-{i}")),
                c,
                look,
                cx,
            ));
        }
        div()
            .id("setup-message")
            .role(Role::Dialog)
            .aria_label("hippocamp is not set up")
            .flex_1()
            .bg(look.c.centre)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .max_w(look.px(720.))
                    .gap(look.px(8.))
                    .p(look.px(24.))
                    .text_size(look.text(13.))
                    .text_color(look.c.text)
                    .child(
                        svg()
                            .path("hippocamp-mark.svg")
                            .w(look.px(48.))
                            .h(look.px(40.))
                            .text_color(look.c.text_muted),
                    )
                    .child(div().pt(look.px(8.)).child(message.headline.clone()))
                    .children(message.detail.clone().map(|detail| {
                        div()
                            .text_size(look.text(12.))
                            .text_color(look.c.error)
                            .child(detail)
                    }))
                    .children(rows)
                    .child(
                        div()
                            .pt(look.px(8.))
                            .text_size(look.text(11.))
                            .text_color(look.c.text_muted)
                            .child("Then press ⌘R to try again."),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn each_state_has_its_message() {
        let home = PathBuf::from("/h/.hippocamp");
        let not_set_up = message(
            &Setup::NotSetUp {
                home: home.clone(),
                error: "no config.toml".into(),
            },
            &home,
        )
        .unwrap();
        assert_eq!(
            not_set_up.headline,
            "hippocamp is not set up for /h/.hippocamp. Run:"
        );
        assert_eq!(not_set_up.commands, ["init", "schedule install"]);
        assert_eq!(not_set_up.detail.as_deref(), Some("no config.toml"));

        let missing = message(
            &Setup::WorkspaceMissing {
                path: PathBuf::from("/w/.hippocamp/hippocamp.toml"),
            },
            &home,
        )
        .unwrap();
        assert!(
            missing
                .headline
                .contains("/w/.hippocamp/hippocamp.toml is missing")
        );
        assert!(missing.headline.contains("/h/.hippocamp"));
        assert_eq!(missing.commands.len(), 2);

        let denied = message(
            &Setup::AccessDenied {
                path: PathBuf::from("/Volumes/X/ws"),
            },
            &home,
        )
        .unwrap();
        assert!(
            denied
                .headline
                .starts_with("macOS has not allowed Hippocamp to read /Volumes/X/ws.")
        );
        assert!(
            denied
                .headline
                .contains("Privacy & Security › Files and Folders")
        );
    }
}
