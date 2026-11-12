//! The context preview tab: a task field run on Return, and what the
//! installed `hippocamp context --task <task> --json` prints: the block, its size
//! against the budget, what was left out, and the notes it names.

use gpui::{Context, Role, SharedString, div, prelude::*};
use hippocamp_view::text::visible;

use super::Shell;
use super::theme::Look;
use crate::cli::{self, ContextOutput};
use crate::state::Tab;

/// The preview's state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextState {
    /// The task last run.
    pub task: Option<String>,
    pub running: bool,
    /// The last result: the output, or the message to show.
    pub result: Option<Result<ContextOutput, String>>,
}

/// `1234` as `1.2 KB`.
pub fn size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    }
}

impl Shell {
    /// Runs the installed CLI for the task in the field.
    pub(crate) fn run_context(&mut self, cx: &mut Context<Self>) {
        let task = self.context_task.read(cx).text().trim().to_string();
        if task.is_empty() || self.context.running {
            return;
        }
        let Some(workspace) = self.workspace_root() else {
            return;
        };
        self.context.running = true;
        self.context.task = Some(task.clone());
        let installation = self.installation.clone();
        cx.spawn(async move |this, cx| {
            let out = cx
                .background_executor()
                .spawn(async move {
                    cli::run(
                        &installation.binary,
                        &installation.home,
                        &workspace,
                        &cli::context_args(&task),
                        cli::Limits::commands(),
                    )
                })
                .await;
            let result = match out {
                Ok(out) => cli::parse_context(&out).map_err(|e| e.message()),
                Err(e) => Err(format!(
                    "hippocamp not found at {}: {e}",
                    this.read_with(cx, |s, _| s.installation.binary.display().to_string())
                        .unwrap_or_default()
                )),
            };
            this.update(cx, |this, cx| {
                this.context.running = false;
                this.context.result = Some(result);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn render_context_tab(
        &self,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let state = &self.context;
        let mut column = div()
            .id("context-tab")
            .role(Role::Region)
            .aria_label("Context preview")
            .flex()
            .flex_col()
            .w_full()
            .max_w(look.px(720.))
            .p(look.px(16.))
            .gap(look.px(4.))
            .text_size(look.text(13.))
            .child(
                div()
                    .text_size(look.text(15.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Context preview"),
            )
            .child(
                div()
                    .text_size(look.text(11.))
                    .text_color(look.c.text_muted)
                    .child("What an agent gets for a task: hippocamp context --task <task>. Type a task and press Return."),
            )
            .child(
                div()
                    .key_context("ContextTask")
                    .pt(look.px(8.))
                    .child(self.context_task.clone()),
            );

        if state.running {
            column = column.child(
                div()
                    .pt(look.px(8.))
                    .text_color(look.c.text_muted)
                    .child("Running hippocamp context…"),
            );
        }
        match &state.result {
            None => {}
            Some(Err(message)) => {
                column = column.child(
                    div()
                        .pt(look.px(8.))
                        .text_color(look.c.error)
                        .child(visible(message)),
                );
            }
            Some(Ok(out)) => {
                let mut facts = vec![format!("{} of {}", size(out.bytes), size(out.budget))];
                if !out.omitted.is_empty() {
                    facts.push(format!(
                        "left out: {}",
                        out.omitted
                            .iter()
                            .map(|o| format!("{} ({})", visible(&o.part), size(o.bytes)))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                column = column.child(
                    div()
                        .pt(look.px(8.))
                        .text_size(look.text(11.))
                        .text_color(look.c.text_muted)
                        .child(facts.join(" · ")),
                );
                let notes: Vec<(&str, &std::path::PathBuf)> = out
                    .required
                    .iter()
                    .map(|p| ("required", p))
                    .chain(out.relevant.iter().map(|p| ("relevant", p)))
                    .collect();
                if !notes.is_empty() {
                    column = column.child(Self::section_heading(look, "Notes it names"));
                    for (i, (why, path)) in notes.into_iter().enumerate() {
                        let tab = Tab::Note(path.clone());
                        let title = self.tab_title(&tab);
                        column = column.child(
                            div()
                                .id(SharedString::from(format!("context-note-{i}")))
                                .role(Role::Link)
                                .flex()
                                .gap(look.px(8.))
                                .h(look.px(24.))
                                .items_center()
                                .cursor_pointer()
                                .child(div().text_color(look.c.text_accent).child(title))
                                .child(
                                    div()
                                        .text_size(look.text(11.))
                                        .text_color(look.c.text_muted)
                                        .child(format!(
                                            "{why} · {}",
                                            visible(&path.display().to_string())
                                        )),
                                )
                                .on_click(cx.listener(
                                    move |this, e: &gpui::ClickEvent, window, cx| {
                                        this.state.tabs.follow(tab.clone(), e.click_count() >= 2);
                                        this.after_state_change(window, cx);
                                    },
                                )),
                        );
                    }
                }
                column = column
                    .child(Self::section_heading(look, "The block"))
                    .child(
                        div()
                            .p(look.px(12.))
                            .rounded(look.px(4.))
                            .bg(look.c.hover)
                            .font_family("Menlo")
                            .text_size(look.text(12.))
                            .line_height(gpui::relative(1.45))
                            .child(visible(&out.text)),
                    );
            }
        }
        column.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_as_bytes_or_kilobytes() {
        assert_eq!(size(900), "900 B");
        assert_eq!(size(2048), "2.0 KB");
        assert_eq!(size(16_000), "15.6 KB");
    }
}
