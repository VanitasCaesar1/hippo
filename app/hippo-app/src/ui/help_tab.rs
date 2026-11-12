//! The Help tab: the key map and the commands the app does not offer.

use gpui::{Role, div, prelude::*};

use super::theme::Look;
use crate::actions::{TABLE, display_keys};

/// Each action with a name and a key: its name and every key it has.
pub fn key_rows() -> Vec<(String, String)> {
    TABLE
        .iter()
        .filter(|d| d.palette.is_some() && !d.keys.is_empty())
        .map(|d| {
            let keys = d
                .keys
                .iter()
                .map(|k| display_keys(k))
                .collect::<Vec<_>>()
                .join(", ");
            (d.palette.unwrap_or(d.id).to_string(), keys)
        })
        .collect()
}

/// Facts the app shows and where they are outside it: nothing is app-only.
pub const CLI_ROWS: &[(&str, &str)] = &[
    (
        "Ask: sources and a cited answer",
        "hippo ask <question> (--json, --sources-only)",
    ),
    ("Fleet and headroom", "hippo fleet (--json)"),
    (
        "Resume, on this profile or another",
        "hippo resume <id> (--on <profile>)",
    ),
    ("Looks right", "hippo review <receipt> --ok"),
    (
        "Context preview",
        "hippo context --task <task> (--json for the paths)",
    ),
    (
        "A note's lint findings and change receipts",
        "hippo show note <path>",
    ),
    ("Sessions", "hippo log --stdout; hippo show session <id>"),
    ("Search one phrase", "hippo recall <phrase>"),
    (
        "A proposal with its receipts and evidence",
        "hippo show proposal <id>",
    ),
    (
        "Revert",
        "hippo revert <receipt>; hippo revert --force <receipt>",
    ),
    ("Last ingest", "hippo doctor"),
];

/// Commands the app does not offer.
pub const NOT_IN_APP: &[&str] = &[
    "hippo dream",
    "hippo dream --dry-run",
    "hippo ingest",
    "hippo doctor",
];

pub fn render(look: &Look) -> impl IntoElement {
    let section = |title: &'static str| {
        div()
            .mt(look.px(16.))
            .mb(look.px(4.))
            .text_size(look.text(14.))
            .text_color(look.c.text)
            .child(title)
    };
    let row = |left: String, right: String| {
        div()
            .flex()
            .gap(look.px(16.))
            .h(look.px(24.))
            .items_center()
            .child(div().w(look.px(280.)).flex_shrink_0().child(left))
            .child(div().text_color(look.c.text_muted).child(right))
    };
    div()
        .id("help")
        .role(Role::List)
        .aria_label("Help")
        .flex()
        .flex_col()
        .size_full()
        .overflow_hidden()
        .p(look.px(24.))
        .text_size(look.text(13.))
        .text_color(look.c.text)
        .child(div().text_size(look.text(18.)).child("Hippo Help"))
        .child(section("Keys"))
        .children(key_rows().into_iter().map(|(n, k)| row(n, k)))
        .child(section("The same facts from the command line"))
        .children(
            CLI_ROWS
                .iter()
                .map(|(a, b)| row((*a).to_string(), (*b).to_string())),
        )
        .child(section("Commands the app does not offer"))
        .children(NOT_IN_APP.iter().map(|c| {
            div()
                .h(look.px(24.))
                .flex()
                .items_center()
                .text_color(look.c.text_muted)
                .child(*c)
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_map_lists_the_keys_of_the_design() {
        let rows = key_rows();
        let keys = |name: &str| {
            rows.iter()
                .find(|(n, _)| n == name)
                .unwrap_or_else(|| panic!("{name} missing"))
                .1
                .clone()
        };
        assert_eq!(keys("View: Command Palette"), "⌘⇧P");
        assert_eq!(keys("Memory: Toggle Notes List"), "⌘B");
        assert_eq!(keys("View: Home"), "⌘1");
        assert_eq!(keys("View: Inbox"), "⌘I, ⌘2");
        assert_eq!(keys("View: Fleet"), "⌘F, ⌘3");
        assert_eq!(keys("View: Memory"), "⌘4");
        assert_eq!(keys("Window: Minimize"), "⌘M");
        assert_eq!(keys("Hippo: Manage"), "⌘,, ⌘5");
        assert_eq!(keys("View: Ask"), "⌘K");
        assert_eq!(keys("View: Back"), "⌘[, ⎋");
        assert_eq!(keys("Theme: Select Theme"), "⌘⇧T");
    }

    #[test]
    fn the_commands_the_app_does_not_offer_are_listed() {
        assert!(NOT_IN_APP.contains(&"hippo ingest"));
        assert_eq!(NOT_IN_APP.len(), 4);
    }
}
