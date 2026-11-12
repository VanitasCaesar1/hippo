//! Small parts the views share: buttons, chips, cards, meters, profile
//! avatars and key caps, plus the status word, the tool line, the
//! task row, the pane card, the segmented control and the usage ring.

use gpui::{
    FontWeight, Hsla, PathBuilder, Role, canvas, div, point, prelude::*, px, relative, svg,
};

use super::theme::Look;

/// The secondary text, a little fainter: facts under a title.
pub fn dim(look: &Look) -> Hsla {
    look.c.text_muted.opacity(0.8)
}

/// The colour that means working: the theme's added-lines colour.
pub fn ok(look: &Look) -> Hsla {
    look.c.created
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Soft: banners and toasts
pub enum Kind {
    /// The one action that matters in a view: the inverted colour.
    Primary,
    /// Outlined: the second choice.
    Plain,
    /// No border, muted: the third choice and navigation.
    Ghost,
    /// A raised fill: an action inside a banner or toast.
    Soft,
    /// Dimmed, and its click does nothing: a later step.
    Disabled,
}

/// A button: 28 points high, radius 8, its label on one line.
pub fn button(
    look: &Look,
    id: impl Into<gpui::ElementId>,
    label: impl Into<String>,
    kind: Kind,
) -> gpui::Stateful<gpui::Div> {
    button_key(look, id, label, "", kind)
}

/// A button with its own key written inside after the label (`Keep ↵`,
/// `Copy resume ⌘C`), in micro type: faint, or the primary text at 55 %.
pub fn button_key(
    look: &Look,
    id: impl Into<gpui::ElementId>,
    label: impl Into<String>,
    key: &str,
    kind: Kind,
) -> gpui::Stateful<gpui::Div> {
    button_icon(look, id, None, label, key, kind)
}

/// [`button_key`] led by a 14-point Lucide icon in the label's colour.
pub fn button_icon(
    look: &Look,
    id: impl Into<gpui::ElementId>,
    lead: Option<&str>,
    label: impl Into<String>,
    key: &str,
    kind: Kind,
) -> gpui::Stateful<gpui::Div> {
    let label = label.into();
    let key_color = match kind {
        Kind::Primary => look.c.primary_text.opacity(0.55),
        _ => look.c.faint,
    };
    let lead_color = match kind {
        Kind::Primary => look.c.primary_text,
        Kind::Ghost => look.c.text_muted,
        Kind::Disabled => look.c.faint,
        _ => look.c.text,
    };
    let d = div()
        .id(id.into())
        .role(Role::Button)
        .aria_label(label.clone())
        .flex_shrink_0()
        .h(look.px(28.))
        .px(look.px(10.))
        .flex()
        .items_center()
        .justify_center()
        .gap(look.px(6.))
        .rounded(look.px(8.))
        .border(look.fixed(1.))
        .border_color(gpui::transparent_black())
        .text_size(look.text(12.5))
        .whitespace_nowrap()
        .children(lead.map(|n| icon(look, n, 14., lead_color)))
        .child(label)
        .when(!key.is_empty(), |d| {
            d.child(
                div()
                    .text_size(look.text(11.))
                    .text_color(key_color)
                    .child(key.to_owned()),
            )
        });
    match kind {
        Kind::Primary => d
            .bg(look.c.primary)
            .border_color(look.c.primary)
            .text_color(look.c.primary_text)
            .font_weight(FontWeight::MEDIUM)
            .cursor_pointer()
            .hover(|h| h.opacity(0.9))
            .active(|a| a.opacity(0.8)),
        Kind::Plain => d
            .border_color(look.c.border_strong)
            .text_color(look.c.text)
            .cursor_pointer()
            .hover(|h| h.bg(look.c.hover))
            .active(|a| a.bg(look.c.raised)),
        Kind::Ghost => d
            .text_color(look.c.text_muted)
            .cursor_pointer()
            .hover(|h| h.bg(look.c.raised).text_color(look.c.text))
            .active(|a| a.bg(look.c.text.opacity(0.14))),
        Kind::Soft => d
            .bg(look.c.raised)
            .text_color(look.c.text)
            .cursor_pointer()
            .hover(|h| h.opacity(0.9))
            .active(|a| a.opacity(0.8)),
        Kind::Disabled => d.border_color(look.c.border).text_color(look.c.faint),
    }
}

/// A 24-point square icon button: a Lucide glyph in the faint colour that
/// turns to the text colour on hover; `on` gives it the raised fill.
pub fn icon_btn(
    look: &Look,
    id: impl Into<gpui::ElementId>,
    icon_name: &str,
    label: &'static str,
    on: bool,
) -> gpui::Stateful<gpui::Div> {
    let color = if on { look.c.text } else { look.c.faint };
    div()
        .id(id.into())
        .role(Role::Button)
        .aria_label(label)
        .flex_shrink_0()
        .size(look.px(24.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(look.px(6.))
        .when(on, |d| d.bg(look.c.raised))
        .cursor_pointer()
        .hover(|h| h.bg(look.c.raised))
        .active(|a| a.bg(look.c.text.opacity(0.14)))
        .child(icon(look, icon_name, 14., color))
        .tooltip(super::tooltip::Tooltip::text(label))
}

/// A small borderless icon button (for rows: ✓ ✎ ✕), its words in a
/// tooltip set by the caller.
pub fn icon_button(
    look: &Look,
    id: impl Into<gpui::ElementId>,
    glyph: &'static str,
    label: &'static str,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id.into())
        .role(Role::Button)
        .aria_label(label)
        .flex_shrink_0()
        .size(look.px(24.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(look.px(6.))
        .text_size(look.text(12.5))
        .text_color(look.c.faint)
        .cursor_pointer()
        .hover(|h| h.bg(look.c.raised).text_color(look.c.text))
        .active(|a| a.bg(look.c.text.opacity(0.14)))
        .child(match glyph {
            "✓" => icon(look, "check", 14., look.c.faint).into_any_element(),
            "✎" => icon(look, "pencil", 14., look.c.faint).into_any_element(),
            "✕" => icon(look, "x", 14., look.c.faint).into_any_element(),
            other => other.into_any_element(),
        })
}

/// A rounded chip: a suggestion, a filter, a count.
pub fn chip(
    look: &Look,
    id: impl Into<gpui::ElementId>,
    label: impl Into<String>,
    on: bool,
) -> gpui::Stateful<gpui::Div> {
    let label = label.into();
    div()
        .id(id.into())
        .role(Role::Button)
        .aria_label(label.clone())
        .aria_selected(on)
        .flex_shrink_0()
        .h(look.px(22.))
        .px(look.px(8.))
        .flex()
        .items_center()
        .gap(look.px(5.))
        .rounded_full()
        .border(look.fixed(1.))
        .border_color(if on {
            look.c.border_strong
        } else {
            look.c.border
        })
        .bg(if on {
            look.c.raised
        } else {
            gpui::transparent_black()
        })
        .text_size(look.text(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if on { look.c.text } else { look.c.text_muted })
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|h| h.bg(look.c.hover))
        .active(|a| a.bg(look.c.raised))
        .child(label)
}

/// A count in a tinted pill: `3 live`.
pub fn pill(look: &Look, label: impl Into<String>, color: Hsla) -> gpui::Div {
    div()
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(look.px(5.))
        .h(look.px(20.))
        .px(look.px(8.))
        .rounded_full()
        .bg(color.opacity(if look.c.dark { 0.12 } else { 0.09 }))
        .text_color(color)
        .text_size(look.text(12.))
        .font_weight(FontWeight::MEDIUM)
        .whitespace_nowrap()
        .child(label.into())
}

/// A pill led by a dot: `● 3 live`.
pub fn dot_pill(look: &Look, label: impl Into<String>, color: Hsla) -> gpui::Div {
    div()
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(look.px(6.))
        .h(look.px(22.))
        .px(look.px(9.))
        .rounded_full()
        .bg(color.opacity(if look.c.dark { 0.12 } else { 0.09 }))
        .text_color(color)
        .text_size(look.text(12.5))
        .whitespace_nowrap()
        .child(dot(look, color))
        .child(label.into())
}

/// A dot of `color`.
pub fn dot(look: &Look, color: Hsla) -> gpui::Div {
    div()
        .flex_shrink_0()
        .size(look.px(6.))
        .rounded_full()
        .bg(color)
}

/// A card: the surface with a hairline, radius 12.
pub fn card(look: &Look) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .rounded(look.px(12.))
        .border(look.fixed(1.))
        .border_color(look.c.border)
        .bg(look.c.surface)
}

/// A label (sentence case, never small capitals): 12 points,
/// weight 500.
pub fn label(look: &Look, text: &str, color: Hsla) -> gpui::Div {
    div()
        .flex_shrink_0()
        .text_size(look.text(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(color)
        .whitespace_nowrap()
        .child(sentence(text))
}

/// `LIVE NOW` → `Live now`: labels written in capitals read in sentence
/// case.
pub fn sentence(text: &str) -> String {
    if text.chars().any(char::is_lowercase) {
        return text.to_owned();
    }
    let lower = text.to_lowercase();
    let mut c = lower.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// A label with a rule after it: a group inside a card.
pub fn group(look: &Look, text: &str, color: Hsla) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(look.px(10.))
        .pt(look.px(10.))
        .pb(look.px(4.))
        .child(label(look, text, color))
        .child(div().flex_1().h(look.fixed(1.)).bg(look.c.border))
}

/// A horizontal meter, `frac` of it filled in `color`.
pub fn meter(look: &Look, frac: f32, color: Hsla) -> gpui::Div {
    let frac = frac.clamp(0.0, 1.0);
    div()
        .w_full()
        .h(look.px(4.))
        .rounded(look.px(2.))
        .bg(look.c.raised)
        .child(
            div()
                .h_full()
                .w(relative(frac))
                .rounded(look.px(2.))
                .bg(color),
        )
}

/// An `ⓘ` beside a heading: the caveat that qualifies it, in a tooltip
/// (caveats leave the status bar).
pub fn info(
    look: &Look,
    id: impl Into<gpui::ElementId>,
    text: impl Into<String>,
) -> gpui::Stateful<gpui::Div> {
    let text = text.into();
    div()
        .id(id.into())
        .role(Role::Button)
        .aria_label(text.clone())
        .flex_shrink_0()
        .size(look.px(16.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(look.px(8.))
        .border(look.fixed(1.))
        .border_color(look.c.border)
        .text_size(look.text(10.))
        .text_color(look.c.text_muted)
        .child("i")
        .tooltip(super::tooltip::Tooltip::text(text))
}

/// A key cap: `⌘K`.
pub fn kbd(look: &Look, keys: &str) -> gpui::Div {
    div()
        .flex_shrink_0()
        .px(look.px(5.))
        .h(look.px(18.))
        .flex()
        .items_center()
        .rounded(look.px(4.))
        .border(look.fixed(1.))
        .border_color(look.c.border_strong)
        .bg(look.c.surface)
        .text_size(look.text(11.))
        .text_color(look.c.text_muted)
        .whitespace_nowrap()
        .child(keys.to_owned())
}

/// A profile's display name: its folder's suffix (`work`),
/// `default` for `~/.claude`, `codex`. Never `claude-`.
pub fn profile_name(profile: &str) -> String {
    hippocamp_view::fleet::profile_label(profile).to_owned()
}

/// The profiles the app shows, with their marks and hues, set from
/// the fleet at each draw ([`sync_profiles`]).
static IDENTITIES: std::sync::LazyLock<
    std::sync::RwLock<std::collections::BTreeMap<String, (String, usize)>>,
> = std::sync::LazyLock::new(Default::default);

/// Records the profiles shown, so their marks are unique among them.
#[cfg(test)]
pub fn sync_profiles<'a>(profiles: impl IntoIterator<Item = &'a str>) {
    sync_profiles_with(profiles, std::iter::empty());
}

/// [`sync_profiles`] for the fleet's profiles, plus `extra` names (Manage's
/// folders) that get marks unique against them (`Sh` and `Su`,
/// never `Sh` twice) without moving any fleet profile's mark or hue.
pub fn sync_profiles_with<'a>(
    profiles: impl IntoIterator<Item = &'a str>,
    extra: impl IntoIterator<Item = &'a str>,
) {
    let mut names: Vec<&str> = profiles.into_iter().collect();
    names.sort_unstable();
    names.dedup();
    let mut more: Vec<&str> = extra.into_iter().filter(|e| !names.contains(e)).collect();
    more.sort_unstable();
    more.dedup();
    let total = names.len() + more.len();
    let same = IDENTITIES
        .read()
        .map(|m| m.len() == total && names.iter().chain(more.iter()).all(|n| m.contains_key(*n)))
        .unwrap_or(false);
    if !same && let Ok(mut m) = IDENTITIES.write() {
        let mut ids = hippocamp_view::fleet::identities(&names);
        for e in more {
            let used: Vec<String> = ids.values().map(|v| v.0.clone()).collect();
            let chars: Vec<char> = e.chars().filter(|c| c.is_alphanumeric()).collect();
            let first = chars.first().map_or('?', |c| c.to_ascii_uppercase());
            let mark = chars
                .iter()
                .skip(1)
                .map(|c| format!("{first}{}", c.to_ascii_lowercase()))
                .find(|m| !used.contains(m))
                .unwrap_or_else(|| format!("{first}{}", used.len()));
            let hue = hippocamp_view::fleet::identities(&[e])
                .get(e)
                .map_or(0, |v| v.1);
            ids.insert(e.to_owned(), (mark, hue));
        }
        *m = ids;
    }
}

fn identity(profile: &str) -> (String, usize) {
    if let Some(id) = IDENTITIES.read().ok().and_then(|m| m.get(profile).cloned()) {
        return id;
    }
    hippocamp_view::fleet::identities(&[profile])
        .remove(profile)
        .unwrap_or_else(|| ("?".to_owned(), 0))
}

/// A profile's two-letter mark (`An`, `Wo`, `De`), unique among the
/// profiles shown.
pub fn profile_letter(profile: Option<&str>) -> String {
    profile.map_or_else(|| "?".to_owned(), |p| identity(p).0)
}

/// The six profile hues: blue, violet, magenta, teal, orange, lime. None
/// is the accent amber, the error red or the working green.
const HUES: [f32; 6] = [212., 268., 322., 178., 24., 88.];

/// A profile's fixed hue, on its avatar, card dot and Fleet chip.
pub fn profile_hue(profile: &str) -> Hsla {
    let (_, i) = identity(profile);
    gpui::hsla(HUES[i % HUES.len()] / 360., 0.62, 0.64, 1.)
}

/// A profile's avatar: its mark in a rounded square tinted with its hue,
/// ringed in `ring` when given (live is the working colour).
pub fn avatar(look: &Look, profile: Option<&str>, ring: Option<Hsla>) -> gpui::Div {
    let hue = profile.map(profile_hue).unwrap_or(look.c.text_muted);
    mark_sized(look, profile, 24., ring).border_color(ring.unwrap_or(hue.opacity(0.3)))
}

/// An account mark: the profile's two letters in its hue on a
/// rounded square, fill 18 %, ring 30 %: 18 points in rows, 15 in meta
/// lines, 24 in tables.
pub fn mark(look: &Look, profile: Option<&str>, size: f32) -> gpui::Div {
    mark_sized(look, profile, size, None)
}

fn mark_sized(look: &Look, profile: Option<&str>, size: f32, ring: Option<Hsla>) -> gpui::Div {
    let hue = profile.map(profile_hue).unwrap_or(look.c.text_muted);
    let radius = if size >= 22. {
        6.
    } else if size >= 17. {
        5.
    } else {
        4.
    };
    div()
        .flex_shrink_0()
        .size(look.px(size))
        .flex()
        .items_center()
        .justify_center()
        .rounded(look.px(radius))
        .border(look.fixed(if ring.is_some() { 1.5 } else { 1. }))
        .border_color(ring.unwrap_or(hue.opacity(0.3)))
        .bg(hue.opacity(0.18))
        .text_size(look.text((size * 0.5).clamp(9.5, 11.5)))
        .font_weight(FontWeight::BOLD)
        .text_color(hue)
        .child(profile_letter(profile))
}

/// A project's mark: its first two letters, muted, on the raised fill.
pub fn project_mark(look: &Look, project: &str, size: f32) -> gpui::Div {
    let letters: String = {
        let mut c = project.chars().filter(|c| c.is_alphanumeric());
        let a = c.next().map(|c| c.to_ascii_lowercase());
        let b = c.next().map(|c| c.to_ascii_lowercase());
        a.into_iter().chain(b).collect()
    };
    div()
        .flex_shrink_0()
        .size(look.px(size))
        .flex()
        .items_center()
        .justify_center()
        .rounded(look.px(4.))
        .bg(look.c.raised)
        .text_size(look.text(9.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(look.c.text_muted)
        .child(letters)
}

/// An icon from the app's assets, in the current text colour.
pub fn icon(look: &Look, name: &str, size: f32, color: Hsla) -> gpui::Svg {
    svg()
        .path(format!("icons/{name}.svg"))
        .flex_shrink_0()
        .size(look.px(size))
        .text_color(color)
}

/// `HH:MM` in local time.
pub fn hm(t: time::OffsetDateTime) -> String {
    let t = t.to_offset(super::status::local_offset());
    format!("{:02}:{:02}", t.hour(), t.minute())
}

/// `1h 12m`, `24m`, `40s`, `2d 3h`: the one duration form.
pub fn span(d: time::Duration) -> String {
    hippocamp_view::fleet::span(d.whole_seconds())
}

/// A title as the app shows it: trimmed, without a trailing full
/// stop (`Rename the sign helper`).
pub fn title(s: &str) -> String {
    let t = s.trim();
    match t.strip_suffix('.') {
        Some(rest) if !rest.ends_with('.') => rest.trim_end().to_owned(),
        _ => t.to_owned(),
    }
}

/// A small filled tag in the accent (the accent is a fill, never
/// text): `use next`.
pub fn tag(look: &Look, label: impl Into<String>) -> gpui::Div {
    div()
        .flex_shrink_0()
        .flex()
        .items_center()
        .h(look.px(18.))
        .px(look.px(7.))
        .rounded_full()
        .bg(look.c.accent_surface)
        .text_color(look.c.accent_text)
        .text_size(look.text(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .whitespace_nowrap()
        .child(label.into())
}

/// A text link: the text colour, underlined on hover.
pub fn link(
    look: &Look,
    id: impl Into<gpui::ElementId>,
    label: impl Into<String>,
) -> gpui::Stateful<gpui::Div> {
    let label = label.into();
    div()
        .id(id.into())
        .role(Role::Link)
        .aria_label(label.clone())
        .cursor_pointer()
        .text_color(look.c.text)
        .hover(|h| h.underline())
        .child(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_read_without_claude_and_marks_are_unique() {
        assert_eq!(profile_name("default"), "default");
        assert_eq!(profile_name("work"), "work");
        assert_eq!(profile_name("codex"), "codex");
        sync_profiles(["Ana", "Andy", "default", "work", "codex"]);
        assert_eq!(profile_letter(Some("Ana")), "An");
        assert_eq!(profile_letter(Some("Andy")), "Ad");
        assert_eq!(profile_letter(Some("default")), "De");
        assert_eq!(profile_letter(None), "?");
        assert_ne!(profile_hue("Ana"), profile_hue("Andy"));
    }

    #[test]
    fn a_command_reads_as_its_real_words() {
        assert_eq!(command_words("cargo test"), "cargo test");
        assert_eq!(
            command_words("cd /work/tern && cargo test --locked"),
            "cargo test --locked"
        );
        assert_eq!(
            command_words("S=/tmp/x; source $S/h.sh; export HPID=1; front; size 760 520"),
            "front; size 760 520"
        );
        assert_eq!(
            command_words("CARGO_TARGET_DIR=/tmp/t nice cargo build --release"),
            "cargo build --release"
        );
        assert_eq!(command_words("env -u HOME ls -la"), "ls -la");
        assert_eq!(
            command_words("THEME=\"Hippocamp Light\" ./run.sh --fast"),
            "./run.sh --fast"
        );
        assert_eq!(
            command_words("source ~/.zshrc && git status -sb"),
            "git status -sb"
        );
        // A `;` inside quotes is not a separator.
        assert_eq!(
            command_words("cd /a; echo 'x; y' && ls"),
            "echo 'x; y' && ls"
        );
        // All plumbing: shown as it was.
        assert_eq!(command_words("cd /work/tern"), "cd /work/tern");
        // A long path reads as its last part.
        assert_eq!(
            command_words("cat /private/tmp/agent/scratch/run.log"),
            "cat …/run.log"
        );
        assert_eq!(command_words("ls src/ui"), "ls src/ui");
        // A heredoc's marker is left out, and a long command keeps
        // its first eight words.
        assert_eq!(command_words("python3 - <<'EOF'"), "python3");
        assert_eq!(command_words("cd /w && cat > run.sh <<EOF"), "cat > run.sh");
        assert_eq!(
            command_words("sqlite3 db.sqlite select a, b, c from turns where x = 1"),
            "sqlite3 db.sqlite select a, b, c from turns …"
        );
    }

    #[test]
    fn prose_reads_as_one_plain_paragraph() {
        assert_eq!(
            plain("Report:\n\n- **Sign-in:** built.\n- See https://github.com/o/r/pull/7 now"),
            "Report: Sign-in: built. See github.com/…/7 now"
        );
    }

    #[test]
    fn spans_read_in_hours_minutes_or_seconds() {
        assert_eq!(span(time::Duration::minutes(64)), "1h 4m");
        assert_eq!(span(time::Duration::minutes(72)), "1h 12m");
        assert_eq!(span(time::Duration::minutes(24)), "24m");
        assert_eq!(span(time::Duration::seconds(40)), "40s");
    }
}

// ---------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------

/// A session's state as one glyph and one word, the same everywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Waiting, Failed, Done, Quiet: Fleet and the x-ray
pub enum Status {
    Working,
    /// On a sub-agent.
    Waiting,
    /// A plan waits for approval.
    Approval,
    /// The agent asked a question.
    Input,
    /// Stopped at a usage limit.
    Limited,
    Failed,
    Done,
    Quiet,
    Interrupted,
}

impl Status {
    /// The glyph and its colour: T3 Code's (`ThreadStatusIndicators`,
    /// MIT), in Hippocamp's state colours (Input stays amber; T3 Code
    /// draws it indigo).
    pub fn glyph(self, look: &Look) -> (&'static str, Hsla) {
        match self {
            Status::Working => ("circle-dashed", look.c.info),
            Status::Waiting => ("circle-dot-dashed", look.c.text_muted),
            Status::Approval => ("shield-question", look.c.warn),
            Status::Input => ("message-circle-question", look.c.warn),
            Status::Limited => ("circle-alert", look.c.warn),
            Status::Failed => ("circle-alert", look.c.err),
            Status::Done => ("circle-check", look.c.ok),
            Status::Quiet => ("clock", look.c.text_muted),
            Status::Interrupted => ("circle-pause", look.c.text_muted),
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Status::Working => "Working",
            Status::Waiting => "Waiting",
            Status::Approval => "Approval",
            Status::Input => "Input",
            Status::Limited => "Limited",
            Status::Failed => "Failed",
            Status::Done => "Done",
            Status::Quiet => "Quiet",
            Status::Interrupted => "Interrupted",
        }
    }

    /// The word's colour: the glyph's, except Done (a muted word beside a
    /// green check).
    pub fn color(self, look: &Look) -> Hsla {
        match self {
            Status::Done => look.c.text_muted,
            s => s.glyph(look).1,
        }
    }
}

/// `◌ Working 49m`: 12 points, weight 500, a 14-point glyph, 5 apart.
pub fn status(look: &Look, s: Status, detail: Option<String>) -> gpui::Div {
    let (glyph, gc) = s.glyph(look);
    let word = match detail {
        Some(d) if !d.is_empty() => format!("{} {d}", s.word()),
        _ => s.word().to_owned(),
    };
    div()
        .min_w_0()
        .max_w_full()
        .flex()
        .items_center()
        .gap(look.px(5.))
        .text_size(look.text(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(s.color(look))
        .child(icon(look, glyph, 14., gc))
        // A word too long for its column ends in an ellipsis.
        .child(div().min_w_0().truncate().child(word))
}

/// A sidebar row's status (T3 Code's top status): a 16-point
/// glyph, 4 apart, the word and its time in 12-point medium, all in the
/// state's colour (Done green too).
pub fn top_status(look: &Look, s: Status, detail: Option<String>) -> gpui::Div {
    let (glyph, gc) = s.glyph(look);
    let word = match detail {
        Some(d) if !d.is_empty() => format!("{} {d}", s.word()),
        _ => s.word().to_owned(),
    };
    div()
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(look.px(4.))
        .text_size(look.text(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(gc)
        .whitespace_nowrap()
        .child(icon(look, glyph, 16., gc))
        .child(word)
}

/// A pull request's tone (T3 Code's `PULL_REQUEST_STATE_PRESENTATION`,
/// MIT): open emerald, merged violet, at 90 % in dark.
pub fn pr_tone(look: &Look, merged: bool) -> Hsla {
    let hex = match (merged, look.c.dark) {
        (false, true) => 0x6ee7b7e6,
        (true, true) => 0xc4b5fde6,
        (false, false) => 0x059669ff,
        (true, false) => 0x7c3aedff,
    };
    gpui::rgba(hex).into()
}

/// A pull-request badge (T3 Code's sidebar badge): the 12-point glyph in
/// its tone, then the number in the meta colour (`2`, or `3 open`).
pub fn pr_badge(look: &Look, merged: bool, text: impl Into<String>) -> gpui::Div {
    let tone = pr_tone(look, merged);
    div()
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(look.px(3.))
        .text_size(look.text(12.))
        .text_color(look.c.faint)
        .whitespace_nowrap()
        .child(icon(
            look,
            if merged {
                "git-merge"
            } else {
                "git-pull-request-arrow"
            },
            12.,
            tone,
        ))
        .child(text.into())
}

/// A path cut in the middle to at most `max` characters: `src/…/kit.rs`.
pub fn middle(path: &str, max: usize) -> String {
    let n = path.chars().count();
    if n <= max || max < 5 {
        return path.to_owned();
    }
    let keep = max - 1;
    let tail = keep / 2 + keep % 2;
    let head = keep - tail;
    let a: String = path.chars().take(head).collect();
    let b: String = path.chars().skip(n - tail).collect();
    format!("{a}…{b}")
}

/// The mono face.
pub const MONO: &str = "SF Mono";

/// A mono chip: the target of a tool line (`billing/InvoiceTable.tsx`).
pub fn mono_chip(look: &Look, text: impl Into<String>, quiet: bool) -> gpui::Div {
    div()
        .flex_shrink(1.)
        .min_w_0()
        .h(look.px(if quiet { 16. } else { 18. }))
        .px(look.px(5.))
        .flex()
        .items_center()
        .rounded(look.px(4.))
        .bg(look.c.raised)
        .font_family(MONO)
        .text_size(look.text(11.5))
        .text_color(look.c.text_muted)
        .overflow_hidden()
        // The text in its own truncating box, so a long command
        // ends in an ellipsis, not cut mid-word at the chip's edge.
        .child(div().min_w_0().truncate().child(text.into()))
}

/// `+28 −9` in the diff colours, mono and tabular. Nothing when both are 0.
pub fn diff_counts(look: &Look, added: u64, removed: u64) -> Option<gpui::Div> {
    (added + removed > 0).then(|| {
        div()
            .flex_shrink_0()
            .flex()
            .gap(look.px(4.))
            .font_family(MONO)
            .text_size(look.text(11.5))
            .whitespace_nowrap()
            .child(div().text_color(look.c.diff_add).child(format!("+{added}")))
            .child(
                div()
                    .text_color(look.c.diff_del)
                    .child(format!("−{removed}")),
            )
    })
}

/// `+28 −9` in the faint colour (the sidebar's meta line, where
/// line counts are not news). Nothing when both are 0.
pub fn diff_counts_quiet(look: &Look, added: u64, removed: u64) -> Option<gpui::Div> {
    (added + removed > 0).then(|| {
        div()
            .flex_shrink_0()
            .font_family(MONO)
            .text_size(look.text(11.5))
            .text_color(look.c.faint)
            .whitespace_nowrap()
            .child(format!("+{added} −{removed}"))
    })
}

/// A shell command as a line shows it: the real command's words,
/// without the plumbing before them. Leading `cd …`, `source …`, `. …`,
/// `export …`, `setopt …` and bare assignments are dropped, as are a
/// command's own `VAR=value`, `env`, `time` and `nice` prefixes; what is
/// left starts at the first command that does work. A command that is
/// all plumbing stays as it was. A heredoc's marker (`python3 - <<'EOF'`)
/// is left out, and a long command keeps its first eight words (a sub-agent's
/// calls rarely carry a description, so their raw first lines showed).
pub fn command_words(cmd: &str) -> String {
    first_words(&without_heredoc(&plain_command_words(cmd)), 8)
}

/// `cmd` without a heredoc's `<<'EOF'` marker and the `-` that reads it.
fn without_heredoc(cmd: &str) -> String {
    let words: Vec<&str> = cmd.split(' ').collect();
    let mut out: Vec<&str> = Vec::with_capacity(words.len());
    for w in &words {
        let marker = w.starts_with("<<") && !w.starts_with("<<<");
        if marker {
            if out.last() == Some(&"-") {
                out.pop();
            }
            continue;
        }
        out.push(w);
    }
    let joined = out.join(" ");
    let trimmed = joined.trim();
    if trimmed.is_empty() {
        cmd.to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// The first `n` words of `text`, then `…` when there were more.
fn first_words(text: &str, n: usize) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() <= n {
        words.join(" ")
    } else {
        format!("{} …", words[..n].join(" "))
    }
}

/// [`command_words`] before the heredoc marker and the cut.
fn plain_command_words(cmd: &str) -> String {
    let cmd = cmd.trim();
    let mut rest = cmd;
    loop {
        let (head, tail) = split_first_command(rest);
        let words = strip_prefixes(head.trim());
        let first = words.split_whitespace().next().unwrap_or("");
        let plumbing = words.is_empty()
            || matches!(
                first,
                "cd" | "source" | "." | "export" | "setopt" | "unset" | "set"
            )
            || is_assignment(first) && words.split_whitespace().count() == 1;
        match (plumbing, tail) {
            (true, Some(t)) => rest = t,
            (true, None) => return cmd.to_owned(),
            (false, Some(t)) => {
                return short_paths(&format!("{words}{}{}", sep_of(rest, head), t.trim_start()));
            }
            (false, None) => return short_paths(words),
        }
    }
}

/// Long paths in a command as their last part (`…/run.log`), so a line
/// never reads as a temporary directory.
pub fn short_paths(cmd: &str) -> String {
    cmd.split(' ')
        .map(|w| {
            let bare = w.trim_matches(|c| c == '"' || c == '\'');
            let rooted = bare.starts_with('/') || bare.starts_with("~/") || bare.starts_with('$');
            let parts = bare.split('/').filter(|p| !p.is_empty()).count();
            match bare.trim_end_matches('/').rsplit_once('/') {
                Some((_, last)) if rooted && parts > 2 && !last.is_empty() => format!("…/{last}"),
                _ => w.to_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Prose as one paragraph for a clamped line: new lines and
/// list marks become spaces, Markdown's `**`, `__` and backticks go, and a
/// web address reads as its host and last part.
pub fn plain(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        let l = l
            .strip_prefix("- ")
            .or_else(|| l.strip_prefix("* "))
            .unwrap_or(l)
            .trim_start_matches('#')
            .trim();
        if l.is_empty() {
            continue;
        }
        out.push(l.replace("**", "").replace("__", "").replace('`', ""));
    }
    out.join(" ")
        .split_whitespace()
        .map(|w| match w.split_once("://") {
            Some((_, rest)) => {
                let rest = rest.trim_end_matches(['/', ',', '.', ')']);
                match (rest.split('/').next(), rest.rsplit_once('/')) {
                    (Some(host), Some((_, last))) if rest.contains('/') => {
                        format!("{host}/…/{last}")
                    }
                    _ => rest.to_owned(),
                }
            }
            None => w.to_owned(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The first command of `s` and what follows its `&&`, `;`, `||` or new
/// line, outside quotes.
fn split_first_command(s: &str) -> (&str, Option<&str>) {
    let b = s.as_bytes();
    let (mut q, mut i) = (0u8, 0);
    while i < b.len() {
        let c = b[i];
        if q != 0 {
            if c == q {
                q = 0;
            }
        } else if c == b'\'' || c == b'"' {
            q = c;
        } else if c == b';' || c == b'\n' {
            return (&s[..i], Some(&s[i + 1..]));
        } else if (c == b'&' || c == b'|') && b.get(i + 1) == Some(&c) {
            return (&s[..i], Some(&s[i + 2..]));
        }
        i += 1;
    }
    (s, None)
}

/// The separator that followed `head` in `s`, written back (`; `, ` && `).
fn sep_of(s: &str, head: &str) -> &'static str {
    match s[head.len()..].chars().next() {
        Some('&') => " && ",
        Some('|') => " || ",
        _ => "; ",
    }
}

fn is_assignment(w: &str) -> bool {
    w.split_once('=').is_some_and(|(k, _)| {
        !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// A command without its `VAR=value`, `env [-u X]`, `time` and `nice`
/// prefixes.
fn strip_prefixes(mut s: &str) -> &str {
    loop {
        let t = s.trim_start();
        let w = t.split_whitespace().next().unwrap_or("");
        let quoted = is_assignment(w)
            .then(|| w.split_once('=').map(|(_, v)| v))
            .flatten()
            .and_then(|v| v.chars().next().filter(|c| *c == '"' || *c == '\''));
        let skip = if let Some(q) = quoted {
            // `NAME="a b"`: up to the closing quote.
            let open = t.find(q).unwrap_or(0);
            match t[open + 1..].find(q) {
                Some(close) => open + 1 + close + 1,
                None => return t,
            }
        } else if is_assignment(w) || matches!(w, "time" | "nice" | "env" | "command" | "exec") {
            w.len()
        } else if w == "-u" {
            // `env -u NAME`: the flag and its name.
            let after = t[2..].trim_start();
            t.len() - after.len() + after.split_whitespace().next().map_or(0, str::len)
        } else {
            return t;
        };
        if t.len() == skip {
            return "";
        }
        s = &t[skip..];
    }
}

/// A run of the same step: consecutive calls with the same verb
/// and the same description or file, newest first, as one line with `×n`.
/// `call` is the newest of the run (its age and whether it runs) with the
/// run's lines added up.
#[derive(Debug, Clone)]
pub struct Run {
    pub call: hippocamp_view::fleet::tail::Activity,
    pub n: usize,
}

/// Two calls are the same step: the same verb and the same description,
/// or, with none, the same file or command.
pub fn same_step(
    a: &hippocamp_view::fleet::tail::Activity,
    b: &hippocamp_view::fleet::tail::Activity,
) -> bool {
    a.verb == b.verb
        && match (&a.step, &b.step) {
            (Some(x), Some(y)) => x == y,
            (None, None) => a.target == b.target,
            _ => false,
        }
}

/// Calls (newest first, as a turn holds them) as runs of the same step,
/// newest first.
pub fn runs(calls: &[hippocamp_view::fleet::tail::Activity]) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    for c in calls {
        if let Some(last) = out.last_mut()
            && same_step(&last.call, c)
        {
            last.n += 1;
            last.call.lines = match (last.call.lines, c.lines) {
                (Some(mut a), Some(b)) => {
                    a.add(b);
                    Some(a)
                }
                (a, b) => a.or(b),
            };
            continue;
        }
        out.push(Run {
            call: c.clone(),
            n: 1,
        });
    }
    out
}

/// `×4`: how many times a step ran in a row, with a tooltip saying so;
/// nothing for one.
pub fn times(look: &Look, at: time::OffsetDateTime, n: usize) -> Option<gpui::Stateful<gpui::Div>> {
    (n > 1).then(|| {
        div()
            .id(gpui::ElementId::Name(
                format!("times-{}-{n}", at.unix_timestamp_nanos()).into(),
            ))
            .tooltip(super::tooltip::Tooltip::text(format!(
                "The same step {n} times in a row"
            )))
            .flex_shrink_0()
            .text_size(look.text(12.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(look.c.text_muted)
            .whitespace_nowrap()
            .child(format!("×{n}"))
    })
}

/// A tool line: the bold verb, the target in a mono chip, the
/// lines it changed, `×n` for a repeated step, then its age. A call
/// with its own description is that description, muted, with a chevron (a
/// step line). `quiet` is the form inside a sidebar row: the verb muted,
/// the chip smaller.
pub fn tool_line(
    look: &Look,
    call: &hippocamp_view::fleet::tail::Activity,
    n: usize,
    age: Option<String>,
    quiet: bool,
) -> gpui::Div {
    let returned = !call.running;
    let verb_color = if quiet || returned {
        look.c.text_muted
    } else {
        look.c.text
    };
    let row = div()
        .flex()
        .items_center()
        .gap(look.px(6.))
        .min_w_0()
        .overflow_hidden()
        .text_size(look.text(if quiet { 12. } else { 12.5 }));
    let row = match &call.step {
        Some(step) if matches!(call.verb.as_str(), "Bash" | "Agent" | "Asked") => row
            // A question keeps its verb: `Asked Which plan…`.
            .when(call.verb == "Asked", |d| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(verb_color)
                        .child("Asked"),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(3.))
                    .min_w_0()
                    .text_color(look.c.text_muted)
                    .child(div().min_w_0().truncate().child(step.clone()))
                    .when(call.verb != "Asked", |d| {
                        d.child(icon(look, "chevron-right", 12., look.c.faint))
                    }),
            ),
        _ => {
            let target = call.target.as_deref().map(|t| {
                if call.verb == "Bash" {
                    // The real command's words, not shell plumbing.
                    command_words(t)
                } else {
                    middle(&short_path(t), 34)
                }
            });
            row.child(
                div()
                    .flex_shrink_0()
                    .font_weight(if quiet || returned {
                        FontWeight::MEDIUM
                    } else {
                        FontWeight::SEMIBOLD
                    })
                    .text_color(verb_color)
                    .child(call.verb.clone()),
            )
            .children(target.map(|t| mono_chip(look, t, quiet)))
            .children(
                call.lines
                    .and_then(|l| diff_counts(look, l.added, l.removed)),
            )
        }
    };
    row.children(times(look, call.at, n)).children(age.map(|a| {
        div()
            .flex_shrink_0()
            .text_color(look.c.faint)
            .text_size(look.text(12.))
            .child(a)
    }))
}

/// A path as a tool line shows it: from the repository's `src`, `app`,
/// `crates` or the last three parts.
pub fn short_path(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if let Some(i) = parts
        .iter()
        .rposition(|p| matches!(*p, "src" | "app" | "crates" | "lib" | "docs"))
    {
        let from = i.saturating_sub(1);
        return parts[from..].join("/");
    }
    let n = parts.len();
    parts[n.saturating_sub(3)..].join("/")
}

/// A kind chip (`Agent`, `Bash`, `Question`): 18 high, radius 5.
pub fn kind_chip(look: &Look, label: &str) -> gpui::Div {
    div()
        .flex_shrink_0()
        .h(look.px(18.))
        .px(look.px(6.))
        .flex()
        .items_center()
        .rounded(look.px(5.))
        .bg(look.c.raised)
        .text_size(look.text(11.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(look.c.text_muted)
        .whitespace_nowrap()
        .child(label.to_owned())
}

/// A count badge: `accent.surface` fill, `accent.text`, or the warning
/// colour for Needs you.
pub fn count_badge(look: &Look, n: usize, warn: bool) -> gpui::Div {
    let (bg, fg) = if warn {
        (
            look.c.warn.opacity(if look.c.dark { 0.16 } else { 0.12 }),
            look.c.warn,
        )
    } else {
        (look.c.accent_surface, look.c.accent_text)
    };
    div()
        .flex_shrink_0()
        .min_w(look.px(18.))
        .h(look.px(18.))
        .px(look.px(5.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(bg)
        .text_color(fg)
        .text_size(look.text(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(if n > 99 {
            "99+".to_owned()
        } else {
            n.to_string()
        })
}

/// A pane card: the surface, a hairline, radius 12; its title
/// row is 38 high with the pane's name and one fact at the right. The
/// caller adds the body.
pub fn pane_card(look: &Look, title: &str, fact: Option<String>) -> gpui::Div {
    card(look).overflow_hidden().child(
        div()
            .flex()
            .items_center()
            .gap(look.px(8.))
            .h(look.px(38.))
            .px(look.px(14.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(look.text(12.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title.to_owned()),
            )
            .children(fact.map(|f| {
                div()
                    .flex_shrink_0()
                    .text_size(look.text(12.))
                    .text_color(look.c.faint)
                    .child(f)
            })),
    )
}

/// The segmented control's track: 28 high on the surface with a hairline
/// and a 2-point inset. Its children are [`segment`]s.
pub fn segmented(look: &Look) -> gpui::Div {
    div()
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(look.px(2.))
        .h(look.px(30.))
        .p(look.px(2.))
        .rounded(look.px(8.))
        .bg(look.c.surface)
        .border(look.fixed(1.))
        .border_color(look.c.border)
}

/// One segment: 24 high, radius 6, 12.5 points; the chosen one raised.
pub fn segment(
    look: &Look,
    id: impl Into<gpui::ElementId>,
    icon_name: Option<&str>,
    label: impl Into<String>,
    on: bool,
) -> gpui::Stateful<gpui::Div> {
    let label = label.into();
    let color = if on { look.c.text } else { look.c.text_muted };
    div()
        .id(id.into())
        .role(Role::Tab)
        .aria_label(label.clone())
        .aria_selected(on)
        .flex()
        .items_center()
        .gap(look.px(5.))
        .h(look.px(24.))
        .px(look.px(9.))
        .rounded(look.px(6.))
        .text_size(look.text(12.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(color)
        .when(on, |d| {
            d.bg(look.c.raised)
                .border(look.fixed(1.))
                .border_color(look.c.border)
        })
        .cursor_pointer()
        .hover(|h| h.text_color(look.c.text))
        .active(|a| a.bg(look.c.hover))
        .children(icon_name.map(|n| icon(look, n, 14., color)))
        .child(label)
}

/// A segment led by an element (an account mark) instead of an icon.
pub fn segment_lead(
    look: &Look,
    id: impl Into<gpui::ElementId>,
    lead: impl IntoElement,
    label: impl Into<String>,
    on: bool,
) -> gpui::Stateful<gpui::Div> {
    let label = label.into();
    let color = if on { look.c.text } else { look.c.text_muted };
    div()
        .id(id.into())
        .role(Role::Tab)
        .aria_label(label.clone())
        .aria_selected(on)
        .flex()
        .items_center()
        .gap(look.px(6.))
        .h(look.px(24.))
        .px(look.px(8.))
        .rounded(look.px(6.))
        .text_size(look.text(12.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(color)
        .whitespace_nowrap()
        .when(on, |d| {
            d.bg(look.c.raised)
                .border(look.fixed(1.))
                .border_color(look.c.border)
        })
        .cursor_pointer()
        .hover(|h| h.text_color(look.c.text))
        .active(|a| a.bg(look.c.hover))
        .child(lead)
        .child(label)
}

/// The usage ring's and bars' colour for a fraction left: a muted fill, warn under 25 %, err
/// under 10 %.
pub fn left_color(look: &Look, left: f64) -> Hsla {
    // Colour is state: healthy headroom is not a
    // state, so its bar and ring are a muted fill; amber under 25 %, red
    // under 10 %.
    if left < 0.10 {
        look.c.err
    } else if left < 0.25 {
        look.c.warn
    } else {
        look.c
            .text_muted
            .opacity(if look.c.dark { 0.55 } else { 0.5 })
    }
}

/// A usage ring: an account's tighter limit as headroom left,
/// draining clockwise from 12 o'clock; an inferred figure is a dashed
/// track with no arc.
pub fn usage_ring(look: &Look, left: f64, inferred: bool, size: f32) -> impl IntoElement {
    let track = look.c.border_strong;
    let arc = left_color(look, left);
    let stroke = look.px(size * 2.25 / 14.);
    let frac = left.clamp(0.0, 1.0) as f32;
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let r = (bounds.size.width.min(bounds.size.height) - stroke) / 2.;
            let c = bounds.center();
            let at = |t: f32| {
                let a = t * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                point(c.x + r * a.cos(), c.y + r * a.sin())
            };
            // The track: a full circle as two half arcs.
            let mut b = PathBuilder::stroke(stroke);
            if inferred {
                b = b.dash_array(&[px(2.), px(2.)]);
            }
            b.move_to(at(0.));
            b.arc_to(point(r, r), px(0.), false, true, at(0.5));
            b.arc_to(point(r, r), px(0.), false, true, at(0.9999));
            if let Ok(p) = b.build() {
                window.paint_path(p, track);
            }
            if !inferred && frac > 0.005 {
                let mut a = PathBuilder::stroke(stroke);
                a.move_to(at(0.));
                let end = frac.min(0.9999);
                if end > 0.5 {
                    a.arc_to(point(r, r), px(0.), false, true, at(0.5));
                }
                a.arc_to(point(r, r), px(0.), false, true, at(end));
                if let Ok(p) = a.build() {
                    window.paint_path(p, arc);
                }
            }
        },
    )
    .flex_shrink_0()
    .size(look.px(size))
}

/// A sidebar row's fills (T3 Code's `--sidebar-row-*`, MIT): in
/// dark the text colour at 8 % on hover and 11 % for the open row; in
/// light a lighter hover and the open row white with a hairline ring.
pub fn sidebar_fill(
    d: gpui::Stateful<gpui::Div>,
    look: &Look,
    active: bool,
) -> gpui::Stateful<gpui::Div> {
    if look.c.dark {
        let hover = look.c.text.opacity(0.08);
        let pressed = look.c.text.opacity(0.14);
        if active {
            d.bg(look.c.text.opacity(0.11))
        } else {
            d.hover(move |h| h.bg(hover)).active(move |a| a.bg(pressed))
        }
    } else if active {
        d.bg(gpui::white())
            .border(look.fixed(1.))
            .border_color(look.c.border.opacity(0.7))
    } else {
        let (hover, pressed) = (look.c.canvas, look.c.raised);
        d.border(look.fixed(1.))
            .border_color(gpui::transparent_black())
            .hover(move |h| h.bg(hover))
            .active(move |a| a.bg(pressed))
    }
}

/// A row's hover and selection fills: hover `bg.hover`, selected
/// `bg.raised`.
pub fn row_fill(
    d: gpui::Stateful<gpui::Div>,
    look: &Look,
    selected: bool,
) -> gpui::Stateful<gpui::Div> {
    if selected {
        d.bg(look.c.raised)
    } else {
        let (hover, pressed) = (look.c.hover, look.c.raised);
        d.hover(move |h| h.bg(hover)).active(move |a| a.bg(pressed))
    }
}

/// A sub-agent's report as it reads: without what its client
/// wrapped around it. Leading lines that are one tag (`<tools-used-before-
/// summary>…</…>`) or a bracketed note (`[Harness note, …]`) go, and a
/// frame (a first line in brackets, `[Subagent hand-back] …`, over a body
/// whose every line is indented two spaces) gives its body, dedented.
pub fn report_body(text: &str) -> String {
    let mut lines: Vec<&str> = text.lines().collect();
    while let Some(first) = lines.first().map(|l| l.trim()) {
        let tag = first.starts_with('<')
            && first.ends_with('>')
            && first.find('>').is_some_and(|i| {
                first[..i]
                    .chars()
                    .skip(1)
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
            })
            && first.contains("</");
        let note = first.starts_with('[') && first.ends_with(']');
        if first.is_empty() || tag || note {
            lines.remove(0);
        } else {
            break;
        }
    }
    let framed = lines
        .first()
        .is_some_and(|l| l.starts_with('[') && l[1..].find(']').is_some_and(|i| i > 0 && i < 40))
        && lines.len() > 1
        && lines[1..]
            .iter()
            .all(|l| l.trim().is_empty() || l.starts_with("  "));
    if framed {
        return lines[1..]
            .iter()
            .map(|l| l.strip_prefix("  ").unwrap_or(l.trim_start()))
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_owned();
    }
    lines.join("\n").trim().to_owned()
}

#[cfg(test)]
mod report_tests {
    #[test]
    fn a_report_reads_without_its_clients_wrapping() {
        let framed = "<tools-used-before-summary>Glob, Grep</tools-used-before-summary>\n  \n[Subagent hand-back] The text below is the final report. The report follows:\n  # Review\n  \n  Two faults.\n    - nested";
        assert_eq!(
            super::report_body(framed),
            "# Review\n\nTwo faults.\n  - nested"
        );
        let noted =
            "[Harness note, not part of the agent's report: no files]\nThe flags are -a and -b.";
        assert_eq!(super::report_body(noted), "The flags are -a and -b.");
        // A report that starts with a link or a list is kept as it is.
        assert_eq!(
            super::report_body("[PR #3](https://x/3) adds it.\nDone."),
            "[PR #3](https://x/3) adds it.\nDone."
        );
        assert_eq!(super::report_body("Plain."), "Plain.");
    }
}

#[cfg(test)]
mod run_tests {
    use super::*;
    use hippocamp_core::rollup::changes::Lines;
    use hippocamp_view::fleet::tail::Activity;
    use time::macros::datetime;

    fn call(verb: &str, step: Option<&str>, target: Option<&str>, min: u8) -> Activity {
        Activity {
            label: String::new(),
            at: datetime!(2026-01-01 10:00 UTC) + time::Duration::minutes(i64::from(min)),
            running: false,
            delegates: false,
            verb: verb.to_owned(),
            target: target.map(str::to_owned),
            step: step.map(str::to_owned),
            lines: None,
        }
    }

    #[test]
    fn the_same_step_in_a_row_reads_once_with_its_count() {
        // Newest first, as a turn holds them.
        let wait = |m| call("Bash", Some("Wait for the build"), Some("sleep 60"), m);
        let calls = vec![
            wait(9),
            wait(8),
            wait(7),
            wait(6),
            call("Read", None, Some("src/a.rs"), 5),
            wait(4),
        ];
        let runs = runs(&calls);
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[0].n, 4);
        // The run keeps its newest call's time.
        assert_eq!(runs[0].call.at, calls[0].at);
        assert_eq!(runs[1].n, 1);
        // Not consecutive: its own line.
        assert_eq!(runs[2].n, 1);
    }

    #[test]
    fn a_step_is_its_description_else_its_file() {
        // The same description with another command is the same step.
        assert!(same_step(
            &call("Bash", Some("Wait"), Some("sleep 60"), 1),
            &call("Bash", Some("Wait"), Some("sleep 120"), 2)
        ));
        assert!(!same_step(
            &call("Bash", Some("Wait"), None, 1),
            &call("Bash", Some("Build"), None, 2)
        ));
        assert!(!same_step(
            &call("Edit", None, Some("a.rs"), 1),
            &call("Read", None, Some("a.rs"), 2)
        ));
        assert!(!same_step(
            &call("Edit", None, Some("a.rs"), 1),
            &call("Edit", None, Some("b.rs"), 2)
        ));
    }

    #[test]
    fn repeated_edits_add_their_lines() {
        let mut a = call("Edit", None, Some("a.rs"), 2);
        a.lines = Some(Lines {
            added: 3,
            removed: 1,
        });
        let mut b = call("Edit", None, Some("a.rs"), 1);
        b.lines = Some(Lines {
            added: 2,
            removed: 0,
        });
        let r = runs(&[a, b]);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].n, 2);
        assert_eq!(
            r[0].call.lines,
            Some(Lines {
                added: 5,
                removed: 1
            })
        );
    }
}
