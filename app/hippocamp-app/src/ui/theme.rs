//! The theme in use as a GPUI global, and the lengths drawn from it.
//!
//! `hippocamp_core::theme::Theme` holds colours as `0xRRGGBBAA`. This
//! module turns them into GPUI colours, follows the system appearance, and
//! holds `px_at`, which turns a length in points at the 13-point base
//! into pixels.

#![allow(dead_code)] // the finder and theme picker (items 8 and 14) use the rest

use gpui::{App, Global, Hsla, Pixels, WindowAppearance, px, rgba};
use hippocamp_core::theme::{self, Appearance, Rgba, Theme};

/// The theme in use.
#[derive(Clone)]
pub struct ActiveTheme {
    pub theme: Theme,
    /// The theme follows the system's light or dark appearance.
    pub system: bool,
}

impl Global for ActiveTheme {}

/// The session's text scale, 0.8 to 2.0 (`state::TextScale`), as a global so
/// that every view draws with it.
pub struct UiScale(pub f32);

impl Global for UiScale {}

impl ActiveTheme {
    pub fn system(appearance: WindowAppearance) -> ActiveTheme {
        ActiveTheme {
            theme: system_theme(appearance),
            system: true,
        }
    }

    /// The name the status bar shows.
    pub fn label(&self) -> String {
        if self.system {
            format!("System ({})", self.theme.name)
        } else {
            self.theme.name.clone()
        }
    }
}

/// Hippocamp Light for a light window, Hippocamp Dark for a dark one, with the focus
/// floor applied.
pub fn system_theme(appearance: WindowAppearance) -> Theme {
    let which = match appearance {
        WindowAppearance::Light | WindowAppearance::VibrantLight => Appearance::Light,
        WindowAppearance::Dark | WindowAppearance::VibrantDark => Appearance::Dark,
    };
    let [light, dark] = theme::builtin();
    let mut theme = match which {
        Appearance::Light => light,
        Appearance::Dark => dark,
    };
    theme::apply_focus_floor(&mut theme);
    theme
}

/// The base text size, in points. Every length of the shell is given
/// at this size.
pub const BASE: f32 = 13.0;
/// Text is never drawn smaller than this, in points.
pub const MIN_TEXT: f32 = 10.0;
/// The focus ring and the selection marker, in points; they do not scale.
pub const RING: f32 = 2.0;

/// `points × font_size × scale ÷ 13`, to the nearest half point.
pub fn scaled(points: f32, font_size: f32, scale: f32) -> f32 {
    let v = points * font_size * scale / BASE;
    (v * 2.0).round() / 2.0
}

/// A text size: [`scaled`], never under 10 points.
pub fn scaled_text(points: f32, font_size: f32, scale: f32) -> f32 {
    scaled(points, font_size, scale).max(MIN_TEXT)
}

pub fn color(c: Rgba) -> Hsla {
    rgba(c.0).into()
}

/// The theme's colours as GPUI colours.
#[derive(Clone, Copy)]
pub struct Palette {
    pub window: Hsla,
    pub dock: Hsla,
    pub centre: Hsla,
    pub title_bar: Hsla,
    pub status_bar: Hsla,
    pub tab_bar: Hsla,
    pub tab_active: Hsla,
    pub tab_inactive: Hsla,
    pub elevated: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub text_placeholder: Hsla,
    pub text_accent: Hsla,
    pub border: Hsla,
    pub border_variant: Hsla,
    pub focus: Hsla,
    pub selected_outline: Hsla,
    pub hover: Hsla,
    pub selected: Hsla,
    pub error: Hsla,
    pub warning: Hsla,
    /// Added and removed lines of a diff, and their backgrounds.
    pub created: Hsla,
    pub created_background: Hsla,
    pub deleted: Hsla,
    pub deleted_background: Hsla,
    pub search_match: Hsla,
    pub scrollbar_thumb: Hsla,
    // The look's tokens, drawn from the theme's keys.
    /// The sidebar (`panel.background`).
    pub sidebar: Hsla,
    /// The window and main pane (`background`).
    pub canvas: Hsla,
    /// Cards, the composer, inputs, dialogs (`surface.background`).
    pub surface: Hsla,
    /// A selected row, a chip, a segmented thumb (`element.selected`).
    pub raised: Hsla,
    /// A command well (the sidebar's colour).
    pub inset: Hsla,
    /// Inputs, outline buttons, key caps (`border.variant`).
    pub border_strong: Hsla,
    /// Ages, counts, placeholders (`text.placeholder`).
    pub faint: Hsla,
    /// Focus ring, switch on, selection (`border.focused`).
    pub accent: Hsla,
    /// Text on an accent fill.
    pub accent_on: Hsla,
    /// The count badge's text and fill.
    pub accent_text: Hsla,
    pub accent_surface: Hsla,
    /// The one primary button: inverted.
    pub primary: Hsla,
    pub primary_text: Hsla,
    /// Done, working, waiting on you, failed.
    pub ok: Hsla,
    pub info: Hsla,
    pub warn: Hsla,
    pub err: Hsla,
    /// A tool line's `+28` and `−9`.
    pub diff_add: Hsla,
    pub diff_del: Hsla,
    pub dark: bool,
}

impl Palette {
    pub fn of(theme: &Theme) -> Palette {
        let c = &theme.colors;
        let dark = theme.appearance == Appearance::Dark;
        Palette {
            window: color(c.window),
            dock: color(c.dock),
            centre: color(c.centre),
            title_bar: color(c.title_bar),
            status_bar: color(c.status_bar),
            tab_bar: color(c.tab_bar),
            tab_active: color(c.tab_active),
            tab_inactive: color(c.tab_inactive),
            elevated: color(c.elevated),
            text: color(c.text),
            text_muted: color(c.text_muted),
            text_placeholder: color(c.text_placeholder),
            text_accent: color(c.text_accent),
            border: color(c.border),
            border_variant: color(c.border_variant),
            focus: color(c.focus),
            selected_outline: color(c.selected_outline),
            hover: color(c.hover),
            selected: color(c.selected),
            error: color(c.error),
            warning: color(c.warning),
            created: color(c.created),
            created_background: color(c.created_background),
            deleted: color(c.deleted),
            deleted_background: color(c.deleted_background),
            search_match: color(c.search_match),
            scrollbar_thumb: color(c.scrollbar_thumb),
            sidebar: color(c.dock),
            canvas: color(c.window),
            surface: color(c.elevated),
            raised: color(c.selected),
            inset: color(c.dock),
            border_strong: color(c.border_variant),
            faint: color(c.text_placeholder),
            accent: color(c.focus),
            accent_on: if dark { color(c.window) } else { gpui::white() },
            accent_text: color(c.selected_outline),
            accent_surface: color(c.focus).opacity(if dark { 0.12 } else { 0.09 }),
            primary: color(c.text),
            primary_text: color(c.window),
            ok: color(c.success),
            info: color(c.selected_outline),
            warn: color(c.warning),
            err: color(c.error),
            diff_add: color(c.created),
            diff_del: color(c.deleted),
            dark,
        }
    }
}

/// What every view draws with: colours, the font and the size scale.
#[derive(Clone)]
pub struct Look {
    pub c: Palette,
    pub font_family: Option<String>,
    pub font_size: f32,
    pub scale: f32,
}

impl Look {
    pub fn new(active: &ActiveTheme, scale: f32) -> Look {
        Look {
            c: Palette::of(&active.theme),
            font_family: active.theme.font_family.clone(),
            font_size: active.theme.font_size,
            scale,
        }
    }

    pub fn from_app(cx: &App) -> Look {
        Look::new(cx.global::<ActiveTheme>(), cx.global::<UiScale>().0)
    }

    /// A length at the 13-point base, in pixels.
    pub fn px(&self, points: f32) -> Pixels {
        px(scaled(points, self.font_size, self.scale))
    }

    /// A text size at the 13-point base, in pixels, never under 10 points.
    pub fn text(&self, points: f32) -> Pixels {
        px(scaled_text(points, self.font_size, self.scale))
    }

    /// A length that does not scale (the focus ring).
    pub fn fixed(&self, points: f32) -> Pixels {
        px(points)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_round_to_half_a_point_and_scale_with_size_and_text_scale() {
        assert_eq!(scaled(13.0, 13.0, 1.0), 13.0);
        assert_eq!(scaled(24.0, 13.0, 1.0), 24.0);
        assert_eq!(scaled(24.0, 13.0, 1.5), 36.0);
        // 24 x 14 / 13 = 25.846, to the nearest half point.
        assert_eq!(scaled(24.0, 14.0, 1.0), 26.0);
        // 10 x 13 x 0.8 / 13 = 8.
        assert_eq!(scaled(10.0, 13.0, 0.8), 8.0);
        // 7 x 15 / 13 = 8.077 rounds to 8.0; 3 x 15 / 13 = 3.46 rounds to 3.5.
        assert_eq!(scaled(7.0, 15.0, 1.0), 8.0);
        assert_eq!(scaled(3.0, 15.0, 1.0), 3.5);
    }

    #[test]
    fn text_is_never_under_ten_points() {
        assert_eq!(scaled_text(13.0, 13.0, 0.8), 10.5);
        assert_eq!(scaled_text(11.0, 13.0, 0.8), 10.0);
        assert_eq!(scaled_text(13.0, 13.0, 2.0), 26.0);
    }

    #[test]
    fn the_system_theme_follows_the_appearance() {
        let name = |a| system_theme(a).name;
        assert_eq!(name(WindowAppearance::Light), "Hippocamp Light");
        assert_eq!(name(WindowAppearance::VibrantLight), "Hippocamp Light");
        assert_eq!(name(WindowAppearance::Dark), "Hippocamp Dark");
        assert_eq!(name(WindowAppearance::VibrantDark), "Hippocamp Dark");
    }

    #[test]
    fn the_built_in_themes_keep_their_focus_ring() {
        for appearance in [WindowAppearance::Light, WindowAppearance::Dark] {
            let t = system_theme(appearance);
            assert!(t.warnings.is_empty(), "{:?}", t.warnings);
        }
    }

    #[test]
    fn colours_keep_their_channels() {
        let c = color(Rgba(0xFF0000FF));
        let r: gpui::Rgba = c.into();
        assert!((r.r - 1.0).abs() < 1e-3 && r.g < 1e-3 && r.b < 1e-3 && (r.a - 1.0).abs() < 1e-3);
    }

    #[test]
    fn the_label_says_system_and_the_theme() {
        let a = ActiveTheme::system(WindowAppearance::Dark);
        assert_eq!(a.label(), "System (Hippocamp Dark)");
    }
}
