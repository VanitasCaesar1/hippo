//! The title bar, drawn over the transparent system title bar, as Zed's
//! `platform_title_bar` is at the pinned commit: `hippocamp · <context>` in the
//! middle and hippocamp's health as one dot at the right.

use gpui::{Context, FontWeight, Role, Window, WindowControlArea, div, prelude::*};
use hippocamp_view::setup::Setup;

use super::Shell;
use super::kit::{self, Kind};
use super::theme::Look;
use crate::state::Tab;

/// `hippocamp · <workspace folder>`, or `hippocamp` when there is no workspace.
pub fn title(setup: Option<&Setup>) -> String {
    match setup {
        Some(Setup::Ready { ws, .. }) => match ws.root.file_name() {
            Some(name) => format!("hippocamp · {}", name.to_string_lossy()),
            None => "hippocamp".to_string(),
        },
        _ => "hippocamp".to_string(),
    }
}

/// `Mon 9 Nov · 20:24`, local.
pub fn now_label() -> String {
    let t = time::OffsetDateTime::now_utc();
    let local = t.to_offset(super::status::local_offset());
    format!(
        "{} · {}",
        crate::state::edition_title(local.date()),
        kit::hm(t)
    )
}

/// The title bar's height: 44 points, 36 under 900. The traffic
/// lights are placed in the sidebar head's vertical middle by
/// [`super::window_options`].
pub const HEIGHT: f32 = 44.0;
/// Room for the traffic lights at the row's left: macOS 26 and later draw
/// larger buttons that end about 76 points in (the sidebar head's
/// inset too, at every width and in full screen).
pub const TRAFFIC_LIGHTS: f32 = super::sidebar::LIGHTS;

impl Shell {
    /// The title bar: over the main column only, on the canvas
    /// with no rule. Left, the breadcrumb (`Fleet / <session>`); right, the
    /// date on Home and one toggle per pane the view has. Dragging it moves
    /// the window and a double-click zooms it.
    pub(super) fn render_title_bar(
        &self,
        look: &Look,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let ready = self
            .loaded
            .as_ref()
            .is_some_and(crate::state::Loaded::ready);
        let narrow = f32::from(window.viewport_size().width) < super::sidebar::NARROW;
        // Where the sidebar does not dock (below 900 points, or
        // folded away), the title bar holds the traffic lights' inset, then
        // the sidebar's toggle, then the breadcrumb, as T3 Code's does.
        let docked = self.sidebar_docked(window);
        let onboarding = self.onboarding_page().is_some();
        let active_tab = self.state.tabs.active_tab().cloned();
        let crumbs: Vec<String> = if onboarding {
            Vec::new()
        } else if !ready {
            vec![title(self.loaded.as_ref().map(|l| &l.setup))]
        } else {
            let section = self.state.tabs.section().name().to_owned();
            match &active_tab {
                None | Some(Tab::Home) | Some(Tab::Inbox) | Some(Tab::Fleet)
                | Some(Tab::Incidents) | Some(Tab::Memory) | Some(Tab::Manage) => {
                    vec![section]
                }
                // A sub-task under the sessions above it.
                Some(Tab::Task { session, ord }) => {
                    let mut c = vec![section];
                    match self.task_crumbs(session, *ord) {
                        Some((above, own)) => {
                            c.extend(above.into_iter().map(|(t, _)| kit::title(&t)));
                            c.push(kit::title(&own));
                        }
                        None => c.push(kit::title(
                            &self
                                .session_title(session)
                                .unwrap_or_else(|| "Sub-task".into()),
                        )),
                    }
                    c
                }
                // A delegated session under the thread that delegated it.
                Some(t @ Tab::Xray(id)) => match self.delegated_parent(id) {
                    Some(p) => vec![
                        section,
                        kit::title(
                            &self
                                .session_title(&p)
                                .unwrap_or_else(|| crate::rows::id_title(&p)),
                        ),
                        kit::title(&self.tab_title(t)),
                    ],
                    None => vec![section, kit::title(&self.tab_title(t))],
                },
                Some(t) => vec![section, kit::title(&self.tab_title(t))],
            }
        };
        // Where each crumb between the section and the last goes.
        let crumb_tabs: Vec<Tab> = match &active_tab {
            Some(Tab::Task { session, ord }) => self
                .task_crumbs(session, *ord)
                .map(|(above, _)| above.into_iter().map(|(_, t)| t).collect())
                .unwrap_or_default(),
            Some(Tab::Xray(id)) => self
                .delegated_parent(id)
                .map(Tab::Xray)
                .into_iter()
                .collect(),
            _ => Vec::new(),
        };
        let home = matches!(active_tab, None | Some(Tab::Home)) && ready && !onboarding;
        // The same inset at every width and in full screen.
        let left = if !ready || onboarding || !docked {
            TRAFFIC_LIGHTS
        } else {
            16.
        };
        let last = crumbs.len().saturating_sub(1);
        let mut crumb = div()
            .flex()
            .items_center()
            .gap(look.px(6.))
            .min_w_0()
            .text_size(look.text(13.5))
            .font_weight(FontWeight::MEDIUM);
        for (i, c) in crumbs.iter().enumerate() {
            if i > 0 {
                crumb = crumb.child(div().text_color(look.c.faint).child("/"));
            }
            // The crumbs are the way back (the back bar is gone):
            // a section's name returns to its own view.
            let up = i < last;
            crumb = crumb.child(
                div()
                    .id(("crumb", i))
                    .when(i == last, |d| d.min_w_0().truncate())
                    .flex_shrink_0()
                    .when(i == last, |d| d.flex_shrink(1.))
                    // Crumbs between the section and the last (the
                    // sessions above a sub-task) give way first.
                    .when(i > 0 && i < last, |d| {
                        d.min_w_0().max_w(look.px(220.)).flex_shrink(4.).truncate()
                    })
                    .text_color(if i == last {
                        look.c.text
                    } else {
                        look.c.text_muted
                    })
                    .when(up, |d| {
                        let to = i.checked_sub(1).and_then(|k| crumb_tabs.get(k).cloned());
                        d.cursor_pointer()
                            .hover(|h| h.text_color(look.c.text))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                match to.clone() {
                                    Some(tab) => {
                                        this.state.tabs.back_to(tab);
                                        this.after_state_change(window, cx);
                                    }
                                    None => {
                                        let section = this.state.tabs.section();
                                        this.show_section(section, window, cx);
                                    }
                                }
                            }))
                    })
                    .child(c.clone()),
            );
        }
        let toggle = |id: &'static str,
                      icon: &'static str,
                      label: &'static str,
                      cx: &mut Context<Self>| {
            let on = !self.panes_off.contains(id);
            kit::icon_btn(look, id, icon, label, on).on_click(cx.listener(move |this, _, _, cx| {
                if !this.panes_off.remove(id) {
                    this.panes_off.insert(id);
                }
                cx.notify();
            }))
        };
        div()
            .id("title-bar")
            .role(Role::TitleBar)
            .aria_label(crumbs.join(" / "))
            .flex()
            .flex_row()
            .items_center()
            .gap(look.px(8.))
            .flex_shrink_0()
            .w_full()
            .h(look.fixed(HEIGHT))
            .pl(look.fixed(left))
            .pr(look.px(12.))
            .window_control_area(WindowControlArea::Drag)
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, _| this.title_drag = true),
            )
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, _| this.title_drag = false),
            )
            .on_mouse_move(cx.listener(|this, _, window, _| {
                if this.title_drag {
                    this.title_drag = false;
                    window.start_window_move();
                }
            }))
            .on_click(|event, window, _| {
                if event.click_count() == 2 {
                    window.titlebar_double_click();
                }
            })
            .when(!docked && ready && !onboarding, |d| {
                d.child(self.sidebar_toggle(look, cx))
                    .child(div().w(look.px(4.)))
            })
            .child(crumb)
            .child(div().flex_1())
            .when(ready && !onboarding, |d| {
                d.children(self.title_actions(active_tab.as_ref(), narrow, look, window, cx))
            })
            .when(home && !narrow, |d| {
                let fits = f32::from(window.viewport_size().width) >= super::home_panes::PANES_FROM;
                d.child(
                    div()
                        .flex_shrink_0()
                        .text_size(look.text(12.5))
                        .text_color(look.c.text_muted)
                        .child(now_label()),
                )
                .child(
                    div()
                        .w(look.fixed(1.))
                        .h(look.px(18.))
                        .mx(look.px(4.))
                        .bg(look.c.border),
                )
                .child(toggle("pane-headroom", "gauge", "Headroom", cx))
                .when(fits, |d| {
                    d.child(toggle("pane-memory", "brain", "Memory", cx))
                })
            })
    }
}

impl Shell {
    /// The health detail over the status bar's pill, while open (the title
    /// bar has no pill).
    pub(super) fn render_health_popover(
        &self,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        if !self.health_open {
            return None;
        }
        let (word, _) = self.health_word();
        Some(
            div()
                .id("health-popover")
                .absolute()
                .bottom(look.px(30.))
                .left(look.px(8.))
                .w(look.px(420.))
                .max_w(gpui::relative(0.9))
                .p(look.px(16.))
                .rounded(look.px(10.))
                .border(look.fixed(1.))
                .border_color(look.c.border)
                .bg(look.c.elevated)
                .shadow_lg()
                .occlude()
                .text_size(look.text(13.))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.health_open = false;
                    cx.notify();
                }))
                .child(
                    div()
                        .pb(look.px(10.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(format!("Health · {word}")),
                )
                .child(self.render_health(look)),
        )
    }
}

impl Shell {
    /// The title bar's actions for the view shown: Fleet's
    /// Copy resume and Open x-ray for the selected row; the x-ray's Normal
    /// | Verbose, Open thread, Copy resume and its pane toggles.
    fn title_actions(
        &self,
        tab: Option<&Tab>,
        narrow: bool,
        look: &Look,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let bar = || div().flex().flex_shrink_0().items_center().gap(look.px(6.));
        match tab? {
            Tab::Incidents => Some(self.incidents_filter_bar(
                f32::from(window.viewport_size().width) < super::home::COMPACT,
                look,
                cx,
            )),
            Tab::Incident(id) => self.incident_page_actions(id, look, cx),
            Tab::Fleet => {
                let sel = self.fleet_selected_row();
                let has = sel.is_some();
                let copied = sel
                    .as_ref()
                    .is_some_and(|r| self.copied.as_deref() == Some(&format!("resume:{}", r.id)));
                let row = sel.clone();
                Some(
                    bar()
                        .child(
                            kit::button_icon(
                                look,
                                "fleet-copy-resume",
                                Some("square-terminal"),
                                if copied { "Copied" } else { "Copy resume" },
                                "R",
                                if has { Kind::Ghost } else { Kind::Disabled },
                            )
                            .when(has, |d| {
                                d.on_click(cx.listener(|this, _, _, cx| {
                                    this.copy_selected_resume(cx);
                                }))
                            }),
                        )
                        .child(
                            kit::button_key(
                                look,
                                "fleet-open-xray",
                                "Open x-ray",
                                "↵",
                                if has { Kind::Plain } else { Kind::Disabled },
                            )
                            .when_some(row, |d, r| {
                                d.on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_xray(&r.id, window, cx);
                                }))
                            }),
                        )
                        .into_any_element(),
                )
            }
            Tab::Task { .. } => {
                let verbose = self.xray.verbose;
                Some(
                    bar()
                        .child(
                            kit::segmented(look)
                                .child(
                                    kit::segment(look, "task-normal", None, "Normal", !verbose)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.xray.verbose = false;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    kit::segment(look, "task-verbose", None, "Verbose", verbose)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.xray.verbose = true;
                                            cx.notify();
                                        })),
                                ),
                        )
                        .into_any_element(),
                )
            }
            Tab::Xray(id) => {
                let verbose = self.xray.verbose;
                let modes = kit::segmented(look)
                    .child(
                        kit::segment(look, "xray-normal", None, "Normal", !verbose).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.xray.verbose = false;
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        kit::segment(look, "xray-verbose", None, "Verbose", verbose).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.xray.verbose = true;
                                cx.notify();
                            }),
                        ),
                    );
                let row = self
                    .loaded
                    .as_ref()
                    .and_then(|l| l.fleet.as_ref())
                    .and_then(|f| f.row(id).cloned());
                let thread_id = id.clone();
                let resume = self
                    .xray
                    .data
                    .get(id)
                    .and_then(|d| d.as_ref().ok())
                    .and_then(|d| d.resume.clone());
                let copied = self.xray.copied_resume;
                let key = id.clone();
                let wide = f32::from(window.viewport_size().width) >= super::xray::PANES_FROM;
                let toggle = |pane: &'static str,
                              icon: &'static str,
                              label: &'static str,
                              cx: &mut Context<Self>| {
                    let on = !self.panes_off.contains(pane);
                    kit::icon_btn(look, pane, icon, label, on).on_click(cx.listener(
                        move |this, _, _, cx| {
                            if !this.panes_off.remove(pane) {
                                this.panes_off.insert(pane);
                            }
                            cx.notify();
                        },
                    ))
                };
                Some(
                    bar()
                        .when(!narrow, |d| d.child(modes).child(div().w(look.px(4.))))
                        .child(
                            kit::button_icon(
                                look,
                                "xray-thread",
                                Some("arrow-up-right"),
                                "Open thread",
                                "",
                                Kind::Ghost,
                            )
                                .tooltip(super::tooltip::Tooltip::text(
                                    "Opens the thread in T3 Code or the agent's app; else its transcript.",
                                ))
                                .on_click(cx.listener(move |this, _, window, cx| match row.clone() {
                                    Some(r) => this.open_session(r, window, cx),
                                    None => {
                                        this.state.tabs.open(Tab::Session {
                                            id: thread_id.clone(),
                                            ordinal: None,
                                        });
                                        this.after_state_change(window, cx);
                                    }
                                })),
                        )
                        .children(resume.map(|(command, _)| {
                            kit::button_icon(
                                look,
                                "xray-resume",
                                Some("square-terminal"),
                                if copied { "Copied" } else { "Copy resume" },
                                "⌘C",
                                Kind::Plain,
                            )
                            .tooltip(super::tooltip::Tooltip::text(format!(
                                "Copies the resume command and a short handoff:\n{command}"
                            )))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.copy_xray_resume(&key, cx);
                            }))
                        }))
                        .when(wide, |d| {
                            d.child(
                                div()
                                    .w(look.fixed(1.))
                                    .h(look.px(18.))
                                    .mx(look.px(4.))
                                    .bg(look.c.border),
                            )
                            .child(toggle("pane-session", "info", "Session", cx))
                            .child(toggle("pane-tasks", "list-checks", "Tasks", cx))
                            .child(toggle("pane-changes", "git-branch", "Changes", cx))
                            .child(toggle("pane-searches", "text-search", "Searches", cx))
                        })
                        .into_any_element(),
                )
            }
            Tab::Brief if self.brief.brief().is_some() => Some(
                kit::button_icon(
                    look,
                    "brief-copy",
                    Some("copy"),
                    "Copy block",
                    "",
                    Kind::Ghost,
                )
                .on_click(cx.listener(|this, _, _, cx| this.brief_copy(cx)))
                .into_any_element(),
            ),
            Tab::Memory => Some(self.memory_title_controls(
                f32::from(window.viewport_size().width) < 1100.,
                look,
                cx,
            )),
            Tab::Manage => {
                let ws = self
                    .workspace_root()
                    .and_then(|r| r.file_name().map(|n| n.to_string_lossy().into_owned()));
                let home = super::manage_page::middle_ellipsis(
                    &super::manage_page::tilde(&self.installation.home),
                    32,
                );
                Some(
                    div()
                        .flex_shrink_0()
                        .text_size(look.text(12.))
                        .text_color(look.c.faint)
                        .child(match ws {
                            Some(w) => format!("workspace {w} · {home}"),
                            None => home,
                        })
                        .into_any_element(),
                )
            }
            Tab::Inbox => {
                use super::lessons::Filter;
                let seg = Filter::ALL.iter().fold(kit::segmented(look), |d, &f| {
                    let on = f == self.inbox.filter;
                    d.child(
                        kit::segment(
                            look,
                            ("inbox-filter", f as usize),
                            None,
                            format!("{} {}", f.name(), self.count(f)),
                            on,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.inbox.filter = f;
                            this.inbox.selected = None;
                            this.inbox.editing = None;
                            cx.notify();
                        })),
                    )
                });
                Some(bar().child(seg).into_any_element())
            }
            _ => None,
        }
    }
}

impl Shell {
    /// The title bar's sidebar toggle where the sidebar does not dock: the
    /// `panel-left` icon with a small badge, the Needs you count in warn
    /// (else the working count in info), so a hidden sidebar still says
    /// when something waits.
    fn sidebar_toggle(&self, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        // The shelves' counts: the badge and the sidebar agree.
        let (needs, working) = self.tally().map_or((0, 0), |t| (t.needs_you, t.working));
        let badge = match (needs, working) {
            (0, 0) => None,
            (0, w) => Some((w, look.c.info)),
            (n, _) => Some((n, look.c.warn)),
        };
        div()
            .relative()
            .flex_shrink_0()
            .child(self.fold_button(look, false, cx))
            .children(badge.map(|(n, color)| {
                div()
                    .absolute()
                    .top(look.px(-3.))
                    .right(look.px(-5.))
                    .min_w(look.px(14.))
                    .h(look.px(14.))
                    .px(look.px(3.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(color)
                    .text_size(look.text(9.5))
                    .font_weight(FontWeight::BOLD)
                    .text_color(look.c.canvas)
                    .child(if n > 9 {
                        "9+".to_owned()
                    } else {
                        n.to_string()
                    })
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_names_the_workspace_folder_when_there_is_one() {
        assert_eq!(title(None), "hippocamp");
        let missing = Setup::AccessDenied { path: "/x".into() };
        assert_eq!(title(Some(&missing)), "hippocamp");
    }
}
