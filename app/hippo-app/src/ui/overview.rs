//! The first-run steps with their exact commands, which Home shows while
//! something is missing, and the shared section heading and list row.

use std::path::Path;

use gpui::{ClipboardItem, Context, Role, SharedString, div, prelude::*};
use hippo_view::status::Status;

use super::Shell;
use super::theme::Look;
use crate::cli::{Installation, Source};
use crate::state::Tab;

/// A missing piece of a first run and the command that supplies it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub what: String,
    pub command: String,
}

/// The command as the user types it: with `HIPPO_HOME=` first when the app
/// runs against a home from its environment, so that it acts on that home.
pub fn command(installation: &Installation, args: &str) -> String {
    match installation.source {
        Source::Environment => {
            format!("HIPPO_HOME={} hippo {args}", shell_word(&installation.home))
        }
        _ => format!("hippo {args}"),
    }
}

/// A path as one shell word: quoted only when it needs to be.
fn shell_word(path: &Path) -> String {
    let s = path.display().to_string();
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+:@".contains(c))
    {
        s
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// The steps a first run still lacks, in order. `notes` is the
/// count of notes and the notes directory, once the notes are read.
pub fn steps(
    status: &Status,
    installation: &Installation,
    dream_at: &str,
    notes: Option<(usize, &Path)>,
) -> Vec<Step> {
    let mut out = Vec::new();
    if let Some((0, dir)) = notes {
        out.push(Step {
            what: format!(
                "No notes in {}. Write one there, or set notes_dir in .hippo/hippo.toml.",
                dir.display()
            ),
            command: format!("mkdir -p {}", shell_word(dir)),
        });
    }
    if status.last_ingest.is_none() {
        out.push(Step {
            what: "No sessions indexed yet. Ingest reads your agents' transcripts.".into(),
            command: command(installation, "ingest"),
        });
    }
    if matches!(&installation.source, Source::Fallback(r) if r.contains("no launchd job")) {
        out.push(Step {
            what: "The nightly ingest and dreaming jobs are not installed.".into(),
            command: command(installation, "schedule install"),
        });
    }
    if status.report.is_none() {
        out.push(Step {
            what: format!(
                "No dreaming run yet. The nightly job runs at {dream_at}; try one now without changing notes:"
            ),
            command: command(installation, "dream --dry-run"),
        });
    }
    out
}

impl Shell {
    /// A command in monospace with a Copy button.
    pub(super) fn render_command(
        &self,
        id: impl Into<SharedString>,
        command: String,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let id: SharedString = id.into();
        let copied = self.copied.as_deref() == Some(command.as_str());
        // The button first, so that a long command wraps beside it.
        div()
            .flex()
            .items_start()
            .gap(look.px(8.))
            .child(
                div()
                    .id(id)
                    .role(Role::Button)
                    .aria_label(format!("Copy {command}"))
                    .flex_shrink_0()
                    .w(look.px(48.))
                    .py(look.px(2.))
                    .rounded(look.px(3.))
                    .border(look.fixed(1.))
                    .border_color(look.c.border)
                    .flex()
                    .justify_center()
                    .text_size(look.text(11.))
                    .text_color(if copied {
                        look.c.text
                    } else {
                        look.c.text_muted
                    })
                    .cursor_pointer()
                    .hover(|h| h.bg(look.c.hover))
                    .child(if copied { "copied" } else { "copy" })
                    .on_click({
                        let command = command.clone();
                        cx.listener(move |this, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(command.clone()));
                            this.copied = Some(command.clone());
                            cx.notify();
                        })
                    }),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .px(look.px(6.))
                    .py(look.px(2.))
                    .rounded(look.px(3.))
                    .bg(look.c.hover)
                    .font_family("Menlo")
                    .text_size(look.text(12.))
                    .text_color(look.c.text)
                    .child(command),
            )
            .into_any_element()
    }

    /// A section heading in secondary text.
    pub(super) fn section_heading(look: &Look, text: &str) -> impl IntoElement {
        div()
            .pt(look.px(16.))
            .pb(look.px(4.))
            .text_size(look.text(11.))
            .text_color(look.c.text_muted)
            .child(super::kit::sentence(&text.to_uppercase()))
    }

    /// What a first run still lacks, each with its exact command and a
    /// Copy button: shown on Home while there is any.
    pub(super) fn render_get_started(
        &self,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Div> {
        let loaded = self.loaded.as_ref()?;
        let (Some(status), hippo_view::setup::Setup::Ready { cfg, .. }) =
            (&loaded.status, &loaded.setup)
        else {
            return None;
        };
        let panels = loaded.panels.as_ref();
        let notes_dir = panels.map(|p| (p.brain.rows.len(), p.notes_dir.clone()));
        let steps = steps(
            status,
            &self.installation,
            &cfg.dream_at,
            notes_dir
                .as_ref()
                .filter(|_| panels.is_some_and(|p| p.brain.error.is_none()))
                .map(|(n, d)| (*n, d.as_path())),
        );
        if steps.is_empty() {
            return None;
        }
        let mut column = super::kit::card(look)
            .p(look.px(16.))
            .child(super::kit::label(look, "Get started", look.c.text_accent));
        for (i, step) in steps.into_iter().enumerate() {
            column = column.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(look.px(4.))
                    .py(look.px(6.))
                    .child(div().child(step.what))
                    .child(self.render_command(
                        SharedString::from(format!("copy-step-{i}")),
                        step.command,
                        look,
                        cx,
                    )),
            );
        }
        Some(
            column.child(
                div()
                    .pt(look.px(4.))
                    .text_size(look.text(11.))
                    .text_color(look.c.text_muted)
                    .child("Run these in a terminal, then press ⌘R to reload."),
            ),
        )
    }

    /// A row of the overview that opens `tab` in a normal tab.
    pub(super) fn overview_row(
        &self,
        id: SharedString,
        title: String,
        meta: Option<String>,
        tab: Tab,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        div()
            .id(id)
            .role(Role::Link)
            .aria_label(title.clone())
            .flex()
            .flex_col()
            .justify_center()
            .h(look.px(if meta.is_some() { 40. } else { 24. }))
            .px(look.px(8.))
            .mx(look.px(-8.))
            .rounded(look.px(4.))
            .cursor_pointer()
            .hover(|h| h.bg(look.c.hover))
            .child(div().truncate().child(title))
            .children(meta.map(|m| {
                div()
                    .truncate()
                    .text_size(look.text(11.))
                    .text_color(look.c.text_muted)
                    .child(m)
            }))
            .on_click(cx.listener(move |this, e: &gpui::ClickEvent, window, cx| {
                // As a dock row: a click previews, a double click opens.
                this.state.tabs.follow(tab.clone(), e.click_count() >= 2);
                this.after_state_change(window, cx);
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hippo_core::dream::report::ReportStatus;
    use std::path::PathBuf;

    fn status() -> Status {
        Status {
            report: None,
            warnings: Vec::new(),
            last_ingest: None,
            ingest_stale: false,
            index_format: None,
            errors: Vec::new(),
        }
    }

    fn inst(source: Source) -> Installation {
        Installation {
            binary: PathBuf::from("/bin/hippo"),
            home: PathBuf::from("/tmp/demo home"),
            source,
        }
    }

    #[test]
    fn a_first_run_lists_each_missing_piece_with_its_command() {
        let i = inst(Source::Fallback(
            "no launchd job (run hippo schedule install)".into(),
        ));
        let s = steps(&status(), &i, "03:30", Some((0, Path::new("/w/notes"))));
        let commands: Vec<&str> = s.iter().map(|s| s.command.as_str()).collect();
        assert_eq!(
            commands,
            [
                "mkdir -p /w/notes",
                "hippo ingest",
                "hippo schedule install",
                "hippo dream --dry-run"
            ]
        );
        assert!(s[3].what.contains("03:30"));
    }

    #[test]
    fn a_dev_home_puts_hippo_home_in_front_quoted() {
        let i = inst(Source::Environment);
        let s = steps(&status(), &i, "03:30", Some((3, Path::new("/w/notes"))));
        assert_eq!(s[0].command, "HIPPO_HOME='/tmp/demo home' hippo ingest");
        assert_eq!(s.len(), 2, "notes exist and launchd is not judged");
    }

    #[test]
    fn nothing_missing_means_no_steps() {
        let mut st = status();
        st.last_ingest = Some(time::OffsetDateTime::UNIX_EPOCH);
        st.report = Some(ReportStatus {
            path: PathBuf::from("r.md"),
            date: time::macros::date!(2026 - 11 - 08),
            seq: 1,
            failed: false,
            error: None,
            dry_run: false,
        });
        assert!(steps(&st, &inst(Source::LaunchAgent), "03:30", None).is_empty());
    }
}
