//! The composer (Home's prompt and Brief's) and the tighter of an
//! account's windows.

use gpui::{Context, Focusable, Role, Window, div, prelude::*};
use hippo_core::usage::{Headroom, Left};

use super::Shell;
use super::kit;
use super::theme::Look;

/// The tighter of an account's two windows, an inferred one first on a tie.
pub fn tighter(h: &Headroom) -> Option<Left> {
    [h.five, h.seven].into_iter().flatten().min_by(|a, b| {
        a.frac
            .total_cmp(&b.frac)
            .then_with(|| hippo_view::fleet::inferred(b).cmp(&hippo_view::fleet::inferred(a)))
    })
}

impl Shell {
    /// The composer: the Ask field over a foot row of `Ask |
    /// Brief`, the Use next account chip with its ring, `⌘K` and the round
    /// send button.
    pub(super) fn render_composer(
        &self,
        on_home: bool,
        look: &Look,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let field = self.ask_field.clone();
        let focus = field.focus_handle(cx);
        let focused = focus.is_focused(window);
        let brief = self.brief.mode;
        let narrow = f32::from(window.viewport_size().width) < super::sidebar::NARROW;
        let empty = self.ask_field.read(cx).text().trim().is_empty();
        let fleet = self.loaded.as_ref().and_then(|l| l.fleet.as_ref());
        let account: Option<gpui::AnyElement> = (on_home && !narrow)
            .then(|| {
                let f = fleet?;
                let p = f.use_next.clone()?;
                let h = f.room(&p)?;
                let l = tighter(h)?;
                Some(
                    div()
                        .id("composer-account")
                        .flex()
                        .items_center()
                        .gap(look.px(8.))
                        .tooltip(super::tooltip::Tooltip::text(format!(
                            "Use next: {}, its tighter limit {} left",
                            kit::profile_name(&p),
                            super::home::pct_text(&l)
                        )))
                        .child(kit::cap(look, "Account", look.c.faint))
                        // The share left is in Home's readouts; a second
                        // figure here (the tighter of the two limits) read
                        // as a contradiction.
                        .child(kit::cap(look, &kit::profile_name(&p), look.c.text))
                        .into_any_element(),
                )
            })
            .flatten();
        let send_focus = focus.clone();
        // Brief's composer: the repository and
        // account chips beside Ask | Brief, and once the brief is built its
        // actions in place of the send button.
        let in_brief = brief && !on_home;
        let chips = if in_brief {
            self.brief_chips(look, cx)
        } else {
            Vec::new()
        };
        let actions = if in_brief {
            self.brief_actions(look, cx)
        } else {
            None
        };
        let has_actions = actions.is_some();
        div()
            .id(if on_home { "composer-home" } else { "composer" })
            .key_context("AskField")
            .flex()
            .flex_col()
            .gap(look.px(8.))
            .w_full()
            .pt(look.px(14.))
            .pb(look.px(10.))
            .pl(look.px(16.))
            .pr(look.px(12.))
            .rounded(look.px(3.))
            .border(look.fixed(1.))
            .border_color(if focused {
                look.c.text_muted
            } else {
                look.c.border_strong
            })
            .bg(look.c.surface)
            .cursor_text()
            .on_click(move |_, window, cx| window.focus(&focus, cx))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(look.px(12.))
                    .child(kit::icon(look, "chevron-right", 14., look.c.text_muted))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .min_h(look.px(22.))
                            .text_size(look.text(14.))
                            .overflow_hidden()
                            .child(field),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(look.px(8.))
                    .child(
                        kit::segmented(look)
                            .id("ask-mode")
                            .child(kit::segment(look, "seg-ask", None, "Ask", !brief).on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.set_brief_mode(false, window, cx)
                                }),
                            ))
                            .child(
                                kit::segment(look, "seg-brief", None, "Brief", brief).on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.set_brief_mode(true, window, cx)
                                    }),
                                ),
                            ),
                    )
                    .children(chips)
                    .when(!has_actions, |d| d.child(div().flex_1()))
                    .children(actions)
                    .children(account)
                    .when(on_home && !narrow, |d| {
                        d.child(
                            div()
                                .text_size(look.text(12.))
                                .text_color(look.c.faint)
                                .child("⌘K"),
                        )
                    })
                    .when(!has_actions, |d| {
                        d.child(
                            div()
                                .id("composer-send")
                                .role(Role::Button)
                                .aria_label(if brief { "Brief" } else { "Ask" })
                                .flex_shrink_0()
                                .size(look.px(30.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .bg(if empty { look.c.raised } else { look.c.primary })
                                .cursor_pointer()
                                .child(kit::icon(
                                    look,
                                    "arrow-up",
                                    16.,
                                    if empty {
                                        look.c.faint
                                    } else {
                                        look.c.primary_text
                                    },
                                ))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    window.focus(&send_focus, cx);
                                    if this.brief.mode {
                                        window.dispatch_action(
                                            Box::new(crate::actions::AskSubmit),
                                            cx,
                                        );
                                    } else {
                                        this.submit_ask(false, window, cx);
                                    }
                                })),
                        )
                    }),
            )
            .into_any_element()
    }
}
