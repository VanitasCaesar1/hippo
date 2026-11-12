//! `pack.toml`: the manifest of a pack.

use serde::Deserialize;

use crate::records::check_protocol;

/// Names a pack may not take.
pub const RESERVED_NAMES: &[&str] = &[
    "core",
    "hippocamp",
    "builtin",
    "local",
    "registry",
    "official",
    "test",
    "tests",
];

/// What a pack is: passes for dreaming, or a theme.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Passes,
    Theme,
}

impl Kind {
    /// The word `pack.toml` and the registry index use.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Passes => "passes",
            Kind::Theme => "theme",
        }
    }
}

/// A parsed `pack.toml`; `protocol` is checked and not kept.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub authors: Vec<String>,
    pub license: String,
    #[serde(default)]
    pub kind: Kind,
    /// The web theme contract `theme.css` is written against; a theme pack
    /// only.
    pub theme_contract: Option<u32>,
    /// The desktop theme format of `theme.json`; a theme pack only.
    pub desktop_theme_format: Option<u32>,
}

/// Whether `s` matches `^[a-z][a-z0-9]*(-[a-z0-9]+)*$`.
pub fn is_pack_name(s: &str) -> bool {
    let mut previous_dash = true; // a leading `-` or digit is not allowed
    let mut first = true;
    for c in s.chars() {
        match c {
            'a'..='z' => {}
            '0'..='9' if !first => {}
            '-' if !first && !previous_dash => {
                previous_dash = true;
                continue;
            }
            _ => return false,
        }
        first = false;
        previous_dash = false;
    }
    !s.is_empty() && !previous_dash
}

fn is_version(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|p| {
            (1..=4).contains(&p.len())
                && p.bytes().all(|b| b.is_ascii_digit())
                && (*p == "0" || !p.starts_with('0'))
        })
}

fn one_line(s: &str, max: usize) -> bool {
    (1..=max).contains(&s.chars().count()) && !s.chars().any(char::is_control)
}

/// Parses `pack.toml` and checks every manifest rule. The error is the
/// TOML parser's, the protocol error, or `<key>: <rule>`; the caller adds
/// the `pack.toml: ` prefix.
pub fn parse(text: &str) -> std::result::Result<Manifest, String> {
    let mut table: toml::Table = toml::from_str(text).map_err(first_line)?;
    check_protocol(table.remove("protocol").map(|found| match found {
        toml::Value::String(text) => text,
        other => other.to_string(),
    }))
    .map_err(|e| e.to_string())?;
    let manifest: Manifest = table.try_into().map_err(first_line)?;

    let name = &manifest.name;
    if !is_pack_name(name) {
        return Err("name: must match ^[a-z][a-z0-9]*(-[a-z0-9]+)*$".to_owned());
    }
    if !(2..=40).contains(&name.len()) {
        return Err("name: must be 2 to 40 bytes".to_owned());
    }
    if RESERVED_NAMES.contains(&name.as_str()) {
        return Err(format!("name: {name} is reserved"));
    }
    if !is_version(&manifest.version) {
        return Err("version: must be MAJOR.MINOR.PATCH, each a number of at most four digits without leading zeros".to_owned());
    }
    let description = &manifest.description;
    if !one_line(description, 160) || description.trim() != description {
        return Err("description: must be one line of 1 to 160 characters with no leading or trailing space".to_owned());
    }
    if manifest.authors.is_empty() || manifest.authors.len() > 5 {
        return Err("authors: needs 1 to 5 names".to_owned());
    }
    if !manifest.authors.iter().all(|a| one_line(a, 80)) {
        return Err("authors: each name must be one line of 1 to 80 characters".to_owned());
    }
    let license = &manifest.license;
    let spdx = (1..=64).contains(&license.len())
        && license
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-'));
    if !spdx {
        return Err("license: must match ^[A-Za-z0-9.+-]{1,64}$".to_owned());
    }
    for (key, value) in [
        ("theme_contract", manifest.theme_contract),
        ("desktop_theme_format", manifest.desktop_theme_format),
    ] {
        match value {
            Some(_) if manifest.kind != Kind::Theme => {
                return Err(format!("{key}: only a theme pack has it"));
            }
            Some(0) => return Err(format!("{key}: must be a whole number from 1")),
            _ => {}
        }
    }
    Ok(manifest)
}

/// The `kind` a manifest text declares, read as loosely as possible so a
/// pack whose manifest has another fault is still checked as the kind it
/// says it is. Anything unreadable is a pass pack.
pub fn peek_kind(text: &str) -> Kind {
    let theme = text
        .parse::<toml::Table>()
        .ok()
        .and_then(|table| {
            table
                .get("kind")
                .and_then(toml::Value::as_str)
                .map(str::to_owned)
        })
        .is_some_and(|kind| kind == "theme");
    if theme {
        Kind::Theme
    } else {
        Kind::Passes
    }
}

/// The reason of a TOML error, without its source excerpt.
fn first_line(err: toml::de::Error) -> String {
    err.message().lines().next().unwrap_or_default().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASS: &str =
        "protocol = \"HIPPOCAMP_V1\"\nname = \"glossary-terms\"\nversion = \"1.0.0\"\n\
        description = \"Adds the terms sessions define to the glossary note.\"\n\
        authors = [\"Ada Lovelace\"]\nlicense = \"MIT\"\n";
    fn swap(text: &str, from: &str, to: &str) -> String {
        assert!(text.contains(from), "{from}");
        text.replace(from, to)
    }

    #[test]
    fn a_valid_manifest_parses() {
        let pass = parse(PASS).unwrap();
        assert_eq!(pass.name, "glossary-terms");
    }

    #[test]
    fn each_broken_rule_is_refused() {
        let bad = |from: &str, to: &str| parse(&swap(PASS, from, to)).unwrap_err();
        assert_eq!(
            bad("protocol = \"HIPPOCAMP_V1\"\n", ""),
            "expected protocol HIPPOCAMP_V1, found none"
        );
        assert_eq!(
            bad("HIPPOCAMP_V1", "HIPPOCAMP_V0"),
            "expected protocol HIPPOCAMP_V1, found HIPPOCAMP_V0"
        );
        assert_eq!(
            bad("name = \"glossary-terms\"", "name = \"core\""),
            "name: core is reserved"
        );
        for name in ["Glossary", "-a", "a-", "a--b", "1a", "a_b", ""] {
            let err = bad("glossary-terms", name);
            assert!(err.starts_with("name: "), "{name}: {err}");
        }
        assert_eq!(bad("glossary-terms", "a"), "name: must be 2 to 40 bytes");
        assert_eq!(
            bad("glossary-terms", &"a".repeat(41)),
            "name: must be 2 to 40 bytes"
        );
        for version in ["1.0", "01.0.0", "1.0.0.0", "10000.0.0", "1.x.0", "v1.0.0"] {
            let err = bad("\"1.0.0\"", &format!("\"{version}\""));
            assert!(err.starts_with("version: "), "{version}: {err}");
        }
        for description in ["", " lead", "trail ", "two\\nlines"] {
            let err = bad(
                "Adds the terms sessions define to the glossary note.",
                description,
            );
            assert!(err.starts_with("description: "), "{description:?}: {err}");
        }
        let long = "d".repeat(161);
        assert!(bad(
            "Adds the terms sessions define to the glossary note.",
            &long
        )
        .starts_with("description: "));
        assert_eq!(
            bad("[\"Ada Lovelace\"]", "[]"),
            "authors: needs 1 to 5 names"
        );
        assert_eq!(
            bad(
                "[\"Ada Lovelace\"]",
                "[\"a\",\"b\",\"c\",\"d\",\"e\",\"f\"]"
            ),
            "authors: needs 1 to 5 names"
        );
        assert!(bad("[\"Ada Lovelace\"]", "[\"\"]").starts_with("authors: each"));
        assert!(bad("[\"Ada Lovelace\"]", "[\"a\\nb\"]").starts_with("authors: each"));
        assert!(bad("\"MIT\"", "\"MIT OR X\"").starts_with("license: "));
        assert!(bad("\"MIT\"", "\"\"").starts_with("license: "));
    }

    #[test]
    fn unknown_keys_are_refused() {
        let err = parse(&format!("{PASS}extra = 1\n")).unwrap_err();
        assert!(err.contains("unknown field `extra`"), "{err}");
        assert!(parse("not toml =").is_err());
    }

    const THEME: &str = "protocol = \"HIPPOCAMP_V1\"\nname = \"high-contrast\"\nkind = \"theme\"\n\
        version = \"1.0.0\"\ndescription = \"Text at 7:1.\"\nauthors = [\"Ada\"]\n\
        license = \"MIT\"\ntheme_contract = 1\ndesktop_theme_format = 1\n";

    #[test]
    fn kind_defaults_to_passes_and_a_theme_manifest_parses() {
        assert_eq!(parse(PASS).unwrap().kind, Kind::Passes);
        assert_eq!(parse(PASS).unwrap().theme_contract, None);
        let theme = parse(THEME).unwrap();
        assert_eq!(theme.kind, Kind::Theme);
        assert_eq!(theme.kind.as_str(), "theme");
        assert_eq!(theme.theme_contract, Some(1));
        assert_eq!(theme.desktop_theme_format, Some(1));
        // Each key alone is fine in a theme pack.
        let web_only = THEME.replace("desktop_theme_format = 1\n", "");
        assert_eq!(parse(&web_only).unwrap().desktop_theme_format, None);
        assert_eq!(
            parse(&format!("{PASS}kind = \"passes\"\n")).unwrap().kind,
            Kind::Passes
        );
    }

    #[test]
    fn theme_keys_are_for_theme_packs_and_start_at_one() {
        for key in ["theme_contract = 1", "desktop_theme_format = 1"] {
            let err = parse(&format!("{PASS}{key}\n")).unwrap_err();
            assert!(err.ends_with(": only a theme pack has it"), "{key}: {err}");
        }
        let err = parse(&THEME.replace("theme_contract = 1", "theme_contract = 0")).unwrap_err();
        assert_eq!(err, "theme_contract: must be a whole number from 1");
        for bad in ["\"1\"", "1.5", "-1"] {
            let swapped = THEME.replace("theme_contract = 1", &format!("theme_contract = {bad}"));
            assert!(parse(&swapped).is_err(), "{bad}");
        }
        assert!(parse(&format!("{PASS}kind = \"themes\"\n")).is_err());
    }

    #[test]
    fn peeking_at_the_kind() {
        assert_eq!(peek_kind(THEME), Kind::Theme);
        assert_eq!(peek_kind(PASS), Kind::Passes);
        assert_eq!(peek_kind("not toml ="), Kind::Passes);
        assert_eq!(peek_kind("kind = 3"), Kind::Passes);
    }

    #[test]
    fn pack_names() {
        for ok in ["ab", "a1", "a-b-c1", "glossary-terms"] {
            assert!(is_pack_name(ok), "{ok}");
        }
        for bad in ["", "A", "a-", "-a", "a--b", "1a", "a b"] {
            assert!(!is_pack_name(bad), "{bad}");
        }
    }
}
