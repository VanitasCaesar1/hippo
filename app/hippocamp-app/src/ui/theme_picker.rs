//! The theme picker (Command-Shift-T) and the shell's part in themes: the
//! once-a-second look at the theme in use's file, listing the theme
//! directories again, and applying a theme. The catalogue and the rules live
//! in [`crate::themes`].

use std::path::Path;
use std::time::Duration;

use gpui::{AppContext as _, Context, Window};
use hippocamp_core::theme::CatalogEntry;

use super::picker::{Picker, PickerEvent, PickerItem};
use super::theme::ActiveTheme;
use super::{OpenPicker, Shell};
use crate::themes::{self, Probe, shown};

/// The picker's rows, with the theme id of each.
pub struct Rows {
    pub ids: Vec<String>,
    pub items: Vec<PickerItem>,
    /// The row of the theme in use.
    pub current: Option<usize>,
}

/// System, the built-in themes, the user's themes, then theme packs' themes,
/// each as its name with its source in secondary text; then, dimmed with
/// their errors, the files that cannot be used and the duplicates.
pub fn rows(catalog: &[CatalogEntry], user_error: Option<&str>, in_use: &str) -> Rows {
    let mut usable = Vec::new();
    let mut unusable = Vec::new();
    for entry in catalog {
        let mut item = PickerItem::new(shown(&entry.name));
        let detail = if entry.id == "system" {
            Some("follows the light or dark appearance".to_string())
        } else if entry.source != entry.name {
            Some(shown(&entry.source))
        } else {
            None
        };
        match (&entry.theme, entry.duplicate) {
            (Ok(_), true) => {
                item.detail = detail;
                item.unavailable = Some("repeats a theme name in this file".into());
                unusable.push((entry.id.clone(), item));
            }
            (Err(e), _) => {
                item.detail = detail;
                item.unavailable = Some(shown(e));
                unusable.push((entry.id.clone(), item));
            }
            (Ok(theme), false) => {
                let warnings = match theme.warnings.as_slice() {
                    [] => None,
                    [one] => Some(format!("warning: {}", shown(one))),
                    many => Some(format!("{} warnings", many.len())),
                };
                item.detail = match (detail, warnings) {
                    (Some(d), Some(w)) => Some(format!("{d} · {w}")),
                    (d, w) => d.or(w),
                };
                usable.push((entry.id.clone(), item));
            }
        }
    }
    if let Some(e) = user_error {
        let mut item = PickerItem::new("Your themes");
        item.unavailable = Some(shown(e));
        unusable.insert(0, (String::new(), item));
    }
    let current = usable.iter().position(|(id, _)| id == in_use);
    let (ids, items) = usable.into_iter().chain(unusable).unzip();
    Rows {
        ids,
        items,
        current,
    }
}

impl Shell {
    /// Opens the theme picker over a fresh listing of the theme directories,
    /// on the theme in use. Opened again, it closes.
    pub(crate) fn open_theme_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = &self.picker {
            let was_theme = open.theme_before.is_some();
            self.close_picker(window, cx);
            if was_theme {
                return;
            }
        }
        self.themes.rescan();
        let before = self.themes.id().to_string();
        let rows = rows(
            self.themes.catalog(),
            self.themes.user_error(),
            self.themes.id(),
        );
        let restore = window.focused(cx);
        let items = rows.items.clone();
        let entity = cx.new(|cx| {
            Picker::new("themes", "Theme", "Select a theme", items, window, cx).holding_previews()
        });
        if let Some(i) = rows.current {
            entity.update(cx, |p, cx| p.select_item(i, cx));
        }
        let ids = rows.ids;
        let subscription = cx.subscribe_in(
            &entity,
            window,
            move |this, _, event: &PickerEvent, window, cx| match event {
                PickerEvent::Selected(i) => {
                    if let Some(id) = ids.get(*i) {
                        this.preview_theme(id, window, cx);
                    }
                }
                PickerEvent::Confirmed(i) => {
                    if let Some(id) = ids.get(*i) {
                        this.confirm_theme(id, window, cx);
                    }
                }
                PickerEvent::Dismissed => this.close_picker(window, cx),
            },
        );
        self.picker = Some(OpenPicker {
            entity,
            restore,
            theme_before: Some(before),
            _subscription: subscription,
        });
        self.after_state_change(window, cx);
    }

    fn show_theme(&mut self, active: ActiveTheme, window: &mut Window, cx: &mut Context<Self>) {
        cx.set_global(active);
        window.refresh();
        cx.notify();
    }

    /// A selection in the picker: its colours and font at once, without the
    /// background image. A row that cannot be used changes nothing.
    fn preview_theme(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(active) = self.themes.preview(id, window.appearance()) {
            self.show_theme(active, window, cx);
        }
    }

    /// Return: keep the theme and save it.
    fn confirm_theme(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        match self.themes.commit(id, window.appearance()) {
            Ok(active) => self.show_theme(active, window, cx),
            Err(notice) => self.themes.set_notice(Some(notice)),
        }
        // The choice is made: closing must not put the old theme back.
        if let Some(open) = self.picker.as_mut() {
            open.theme_before = None;
        }
        self.close_picker(window, cx);
    }

    /// Escape: the theme in use before, found again by its id.
    pub(super) fn restore_theme(
        &mut self,
        before: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let active = self.themes.restore(before, window.appearance());
        self.show_theme(active, window, cx);
    }

    /// "Theme: Use System": System, saved, without the picker.
    pub(super) fn use_system_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.themes.use_system(window.appearance());
        self.show_theme(active, window, cx);
    }

    /// Lists the theme directories again on the background executor.
    pub(super) fn rescan_themes(&mut self, cx: &mut Context<Self>) {
        let user = self.themes.themes_dir().to_path_buf();
        let packs = self.themes.packs_dir().map(Path::to_path_buf);
        cx.spawn(async move |this, cx| {
            let scanned = cx
                .background_executor()
                .spawn(async move { themes::scan(&user, packs.as_deref()) })
                .await;
            this.update(cx, |this, cx| {
                this.themes.set_scanned(scanned);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Once a second, on the background executor, looks at the size and
    /// modification time of the theme in use's file and reads it again when
    /// either changed. GPUI has no file watching, and `notify`'s licence is
    /// not on the allowlist: this is a poll of one file.
    pub(super) fn start_theme_poll(&mut self, cx: &mut Context<Self>) {
        self._theme_poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let Ok(request) = this.update(cx, |this, _| this.themes.probe_request()) else {
                    break;
                };
                let Some(request) = request else { continue };
                let probe = cx
                    .background_executor()
                    .spawn(async move { themes::probe(request) })
                    .await;
                if this
                    .update(cx, |this, cx| this.apply_probe(probe, cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    /// One look at the theme file, now (tests drive the poll by hand).
    #[cfg(test)]
    pub(crate) fn poll_theme_now(&mut self, cx: &mut Context<Self>) {
        if let Some(active) = self.themes.poll_now() {
            cx.set_global(active);
            cx.notify();
        }
    }

    /// What a look at the theme file found: a new version is applied (except
    /// under the picker's preview, which the picker puts right when it
    /// closes), and a file that stops parsing leaves the last good version.
    pub(crate) fn apply_probe(&mut self, probe: Probe, cx: &mut Context<Self>) {
        // An unchanged file (nearly every second) redraws nothing.
        let unchanged = probe.unchanged();
        let changed = self.themes.apply_probe(probe);
        if let Some(active) = changed
            && !self.themes.previewing
        {
            cx.set_global(active);
        }
        if !unchanged {
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn entry_rows(dir: &Path) -> Rows {
        let mut s = themes::ThemeState::new(
            dir,
            None,
            dir.join("prefs.json"),
            HashSet::new(),
            gpui::WindowAppearance::Dark,
        );
        s.rescan();
        rows(s.catalog(), s.user_error(), "builtin:Hippocamp Dark")
    }

    #[test]
    #[allow(clippy::disallowed_methods)] // the test writes theme files
    fn usable_rows_come_first_and_unusable_ones_are_dimmed_with_their_errors() {
        let dir = tempfile::tempdir().unwrap();
        let themes_dir = dir.path().join("themes");
        std::fs::create_dir_all(&themes_dir).unwrap();
        std::fs::write(themes_dir.join("a-bad.json"), "{ nope").unwrap();
        std::fs::write(
            themes_dir.join("b-twice.json"),
            r##"{"themes":[{"name":"Twice","appearance":"dark","style":{"background":"#111111ff"}},
                 {"name":"Twice","appearance":"dark","style":{"background":"#222222ff"}}]}"##
                .replace("{\"themes\"", "{\"name\":\"F\",\"themes\""),
        )
        .unwrap();
        let r = entry_rows(dir.path());
        let texts: Vec<&str> = r.items.iter().map(|i| i.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "System",
                "Hippocamp Light",
                "Hippocamp Dark",
                "Twice",
                "a-bad.json",
                "Twice"
            ]
        );
        assert_eq!(r.items[1].detail.as_deref(), Some("Built-in"));
        assert!(
            r.items[3]
                .detail
                .as_deref()
                .unwrap()
                .starts_with("b-twice.json")
        );
        assert!(
            r.items[4]
                .unavailable
                .as_deref()
                .unwrap()
                .contains("not valid JSON")
        );
        assert!(r.items[5].unavailable.is_some(), "the repeat is dimmed");
        assert_eq!(r.current, Some(2), "opens on the theme in use");
        assert!(r.items[..4].iter().all(|i| i.unavailable.is_none()));
    }
}
