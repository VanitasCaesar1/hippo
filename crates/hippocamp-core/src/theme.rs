//! The desktop theme format.
//!
//! A theme file is a theme family in the JSON format of the Zed editor's
//! themes (version 0.2.0) with one optional `hippocamp` object per theme. This
//! module reads it two ways:
//!
//! - [`parse`] is lenient, for a file the user put in `$HIPPOCAMP_HOME/themes/`,
//!   so that a Zed theme works as it is. Small faults become warnings.
//! - [`check_desktop_theme`] is strict, for a theme pack's `theme.json`,
//!   which comes from other people. Anything unknown is an error.
//!
//! Themes are data. Nothing here runs, includes or fetches anything. The one
//! thing it does not do is decode a background image: [`read_background`]
//! checks its path, its kind and its size from the file's header.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::records::Listed;

/// The version of the strict format that a pack declares as
/// `desktop_theme_format` in its `pack.toml`.
pub const DESKTOP_THEME_FORMAT: u32 = 1;

/// A user's theme file larger than this is listed as too large.
const MAX_USER_FILE_BYTES: u64 = 1024 * 1024;
/// At most this many theme files are read from one place.
const MAX_FILES: usize = 500;
/// A background image larger than this is refused.
const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;

/// A colour, `0xRRGGBBAA`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgba(pub u32);

impl Rgba {
    fn channels(self) -> [u8; 4] {
        self.0.to_be_bytes()
    }
}

/// Whether a theme is for a light or a dark window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    Light,
    Dark,
}

/// A colour that is `#rrggbb` or `#rrggbbaa`, in either case. `#rgb` is not
/// a colour.
pub fn parse_colour(text: &str) -> Option<Rgba> {
    let hex = text.strip_prefix('#')?;
    if !hex.is_ascii() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    match hex.len() {
        6 => u32::from_str_radix(hex, 16)
            .ok()
            .map(|v| Rgba(v << 8 | 0xff)),
        8 => u32::from_str_radix(hex, 16).ok().map(Rgba),
        _ => None,
    }
}

/// Every colour the app draws with, one variant per field of [`Colors`].
macro_rules! colors {
    ($($variant:ident => $field:ident),* $(,)?) => {
        /// A field of [`Colors`], named so that [`STYLE_KEYS`] can say which
        /// colours a style key feeds.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum ColorField {
            $( $variant, )*
        }

        impl ColorField {
            /// Every field, in the order of [`Colors`].
            pub const ALL: [ColorField; 28] = [$(ColorField::$variant),*];

            /// The field's name in [`Colors`].
            pub fn name(self) -> &'static str {
                match self {
                    $( ColorField::$variant => stringify!($field), )*
                }
            }
        }

        /// Every colour the app draws with.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct Colors {
            $( pub $field: Rgba, )*
        }

        impl Colors {
            /// One colour by its field.
            pub fn get(&self, field: ColorField) -> Rgba {
                match field {
                    $( ColorField::$variant => self.$field, )*
                }
            }

            const BLANK: Colors = Colors { $( $field: Rgba(0), )* };

            fn set(&mut self, field: ColorField, value: Rgba) {
                match field {
                    $( ColorField::$variant => self.$field = value, )*
                }
            }
        }
    };
}

colors! {
    Window => window,
    Dock => dock,
    Centre => centre,
    TitleBar => title_bar,
    StatusBar => status_bar,
    TabBar => tab_bar,
    TabActive => tab_active,
    TabInactive => tab_inactive,
    Elevated => elevated,
    Text => text,
    TextMuted => text_muted,
    TextPlaceholder => text_placeholder,
    TextAccent => text_accent,
    Border => border,
    BorderVariant => border_variant,
    Focus => focus,
    SelectedOutline => selected_outline,
    Hover => hover,
    Selected => selected,
    Deleted => deleted,
    DeletedBackground => deleted_background,
    Created => created,
    CreatedBackground => created_background,
    Error => error,
    Warning => warning,
    Success => success,
    SearchMatch => search_match,
    ScrollbarThumb => scrollbar_thumb,
}

use ColorField as F;

/// The one table of the 31 style keys the app reads: each Zed key and the
/// colours it feeds. For one colour, its keys are tried in the order they
/// have here, and the first present colour is used. It drives the lenient
/// mapping and the strict check's list of allowed keys, and a test checks
/// each built-in file against it, so they cannot drift.
///
/// A key feeds more than one colour in two cases: `background` is the window
/// and the centre pane's second choice, `surface.background` the dock's and
/// the pickers' second choice.
pub const STYLE_KEYS: [(&str, &[ColorField]); 31] = [
    ("panel.background", &[F::Dock]),
    ("editor.background", &[F::Centre]),
    ("elevated_surface.background", &[F::Elevated]),
    ("surface.background", &[F::Dock, F::Elevated]),
    ("background", &[F::Window, F::Centre]),
    ("title_bar.background", &[F::TitleBar]),
    ("status_bar.background", &[F::StatusBar]),
    ("tab_bar.background", &[F::TabBar]),
    ("tab.active_background", &[F::TabActive]),
    ("tab.inactive_background", &[F::TabInactive]),
    ("text", &[F::Text]),
    ("text.muted", &[F::TextMuted]),
    ("text.placeholder", &[F::TextPlaceholder]),
    ("text.accent", &[F::TextAccent]),
    ("border", &[F::Border]),
    ("border.variant", &[F::BorderVariant]),
    ("border.focused", &[F::Focus]),
    ("border.selected", &[F::SelectedOutline]),
    ("ghost_element.hover", &[F::Hover]),
    ("element.hover", &[F::Hover]),
    ("ghost_element.selected", &[F::Selected]),
    ("element.selected", &[F::Selected]),
    ("deleted", &[F::Deleted]),
    ("deleted.background", &[F::DeletedBackground]),
    ("created", &[F::Created]),
    ("created.background", &[F::CreatedBackground]),
    ("error", &[F::Error]),
    ("warning", &[F::Warning]),
    ("success", &[F::Success]),
    ("search.match_background", &[F::SearchMatch]),
    ("scrollbar.thumb.background", &[F::ScrollbarThumb]),
];

fn is_style_key(key: &str) -> bool {
    STYLE_KEYS.iter().any(|(k, _)| *k == key)
}

/// The colour of `field`: the first key of [`STYLE_KEYS`] that feeds it and
/// holds a colour.
fn pick(style: &Map<String, Value>, field: ColorField) -> Option<Rgba> {
    STYLE_KEYS
        .iter()
        .filter(|(_, fields)| fields.contains(&field))
        .find_map(|(key, _)| {
            style
                .get(*key)
                .and_then(Value::as_str)
                .and_then(parse_colour)
        })
}

impl Colors {
    fn resolve(
        style: &Map<String, Value>,
        fallback: Option<&Colors>,
        warnings: &mut Vec<String>,
    ) -> Colors {
        let mut colors = fallback.copied().unwrap_or(Colors::BLANK);
        for field in ColorField::ALL {
            match pick(style, field) {
                Some(c) => colors.set(field, c),
                None => {
                    let keys: Vec<&str> = STYLE_KEYS
                        .iter()
                        .filter(|(_, f)| f.contains(&field))
                        .map(|(k, _)| *k)
                        .collect();
                    warnings.push(format!(
                        "no colour for {} (style keys {}): using the built-in theme's",
                        field.name(),
                        keys.join(", ")
                    ));
                }
            }
        }
        colors
    }
}

/// A theme's background image and how strongly it shows.
#[derive(Debug, Clone, PartialEq)]
pub struct Background {
    /// The image's path: the theme file's directory joined with the checked
    /// relative path.
    pub image: PathBuf,
    pub opacity: f32,
}

/// One theme, ready for the app to draw with.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub name: String,
    pub appearance: Appearance,
    pub colors: Colors,
    pub font_family: Option<String>,
    pub font_size: f32,
    pub background: Option<Background>,
    /// Faults that did not stop the theme loading, for the theme picker.
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------------
// Built-in themes
// ---------------------------------------------------------------------

const LIGHT_JSON: &str = include_str!("../themes/hippocamp-light.json");
const DARK_JSON: &str = include_str!("../themes/hippocamp-dark.json");

/// Hippocamp Light and Hippocamp Dark, in that order.
pub fn builtin() -> [Theme; 2] {
    static CACHE: OnceLock<[Theme; 2]> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let load = |text: &str| {
                let file: DesktopThemeFile =
                    serde_json::from_str(text).expect("built-in theme file is valid");
                file.themes_with(false).remove(0)
            };
            [load(LIGHT_JSON), load(DARK_JSON)]
        })
        .clone()
}

fn builtin_for(appearance: Appearance) -> Theme {
    let [light, dark] = builtin();
    match appearance {
        Appearance::Light => light,
        Appearance::Dark => dark,
    }
}

// ---------------------------------------------------------------------
// Building a theme from a style map and a hippocamp object
// ---------------------------------------------------------------------

struct HippocampSettings {
    font_family: Option<String>,
    font_size: f32,
    background: Option<Background>,
    warnings: Vec<String>,
}

impl HippocampSettings {
    fn none() -> Self {
        HippocampSettings {
            font_family: None,
            font_size: 13.0,
            background: None,
            warnings: Vec::new(),
        }
    }
}

fn make_theme(
    name: &str,
    appearance: Appearance,
    style: &Map<String, Value>,
    hippocamp: HippocampSettings,
    mut warnings: Vec<String>,
    with_fallback: bool,
) -> Theme {
    for (key, _) in STYLE_KEYS {
        if let Some(v) = style.get(key) {
            if v.as_str().and_then(parse_colour).is_none() {
                warnings.push(format!(
                    "style `{key}` is not a colour (#rrggbb or #rrggbbaa): treated as missing"
                ));
            }
        }
    }
    let fallback = with_fallback.then(|| builtin_for(appearance).colors);
    let colors = Colors::resolve(style, fallback.as_ref(), &mut warnings);
    warnings.extend(hippocamp.warnings);
    Theme {
        name: name.to_string(),
        appearance,
        colors,
        font_family: hippocamp.font_family,
        font_size: hippocamp.font_size,
        background: hippocamp.background,
        warnings,
    }
}

// ---------------------------------------------------------------------
// The lenient reading
// ---------------------------------------------------------------------

fn file_label(file: &Path) -> String {
    file.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| file.display().to_string())
}

/// The themes of a user's theme file, read leniently. The error names the
/// file and the first fault that makes it unusable.
pub fn parse(bytes: &[u8], file: &Path) -> std::result::Result<Vec<Theme>, String> {
    let label = file_label(file);
    let value: Value = serde_json::from_slice(bytes).map_err(|e| {
        format!("{label}: not valid JSON (comments and trailing commas are errors): {e}")
    })?;
    let root = value
        .as_object()
        .ok_or_else(|| format!("{label}: the top level is not an object"))?;
    let themes = match root.get("themes") {
        Some(Value::Array(a)) if !a.is_empty() => a,
        Some(Value::Array(_)) => return Err(format!("{label}: `themes` is empty")),
        Some(_) => return Err(format!("{label}: `themes` is not an array")),
        None => return Err(format!("{label}: `themes` is missing")),
    };
    let mut family_warnings = Vec::new();
    if let Some(Value::String(schema)) = root.get("$schema") {
        if !schema.contains("0.2.0") {
            family_warnings.push(format!(
                "`$schema` names a version other than 0.2.0 ({}): the file may use keys this reader does not know",
                truncate(schema, 80)
            ));
        }
    }
    let dir = file.parent().unwrap_or_else(|| Path::new("."));
    let mut out = Vec::new();
    for (i, t) in themes.iter().enumerate() {
        let obj = t
            .as_object()
            .ok_or_else(|| format!("{label}: themes[{i}] is not an object"))?;
        let name = match obj.get("name") {
            Some(Value::String(s)) if !s.trim().is_empty() => s.as_str(),
            _ => {
                return Err(format!(
                    "{label}: themes[{i}] has no `name` (a non-empty string)"
                ))
            }
        };
        let appearance = match obj.get("appearance").and_then(Value::as_str) {
            Some("light") => Appearance::Light,
            Some("dark") => Appearance::Dark,
            _ => {
                return Err(format!(
                    "{label}: theme `{name}` has an `appearance` other than \"light\" or \"dark\""
                ))
            }
        };
        let mut warnings = family_warnings.clone();
        let empty = Map::new();
        let style = match obj.get("style") {
            Some(Value::Object(m)) => m,
            Some(_) => {
                warnings.push("`style` is not an object: no style keys read".into());
                &empty
            }
            None => {
                warnings.push("no `style`: no style keys read".into());
                &empty
            }
        };
        let hippocamp = match obj.get("hippocamp") {
            Some(Value::Object(m)) => lenient_hippocamp(m, dir),
            Some(_) => {
                let mut h = HippocampSettings::none();
                h.warnings
                    .push("`hippocamp` is not an object: ignored".into());
                h
            }
            None => HippocampSettings::none(),
        };
        out.push(make_theme(
            name, appearance, style, hippocamp, warnings, true,
        ));
    }
    Ok(out)
}

fn lenient_hippocamp(obj: &Map<String, Value>, dir: &Path) -> HippocampSettings {
    let mut h = HippocampSettings::none();
    if let Some(v) = obj.get("font_family") {
        match v.as_str() {
            Some(s) if !s.trim().is_empty() => h.font_family = Some(s.to_string()),
            _ => h
                .warnings
                .push("`hippocamp.font_family` is not a non-empty string: ignored".into()),
        }
    }
    if let Some(v) = obj.get("font_size") {
        match v.as_f64() {
            Some(n) if (10.0..=24.0).contains(&n) => h.font_size = n as f32,
            _ => h
                .warnings
                .push("`hippocamp.font_size` is not a number from 10 to 24: ignored".into()),
        }
    }
    if let Some(v) = obj.get("background") {
        match v.as_object() {
            None => h
                .warnings
                .push("`hippocamp.background` is not an object: ignored".into()),
            Some(b) => {
                let opacity = match b.get("opacity") {
                    None => Some(0.15),
                    Some(o) => o.as_f64().filter(|n| (0.0..=1.0).contains(n)),
                };
                match (b.get("image").and_then(Value::as_str), opacity) {
                    (None, _) => h
                        .warnings
                        .push("`hippocamp.background.image` is not a string: background ignored".into()),
                    (_, None) => h.warnings.push(
                        "`hippocamp.background.opacity` is not a number from 0 to 1: background ignored"
                            .into(),
                    ),
                    (Some(rel), Some(opacity)) => match check_background_image(dir, rel) {
                        Ok(image) => {
                            h.background = Some(Background {
                                image,
                                opacity: opacity as f32,
                            })
                        }
                        Err(e) => h
                            .warnings
                            .push(format!("background image `{rel}` refused: {e}")),
                    },
                }
            }
        }
    }
    h
}

/// Check a theme's background image path against the directory of its file.
/// It must be relative, with no `..`, not a URL, no symbolic link on the way,
/// a `.png`, `.jpg` or `.jpeg` file of at most 8 MB. The pixel limit is
/// [`read_background`]'s check. Returns the full path.
pub fn check_background_image(dir: &Path, rel: &str) -> std::result::Result<PathBuf, String> {
    if rel.is_empty() {
        return Err("the path is empty".into());
    }
    if let Some(i) = rel.find(':') {
        let scheme = &rel[..i];
        if !scheme.is_empty()
            && scheme
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-' || b == b'.')
        {
            return Err("a URL is not allowed".into());
        }
    }
    let path = Path::new(rel);
    if path.is_absolute() {
        return Err("the path is absolute".into());
    }
    let mut full = dir.to_path_buf();
    for c in path.components() {
        match c {
            Component::ParentDir => return Err("the path has `..`".into()),
            Component::RootDir | Component::Prefix(_) => return Err("the path is absolute".into()),
            Component::CurDir => continue,
            Component::Normal(part) => {
                full.push(part);
                match fs::symlink_metadata(&full) {
                    Ok(m) if m.file_type().is_symlink() => {
                        return Err("a symbolic link is on the path".into())
                    }
                    Ok(_) => {}
                    Err(e) => return Err(format!("cannot be read: {e}")),
                }
            }
        }
    }
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase());
    if !matches!(ext.as_deref(), Some("png" | "jpg" | "jpeg")) {
        return Err("only .png, .jpg and .jpeg files are allowed".into());
    }
    let meta = fs::symlink_metadata(&full).map_err(|e| format!("cannot be read: {e}"))?;
    if !meta.is_file() {
        return Err("not a regular file".into());
    }
    if meta.len() > MAX_IMAGE_BYTES {
        return Err("larger than 8 MB".into());
    }
    Ok(full)
}

// ---------------------------------------------------------------------
// The strict check of a pack's theme.json
// ---------------------------------------------------------------------

/// A pack's `theme.json`: a theme family with no unknown field anywhere.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopThemeFile {
    pub name: String,
    pub author: String,
    pub themes: Vec<DesktopTheme>,
}

/// One theme of a pack's family.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopTheme {
    pub name: String,
    pub appearance: Appearance,
    pub style: DesktopStyle,
    pub hippocamp: Option<DesktopHippocamp>,
}

/// The `hippocamp` object of a pack's theme: a font and a size, no background.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopHippocamp {
    pub font_family: Option<String>,
    pub font_size: Option<f64>,
}

impl Eq for DesktopHippocamp {}

/// The `style` object of a pack's theme: only keys of [`STYLE_KEYS`], each
/// once, each a string (that the check then requires to be a colour).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DesktopStyle(pub BTreeMap<String, String>);

impl<'de> Deserialize<'de> for DesktopStyle {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Walk;
        impl<'de> serde::de::Visitor<'de> for Walk {
            type Value = DesktopStyle;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an object of style keys and colours")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<DesktopStyle, A::Error> {
                use serde::de::Error;
                let mut out = BTreeMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !is_style_key(&key) {
                        return Err(A::Error::custom(format!("unknown style key `{key}`")));
                    }
                    let value: String = map.next_value()?;
                    if out.insert(key.clone(), value).is_some() {
                        return Err(A::Error::custom(format!("duplicate style key `{key}`")));
                    }
                }
                Ok(DesktopStyle(out))
            }
        }
        d.deserialize_map(Walk)
    }
}

impl DesktopStyle {
    fn to_map(&self) -> Map<String, Value> {
        self.0
            .iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect()
    }
}

/// The strict check of a pack's `theme.json`, the one gate that `hippocamp add`,
/// `hippocamp packs update`, the registry and the app share. Typed structures
/// refuse an unknown field at every level (a duplicate key too); colour,
/// length, count and font-family rules run after parsing, then the pack
/// text rules ([`crate::packs::files::check_text`]) on every
/// decoded string.
pub fn check_desktop_theme(text: &str) -> std::result::Result<DesktopThemeFile, String> {
    let file: DesktopThemeFile =
        serde_json::from_str(text).map_err(|e| format!("theme.json: {e}"))?;
    let bad = |what: String| Err(format!("theme.json: {what}"));
    if file.name.chars().count() > 64 {
        return bad("`name` is longer than 64 characters".into());
    }
    if file.author.chars().count() > 128 {
        return bad("`author` is longer than 128 characters".into());
    }
    if file.themes.is_empty() || file.themes.len() > 16 {
        return bad("`themes` must hold 1 to 16 themes".into());
    }
    for (i, t) in file.themes.iter().enumerate() {
        if t.name.is_empty() || t.name.chars().count() > 64 {
            return bad(format!("themes[{i}].name must be 1 to 64 characters"));
        }
        for (key, v) in &t.style.0 {
            if parse_colour(v).is_none() {
                return bad(format!(
                    "themes[{i}].style.{key}: `{}` is not a colour (#rrggbb or #rrggbbaa)",
                    truncate(v, 40)
                ));
            }
        }
        if let Some(h) = &t.hippocamp {
            if let Some(f) = &h.font_family {
                let ok = !f.is_empty()
                    && f.chars().count() <= 64
                    && f.chars()
                        .all(|c| c.is_alphanumeric() || c == ' ' || c == '-');
                if !ok {
                    return bad(format!(
                        "themes[{i}].hippocamp.font_family must be 1 to 64 letters, digits, spaces and hyphens"
                    ));
                }
            }
            if let Some(n) = h.font_size {
                if !(10.0..=24.0).contains(&n) {
                    return bad(format!(
                        "themes[{i}].hippocamp.font_size must be from 10 to 24"
                    ));
                }
            }
        }
    }
    // The text rules run on the decoded strings, so an escape such as
    // `\u202e` is seen as the character it is.
    for (label, value) in file.decoded_strings() {
        crate::packs::files::check_text(&label, value.as_bytes())?;
    }
    Ok(file)
}

impl DesktopThemeFile {
    /// Every decoded key and value a pack theme holds with its place, such as
    /// `theme.json: themes[0].style.panel.background`, for the text rules. A
    /// style key is checked under the place of its value.
    pub fn decoded_strings(&self) -> Vec<(String, &str)> {
        let mut out = vec![
            ("theme.json: name".to_string(), self.name.as_str()),
            ("theme.json: author".to_string(), self.author.as_str()),
        ];
        for (i, t) in self.themes.iter().enumerate() {
            out.push((format!("theme.json: themes[{i}].name"), t.name.as_str()));
            for (key, v) in &t.style.0 {
                let place = format!("theme.json: themes[{i}].style.{key}");
                out.push((place.clone(), key.as_str()));
                out.push((place, v.as_str()));
            }
            if let Some(f) = t.hippocamp.as_ref().and_then(|h| h.font_family.as_deref()) {
                out.push((format!("theme.json: themes[{i}].hippocamp.font_family"), f));
            }
        }
        out
    }

    /// The family's themes, with the same fallbacks as a user's file.
    pub fn themes(&self) -> Vec<Theme> {
        self.themes_with(true)
    }

    fn themes_with(&self, fallback: bool) -> Vec<Theme> {
        self.themes
            .iter()
            .map(|t| {
                let mut h = HippocampSettings::none();
                if let Some(d) = &t.hippocamp {
                    h.font_family = d.font_family.clone();
                    if let Some(n) = d.font_size {
                        h.font_size = n as f32;
                    }
                }
                make_theme(
                    &t.name,
                    t.appearance,
                    &t.style.to_map(),
                    h,
                    Vec::new(),
                    fallback,
                )
            })
            .collect()
    }
}

// ---------------------------------------------------------------------
// Reading files safely
// ---------------------------------------------------------------------

#[cfg(unix)]
fn open_no_follow(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
}

#[cfg(not(unix))]
fn open_no_follow(path: &Path) -> std::io::Result<fs::File> {
    fs::File::open(path)
}

/// The one reader of theme files: opened without following a symbolic link
/// and without blocking (so a named pipe cannot stall the caller), checked
/// with `fstat` on the open descriptor to be a regular file, and read up to
/// `limit` bytes and one more.
fn read_theme_file(path: &Path, limit: u64) -> std::result::Result<Vec<u8>, String> {
    let file = open_no_follow(path).map_err(|e| {
        if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
            "is a symbolic link: refused".to_string()
        } else {
            format!("cannot be opened: {e}")
        }
    })?;
    let meta = file
        .metadata()
        .map_err(|e| format!("cannot be read: {e}"))?;
    if !meta.is_file() {
        return Err("is not a regular file".into());
    }
    let mut buf = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("cannot be read: {e}"))?;
    if buf.len() as u64 > limit {
        return Err(format!("is too large (over {limit} bytes)"));
    }
    Ok(buf)
}

/// `text` cut to `max` characters, for error messages.
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let head: String = text.chars().take(max).collect();
        format!("{head}...")
    }
}

fn is_hidden(name: &std::ffi::OsStr) -> bool {
    name.to_string_lossy().starts_with('.')
}

/// The `*.json` files directly in `dir`, hidden ones skipped, in path order.
fn user_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| !is_hidden(&e.file_name()))
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    files
}

/// Each pack's `theme.json` path in pack-name order, and whether the pack
/// directory is a symbolic link. A pack with no `theme.json` is not a theme
/// pack and is skipped.
fn pack_files(dir: &Path) -> Vec<(PathBuf, bool)> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| !is_hidden(&e.file_name()))
        .map(|e| e.path())
        .collect();
    dirs.sort();
    let mut out = Vec::new();
    for d in dirs {
        let Ok(meta) = fs::symlink_metadata(&d) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            out.push((d.join("theme.json"), true));
        } else if meta.is_dir() && fs::symlink_metadata(d.join("theme.json")).is_ok() {
            out.push((d.join("theme.json"), false));
        }
    }
    out
}

fn too_many(label: &str) -> String {
    format!("{label}: too many theme files (the limit is {MAX_FILES})")
}

/// Which directory's rules a single file is read by: a user's theme file
/// (lenient, at most 1 MB) or a pack's `theme.json` (strict, with the pack's
/// `pack.toml` format check).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    User,
    Pack,
}

fn read_user(path: &Path) -> std::result::Result<Vec<Theme>, String> {
    let label = file_label(path);
    read_theme_file(path, MAX_USER_FILE_BYTES)
        .map_err(|e| format!("{label}: {e}"))
        .and_then(|bytes| parse(&bytes, path))
}

/// What a pack's `pack.toml` says about desktop themes: `None` when the pack
/// carries no desktop theme (no `pack.toml`, or no `desktop_theme_format`
/// key), an error when the file cannot be read or names another format.
fn pack_theme_format(pack: &str, dir: &Path) -> std::result::Result<Option<u32>, String> {
    let toml_path = dir.join("pack.toml");
    if fs::symlink_metadata(&toml_path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
        return Ok(None);
    }
    let label = format!("{pack}/pack.toml");
    let bytes = read_theme_file(&toml_path, crate::packs::MAX_FILE_BYTES)
        .map_err(|e| format!("{label}: {e}"))?;
    let text = String::from_utf8(bytes).map_err(|_| format!("{label}: is not UTF-8 text"))?;
    let table: toml::Table = text
        .parse()
        .map_err(|e| format!("{label}: not valid TOML: {e}"))?;
    match table.get("desktop_theme_format") {
        None => Ok(None),
        Some(toml::Value::Integer(n)) if *n == i64::from(DESKTOP_THEME_FORMAT) => {
            Ok(Some(DESKTOP_THEME_FORMAT))
        }
        Some(toml::Value::Integer(n)) => Err(format!(
            "{pack}: made for desktop theme format {n}; this Hippocamp reads {DESKTOP_THEME_FORMAT}"
        )),
        Some(_) => Err(format!(
            "{label}: `desktop_theme_format` is not a whole number"
        )),
    }
}

/// One pack's `theme.json` (`path`), or `None` when the pack carries no
/// desktop theme. The pack directory must not be a symbolic link.
fn read_pack(path: &Path, linked: bool) -> Option<std::result::Result<Vec<Theme>, String>> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let pack = file_label(dir);
    let label = format!("{pack}/theme.json");
    if linked {
        return Some(Err(format!(
            "{pack}: the pack directory is a symbolic link: refused"
        )));
    }
    match pack_theme_format(&pack, dir) {
        Ok(Some(_)) => {}
        Ok(None) => return None,
        Err(e) => return Some(Err(e)),
    }
    Some(
        read_theme_file(path, crate::packs::MAX_FILE_BYTES)
            .map_err(|e| format!("{label}: {e}"))
            .and_then(|bytes| {
                String::from_utf8(bytes).map_err(|_| format!("{label}: is not UTF-8 text"))
            })
            .and_then(|text| {
                check_desktop_theme(&text)
                    .map(|f| f.themes())
                    .map_err(|e| format!("{pack}/{e}"))
            }),
    )
}

/// The themes of each `*.json` file directly in `themes_dir`, in path order,
/// read leniently. Hidden entries are skipped; a symbolic link, a file over
/// 1 MB and a file past the 500th are listed with an error; a missing
/// directory gives nothing.
pub fn scan(themes_dir: &Path) -> Vec<Listed<Vec<Theme>>> {
    user_files(themes_dir)
        .into_iter()
        .enumerate()
        .map(|(i, path)| {
            let record = if i >= MAX_FILES {
                Err(too_many(&file_label(&path)))
            } else {
                read_user(&path)
            };
            Listed { path, record }
        })
        .collect()
}

/// The themes of each `<pack>/theme.json` in `packs_dir`, in pack-name order,
/// through [`check_desktop_theme`]. The same reader and file count as
/// [`scan`], with the pack per-file size limit; a pack directory that is
/// a symbolic link is refused. A pack whose `pack.toml` is missing or has no
/// `desktop_theme_format` carries no desktop theme and is not listed; one made
/// for another format is listed with an error. A listed path is always the
/// pack's `theme.json`.
pub fn scan_packs(packs_dir: &Path) -> Vec<Listed<Vec<Theme>>> {
    let mut out = Vec::new();
    for (path, linked) in pack_files(packs_dir) {
        let Some(record) = read_pack(&path, linked) else {
            continue;
        };
        let record = if out.len() >= MAX_FILES {
            let pack = path.parent().map(file_label).unwrap_or_default();
            Err(too_many(&format!("{pack}/theme.json")))
        } else {
            record
        };
        out.push(Listed { path, record });
    }
    out
}

/// One theme file read again, with the reader, limits and checks of [`scan`]
/// (`Source::User`) or [`scan_packs`] (`Source::Pack`, where `path` is the
/// pack's `theme.json`). A pack without a desktop theme is an error here, as
/// the caller named it.
pub fn load_file(path: &Path, source: Source) -> Listed<Vec<Theme>> {
    let record = match source {
        Source::User => read_user(path),
        Source::Pack => {
            let linked = path
                .parent()
                .and_then(|d| fs::symlink_metadata(d).ok())
                .is_some_and(|m| m.file_type().is_symlink());
            read_pack(path, linked).unwrap_or_else(|| {
                let pack = path.parent().map(file_label).unwrap_or_default();
                Err(format!("{pack}: carries no desktop theme"))
            })
        }
    };
    Listed {
        path: path.to_path_buf(),
        record,
    }
}

// ---------------------------------------------------------------------
// The background image
// ---------------------------------------------------------------------

/// A background image's format, sniffed from its first bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    Png,
    Jpeg,
}

/// A checked background image: its bytes as read and its size in pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundImage {
    pub kind: ImageKind,
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// The largest background image side, in pixels.
const MAX_IMAGE_SIDE: u32 = 4096;

/// The width and height from a PNG's `IHDR` chunk, which must come first.
fn png_size(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 24 || &b[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(b[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(b[20..24].try_into().ok()?);
    Some((w, h))
}

/// The width and height from a JPEG's first start-of-frame marker (SOF0 to
/// SOF15 except `0xC4`, `0xC8` and `0xCC`), found by walking the marker
/// segments from the start.
fn jpeg_size(b: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2;
    loop {
        if *b.get(i)? != 0xff {
            return None;
        }
        // Any number of 0xFF fill bytes may precede a marker.
        while *b.get(i)? == 0xff {
            i += 1;
        }
        let marker = *b.get(i)?;
        i += 1;
        match marker {
            // Markers with no length: padding, SOI, TEM and RST0 to RST7.
            0x00 | 0x01 | 0xd0..=0xd8 => continue,
            // The image ended, or its data began, with no frame header.
            0xd9 | 0xda => return None,
            _ => {}
        }
        let len = usize::from(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]));
        if len < 2 {
            return None;
        }
        if matches!(marker, 0xc0..=0xcf) && !matches!(marker, 0xc4 | 0xc8 | 0xcc) {
            let h = u16::from_be_bytes([*b.get(i + 3)?, *b.get(i + 4)?]);
            let w = u16::from_be_bytes([*b.get(i + 5)?, *b.get(i + 6)?]);
            return Some((u32::from(w), u32::from(h)));
        }
        i += len;
    }
}

/// A theme's background image, read and checked: the path rules of
/// [`check_background_image`], the file opened by the one safe reader (no
/// symbolic link, no blocking, regular file, at most 8 MB), its kind taken
/// from its first bytes whatever its extension says, and its size from the
/// PNG header or the first JPEG frame header, refused above 4096 by 4096. The
/// image is not decoded, and no image library is used.
pub fn read_background(
    theme_file: &Path,
    image: &str,
) -> std::result::Result<BackgroundImage, String> {
    let dir = theme_file.parent().unwrap_or_else(|| Path::new("."));
    let full = check_background_image(dir, image)?;
    let bytes = read_theme_file(&full, MAX_IMAGE_BYTES).map_err(|e| format!("the image {e}"))?;
    let (kind, size) = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        (ImageKind::Png, png_size(&bytes))
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        (ImageKind::Jpeg, jpeg_size(&bytes))
    } else {
        return Err("not a PNG or JPEG image (judged by its first bytes, not its name)".into());
    };
    let (width, height) = size.ok_or_else(|| "the image's size cannot be read".to_string())?;
    if width == 0 || height == 0 {
        return Err("the image has no pixels".into());
    }
    if width > MAX_IMAGE_SIDE || height > MAX_IMAGE_SIDE {
        return Err(format!(
            "{width} by {height} pixels is larger than {MAX_IMAGE_SIDE} by {MAX_IMAGE_SIDE}"
        ));
    }
    Ok(BackgroundImage {
        kind,
        bytes,
        width,
        height,
    })
}

// ---------------------------------------------------------------------
// The catalogue
// ---------------------------------------------------------------------

/// One row of the theme picker.
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogEntry {
    /// What the preference stores: `system`, `builtin:<name>`,
    /// `user:<file name>/<name>` or `pack:<pack name>/<name>`.
    pub id: String,
    pub name: String,
    /// Where it comes from, for the picker to show beside the name: `System`,
    /// `Built-in`, the file name, or `Pack <name>`.
    pub source: String,
    /// The theme, or why it cannot be picked. The System entry holds Hippocamp
    /// Light as a stand-in; the app follows the macOS appearance for it.
    pub theme: std::result::Result<Theme, String>,
    /// A name repeated within one file: dimmed and not selectable.
    pub duplicate: bool,
}

/// The picker's list: System, the built-in themes, the user's files in path
/// order, then theme packs in pack-name order. A file that does not load is
/// one entry named after the file, holding its error. Two themes of one name
/// in different files or packs are different themes; a name repeated within
/// one file is a duplicate.
pub fn catalog(user: &[Listed<Vec<Theme>>], packs: &[Listed<Vec<Theme>>]) -> Vec<CatalogEntry> {
    let [light, dark] = builtin();
    let mut out = vec![CatalogEntry {
        id: "system".into(),
        name: "System".into(),
        source: "System".into(),
        theme: Ok(light.clone()),
        duplicate: false,
    }];
    for t in [light, dark] {
        out.push(CatalogEntry {
            id: format!("builtin:{}", t.name),
            name: t.name.clone(),
            source: "Built-in".into(),
            theme: Ok(t),
            duplicate: false,
        });
    }
    for (group, is_pack) in [(user, false), (packs, true)] {
        for listed in group {
            let owner = if is_pack {
                listed.path.parent().map(file_label).unwrap_or_default()
            } else {
                file_label(&listed.path)
            };
            let source = if is_pack {
                format!("Pack {owner}")
            } else {
                owner.clone()
            };
            let id_for =
                |name: &str| format!("{}:{owner}/{name}", if is_pack { "pack" } else { "user" });
            match &listed.record {
                Err(e) => out.push(CatalogEntry {
                    id: id_for(&owner),
                    name: owner.clone(),
                    source,
                    theme: Err(e.clone()),
                    duplicate: false,
                }),
                Ok(themes) => {
                    let mut seen: Vec<&str> = Vec::new();
                    for t in themes {
                        out.push(CatalogEntry {
                            id: id_for(&t.name),
                            name: t.name.clone(),
                            source: source.clone(),
                            theme: Ok(t.clone()),
                            duplicate: seen.contains(&t.name.as_str()),
                        });
                        seen.push(&t.name);
                    }
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------
// Contrast and the keyboard cue
// ---------------------------------------------------------------------

fn over(fg: [u8; 4], bg: [f64; 3]) -> [f64; 3] {
    let a = f64::from(fg[3]) / 255.0;
    let mut out = [0.0; 3];
    for i in 0..3 {
        out[i] = f64::from(fg[i]) * a + bg[i] * (1.0 - a);
    }
    out
}

fn luminance(c: [f64; 3]) -> f64 {
    let lin = |v: f64| {
        let v = v / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
}

/// The WCAG contrast ratio of `a` against `b`. A colour with alpha is
/// composited over `window`, and `window` over opaque white for a light
/// theme or opaque black for a dark one.
pub fn contrast(a: Rgba, b: Rgba, window: Rgba, appearance: Appearance) -> f32 {
    let base = match appearance {
        Appearance::Light => [255.0; 3],
        Appearance::Dark => [0.0; 3],
    };
    let w = over(window.channels(), base);
    let la = luminance(over(a.channels(), w));
    let lb = luminance(over(b.channels(), w));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    ((hi + 0.05) / (lo + 0.05)) as f32
}

/// Keep the keyboard cue visible whatever a theme says: the focus ring and
/// the selection marker against the window, the dock, the centre pane, the
/// pickers and the selected row. Below 3 to 1 on any of them, both take the
/// theme's `text` colour, with a warning naming the ratio. Text contrast is
/// not checked here.
pub fn apply_focus_floor(theme: &mut Theme) {
    let c = theme.colors;
    let surfaces = [
        ("window", c.window),
        ("dock", c.dock),
        ("centre pane", c.centre),
        ("pickers", c.elevated),
        ("selected row", c.selected),
    ];
    let mut warnings = Vec::new();
    for (what, cue) in [
        ("focus ring", c.focus),
        ("selection marker", c.selected_outline),
    ] {
        for (name, surface) in surfaces {
            let r = contrast(cue, surface, c.window, theme.appearance);
            if r < 3.0 {
                warnings.push(format!(
                    "{what} on the {name} is {r:.1} to 1, below 3: the focus ring and selection marker use the text colour"
                ));
            }
        }
    }
    if !warnings.is_empty() {
        theme.colors.focus = c.text;
        theme.colors.selected_outline = c.text;
        theme.warnings.extend(warnings);
    }
}

#[cfg(test)]
mod tests;
