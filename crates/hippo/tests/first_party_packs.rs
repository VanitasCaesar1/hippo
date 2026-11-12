//! Checks every first-party pack under `packs/`: it passes `hippo packs
//! check` with no warning and has a licence; a pass pack's fixture tests
//! pass, it has listable cases, and its template belongs to it alone; a
//! theme pack has no cases or templates, and `high-contrast` really is
//! 7:1.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use hippo_core::notes::Note;
use hippo_core::packs::testing::Case;
use hippo_core::records::Pass;

/// The fewest pass packs `packs/` may hold: 3 in pull request (d), 20 from
/// pull request (e).
const MIN_PASS_PACKS: usize = 20;
/// The fewest theme packs `packs/` may hold.
const MIN_THEME_PACKS: usize = 3;

fn packs_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs")
}

/// Runs `hippo packs <sub> <dir>` with empty hippo and user homes.
fn hippo(sub: &str, dir: &Path) -> (bool, String) {
    let home = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_hippo"))
        .args(["packs", sub])
        .arg(dir)
        .env("HIPPO_HOME", home.path())
        .env("HOME", user.path())
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

fn sorted_entries(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .collect();
    paths.sort();
    paths
}

/// The files below `dir`, as `/`-separated paths relative to it.
fn files_below(dir: &Path) -> Vec<String> {
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>) {
        for path in sorted_entries(dir) {
            let name = format!("{prefix}{}", path.file_name().unwrap().to_string_lossy());
            if path.is_dir() {
                walk(&path, &format!("{name}/"), out);
            } else {
                out.push(name);
            }
        }
    }
    let mut out = Vec::new();
    if dir.is_dir() {
        walk(dir, "", &mut out);
    }
    out
}

struct Pack {
    name: String,
    dir: PathBuf,
    theme: bool,
    passes: Vec<Pass>,
    /// Template paths below `templates/`, each with its frontmatter type.
    templates: Vec<(String, Option<String>)>,
}

fn load(dir: PathBuf) -> Pack {
    let name = dir.file_name().unwrap().to_string_lossy().into_owned();
    let manifest = fs::read_to_string(dir.join("pack.toml")).unwrap_or_default();
    let theme = manifest
        .lines()
        .any(|line| line.replace(' ', "") == "kind=\"theme\"");
    let passes = files_below(&dir.join("passes"))
        .iter()
        .map(|file| {
            let text = fs::read_to_string(dir.join("passes").join(file)).unwrap();
            Pass::from_toml(&text).unwrap_or_else(|e| panic!("{name}: passes/{file}: {e}"))
        })
        .collect();
    let templates = files_below(&dir.join("templates"))
        .into_iter()
        .map(|file| {
            let note = Note::read(&dir.join("templates").join(&file)).unwrap();
            (file, note.frontmatter.r#type)
        })
        .collect();
    Pack {
        name,
        dir,
        theme,
        passes,
        templates,
    }
}

/// The failures of the template rules for `pack` against all `packs`.
fn template_failures(pack: &Pack, packs: &[Pack]) -> Vec<String> {
    let mut failures = Vec::new();
    for (n, pass) in pack.passes.iter().enumerate() {
        if let Some(patterns) = &pass.note_paths {
            // Only the patterns count here, whatever `note_types` says.
            let by_path = Pass {
                note_types: None,
                ..pass.clone()
            };
            if !pack
                .templates
                .iter()
                .any(|(path, _)| by_path.selects(Path::new(path), None))
            {
                failures.push(format!(
                    "{}: no template matches the patterns {patterns:?} of pass {n}",
                    pack.name
                ));
            }
            for other in packs.iter().filter(|other| other.name != pack.name) {
                for (path, _) in &other.templates {
                    if by_path.selects(Path::new(path), None) {
                        failures.push(format!(
                            "{}: pass {n} matches the template {path} of {}",
                            pack.name, other.name
                        ));
                    }
                }
            }
        } else if let Some(types) = &pass.note_types {
            if !pack.templates.iter().any(|(_, found)| {
                found
                    .as_deref()
                    .is_some_and(|t| types.iter().any(|allowed| allowed == t))
            }) {
                failures.push(format!(
                    "{}: no template has a type in {types:?}",
                    pack.name
                ));
            }
        }
    }
    failures
}

#[test]
fn every_first_party_pack_checks_tests_and_owns_its_template() {
    let packs: Vec<Pack> = sorted_entries(&packs_dir())
        .into_iter()
        .filter(|path| path.is_dir())
        .map(load)
        .collect();
    let mut failures = Vec::new();
    let mut pass_packs = 0;
    let mut theme_packs = 0;
    for pack in &packs {
        let (ok, text) = hippo("check", &pack.dir);
        if !ok || text.lines().any(|line| line.starts_with("warning")) {
            failures.push(format!("check {}:\n{text}", pack.name));
        }
        if !pack.dir.join("LICENSE").is_file() {
            failures.push(format!("{}: no LICENSE", pack.name));
        }
        let cases = files_below(&pack.dir.join("tests"))
            .into_iter()
            .filter(|path| path.ends_with("/case.toml"))
            .collect::<Vec<_>>();
        if pack.theme {
            theme_packs += 1;
            // A theme pack has no passes, templates or cases, and no test
            // to run: it is exempt from the template and test-case rules.
            if !cases.is_empty() || !pack.passes.is_empty() || !pack.templates.is_empty() {
                failures.push(format!(
                    "{}: a theme pack has no cases, passes or templates",
                    pack.name
                ));
            }
            let files = files_below(&pack.dir);
            if !files.iter().any(|f| f == "theme.css" || f == "theme.json") {
                failures.push(format!("{}: no theme.css or theme.json", pack.name));
            }
            continue;
        }
        pass_packs += 1;
        if cases.is_empty() {
            failures.push(format!("{}: no case", pack.name));
        }
        for case in &cases {
            let text = fs::read_to_string(pack.dir.join("tests").join(case)).unwrap();
            match Case::parse(&text) {
                Ok(parsed) if parsed.listable() => {}
                Ok(_) => failures.push(format!("{}: tests/{case} is not listable", pack.name)),
                Err(err) => failures.push(format!("{}: tests/{case}: {err}", pack.name)),
            }
        }
        let (ok, text) = hippo("test", &pack.dir);
        if !ok {
            failures.push(format!("test {}:\n{text}", pack.name));
        }
        failures.extend(template_failures(pack, &packs));
    }
    if pass_packs < MIN_PASS_PACKS {
        failures.push(format!(
            "{pass_packs} pass packs, fewer than {MIN_PASS_PACKS}"
        ));
    }
    if theme_packs < MIN_THEME_PACKS {
        failures.push(format!(
            "{theme_packs} theme packs, fewer than {MIN_THEME_PACKS}"
        ));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The custom properties a block sets, in file order: `--hp-x: value;`.
fn declarations(block: &str) -> Vec<(String, String)> {
    block
        .split([';', '{', '}'])
        .filter_map(|part| {
            let (name, value) = part.split_once(':')?;
            let name = name.trim();
            name.starts_with("--hp-")
                .then(|| (name.to_owned(), value.trim().to_owned()))
        })
        .collect()
}

#[test]
fn high_contrast_meets_7_to_1_in_every_scheme_it_sets() {
    let css = fs::read_to_string(packs_dir().join("high-contrast/theme.css")).unwrap();
    let (top, dark) = css
        .split_once("@media (prefers-color-scheme: dark)")
        .expect("high-contrast sets a dark scheme");
    assert!(!top.contains("@media"));
    let light = declarations(top);
    let mut dark_scheme = light.clone();
    for (name, value) in declarations(dark) {
        dark_scheme.retain(|(n, _)| *n != name);
        dark_scheme.push((name, value));
    }
    let mut report = Vec::new();
    for (scheme, props) in [("light", &light), ("dark", &dark_scheme)] {
        let value = |name: &str| {
            props
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.as_str())
                .unwrap_or_else(|| panic!("{scheme}: {name} is not set"))
        };
        for (fg, bg) in hippo_theme::TEXT_PAIRS
            .iter()
            .chain(hippo_theme::CONTROL_PAIRS)
        {
            let ratio = hippo_theme::contrast(value(fg), value(bg))
                .unwrap_or_else(|| panic!("{scheme}: {fg} on {bg} is not a colour"));
            report.push(format!("{scheme} {fg} on {bg}: {ratio:.2}"));
            assert!(ratio >= 7.0, "{scheme}: {fg} on {bg} is {ratio:.2}:1");
        }
    }
    eprintln!("{}", report.join("\n"));
}

#[test]
fn high_contrast_desktop_text_is_7_to_1_on_every_surface() {
    use hippo_core::theme::{check_desktop_theme, contrast, Rgba};
    let json = fs::read_to_string(packs_dir().join("high-contrast/theme.json")).unwrap();
    let file = check_desktop_theme(&json).unwrap();
    let themes = file.themes();
    assert_eq!(themes.len(), 2);
    for theme in themes {
        let c = theme.colors;
        let surfaces: [(&str, Rgba); 9] = [
            ("window", c.window),
            ("dock", c.dock),
            ("centre", c.centre),
            ("title_bar", c.title_bar),
            ("status_bar", c.status_bar),
            ("tab_bar", c.tab_bar),
            ("tab_active", c.tab_active),
            ("tab_inactive", c.tab_inactive),
            ("elevated", c.elevated),
        ];
        for (label, text) in [("text", c.text), ("text_muted", c.text_muted)] {
            for (surface, background) in surfaces {
                let ratio = contrast(text, background, c.window, theme.appearance);
                eprintln!("{} {label} on {surface}: {ratio:.2}", theme.name);
                assert!(
                    ratio >= 7.0,
                    "{}: {label} on {surface} is {ratio:.2}:1",
                    theme.name
                );
            }
        }
    }
}
