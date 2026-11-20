//! A plain text tooltip, drawn on the picker surface.

use gpui::{AnyView, App, AppContext as _, Context, Window, div, prelude::*};

use super::theme::Look;

pub struct Tooltip {
    text: String,
    /// A shortcut shown beside the text (`⌘=`).
    keys: Option<String>,
}

impl Tooltip {
    /// A builder for `.tooltip(…)`: the text is shown after the pointer rests.
    pub fn text(text: impl Into<String>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        let text = text.into();
        move |_, cx| {
            let text = text.clone();
            cx.new(|_| Tooltip { text, keys: None }).into()
        }
    }

    /// As [`Tooltip::text`], with the control's shortcut beside the text.
    pub fn with_keys(
        text: impl Into<String>,
        keys: Option<&str>,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        let text = text.into();
        let keys = keys.map(str::to_owned);
        move |_, cx| {
            let (text, keys) = (text.clone(), keys.clone());
            cx.new(|_| Tooltip { text, keys }).into()
        }
    }
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::from_app(cx);
        // GPUI places a tooltip a point from the pointer; the clear band
        // above it keeps the box off the control it describes.
        div().pt(look.px(18.)).child(
            div()
                .max_w(look.px(320.))
                .px(look.px(8.))
                .py(look.px(4.))
                .rounded(look.px(4.))
                .bg(look.c.elevated)
                .border(look.fixed(1.))
                .border_color(look.c.border)
                .text_size(look.text(12.))
                .text_color(look.c.text)
                .flex()
                .items_center()
                .gap(look.px(8.))
                .child(self.text.clone())
                .children(self.keys.clone().map(|k| {
                    div()
                        .text_size(look.text(11.5))
                        .text_color(look.c.text_muted)
                        .child(k)
                })),
        )
    }
}
