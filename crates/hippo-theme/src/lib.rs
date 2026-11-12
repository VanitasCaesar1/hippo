//! The hippo web theme contract, version 1.
//!
//! hippo's web pages (the feed and the registry's static site) share one
//! stylesheet. This crate holds that stylesheet, the built-in themes, the
//! hippo mark, the lists of custom properties, class names and page names that
//! themes may rely on, and the checks that decide whether a piece of custom CSS
//! is acceptable. It has no dependencies.
//!
//! What each property and class means, the layer order, and the rules for
//! personal CSS and shared theme packs are written in
//! the web theme contract. This crate holds the code for that contract;
//! it does not repeat its text.

use std::collections::BTreeMap;
use std::fmt;

/// The version of the theme contract. The base stylesheet begins with a
/// comment that names it.
pub const CONTRACT_VERSION: u32 = 1;

/// The base stylesheet: the layout of every contract class. Its `:root` block
/// gives every property the `default` theme's value, so the `default` theme is
/// this stylesheet alone and a page using it links no theme file.
pub const BASE_CSS: &str = include_str!("../css/base.css");

/// The `dark` theme: the dark values under `:root`.
pub const DARK_CSS: &str = include_str!("../css/dark.css");

/// The `auto` theme: the `dark` theme inside a `prefers-color-scheme: dark`
/// media query, so the page follows the device.
pub const AUTO_CSS: &str = concat!(
    "@media (prefers-color-scheme: dark) {\n",
    include_str!("../css/dark.css"),
    "}\n"
);

/// The hippo mark as an SVG drawn in `currentColor`. Pages inline it with the
/// class `hp-mark`.
pub const MARK_SVG: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../brand/hippo-mark.svg"
));

/// The page icon: the mark on ink blue, from `brand/hippo-icon.svg`.
pub const ICON_SVG: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../brand/hippo-icon.svg"
));

/// The mark for inlining beside the visible name: [`MARK_SVG`] with the given
/// class and `aria-hidden="true"` in place of its `role` and label, so screen
/// readers skip it. Pages call it with `"hp-mark"`.
pub fn mark_svg(class: &str) -> String {
    let class = class
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;");
    MARK_SVG.replacen(
        " role=\"img\" aria-label=\"Hippo\"",
        &format!(" class=\"{class}\" aria-hidden=\"true\""),
        1,
    )
}

/// Every custom property of the contract, with its leading dashes.
pub const PROPERTIES: &[&str] = &[
    "--hp-page-bg",
    "--hp-content-bg",
    "--hp-text",
    "--hp-muted",
    "--hp-link",
    "--hp-link-visited",
    "--hp-bar-bg",
    "--hp-bar-text",
    "--hp-bar-focus",
    "--hp-vote",
    "--hp-focus",
    "--hp-rule",
    "--hp-code-bg",
    "--hp-input-bg",
    "--hp-input-text",
    "--hp-input-border",
    "--hp-error",
    "--hp-font",
    "--hp-font-mono",
    "--hp-size-title",
    "--hp-size-text",
    "--hp-size-meta",
    "--hp-line",
    "--hp-content-width",
    "--hp-indent",
    "--hp-mark-size",
    "--hp-color-scheme",
];

/// Every class name of the contract, without the dot. The per-page classes
/// `hp-page-<name>` are listed by [`PAGES`] instead.
pub const CLASSES: &[&str] = &[
    "hp-page",
    "hp-skip",
    "hp-content",
    "hp-topbar",
    "hp-home",
    "hp-mark",
    "hp-name",
    "hp-nav",
    "hp-nav-current",
    "hp-sep",
    "hp-account",
    "hp-karma",
    "hp-main",
    "hp-footer",
    "hp-items",
    "hp-item",
    "hp-item-pack",
    "hp-item-discussion",
    "hp-item-theme",
    "hp-rank",
    "hp-vote",
    "hp-vote-button",
    "hp-title",
    "hp-title-link",
    "hp-domain",
    "hp-meta",
    "hp-points",
    "hp-user",
    "hp-age",
    "hp-evidence",
    "hp-install",
    "hp-comments-link",
    "hp-link-button",
    "hp-more",
    "hp-post",
    "hp-post-body",
    "hp-pack-box",
    "hp-install-box",
    "hp-passes",
    "hp-comments",
    "hp-comment",
    "hp-children",
    "hp-comment-meta",
    "hp-comment-body",
    "hp-md",
    "hp-form",
    "hp-field",
    "hp-label",
    "hp-input",
    "hp-textarea",
    "hp-select",
    "hp-button",
    "hp-help",
    "hp-error",
    "hp-notice",
    "hp-placeholder",
    "hp-sr",
    "hp-table",
    "hp-code",
];

/// The page names: a page's `body` carries the class `hp-page-<name>`.
pub const PAGES: &[&str] = &[
    "front",
    "new",
    "past",
    "comments",
    "ask",
    "show",
    "packs",
    "evidence",
    "themes",
    "post",
    "submit",
    "edit",
    "reply",
    "measure",
    "report",
    "settings",
    "hidden",
    "login",
    "moderation",
    "mod",
    "admin",
    "about",
    "error",
    "registry-index",
    "registry-themes",
    "registry-pack",
    "docs",
];

/// Pairs of colour properties, foreground first, whose contrast must be at
/// least 4.5:1 in a built-in or shared theme.
pub const TEXT_PAIRS: &[(&str, &str)] = &[
    ("--hp-text", "--hp-content-bg"),
    ("--hp-muted", "--hp-content-bg"),
    ("--hp-link", "--hp-content-bg"),
    ("--hp-link-visited", "--hp-content-bg"),
    ("--hp-error", "--hp-content-bg"),
    ("--hp-text", "--hp-code-bg"),
    ("--hp-muted", "--hp-code-bg"),
    ("--hp-text", "--hp-page-bg"),
    ("--hp-muted", "--hp-page-bg"),
    ("--hp-input-text", "--hp-input-bg"),
    ("--hp-bar-text", "--hp-bar-bg"),
];

/// Pairs of colour properties, foreground first, whose contrast must be at
/// least 3:1: the parts of controls and the focus outlines.
pub const CONTROL_PAIRS: &[(&str, &str)] = &[
    ("--hp-vote", "--hp-content-bg"),
    ("--hp-focus", "--hp-content-bg"),
    ("--hp-input-border", "--hp-input-bg"),
    ("--hp-bar-focus", "--hp-bar-bg"),
];

/// The largest size of a custom stylesheet, in bytes (64 KiB).
pub const CSS_MAX: usize = 65_536;

const TEXT_FLOOR: f64 = 4.5;
const CONTROL_FLOOR: f64 = 3.0;

/// Why a piece of custom CSS was refused.
///
/// It prints as `theme.css:<line>: <what>`. `line` counts from 1; it is 0 when
/// the problem is the size of the file, not a place in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CssError {
    /// The line of the problem, or 0 for a size error.
    pub line: usize,
    /// What is wrong, in plain words.
    pub what: String,
}

impl CssError {
    fn new(line: usize, what: impl Into<String>) -> Self {
        CssError {
            line,
            what: what.into(),
        }
    }
}

impl fmt::Display for CssError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "theme.css:{}: {}", self.line, self.what)
    }
}

impl std::error::Error for CssError {}

/// Checks a member's personal CSS: at most [`CSS_MAX`] bytes and no control
/// characters other than tab, line feed and carriage return. Any other CSS is
/// allowed, because personal CSS reaches only its author.
pub fn check_personal_css(text: &str) -> Result<(), CssError> {
    if text.len() > CSS_MAX {
        return Err(CssError::new(
            0,
            format!("the file is {} bytes; the limit is {CSS_MAX}", text.len()),
        ));
    }
    let mut line = 1;
    for c in text.chars() {
        if c == '\n' {
            line += 1;
        } else if c.is_control() && c != '\t' && c != '\r' {
            return Err(CssError::new(
                line,
                format!("control character U+{:04X} is not allowed", c as u32),
            ));
        }
    }
    Ok(())
}

/// Checks a theme pack's `theme.css` against the shared theme rules: only
/// `:root` blocks (at the top level or inside a light or dark
/// `prefers-color-scheme` media query), only contract properties, each value
/// in its grammar, and the contrast floors met in every scheme.
pub fn check_theme_pack_css(text: &str) -> Result<(), CssError> {
    check_personal_css(text)?;
    let clean = strip_comments(text)?;
    let blocks = parse_blocks(&clean)?;
    check_contrast(&blocks)
}

/// The WCAG contrast ratio of two colours written `#rgb` or `#rrggbb`, or
/// `None` for any other form. The ratio runs from 1 to 21.
pub fn contrast(foreground: &str, background: &str) -> Option<f64> {
    let a = luminance(parse_hex(foreground)?);
    let b = luminance(parse_hex(background)?);
    let (light, dark) = if a >= b { (a, b) } else { (b, a) };
    Some((light + 0.05) / (dark + 0.05))
}

fn parse_hex(colour: &str) -> Option<[u8; 3]> {
    let digits = colour.strip_prefix('#')?;
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let value = |s: &str| u8::from_str_radix(s, 16).ok();
    match digits.len() {
        3 => {
            let one = |i: usize| value(&digits[i..=i]).map(|v| v * 17);
            Some([one(0)?, one(1)?, one(2)?])
        }
        6 => Some([
            value(&digits[0..2])?,
            value(&digits[2..4])?,
            value(&digits[4..6])?,
        ]),
        _ => None,
    }
}

fn luminance(rgb: [u8; 3]) -> f64 {
    let channel = |v: u8| {
        let c = f64::from(v) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(rgb[0]) + 0.7152 * channel(rgb[1]) + 0.0722 * channel(rgb[2])
}

// ---- the scanner for the closed grammar of shared themes ----

/// Replaces each comment with spaces, keeping line breaks so that line numbers
/// stay right. Text inside quotes is left alone.
fn strip_comments(text: &str) -> Result<String, CssError> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut line = 1;
    let mut quote: Option<char> = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = quote {
            if c == q || c == '\n' {
                quote = None;
            }
        } else if c == '"' || c == '\'' {
            quote = Some(c);
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            let start_line = line;
            i += 2;
            out.push_str("  ");
            loop {
                match chars.get(i) {
                    None => return Err(CssError::new(start_line, "comment is never closed")),
                    Some('*') if chars.get(i + 1) == Some(&'/') => {
                        out.push_str("  ");
                        i += 2;
                        break;
                    }
                    Some('\n') => {
                        out.push('\n');
                        line += 1;
                        i += 1;
                    }
                    Some(_) => {
                        out.push(' ');
                        i += 1;
                    }
                }
            }
            continue;
        }
        if c == '\n' {
            line += 1;
        }
        out.push(c);
        i += 1;
    }
    Ok(out)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Scheme {
    Top,
    Light,
    Dark,
}

struct Decl {
    name: String,
    value: String,
    line: usize,
}

struct Block {
    scheme: Scheme,
    decls: Vec<Decl>,
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn line_at(&self, index: usize) -> usize {
        1 + self.chars[..index].iter().filter(|&&c| c == '\n').count()
    }

    fn skip_space(&mut self) {
        while self.pos < self.chars.len() && self.chars[self.pos].is_whitespace() {
            self.pos += 1;
        }
    }

    fn at_end(&self) -> bool {
        self.pos >= self.chars.len()
    }

    /// Reads a rule's prelude up to its `{` and consumes the brace. Returns the
    /// prelude with its whitespace collapsed and the line it starts on.
    fn prelude(&mut self) -> Result<(String, usize), CssError> {
        let start = self.pos;
        let line = self.line_at(start);
        while self.pos < self.chars.len() {
            match self.chars[self.pos] {
                '{' => {
                    let raw: String = self.chars[start..self.pos].iter().collect();
                    self.pos += 1;
                    let text = raw.split_whitespace().collect::<Vec<_>>().join(" ");
                    return Ok((text, line));
                }
                ';' | '}' => break,
                _ => self.pos += 1,
            }
        }
        let raw: String = self.chars[start..self.pos.min(self.chars.len())]
            .iter()
            .collect();
        Err(CssError::new(
            line,
            format!(
                "only `:root` blocks are allowed, found `{}`",
                raw.split_whitespace().collect::<Vec<_>>().join(" ")
            ),
        ))
    }

    /// Reads declarations up to the closing brace of a `:root` block.
    fn declarations(&mut self, open_line: usize) -> Result<Vec<Decl>, CssError> {
        let mut decls: Vec<Decl> = Vec::new();
        loop {
            self.skip_space();
            if self.at_end() {
                return Err(CssError::new(open_line, "the block is never closed"));
            }
            if self.chars[self.pos] == '}' {
                self.pos += 1;
                return Ok(decls);
            }
            let start = self.pos;
            let line = self.line_at(start);
            let mut depth = 0usize;
            let mut quote: Option<char> = None;
            let end;
            loop {
                let Some(&c) = self.chars.get(self.pos) else {
                    return Err(CssError::new(open_line, "the block is never closed"));
                };
                if let Some(q) = quote {
                    if c == q {
                        quote = None;
                    }
                } else {
                    match c {
                        '"' | '\'' => quote = Some(c),
                        '(' => depth += 1,
                        ')' => depth = depth.saturating_sub(1),
                        '{' => {
                            return Err(CssError::new(line, "rules inside a block are not allowed"))
                        }
                        ';' | '}' if depth == 0 => {
                            end = self.pos;
                            break;
                        }
                        _ => {}
                    }
                }
                self.pos += 1;
            }
            if self.chars[self.pos] == ';' {
                self.pos += 1;
            }
            let text: String = self.chars[start..end].iter().collect();
            if text.trim().is_empty() {
                continue;
            }
            let Some((name, value)) = text.split_once(':') else {
                return Err(CssError::new(line, "expected `property: value`"));
            };
            let name = name.trim();
            let value = value.trim();
            let Some(&known) = PROPERTIES.iter().find(|p| **p == name) else {
                return Err(CssError::new(
                    line,
                    format!("`{name}` is not a property of the contract"),
                ));
            };
            if decls.iter().any(|d| d.name == known) {
                return Err(CssError::new(
                    line,
                    format!("`{known}` is set twice in one block"),
                ));
            }
            if value.to_ascii_lowercase().contains("url(") {
                return Err(CssError::new(line, "`url()` is not allowed in a theme"));
            }
            if value.contains('!') {
                return Err(CssError::new(line, "`!important` is not allowed"));
            }
            check_value(known, value).map_err(|what| CssError::new(line, what))?;
            decls.push(Decl {
                name: known.to_string(),
                value: value.to_string(),
                line,
            });
        }
    }
}

fn parse_blocks(text: &str) -> Result<Vec<Block>, CssError> {
    let mut p = Parser {
        chars: text.chars().collect(),
        pos: 0,
    };
    let mut blocks = Vec::new();
    loop {
        p.skip_space();
        if p.at_end() {
            return Ok(blocks);
        }
        let (head, line) = p.prelude()?;
        if head == ":root" {
            let decls = p.declarations(line)?;
            blocks.push(Block {
                scheme: Scheme::Top,
                decls,
            });
        } else if head.starts_with('@') {
            let squeezed: String = head
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>()
                .to_ascii_lowercase();
            let scheme = match squeezed.as_str() {
                "@media(prefers-color-scheme:light)" => Scheme::Light,
                "@media(prefers-color-scheme:dark)" => Scheme::Dark,
                _ => {
                    let word = head.split_whitespace().next().unwrap_or("@");
                    return Err(CssError::new(
                        line,
                        format!(
                            "`{word}` is not allowed; only `@media (prefers-color-scheme: light)` and `@media (prefers-color-scheme: dark)`"
                        ),
                    ));
                }
            };
            loop {
                p.skip_space();
                if p.at_end() {
                    return Err(CssError::new(line, "the block is never closed"));
                }
                if p.chars[p.pos] == '}' {
                    p.pos += 1;
                    break;
                }
                let (inner, inner_line) = p.prelude()?;
                if inner != ":root" {
                    return Err(CssError::new(
                        inner_line,
                        format!("only `:root` is allowed inside a media block, found `{inner}`"),
                    ));
                }
                let decls = p.declarations(inner_line)?;
                blocks.push(Block { scheme, decls });
            }
        } else {
            return Err(CssError::new(
                line,
                format!("selector `{head}` is not allowed; only `:root`"),
            ));
        }
    }
}

// ---- value grammars ----

fn check_value(name: &str, value: &str) -> Result<(), String> {
    let bad = |what: &str| Err(format!("`{name}`: {what}"));
    match name {
        "--hp-font" | "--hp-font-mono" => check_font(value).or_else(|e| bad(&e)),
        "--hp-size-title" | "--hp-size-text" | "--hp-size-meta" => {
            match length(value, &["rem", "em", "px", "pt"]) {
                Some((n, unit)) => {
                    let px = match unit {
                        "rem" | "em" => n * 16.0,
                        "pt" => n * 4.0 / 3.0,
                        _ => n,
                    };
                    if (10.0 - 1e-9..=32.0 + 1e-9).contains(&px) {
                        Ok(())
                    } else {
                        bad("a size must be from 10 to 32 pixels")
                    }
                }
                None => bad("expected a size in rem, em, px or pt"),
            }
        }
        "--hp-line" => match number(value) {
            Some(n) if (1.0..=2.5).contains(&n) => Ok(()),
            Some(_) => bad("the line height must be from 1 to 2.5"),
            None => bad("expected a number"),
        },
        "--hp-content-width" => match length(value, &["%", "px"]) {
            Some((n, "%")) if (50.0..=100.0).contains(&n) => Ok(()),
            Some((n, "px")) if (320.0..=2000.0).contains(&n) => Ok(()),
            Some(_) => bad("the width must be from 50% to 100%, or from 320px to 2000px"),
            None => bad("expected a percentage or a length in px"),
        },
        "--hp-indent" => {
            if value == "0" {
                return Ok(());
            }
            match length(value, &["px", "rem", "em"]) {
                Some((n, unit)) => {
                    let px = if unit == "px" { n } else { n * 16.0 };
                    if (0.0..=40.0 + 1e-9).contains(&px) {
                        Ok(())
                    } else {
                        bad("the indent must be from 0 to 40 pixels")
                    }
                }
                None => bad("expected a length in px, rem or em"),
            }
        }
        "--hp-mark-size" => match length(value, &["px"]) {
            Some((n, _)) if (10.0..=48.0).contains(&n) => Ok(()),
            Some(_) => bad("the mark size must be from 10px to 48px"),
            None => bad("expected a length in px"),
        },
        "--hp-color-scheme" => {
            if value == "light" || value == "dark" {
                Ok(())
            } else {
                bad("expected `light` or `dark`")
            }
        }
        _ => {
            if parse_hex(value).is_some() {
                Ok(())
            } else {
                bad("expected a colour written #rgb or #rrggbb")
            }
        }
    }
}

/// A plain non-negative decimal number: digits, then optionally a dot and digits.
fn number(text: &str) -> Option<f64> {
    let (int, frac) = match text.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (text, None),
    };
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    // `.5` is a number; `5.` and a bare `.` are not.
    let int_ok = int.is_empty() && frac.is_some() || digits(int);
    if !int_ok || frac.is_some_and(|f| !digits(f)) {
        return None;
    }
    let text = if int.is_empty() {
        format!("0{text}")
    } else {
        text.to_string()
    };
    text.parse().ok()
}

/// A number followed by one of the given units.
fn length<'a>(text: &str, units: &[&'a str]) -> Option<(f64, &'a str)> {
    units.iter().find_map(|&unit| {
        let head = text.strip_suffix(unit)?;
        // `em` is the end of `rem`; the number part decides which one it was.
        number(head).map(|n| (n, unit))
    })
}

fn check_font(value: &str) -> Result<(), String> {
    if value.len() > 200 {
        return Err("a font list is at most 200 characters".into());
    }
    let mut items: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for c in value.chars() {
        match quote {
            Some(q) => {
                current.push(c);
                if c == q {
                    quote = None;
                }
            }
            None if c == ',' => items.push(std::mem::take(&mut current)),
            None => {
                if c == '"' || c == '\'' {
                    quote = Some(c);
                }
                current.push(c);
            }
        }
    }
    if quote.is_some() {
        return Err("a quote is never closed".into());
    }
    items.push(current);
    for item in &items {
        let item = item.trim();
        let (name, quoted) = match item.chars().next() {
            None => return Err("a font name is empty".into()),
            Some(q @ ('"' | '\'')) => (
                item.strip_suffix(q)
                    .and_then(|s| s.strip_prefix(q))
                    .filter(|s| !s.is_empty())
                    .ok_or("a quoted font name holds no quotes or backslashes")?,
                true,
            ),
            Some(_) => (item, false),
        };
        if name.contains("  ") {
            return Err(format!("font name `{name}` has two spaces in a row"));
        }
        if quoted {
            if !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '.'))
            {
                return Err(format!(
                    "quoted font name `{name}` may hold only ASCII letters, digits, spaces, hyphens and dots"
                ));
            }
        } else {
            for word in name.split(' ') {
                let starts = word.starts_with(|c: char| c.is_ascii_alphabetic());
                let plain = word.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
                if !starts || !plain {
                    return Err(format!(
                        "unquoted font name `{name}`: each word must start with a letter and hold only ASCII letters, digits and hyphens (quote the name to use a dot)"
                    ));
                }
            }
        }
        let lower = name.to_ascii_lowercase();
        if ["inherit", "initial", "unset", "revert", "revert-layer"].contains(&lower.as_str()) {
            return Err(format!("`{name}` is a CSS-wide keyword, not a font name"));
        }
    }
    Ok(())
}

// ---- contrast floors ----

/// The first `:root` block of a stylesheet as (property, value) pairs. It is for
/// the built-in files, which are well formed; it is not the shared-theme check.
fn root_values(css: &str) -> Vec<(String, String)> {
    let Some(start) = css.find(":root") else {
        return Vec::new();
    };
    let body = &css[start..];
    let Some(open) = body.find('{') else {
        return Vec::new();
    };
    let Some(close) = body[open..].find('}') else {
        return Vec::new();
    };
    let inner = &body[open + 1..open + close];
    let inner = strip_comments(inner).unwrap_or_default();
    inner
        .split(';')
        .filter_map(|d| {
            let (k, v) = d.split_once(':')?;
            let k = k.trim();
            k.starts_with("--hp-")
                .then(|| (k.to_string(), v.trim().to_string()))
        })
        .collect()
}

fn check_contrast(blocks: &[Block]) -> Result<(), CssError> {
    for scheme in [Scheme::Top, Scheme::Light, Scheme::Dark] {
        if scheme != Scheme::Top && !blocks.iter().any(|b| b.scheme == scheme) {
            continue;
        }
        let mut values: BTreeMap<String, (String, usize)> = root_values(BASE_CSS)
            .into_iter()
            .map(|(k, v)| (k, (v, 0)))
            .collect();
        // Blocks apply in file order, as a browser applies them: a later
        // declaration wins, whether it is top-level or in this scheme's block.
        for block in blocks
            .iter()
            .filter(|b| b.scheme == Scheme::Top || b.scheme == scheme)
        {
            for d in &block.decls {
                values.insert(d.name.clone(), (d.value.clone(), d.line));
            }
        }
        let place = match scheme {
            Scheme::Top => "in the top-level block",
            Scheme::Light => "in the light scheme",
            Scheme::Dark => "in the dark scheme",
        };
        let floors = [(TEXT_PAIRS, TEXT_FLOOR), (CONTROL_PAIRS, CONTROL_FLOOR)];
        for (pairs, floor) in floors {
            for (fg, bg) in pairs {
                let (Some((fv, fl)), Some((bv, bl))) = (values.get(*fg), values.get(*bg)) else {
                    continue;
                };
                let Some(ratio) = contrast(fv, bv) else {
                    continue;
                };
                if ratio < floor {
                    let line = if *fl > 0 { *fl } else { *bl };
                    return Err(CssError::new(
                        line,
                        format!(
                            "{fg} on {bg} has contrast {ratio:.2}:1 {place}; the floor is {floor}:1"
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(values: &[(String, String)], name: &str) -> Option<String> {
        values
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    }

    fn refused(text: &str) -> CssError {
        check_theme_pack_css(text).expect_err("the theme should be refused")
    }

    #[test]
    fn every_class_is_a_selector_in_the_base_stylesheet() {
        for class in CLASSES {
            let selector = format!(".{class}");
            let found = BASE_CSS.match_indices(&selector).any(|(i, _)| {
                // `.hp-vote` must not be satisfied by `.hp-vote-button` alone.
                let after = BASE_CSS[i + selector.len()..].chars().next();
                !after.is_some_and(|c| c.is_ascii_alphanumeric() || c == '-')
            });
            assert!(found, "{selector} is not a selector in base.css");
        }
    }

    #[test]
    fn every_property_the_base_stylesheet_reads_is_in_the_contract() {
        let mut rest = BASE_CSS;
        let mut count = 0;
        while let Some(i) = rest.find("var(") {
            rest = &rest[i + 4..];
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            assert!(
                PROPERTIES.contains(&name.as_str()),
                "{name} is not in PROPERTIES"
            );
            count += 1;
        }
        assert!(count > 20);
    }

    #[test]
    fn the_base_stylesheet_sets_every_property() {
        let root = root_values(BASE_CSS);
        for p in PROPERTIES {
            assert!(
                lookup(&root, p).is_some(),
                "base.css :root does not set {p}"
            );
        }
    }

    #[test]
    fn the_base_stylesheet_names_the_contract_and_draws_no_text() {
        assert!(BASE_CSS.starts_with("/* hippo web theme contract 1 */"));
        assert_eq!(CONTRACT_VERSION, 1);
        assert!(!BASE_CSS.contains("content:"));
        assert!(!BASE_CSS.contains("!important"));
    }

    #[test]
    fn no_colour_is_hard_coded_outside_the_root_block() {
        let end = BASE_CSS.find('}').expect("a :root block");
        let after = &BASE_CSS[end..];
        assert!(
            !after.contains('#'),
            "a colour literal after the :root block"
        );
        assert!(!after.contains("rgb"));
    }

    #[test]
    fn dark_sets_only_contract_properties() {
        let dark = root_values(DARK_CSS);
        assert!(dark.len() > 10);
        for (k, _) in &dark {
            assert!(PROPERTIES.contains(&k.as_str()), "{k} is not in PROPERTIES");
        }
    }

    fn assert_pairs(values: &[(String, String)], fallback: &[(String, String)]) {
        let get = |name: &str| {
            lookup(values, name)
                .or_else(|| lookup(fallback, name))
                .unwrap_or_else(|| panic!("{name} has no value"))
        };
        for (pairs, floor) in [(TEXT_PAIRS, 4.5), (CONTROL_PAIRS, 3.0)] {
            for (fg, bg) in pairs {
                let ratio = contrast(&get(fg), &get(bg)).expect("both are colours");
                assert!(
                    ratio >= floor,
                    "{fg} on {bg} is {ratio:.2}:1, below {floor}:1"
                );
            }
        }
    }

    #[test]
    fn default_meets_the_contrast_floors() {
        assert_pairs(&root_values(BASE_CSS), &[]);
    }

    #[test]
    fn dark_meets_the_contrast_floors() {
        assert_pairs(&root_values(DARK_CSS), &root_values(BASE_CSS));
    }

    #[test]
    fn every_target_has_a_24_pixel_minimum_size() {
        // The selectors are written as they appear in base.css, one rule each.
        for selector in [
            ".hp-vote-button",
            ".hp-link-button",
            ".hp-nav a",
            ".hp-account a",
            ".hp-meta a",
            ".hp-comment-meta a",
            ".hp-footer a",
            ".hp-button",
            ".hp-home",
            ".hp-more",
        ] {
            let found = BASE_CSS.split('}').any(|rule| {
                let Some((head, body)) = rule.split_once('{') else {
                    return false;
                };
                head.split(',').any(|s| s.trim() == selector)
                    && body.contains("min-width: 24px")
                    && body.contains("min-height: 24px")
            });
            assert!(found, "{selector} has no 24px minimum size in base.css");
        }
    }

    #[test]
    fn auto_is_dark_inside_the_media_query() {
        let inner = AUTO_CSS
            .strip_prefix("@media (prefers-color-scheme: dark) {\n")
            .and_then(|s| s.strip_suffix("}\n"))
            .expect("the media wrapper");
        assert_eq!(inner, DARK_CSS);
    }

    #[test]
    fn the_mark_is_drawn_in_current_colour() {
        assert!(MARK_SVG.starts_with("<svg"));
        assert!(MARK_SVG.contains("currentColor"));
    }

    #[test]
    fn the_page_names_are_unique_and_plain() {
        for page in PAGES {
            assert!(page.chars().all(|c| c.is_ascii_lowercase() || c == '-'));
        }
        assert!(PAGES.contains(&"registry-index"));
        assert!(PAGES.contains(&"registry-themes"));
        assert!(PAGES.contains(&"registry-pack"));
        let mut sorted: Vec<_> = PAGES.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), PAGES.len());
    }

    #[test]
    fn personal_css_accepts_tabs_and_newlines() {
        assert!(check_personal_css("a {\n\tcolor: red;\r\n}\n").is_ok());
        assert!(check_personal_css("").is_ok());
    }

    #[test]
    fn personal_css_refuses_control_characters() {
        let e = check_personal_css("a {}\nb {\0}").unwrap_err();
        assert_eq!(e.line, 2);
        let e = check_personal_css("a {\u{1b}}").unwrap_err();
        assert_eq!(e.line, 1);
    }

    #[test]
    fn personal_css_refuses_one_byte_over_the_limit() {
        assert!(check_personal_css(&"a".repeat(CSS_MAX)).is_ok());
        let e = check_personal_css(&"a".repeat(CSS_MAX + 1)).unwrap_err();
        assert_eq!(e.line, 0);
        assert!(e.to_string().starts_with("theme.css:0: "));
    }

    #[test]
    fn the_built_in_themes_pass_the_shared_rules() {
        for css in [DARK_CSS, AUTO_CSS] {
            assert_eq!(check_theme_pack_css(css), Ok(()));
        }
    }

    #[test]
    fn a_theme_with_both_media_forms_is_accepted() {
        let css = "/* a theme */\n:root {\n  --hp-link: #123;\n  --hp-font: \"Georgia\", Cambria, serif;\n  }\n@media (prefers-color-scheme: light) {\n  :root { --hp-content-bg: #fafafa; }\n}\n@media (prefers-color-scheme: dark) {\n  :root {\n    --hp-page-bg: #000000;\n    --hp-content-bg: #101010;\n    --hp-text: #eeeeee;\n    --hp-muted: #b0b0b0;\n    --hp-link: #eeeeee;\n    --hp-link-visited: #b0b0b0;\n    --hp-error: #ff9a90;\n    --hp-code-bg: #202020;\n    --hp-input-bg: #202020;\n    --hp-input-text: #eeeeee;\n    --hp-input-border: #909090;\n    --hp-vote: #909090;\n    --hp-focus: #a9bdf0;\n    --hp-color-scheme: dark;\n  }\n}\n";
        assert_eq!(check_theme_pack_css(css), Ok(()));
    }

    #[test]
    fn a_theme_with_another_selector_is_refused() {
        let e = refused(":root {\n  --hp-text: #111;\n}\n\n.hp-item {\n  display: none;\n}\n");
        assert_eq!(e.line, 5);
        let e = refused("body { color: red }");
        assert_eq!(e.line, 1);
        let e = refused("@media (prefers-color-scheme: dark) {\n  .hp-item { color: red }\n}\n");
        assert_eq!(e.line, 2);
    }

    #[test]
    fn a_theme_with_another_at_rule_is_refused() {
        let e = refused("\n@import url(https://example.com/a.css);\n");
        assert_eq!(e.line, 2);
        let e = refused(":root { --hp-text: #111; }\n@media (max-width: 600px) {\n  :root { --hp-text: #111; }\n}\n");
        assert_eq!(e.line, 2);
        let e = refused("@font-face { font-family: x; }");
        assert_eq!(e.line, 1);
    }

    #[test]
    fn a_theme_with_an_unknown_property_is_refused() {
        let e = refused(":root {\n  --hp-text: #111;\n  --hp-sparkle: #fff;\n}\n");
        assert_eq!(e.line, 3);
        let e = refused(":root {\n  display: none;\n}\n");
        assert_eq!(e.line, 2);
    }

    #[test]
    fn a_theme_with_a_repeated_property_is_refused() {
        let e = refused(":root {\n  --hp-text: #111;\n  --hp-link: #111;\n  --hp-text: #222;\n}\n");
        assert_eq!(e.line, 4);
    }

    #[test]
    fn a_theme_with_important_is_refused() {
        let e = refused(":root {\n  --hp-text: #111 !important;\n}\n");
        assert_eq!(e.line, 2);
    }

    #[test]
    fn a_theme_with_an_rgb_colour_is_refused() {
        let e = refused(":root {\n\n  --hp-text: rgb(0, 0, 0);\n}\n");
        assert_eq!(e.line, 3);
        let e = refused(":root { --hp-text: red; }");
        assert_eq!(e.line, 1);
        let e = refused(":root { --hp-text: #12345; }");
        assert_eq!(e.line, 1);
    }

    #[test]
    fn a_theme_with_a_size_outside_its_range_is_refused() {
        for bad in [
            "--hp-size-text: 0.4rem",
            "--hp-size-text: 2.5rem",
            "--hp-size-meta: 7px",
            "--hp-size-title: 25pt",
            "--hp-size-title: 1vw",
            "--hp-line: 0.9",
            "--hp-line: 3",
            "--hp-content-width: 40%",
            "--hp-content-width: 2100px",
            "--hp-content-width: 90vw",
            "--hp-indent: 41px",
            "--hp-indent: 80px",
            "--hp-indent: 6rem",
            "--hp-mark-size: 9px",
            "--hp-mark-size: 49px",
            "--hp-color-scheme: sepia",
        ] {
            let e = refused(&format!("\n:root {{\n  {bad};\n}}\n"));
            assert_eq!(e.line, 3, "{bad} should be refused at line 3");
        }
        for good in [
            "--hp-size-text: 0.625rem",
            "--hp-size-text: 2rem",
            "--hp-size-meta: 7.5pt",
            "--hp-size-title: 24pt",
            "--hp-line: 2.5",
            "--hp-content-width: 50%",
            "--hp-content-width: 320px",
            "--hp-indent: 0",
            "--hp-indent: 2.5rem",
            "--hp-mark-size: 48px",
        ] {
            assert_eq!(
                check_theme_pack_css(&format!(":root {{ {good}; }}")),
                Ok(()),
                "{good}"
            );
        }
    }

    #[test]
    fn a_theme_with_any_url_is_refused() {
        let e = refused(
            ":root {\n  --hp-text: #111;\n  --hp-font: url(https://example.com/a.woff);\n}\n",
        );
        assert_eq!(e.line, 3);
        let e = refused(":root {\n  --hp-page-bg: URL(data:image/png;base64,AAAA);\n}\n");
        assert_eq!(e.line, 2);
        let e = refused(":root {\n  --hp-page-image: url(data:image/png;base64,AAAA);\n}\n");
        assert_eq!(e.line, 2);
    }

    #[test]
    fn a_theme_with_a_bad_font_list_is_refused() {
        for bad in [
            "--hp-font: \"Georgia\\\", serif",
            "--hp-font: Georgia; color",
            "--hp-font: url(x)",
            "--hp-font: serif,",
            "--hp-font: \"Open Sans, serif",
        ] {
            assert!(
                check_theme_pack_css(&format!(":root {{ {bad}; }}")).is_err(),
                "{bad}"
            );
        }
        let long = format!(":root {{ --hp-font: {}; }}", "a".repeat(201));
        assert!(check_theme_pack_css(&long).is_err());
        assert_eq!(
            check_theme_pack_css(
                ":root { --hp-font: \"Fira Sans 2.0\", 'Noto Sans', Helvetica Neue, sans-serif; }"
            ),
            Ok(())
        );
    }

    #[test]
    fn a_theme_whose_muted_colour_is_too_pale_in_one_scheme_is_refused() {
        let css = ":root {\n  --hp-muted: #555555;\n}\n@media (prefers-color-scheme: dark) {\n  :root {\n    --hp-muted: #eeeeee;\n  }\n}\n";
        let e = refused(css);
        assert_eq!(e.line, 6);
        assert!(e.what.contains("--hp-muted"), "{}", e.what);
        assert!(e.what.contains("dark"), "{}", e.what);
        assert!(e.to_string().starts_with("theme.css:6: "));
    }

    #[test]
    fn a_theme_that_lowers_a_control_colour_is_refused() {
        let e = refused(":root {\n  --hp-vote: #e0e0e0;\n}\n");
        assert_eq!(e.line, 2);
    }

    #[test]
    fn an_unclosed_comment_or_block_is_refused() {
        assert_eq!(refused("\n/* never ends").line, 2);
        assert_eq!(refused("\n:root {\n  --hp-text: #111;\n").line, 2);
    }

    #[test]
    fn numbers_take_digits_with_an_optional_fraction_and_nothing_else() {
        for ok in [
            "--hp-line: 1",
            "--hp-line: 1.5",
            "--hp-line: 2.50",
            "--hp-size-text: .75rem",
        ] {
            assert_eq!(
                check_theme_pack_css(&format!(":root {{ {ok}; }}")),
                Ok(()),
                "{ok}"
            );
        }
        for bad in [
            "--hp-line: +1.5",
            "--hp-line: -1",
            "--hp-line: 1e0",
            "--hp-line: 1.",
            "--hp-line: .",
            "--hp-size-text: 1.e1rem",
            "--hp-size-text: 12",
        ] {
            assert!(
                check_theme_pack_css(&format!(":root {{ {bad}; }}")).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn em_and_rem_count_as_16_pixels_and_pt_as_four_thirds() {
        for ok in [
            "0.625em", "0.625rem", "10px", "7.5pt", "2em", "2rem", "32px", "24pt",
        ] {
            assert_eq!(
                check_theme_pack_css(&format!(":root {{ --hp-size-meta: {ok}; }}")),
                Ok(()),
                "{ok}"
            );
        }
        for bad in [
            "0.62em", "9.9px", "9px", "7.4pt", "2.01rem", "32.1px", "24.1pt",
        ] {
            assert!(
                check_theme_pack_css(&format!(":root {{ --hp-size-meta: {bad}; }}")).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn sizes_start_at_10_pixels_and_indents_end_at_40() {
        assert!(check_theme_pack_css(":root { --hp-size-text: 9px; }").is_err());
        assert_eq!(
            check_theme_pack_css(":root { --hp-size-text: 10px; }"),
            Ok(())
        );
        assert!(check_theme_pack_css(":root { --hp-indent: 41px; }").is_err());
        assert_eq!(check_theme_pack_css(":root { --hp-indent: 40px; }"), Ok(()));
    }

    #[test]
    fn an_unquoted_font_word_must_start_with_a_letter() {
        assert!(check_theme_pack_css(":root { --hp-font: 2fun; }").is_err());
        assert!(check_theme_pack_css(":root { --hp-font: Fira 2fun, serif; }").is_err());
        assert!(check_theme_pack_css(":root { --hp-font: -x; }").is_err());
        assert_eq!(
            check_theme_pack_css(":root { --hp-font: \"2fun\", serif; }"),
            Ok(())
        );
    }

    #[test]
    fn indent_takes_a_bare_zero_but_other_properties_do_not() {
        assert_eq!(check_theme_pack_css(":root { --hp-indent: 0; }"), Ok(()));
        assert!(check_theme_pack_css(":root { --hp-mark-size: 0; }").is_err());
        assert!(check_theme_pack_css(":root { --hp-content-width: 0; }").is_err());
        assert!(check_theme_pack_css(":root { --hp-size-text: 0; }").is_err());
        assert!(check_theme_pack_css(":root { --hp-mark-size: 20%; }").is_err());
    }

    #[test]
    fn font_names_are_ascii_words_with_dots_and_the_css_wide_keywords_are_refused() {
        assert_eq!(
            check_theme_pack_css(
                ":root { --hp-font: \"Noto Sans 2.0\", \"Fira-Code.Light\", serif; }"
            ),
            Ok(())
        );
        for bad in [
            "--hp-font: inherit",
            "--hp-font: serif, Initial",
            "--hp-font: \"unset\"",
            "--hp-font: revert",
            "--hp-font: revert-layer",
            "--hp-font: Fira_Code",
            "--hp-font: \"Caf\u{e9}\"",
            "--hp-font: \"A,B\"",
            "--hp-font-mono: Menlo, inherit",
        ] {
            assert!(
                check_theme_pack_css(&format!(":root {{ {bad}; }}")).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn property_names_are_case_sensitive_and_hex_digits_are_not() {
        let e = refused(":root {\n  --HP-TEXT: #111;\n}\n");
        assert_eq!(e.line, 2);
        assert_eq!(
            check_theme_pack_css(
                ":root { --hp-text: #0A0B0C; --hp-link: #1E2A4A; --hp-rule: #D6D6CC; }"
            ),
            Ok(())
        );
    }

    #[test]
    fn the_last_semicolon_is_optional_and_empty_input_is_accepted() {
        assert_eq!(check_theme_pack_css(":root { --hp-text: #111 }"), Ok(()));
        assert_eq!(check_theme_pack_css(":root { --hp-text: #111; }"), Ok(()));
        assert_eq!(check_theme_pack_css(""), Ok(()));
        assert_eq!(check_theme_pack_css("\n  /* only a comment */\n"), Ok(()));
        assert_eq!(check_theme_pack_css(":root {}"), Ok(()));
        assert_eq!(
            check_theme_pack_css("@media (prefers-color-scheme: dark) {}\n@media (prefers-color-scheme: light) { :root {} }"),
            Ok(())
        );
    }

    #[test]
    fn media_blocks_do_not_nest() {
        let css = "@media (prefers-color-scheme: dark) {\n  @media (prefers-color-scheme: light) {\n    :root { --hp-text: #111; }\n  }\n}\n";
        assert_eq!(refused(css).line, 2);
    }

    #[test]
    fn comments_are_removed_outside_quotes_only() {
        assert!(check_theme_pack_css(":root { --hp-font: \"a/*b\"; }").is_err());
        assert_eq!(
            check_theme_pack_css(":root { --hp-text: /* c */ #111 /* d */; }"),
            Ok(())
        );
    }

    #[test]
    fn blocks_apply_in_file_order_so_a_later_top_level_block_beats_a_media_block() {
        // The dark block is fine on its own; the top-level block after it
        // sets a muted colour that is too dark on the dark background, and in
        // a browser it wins there. Line 16 is that declaration.
        let css = "@media (prefers-color-scheme: dark) {\n  :root {\n    --hp-page-bg: #000000;\n    --hp-content-bg: #000000;\n    --hp-code-bg: #000000;\n    --hp-text: #ffffff;\n    --hp-muted: #cccccc;\n    --hp-link: #ffffff;\n    --hp-link-visited: #cccccc;\n    --hp-error: #ff9999;\n    --hp-vote: #cccccc;\n    --hp-focus: #ffffff;\n  }\n}\n:root {\n  --hp-muted: #5f5f5f;\n}\n";
        let e = refused(css);
        assert_eq!(e.line, 16, "{e}");
        assert!(e.what.contains("--hp-muted"), "{}", e.what);
        assert!(e.what.contains("dark"), "{}", e.what);
        // The same two blocks the other way round: the media block wins.
        let css = ":root {\n  --hp-muted: #5f5f5f;\n}\n@media (prefers-color-scheme: dark) {\n  :root {\n    --hp-page-bg: #000000;\n    --hp-content-bg: #000000;\n    --hp-code-bg: #000000;\n    --hp-text: #ffffff;\n    --hp-muted: #cccccc;\n    --hp-link: #ffffff;\n    --hp-link-visited: #cccccc;\n    --hp-error: #ff9999;\n    --hp-vote: #cccccc;\n    --hp-focus: #ffffff;\n  }\n}\n";
        assert_eq!(check_theme_pack_css(css), Ok(()));
    }

    #[test]
    fn a_contrast_failure_names_the_background_line_when_the_foreground_is_unset() {
        let e = refused(
            ":root {\n  --hp-rule: #cccccc;\n}\n:root {\n\n  --hp-content-bg: #101010;\n}\n",
        );
        assert_eq!(e.line, 6);
        assert!(e.what.contains("--hp-content-bg"), "{}", e.what);
    }

    #[test]
    fn pages_match_the_page_name_table() {
        let table = [
            "front",
            "new",
            "past",
            "comments",
            "ask",
            "show",
            "packs",
            "evidence",
            "themes",
            "post",
            "submit",
            "edit",
            "reply",
            "measure",
            "report",
            "settings",
            "hidden",
            "login",
            "moderation",
            "mod",
            "admin",
            "about",
            "error",
            "registry-index",
            "registry-themes",
            "registry-pack",
            "docs",
        ];
        assert_eq!(PAGES, &table);
    }

    #[test]
    fn the_base_stylesheet_has_no_content_declaration() {
        for line in BASE_CSS.lines() {
            let t = line.trim_start();
            assert!(!t.starts_with("content"), "a content declaration: {line}");
            assert!(!t.contains("{ content"), "a content declaration: {line}");
        }
    }

    #[test]
    fn mark_svg_has_the_class_and_hides_from_screen_readers() {
        let svg = mark_svg("hp-mark");
        assert!(svg.starts_with("<svg "));
        assert!(svg.contains(" class=\"hp-mark\""));
        assert!(svg.contains(" aria-hidden=\"true\""));
        assert!(!svg.contains("role="));
        assert!(!svg.contains("aria-label"));
        assert!(svg.contains("currentColor"));
        assert!(mark_svg("a\" onload=\"x").contains("class=\"a&quot; onload=&quot;x\""));
        assert!(MARK_SVG.contains("role=\"img\""));
    }

    #[test]
    fn the_icon_is_the_app_icon() {
        assert!(ICON_SVG.starts_with("<svg"));
        assert!(ICON_SVG.contains("#1E2A4A"));
    }

    #[test]
    fn personal_css_refuses_del_and_a_c1_character_with_their_lines() {
        assert_eq!(
            check_personal_css("a {}\n\nb {\u{7f}}").unwrap_err().line,
            3
        );
        assert_eq!(check_personal_css("a {}\nb {\u{85}}").unwrap_err().line, 2);
        assert_eq!(check_personal_css("\n\n\u{1b}").unwrap_err().line, 3);
    }

    #[test]
    fn upper_case_units_and_a_49_percent_width_are_refused() {
        for bad in [
            "--hp-size-text: 12PX",
            "--hp-size-text: 1REM",
            "--hp-size-text: 12Px",
            "--hp-indent: 10PX",
            "--hp-content-width: 90PX",
            "--hp-content-width: 49%",
            "--hp-mark-size: 20PX",
            "--hp-size-text: 33px",
            "--hp-size-text: 9px",
        ] {
            assert_eq!(
                refused(&format!("\n:root {{\n  {bad};\n}}\n")).line,
                3,
                "{bad}"
            );
        }
    }

    #[test]
    fn unquoted_font_words_start_with_a_letter_and_hold_no_dots() {
        for bad in [
            "--hp-font: Fira.Code",
            "--hp-font: Fira 2",
            "--hp-font: Fira  Sans",
            "--hp-font: \"Fira  Sans\"",
            "--hp-font: \"Fira_Sans\"",
        ] {
            assert!(
                check_theme_pack_css(&format!(":root {{ {bad}; }}")).is_err(),
                "{bad}"
            );
        }
        assert_eq!(
            check_theme_pack_css(
                ":root { --hp-font: Helvetica Neue, Fira-Code2, \"Noto Sans 2.0\"; }"
            ),
            Ok(())
        );
    }

    #[test]
    fn a_201_character_font_list_is_refused_and_200_is_accepted() {
        let list = |n: usize| format!(":root {{ --hp-font: {}; }}", "a".repeat(n));
        assert_eq!(check_theme_pack_css(&list(200)), Ok(()));
        assert!(check_theme_pack_css(&list(201)).is_err());
        assert!(check_theme_pack_css(&format!(
            ":root {{ --hp-font: {}, serif; }}",
            "a".repeat(200)
        ))
        .is_err());
    }

    #[test]
    fn contrast_of_black_on_white_is_21() {
        let ratio = contrast("#000000", "#ffffff").unwrap();
        assert!((ratio - 21.0).abs() < 1e-9);
        assert!((contrast("#ffffff", "#000000").unwrap() - 21.0).abs() < 1e-9);
        assert!((contrast("#fff", "#000").unwrap() - 21.0).abs() < 1e-9);
        assert!((contrast("#777777", "#777777").unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn contrast_refuses_other_forms() {
        assert_eq!(contrast("red", "#ffffff"), None);
        assert_eq!(contrast("#ffffff", "rgb(0,0,0)"), None);
        assert_eq!(contrast("#12345", "#ffffff"), None);
        assert_eq!(contrast("#gggggg", "#ffffff"), None);
    }
}
