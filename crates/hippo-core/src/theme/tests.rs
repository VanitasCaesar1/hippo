use super::*;
use std::os::unix::fs::symlink;

/// A complete style written for the tests, every one of the 31 keys.
fn full_style() -> Map<String, Value> {
    STYLE_KEYS
        .iter()
        .enumerate()
        .map(|(i, (k, _))| {
            (
                k.to_string(),
                Value::String(format!("#{:02x}{:02x}{:02x}", i, i, i)),
            )
        })
        .collect()
}

fn family(extra_theme: &str, style: &Map<String, Value>) -> String {
    format!(
        r##"{{"name":"Harbour","author":"A. Person","themes":[{{"name":"Harbour Dark","appearance":"dark","style":{}{}}}]}}"##,
        Value::Object(style.clone()),
        extra_theme
    )
}

fn parse_str(text: &str) -> std::result::Result<Vec<Theme>, String> {
    parse(text.as_bytes(), Path::new("/x/harbour.json"))
}

fn dark() -> Theme {
    builtin_for(Appearance::Dark)
}

#[test]
fn builtin_themes_have_every_key_and_no_warnings() {
    let [light, dark] = builtin();
    assert_eq!(light.name, "Hippo Light");
    assert_eq!(dark.name, "Hippo Dark");
    assert_eq!(light.appearance, Appearance::Light);
    for t in [&light, &dark] {
        assert!(t.warnings.is_empty(), "{:?}", t.warnings);
        assert_eq!(t.font_size, 13.0);
        assert!(t.background.is_none() && t.font_family.is_none());
    }
    assert_eq!(light.colors.window, Rgba(0xFCFCFDFF));
    assert_eq!(dark.colors.focus, Rgba(0x4D8DF7FF));
    assert_eq!(dark.colors.success, dark.colors.created);
}

#[test]
fn builtin_files_pass_the_strict_check() {
    for text in [LIGHT_JSON, DARK_JSON] {
        let file = check_desktop_theme(text).unwrap();
        assert_eq!(file.name, "Hippo");
        assert_eq!(file.author, "hippo contributors");
        assert_eq!(file.themes.len(), 1);
        assert_eq!(file.themes()[0].warnings.len(), 0);
    }
}

#[test]
fn the_style_key_table_covers_every_colour_and_every_builtin_file_sets_every_key() {
    assert_eq!(STYLE_KEYS.len(), 31);
    let mut keys: Vec<&str> = STYLE_KEYS.iter().map(|(k, _)| *k).collect();
    keys.sort();
    keys.dedup();
    assert_eq!(keys.len(), 31, "no key twice");
    for field in ColorField::ALL {
        assert!(
            STYLE_KEYS.iter().any(|(_, f)| f.contains(&field)),
            "{} is fed by no key",
            field.name()
        );
    }
    for text in [LIGHT_JSON, DARK_JSON] {
        let v: Value = serde_json::from_str(text).unwrap();
        let style = v["themes"][0]["style"].as_object().unwrap();
        assert_eq!(style.len(), 31);
        for (k, _) in STYLE_KEYS {
            assert!(style.contains_key(k), "built-in file lacks {k}");
        }
    }
}

#[test]
fn colours() {
    assert_eq!(parse_colour("#112233"), Some(Rgba(0x112233ff)));
    assert_eq!(parse_colour("#AbCdEf80"), Some(Rgba(0xabcdef80)));
    for bad in [
        "#fff", "112233", "#12345", "#gggggg", "#1122334", "", "#é12345",
    ] {
        assert_eq!(parse_colour(bad), None, "{bad}");
    }
}

#[test]
fn a_family_with_and_without_the_hippo_object() {
    let style = full_style();
    let plain = parse_str(&family("", &style)).unwrap();
    assert_eq!(plain.len(), 1);
    assert!(plain[0].warnings.is_empty());
    assert_eq!(plain[0].font_size, 13.0);
    assert_eq!(plain[0].font_family, None);
    assert_eq!(
        Some(plain[0].colors.window),
        parse_colour(style["background"].as_str().unwrap())
    );

    let with = parse_str(&family(
        r#","hippo":{"font_family":"IBM Plex Sans","font_size":16}"#,
        &style,
    ))
    .unwrap();
    assert_eq!(with[0].font_family.as_deref(), Some("IBM Plex Sans"));
    assert_eq!(with[0].font_size, 16.0);
}

#[test]
fn each_fallback_row() {
    // Dock: panel.background, then surface.background.
    let mut style = full_style();
    style.remove("panel.background");
    let t = &parse_str(&family("", &style)).unwrap()[0];
    assert_eq!(pick(&style, F::Dock), Some(t.colors.dock));
    assert_eq!(
        Some(t.colors.dock),
        parse_colour(style["surface.background"].as_str().unwrap())
    );
    assert!(t.warnings.is_empty());
    // Centre pane: editor.background, then background.
    let mut style = full_style();
    style.remove("editor.background");
    let t = &parse_str(&family("", &style)).unwrap()[0];
    assert_eq!(t.colors.centre, t.colors.window);
    // Pickers: elevated_surface, then surface.
    let mut style = full_style();
    style.remove("elevated_surface.background");
    let t = &parse_str(&family("", &style)).unwrap()[0];
    assert_eq!(
        Some(t.colors.elevated),
        parse_colour(style["surface.background"].as_str().unwrap())
    );
    // Hover and selected rows.
    let mut style = full_style();
    style.remove("ghost_element.hover");
    style.remove("ghost_element.selected");
    let t = &parse_str(&family("", &style)).unwrap()[0];
    assert_eq!(
        Some(t.colors.hover),
        parse_colour(style["element.hover"].as_str().unwrap())
    );
    assert_eq!(
        Some(t.colors.selected),
        parse_colour(style["element.selected"].as_str().unwrap())
    );
    // Fallbacks do not chain: with neither dock key, the built-in dock is used.
    let mut style = full_style();
    style.remove("panel.background");
    style.remove("surface.background");
    let t = &parse_str(&family("", &style)).unwrap()[0];
    assert_eq!(t.colors.dock, dark().colors.dock);
    assert_eq!(t.warnings.len(), 1);
    assert!(t.warnings[0].contains("dock"));
}

#[test]
fn a_theme_without_style_warns_and_uses_the_builtin() {
    let t = parse_str(r#"{"name":"A","themes":[{"name":"T","appearance":"dark"}]}"#).unwrap();
    assert_eq!(t[0].colors.window, dark().colors.window);
    assert!(t[0].warnings.iter().any(|w| w.contains("no `style`")));
    let t =
        parse_str(r#"{"themes":[{"name":"T","appearance":"dark","style":3,"hippo":4}]}"#).unwrap();
    assert!(t[0].warnings.iter().any(|w| w.contains("not an object")));
    assert!(t[0]
        .warnings
        .iter()
        .any(|w| w.contains("`hippo` is not an object")));
}

#[test]
fn bad_colours_are_missing_and_warn() {
    let mut style = full_style();
    style.insert("text".into(), Value::String("#fff".into()));
    style.insert("border".into(), Value::from(5));
    style.insert("unknown.key".into(), Value::String("nonsense".into()));
    let t = &parse_str(&family("", &style)).unwrap()[0];
    assert_eq!(t.colors.text, dark().colors.text);
    assert_eq!(t.colors.border, dark().colors.border);
    assert!(t
        .warnings
        .iter()
        .any(|w| w.contains("`text` is not a colour")));
    assert!(!t.warnings.iter().any(|w| w.contains("unknown.key")));
}

#[test]
fn unusable_files_name_the_fault() {
    for (text, fault) in [
        ("[]", "top level is not an object"),
        (r#"{"name":"x"}"#, "`themes` is missing"),
        (r#"{"themes":[]}"#, "`themes` is empty"),
        (r#"{"themes":{}}"#, "not an array"),
        (r#"{"themes":[3]}"#, "themes[0] is not an object"),
        (r#"{"themes":[{"appearance":"dark"}]}"#, "no `name`"),
        (
            r#"{"themes":[{"name":"","appearance":"dark"}]}"#,
            "no `name`",
        ),
        (
            r#"{"themes":[{"name":"T","appearance":"sepia"}]}"#,
            "appearance",
        ),
        (r#"{"themes":[{"name":"T"}]}"#, "appearance"),
        (r#"{"themes":[]} // c"#, "not valid JSON"),
        (
            r#"{"themes":[{"name":"T","appearance":"dark",}]}"#,
            "not valid JSON",
        ),
        ("", "not valid JSON"),
    ] {
        let err = parse_str(text).unwrap_err();
        assert!(err.starts_with("harbour.json: "), "{err}");
        assert!(err.contains(fault), "{err} should contain {fault}");
    }
}

#[test]
fn schema_version_other_than_0_2_0_warns() {
    let ok = r#"{"$schema":"https://zed.dev/schema/themes/v0.2.0.json","themes":[{"name":"T","appearance":"light"}]}"#;
    assert!(!parse_str(ok).unwrap()[0]
        .warnings
        .iter()
        .any(|w| w.contains("$schema")));
    let other = ok.replace("0.2.0", "0.9.0");
    assert!(parse_str(&other).unwrap()[0]
        .warnings
        .iter()
        .any(|w| w.contains("$schema")));
}

#[test]
fn hippo_fields_out_of_range_are_ignored_not_clamped() {
    let h = |hippo: &str| {
        parse_str(&format!(
            r#"{{"themes":[{{"name":"T","appearance":"dark","hippo":{hippo}}}]}}"#
        ))
        .unwrap()
        .remove(0)
    };
    let t = h(r#"{"font_size":9}"#);
    assert_eq!(t.font_size, 13.0);
    assert!(t.warnings.iter().any(|w| w.contains("font_size")));
    assert_eq!(h(r#"{"font_size":25}"#).font_size, 13.0);
    assert_eq!(h(r#"{"font_size":10}"#).font_size, 10.0);
    assert_eq!(h(r#"{"font_size":24}"#).font_size, 24.0);
    assert_eq!(h(r#"{"font_family":7}"#).font_family, None);
    let t = h(r#"{"background":{"image":"a.png","opacity":1.5}}"#);
    assert!(t.background.is_none());
    assert!(t.warnings.iter().any(|w| w.contains("opacity")));
}

#[test]
fn background_images_are_checked_against_the_files_directory() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    fs::write(d.join("ok.png"), b"x").unwrap();
    fs::write(d.join("ok.JPG"), b"x").unwrap();
    fs::write(d.join("a.gif"), b"x").unwrap();
    fs::create_dir(d.join("sub")).unwrap();
    fs::write(d.join("sub/in.jpeg"), b"x").unwrap();
    fs::write(d.join("real.png"), b"x").unwrap();
    symlink(d.join("real.png"), d.join("link.png")).unwrap();
    symlink(d.join("sub"), d.join("sublink")).unwrap();

    assert_eq!(
        check_background_image(d, "ok.png").unwrap(),
        d.join("ok.png")
    );
    assert!(check_background_image(d, "ok.JPG").is_ok());
    assert!(check_background_image(d, "./sub/in.jpeg").is_ok());
    for (rel, fault) in [
        ("../ok.png", ".."),
        ("sub/../ok.png", ".."),
        ("/etc/x.png", "absolute"),
        ("https://example.com/a.png", "URL"),
        ("data:image/png;base64,AAAA", "URL"),
        ("a.gif", ".png"),
        ("link.png", "symbolic link"),
        ("sublink/in.jpeg", "symbolic link"),
        ("missing.png", "cannot be read"),
        ("sub", ".png"),
        ("", "empty"),
    ] {
        let err = check_background_image(d, rel).unwrap_err();
        assert!(err.contains(fault), "{rel}: {err}");
    }
    let big = fs::File::create(d.join("big.png")).unwrap();
    big.set_len(MAX_IMAGE_BYTES + 1).unwrap();
    assert!(check_background_image(d, "big.png")
        .unwrap_err()
        .contains("8 MB"));
    let ok = fs::File::create(d.join("edge.png")).unwrap();
    ok.set_len(MAX_IMAGE_BYTES).unwrap();
    assert!(check_background_image(d, "edge.png").is_ok());

    // Through parse: a good image is kept, a bad one warns.
    let file = d.join("t.json");
    let text = r#"{"themes":[{"name":"T","appearance":"dark","hippo":{"background":{"image":"ok.png"}}},
        {"name":"U","appearance":"dark","hippo":{"background":{"image":"../x.png","opacity":0.5}}}]}"#;
    let themes = parse(text.as_bytes(), &file).unwrap();
    let bg = themes[0].background.as_ref().unwrap();
    assert_eq!(bg.image, d.join("ok.png"));
    assert_eq!(bg.opacity, 0.15);
    assert!(themes[1].background.is_none());
    assert!(themes[1].warnings.iter().any(|w| w.contains("refused")));
}

fn strict(style: &str, hippo: &str) -> String {
    format!(
        r##"{{"name":"P","author":"Someone","themes":[{{"name":"T","appearance":"light","style":{style}{hippo}}}]}}"##
    )
}

#[test]
fn strict_check_accepts_a_small_theme() {
    let text = strict(
        r##"{"background":"#ffffff","text":"#000000ff"}"##,
        r#","hippo":{"font_family":"IBM Plex-Sans 2","font_size":14}"#,
    );
    let file = check_desktop_theme(&text).unwrap();
    let t = &file.themes()[0];
    assert_eq!(t.font_size, 14.0);
    assert_eq!(t.colors.text, Rgba(0x000000ff));
    assert!(
        !t.warnings.is_empty(),
        "missing keys fall back with warnings"
    );
}

#[test]
fn strict_check_refuses_what_the_design_lists() {
    let style = r##"{"background":"#ffffff"}"##;
    let cases: Vec<(String, &str)> = vec![
        // unknown fields at every level
        (r##"{"name":"P","author":"A","themes":[{"name":"T","appearance":"dark","style":{}}],"x":1}"##.into(), "unknown field `x`"),
        (strict(r##"{"nonsense":"#ffffff"}"##, ""), "unknown style key `nonsense`"),
        (strict(style, r#","extra":1"#), "unknown field `extra`"),
        (strict(style, r#","hippo":{"colour":1}"#), "unknown field `colour`"),
        // a duplicate key
        (strict(r##"{"text":"#ffffff","text":"#000000"}"##, ""), "duplicate style key `text`"),
        (r##"{"name":"P","name":"Q","author":"A","themes":[{"name":"T","appearance":"dark","style":{}}]}"##.into(), "duplicate field"),
        // $schema is a URL
        (r##"{"$schema":"https://zed.dev/x.json","name":"P","author":"A","themes":[{"name":"T","appearance":"dark","style":{}}]}"##.into(), "unknown field `$schema`"),
        // a background
        (strict(style, r#","hippo":{"background":{"image":"a.png"}}"#), "unknown field `background`"),
        // bad colours
        (strict(r##"{"text":"#fff"}"##, ""), "not a colour"),
        (strict(r##"{"text":"red"}"##, ""), "not a colour"),
        (strict(r##"{"text":5}"##, ""), "invalid type"),
        // font family
        (strict(style, r#","hippo":{"font_family":"a/b"}"#), "font_family"),
        (strict(style, r#","hippo":{"font_family":"a:b"}"#), "font_family"),
        (strict(style, r#","hippo":{"font_family":"https://x"}"#), "font_family"),
        (strict(style, r#","hippo":{"font_family":""}"#), "font_family"),
        (strict(style, &format!(r#","hippo":{{"font_family":"{}"}}"#, "a".repeat(65))), "font_family"),
        (strict(style, r#","hippo":{"font_size":9.9}"#), "font_size"),
        (strict(style, r#","hippo":{"font_size":24.5}"#), "font_size"),
        // appearance, shape, missing style
        (r##"{"name":"P","author":"A","themes":[{"name":"T","appearance":"sepia","style":{}}]}"##.into(), "sepia"),
        (r##"{"name":"P","author":"A","themes":[{"name":"T","appearance":"dark"}]}"##.into(), "missing field `style`"),
        (r##"{"name":"P","themes":[]}"##.into(), "missing field `author`"),
        // counts and lengths
        (r##"{"name":"P","author":"A","themes":[]}"##.into(), "1 to 16"),
        (format!(r##"{{"name":"{}","author":"A","themes":[{{"name":"T","appearance":"dark","style":{{}}}}]}}"##, "n".repeat(65)), "`name`"),
        (format!(r##"{{"name":"P","author":"{}","themes":[{{"name":"T","appearance":"dark","style":{{}}}}]}}"##, "n".repeat(129)), "`author`"),
        (format!(r##"{{"name":"P","author":"A","themes":[{{"name":"{}","appearance":"dark","style":{{}}}}]}}"##, "n".repeat(65)), "themes[0].name"),
        (r##"{"name":"P","author":"A","themes":[{"name":"","appearance":"dark","style":{}}]}"##.into(), "themes[0].name"),
        // comments and trailing commas
        (strict(style, "").replace('}', ",}"), "theme.json"),
    ];
    for (text, fault) in cases {
        let err = check_desktop_theme(&text).unwrap_err();
        assert!(err.starts_with("theme.json: "), "{err}");
        assert!(err.contains(fault), "{err} should contain {fault}\n{text}");
    }
    let seventeen: Vec<String> = (0..17)
        .map(|i| format!(r#"{{"name":"T{i}","appearance":"dark","style":{{}}}}"#))
        .collect();
    let text = format!(
        r#"{{"name":"P","author":"A","themes":[{}]}}"#,
        seventeen.join(",")
    );
    assert!(check_desktop_theme(&text).unwrap_err().contains("1 to 16"));
    // 64 and 128 characters are fine
    let text = format!(
        r#"{{"name":"{}","author":"{}","themes":[{{"name":"{}","appearance":"dark","style":{{}}}}]}}"#,
        "n".repeat(64),
        "a".repeat(128),
        "t".repeat(64)
    );
    assert!(check_desktop_theme(&text).is_ok());
}

#[test]
fn every_decoded_string_has_a_place() {
    let text = strict(
        r##"{"panel.background":"#ffffff"}"##,
        r#","hippo":{"font_family":"Mono"}"#,
    );
    let file = check_desktop_theme(&text).unwrap();
    let strings = file.decoded_strings();
    let labels: Vec<&str> = strings.iter().map(|(l, _)| l.as_str()).collect();
    assert!(labels.contains(&"theme.json: themes[0].style.panel.background"));
    assert!(labels.contains(&"theme.json: themes[0].hippo.font_family"));
    assert!(labels.contains(&"theme.json: name"));
}

#[test]
fn contrast_follows_wcag_and_the_compositing_rule() {
    let black = Rgba(0x000000ff);
    let white = Rgba(0xffffffff);
    let r = contrast(black, white, white, Appearance::Light);
    assert!((r - 21.0).abs() < 0.01, "{r}");
    assert!((contrast(white, white, white, Appearance::Light) - 1.0).abs() < 1e-6);
    // Half-transparent black over a white window is mid grey (about 3.9 to 1).
    let half = Rgba(0x00000080);
    let r = contrast(half, white, white, Appearance::Light);
    assert!(r > 3.5 && r < 4.3, "{r}");
    // A transparent window is composited over white (light) or black (dark).
    let clear = Rgba(0xffffff00);
    assert!((contrast(black, clear, clear, Appearance::Light) - 21.0).abs() < 0.01);
    assert!((contrast(white, clear, clear, Appearance::Dark) - 21.0).abs() < 0.01);
    // Order does not matter.
    assert_eq!(
        contrast(black, white, white, Appearance::Light),
        contrast(white, black, white, Appearance::Light)
    );
}

fn check_pairs(t: &Theme) {
    let c = &t.colors;
    let ap = t.appearance;
    let ratio = |a: Rgba, b: Rgba| contrast(a, b, c.window, ap);
    let name = &t.name;
    let all_surfaces = [
        ("window", c.window),
        ("dock", c.dock),
        ("centre", c.centre),
        ("title bar", c.title_bar),
        ("status bar", c.status_bar),
        ("tab bar", c.tab_bar),
        ("active tab", c.tab_active),
        ("inactive tab", c.tab_inactive),
        ("pickers", c.elevated),
        ("selected row", c.selected),
        ("hover row", c.hover),
    ];
    for (s, bg) in all_surfaces {
        for (what, fg) in [("text", c.text), ("secondary text", c.text_muted)] {
            let r = ratio(fg, bg);
            assert!(r >= 4.5, "{name}: {what} on {s} is {r}");
        }
    }
    let plain = [
        ("window", c.window),
        ("dock", c.dock),
        ("centre", c.centre),
        ("pickers", c.elevated),
    ];
    for (s, bg) in plain {
        for (what, fg) in [
            ("link", c.text_accent),
            ("error", c.error),
            ("warning", c.warning),
            ("success", c.success),
        ] {
            let r = ratio(fg, bg);
            assert!(r >= 4.5, "{name}: {what} on {s} is {r}");
        }
        for (what, fg) in [("focus", c.focus), ("scrollbar", c.scrollbar_thumb)] {
            let r = ratio(fg, bg);
            assert!(r >= 3.0, "{name}: {what} on {s} is {r}");
        }
    }
    assert!(
        ratio(c.focus, c.selected) >= 3.0,
        "{name}: focus on selected row"
    );
    assert!(
        ratio(c.deleted, c.deleted_background) >= 4.5,
        "{name}: removed"
    );
    assert!(
        ratio(c.created, c.created_background) >= 4.5,
        "{name}: added"
    );
    assert!(ratio(c.text, c.search_match) >= 4.5, "{name}: search match");
}

#[test]
fn builtin_themes_meet_aa_on_every_surface() {
    for t in builtin() {
        check_pairs(&t);
    }
}

#[test]
fn the_focus_floor_replaces_a_faint_cue_with_text_and_leaves_a_good_theme_alone() {
    for mut good in builtin() {
        let before = good.clone();
        apply_focus_floor(&mut good);
        assert_eq!(good, before);
    }
    // A faint focus ring takes the text colour, and so does the selection marker.
    let mut faint = dark();
    faint.colors.focus = faint.colors.window; // 1 to 1
    apply_focus_floor(&mut faint);
    assert_eq!(faint.colors.focus, dark().colors.text);
    assert_eq!(faint.colors.selected_outline, dark().colors.text);
    assert!(faint
        .warnings
        .iter()
        .any(|w| w.contains("focus ring") && w.contains("1.0 to 1")));
    // A faint selection marker alone does the same to both.
    let mut faint = dark();
    faint.colors.selected_outline = faint.colors.dock;
    apply_focus_floor(&mut faint);
    assert_eq!(faint.colors.focus, dark().colors.text);
    assert!(faint
        .warnings
        .iter()
        .any(|w| w.contains("selection marker")));
    // Failing only against the selected row is enough.
    let mut faint = dark();
    faint.colors.selected = faint.colors.focus;
    apply_focus_floor(&mut faint);
    assert!(faint.warnings.iter().any(|w| w.contains("selected row")));
    // Text is not floored: unreadable text stays as the theme says.
    let mut bad = dark();
    bad.colors.text = bad.colors.window;
    bad.colors.focus = Rgba(0xffffffff);
    bad.colors.selected_outline = Rgba(0xffffffff);
    let before = bad.clone();
    apply_focus_floor(&mut bad);
    assert_eq!(bad.colors.text, before.colors.text);
}

#[test]
fn a_faint_focus_in_a_user_file_is_floored() {
    let mut style = full_style();
    for (k, v) in [
        ("background", "#18181b"),
        ("text", "#f4f4f5"),
        ("border.focused", "#1a1a1d"),
    ] {
        style.insert(k.into(), Value::String(v.into()));
    }
    for k in [
        "panel.background",
        "editor.background",
        "elevated_surface.background",
        "ghost_element.selected",
    ] {
        style.insert(k.into(), Value::String("#18181b".into()));
    }
    let mut t = parse_str(&family("", &style)).unwrap().remove(0);
    apply_focus_floor(&mut t);
    assert_eq!(t.colors.focus, t.colors.text);
    assert_eq!(t.colors.focus, Rgba(0xf4f4f5ff));
}

fn write_theme(path: &Path, names: &[&str]) {
    let themes: Vec<String> = names
        .iter()
        .map(|n| format!(r#"{{"name":"{n}","appearance":"dark","style":{{}}}}"#))
        .collect();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!(r#"{{"themes":[{}]}}"#, themes.join(","))).unwrap();
}

#[test]
fn scan_lists_files_in_path_order_and_skips_what_it_should() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    write_theme(&d.join("b.json"), &["B"]);
    write_theme(&d.join("a.json"), &["A"]);
    write_theme(&d.join(".hidden.json"), &["H"]);
    fs::write(d.join("notes.txt"), "x").unwrap();
    fs::write(d.join("broken.json"), "{").unwrap();
    // A symbolic link to a good file is refused, not followed.
    symlink(d.join("a.json"), d.join("link.json")).unwrap();
    // Too large.
    fs::write(
        d.join("huge.json"),
        vec![b' '; (MAX_USER_FILE_BYTES + 1) as usize],
    )
    .unwrap();
    let body = r#"{"themes":[{"name":"E","appearance":"dark"}]}"#;
    let padded = format!(
        "{body}{}",
        " ".repeat(MAX_USER_FILE_BYTES as usize - body.len())
    );
    fs::write(d.join("exact.json"), padded).unwrap();
    assert_eq!(
        fs::metadata(d.join("exact.json")).unwrap().len(),
        MAX_USER_FILE_BYTES
    );

    let listed = scan(d);
    let names: Vec<String> = listed.iter().map(|l| file_label(&l.path)).collect();
    assert_eq!(
        names,
        [
            "a.json",
            "b.json",
            "broken.json",
            "exact.json",
            "huge.json",
            "link.json"
        ]
    );
    assert!(listed[0].record.is_ok() && listed[1].record.is_ok());
    assert!(listed[2]
        .record
        .as_ref()
        .unwrap_err()
        .contains("not valid JSON"));
    assert!(listed[3].record.is_ok());
    assert!(listed[4].record.as_ref().unwrap_err().contains("too large"));
    assert!(listed[5]
        .record
        .as_ref()
        .unwrap_err()
        .contains("symbolic link"));
    assert!(scan(&d.join("missing")).is_empty());
}

#[test]
fn scan_stops_reading_at_500_files() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..502 {
        fs::write(dir.path().join(format!("t{i:03}.json")), "{}").unwrap();
    }
    let listed = scan(dir.path());
    assert_eq!(listed.len(), 502);
    assert!(listed[499]
        .record
        .as_ref()
        .unwrap_err()
        .contains("`themes`"));
    assert!(listed[500]
        .record
        .as_ref()
        .unwrap_err()
        .contains("too many"));
    assert!(listed[501]
        .record
        .as_ref()
        .unwrap_err()
        .contains("too many"));
}

#[test]
fn a_named_pipe_is_refused_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("pipe.json");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(status.success());
    let listed = scan(dir.path());
    assert_eq!(listed.len(), 1);
    assert!(listed[0]
        .record
        .as_ref()
        .unwrap_err()
        .contains("not a regular file"));
    // The same inside a pack.
    let packs = tempfile::tempdir().unwrap();
    pack_dir(packs.path(), "p");
    let status = std::process::Command::new("mkfifo")
        .arg(packs.path().join("p/theme.json"))
        .status()
        .unwrap();
    assert!(status.success());
    let listed = scan_packs(packs.path());
    assert!(listed[0]
        .record
        .as_ref()
        .unwrap_err()
        .contains("not a regular file"));
}

const PACK_TOML: &str = "desktop_theme_format = 1\n";

/// A pack directory with the `pack.toml` that declares the desktop format.
fn pack_dir(root: &Path, name: &str) -> PathBuf {
    let d = root.join(name);
    fs::create_dir_all(&d).unwrap();
    fs::write(d.join("pack.toml"), PACK_TOML).unwrap();
    d
}

const PACK_THEME: &str = r##"{"name":"P","author":"A","themes":[{"name":"Pack Dark","appearance":"dark","style":{"background":"#101010"}}]}"##;

#[test]
fn scan_packs_checks_each_theme_json_strictly() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    pack_dir(d, "zeta");
    fs::write(d.join("zeta/theme.json"), PACK_THEME).unwrap();
    pack_dir(d, "alpha");
    fs::write(
        d.join("alpha/theme.json"),
        PACK_THEME.replace("Pack Dark", "Alpha"),
    )
    .unwrap();
    pack_dir(d, "bad");
    fs::write(
        d.join("bad/theme.json"),
        r#"{"name":"P","author":"A","themes":[],"x":1}"#,
    )
    .unwrap();
    fs::create_dir_all(d.join("no-theme")).unwrap();
    fs::write(d.join("no-theme/pack.toml"), "x").unwrap();
    fs::create_dir_all(d.join(".hidden")).unwrap();
    fs::write(d.join(".hidden/theme.json"), PACK_THEME).unwrap();
    fs::write(d.join("loose-file"), "x").unwrap();
    // A symbolic-link pack directory is refused, whatever it points to.
    let target = tempfile::tempdir().unwrap();
    fs::write(target.path().join("theme.json"), PACK_THEME).unwrap();
    symlink(target.path(), d.join("linked")).unwrap();
    // A symbolic-link theme.json inside a pack is refused too.
    pack_dir(d, "linkfile");
    symlink(d.join("zeta/theme.json"), d.join("linkfile/theme.json")).unwrap();

    let listed = scan_packs(d);
    let names: Vec<String> = listed
        .iter()
        .map(|l| {
            if l.path.ends_with("theme.json") {
                l.path
                    .parent()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            } else {
                file_label(&l.path)
            }
        })
        .collect();
    assert_eq!(names, ["alpha", "bad", "linked", "linkfile", "zeta"]);
    assert_eq!(listed[0].record.as_ref().unwrap()[0].name, "Alpha");
    assert!(listed[1]
        .record
        .as_ref()
        .unwrap_err()
        .starts_with("bad/theme.json: "));
    assert!(listed[2]
        .record
        .as_ref()
        .unwrap_err()
        .contains("symbolic link"));
    assert!(listed[3]
        .record
        .as_ref()
        .unwrap_err()
        .contains("symbolic link"));
    assert_eq!(listed[4].record.as_ref().unwrap()[0].name, "Pack Dark");
    assert!(scan_packs(&d.join("missing")).is_empty());
}

fn listed(names: &[&str], path: &str) -> Listed<Vec<Theme>> {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("x.json");
    write_theme(&file, names);
    let themes = parse(&fs::read(&file).unwrap(), Path::new(path)).unwrap();
    Listed {
        path: PathBuf::from(path),
        record: Ok(themes),
    }
}

#[test]
fn catalog_ids_order_and_duplicates() {
    let user = vec![
        listed(&["Harbour", "Other", "Harbour"], "/themes/a.json"),
        listed(&["Harbour", "Hippo Dark", "System"], "/themes/b.json"),
        Listed {
            path: "/themes/c.json".into(),
            record: Err("c.json: not valid JSON".into()),
        },
    ];
    let packs = vec![
        listed(&["Harbour"], "/packs/p/theme.json"),
        listed(&["Harbour"], "/packs/q/theme.json"),
    ];
    let cat = catalog(&user, &packs);
    let rows: Vec<(&str, &str, bool)> = cat
        .iter()
        .map(|e| (e.id.as_str(), e.source.as_str(), e.duplicate))
        .collect();
    assert_eq!(
        rows,
        [
            ("system", "System", false),
            ("builtin:Hippo Light", "Built-in", false),
            ("builtin:Hippo Dark", "Built-in", false),
            ("user:a.json/Harbour", "a.json", false),
            ("user:a.json/Other", "a.json", false),
            ("user:a.json/Harbour", "a.json", true),
            // The same name in another file is another theme, and there are no reserved names.
            ("user:b.json/Harbour", "b.json", false),
            ("user:b.json/Hippo Dark", "b.json", false),
            ("user:b.json/System", "b.json", false),
            ("user:c.json/c.json", "c.json", false),
            ("pack:p/Harbour", "Pack p", false),
            ("pack:q/Harbour", "Pack q", false),
        ]
    );
    assert!(cat[9].theme.is_err() && cat[9].name == "c.json");
    assert!(cat[3].theme.is_ok());
    // Ids parse back the way the preference needs: split at the first slash after the prefix.
    assert_eq!(
        cat[3]
            .id
            .strip_prefix("user:")
            .unwrap()
            .split_once('/')
            .unwrap()
            .0,
        "a.json"
    );
}

#[test]
fn a_zed_style_file_with_extra_keys_works_as_it_is() {
    // Written for this test: a family with keys the app does not read.
    let mut style = full_style();
    style.insert(
        "syntax".into(),
        serde_json::json!({"comment": {"color": "#888888ff"}}),
    );
    style.insert("players".into(), serde_json::json!([{"cursor": "#ffffff"}]));
    style.insert("terminal.ansi.red".into(), Value::String("#ff0000".into()));
    let text = format!(
        r##"{{"$schema":"https://zed.dev/schema/themes/v0.2.0.json","name":"Z","author":"Z","themes":[{{"name":"Z","appearance":"light","style":{}}}]}}"##,
        Value::Object(style)
    );
    let t = parse_str(&text).unwrap();
    assert!(t[0].warnings.is_empty(), "{:?}", t[0].warnings);
    assert_eq!(t[0].appearance, Appearance::Light);
}

#[test]
fn check_desktop_theme_runs_the_text_rules_on_decoded_strings() {
    // A right-to-left override written as an escape passes a check of the
    // raw bytes; it is refused once decoded, with the place of the string.
    let text =
        strict(r##"{"text":"#ffffff"}"##, "").replace(r#""name":"T""#, r#""name":"a\u202eb""#);
    assert!(!text.as_bytes().contains(&0xe2));
    let err = check_desktop_theme(&text).unwrap_err();
    assert!(err.starts_with("theme.json: themes[0].name:"), "{err}");
    assert!(err.contains("U+202E"), "{err}");
    for bad in ["\\u200b", "\\ue000"] {
        let text = strict(r##"{"text":"#ffffff"}"##, "")
            .replace(r#""author":"Someone""#, &format!(r#""author":"x{bad}y""#));
        assert!(
            check_desktop_theme(&text)
                .unwrap_err()
                .starts_with("theme.json: author"),
            "{bad}"
        );
        let text = strict(
            r##"{"text":"#ffffff"}"##,
            &format!(r#","hippo":{{"font_family":"x{bad}y"}}"#),
        );
        assert!(check_desktop_theme(&text).is_err());
    }
}

#[test]
fn a_pack_theme_over_the_packs_file_limit_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    pack_dir(dir.path(), "p");
    let pad = " ".repeat(crate::packs::MAX_FILE_BYTES as usize);
    fs::write(
        dir.path().join("p/theme.json"),
        format!("{PACK_THEME}{pad}"),
    )
    .unwrap();
    let listed = scan_packs(dir.path());
    assert!(listed[0].record.as_ref().unwrap_err().contains("too large"));
}

// ---------------------------------------------------------------------
// Background images
// ---------------------------------------------------------------------

/// A PNG's signature and `IHDR` chunk, for the size the reader looks at. The
/// rest of a real file is not needed.
fn png_bytes(width: u32, height: u32) -> Vec<u8> {
    let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
    b.extend_from_slice(&13u32.to_be_bytes());
    b.extend_from_slice(b"IHDR");
    b.extend_from_slice(&width.to_be_bytes());
    b.extend_from_slice(&height.to_be_bytes());
    b.extend_from_slice(&[8, 2, 0, 0, 0]);
    b.extend_from_slice(&[0; 4]); // the chunk's checksum, unchecked here
    b
}

/// A JPEG with an APP0 segment, a comment and a start-of-frame marker `sof`.
fn jpeg_bytes(sof: u8, width: u16, height: u16) -> Vec<u8> {
    let mut b = vec![0xff, 0xd8];
    b.extend_from_slice(&[0xff, 0xe0, 0x00, 0x04, b'J', b'F']);
    b.extend_from_slice(&[0xff, 0xff, 0xfe, 0x00, 0x05, b'a', b'b', b'c']);
    b.extend_from_slice(&[0xff, sof, 0x00, 0x11, 8]);
    b.extend_from_slice(&height.to_be_bytes());
    b.extend_from_slice(&width.to_be_bytes());
    b.extend_from_slice(&[3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
    b.extend_from_slice(&[0xff, 0xd9]);
    b
}

#[test]
fn read_background_accepts_a_png_and_a_jpeg_with_their_sizes() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let theme = d.join("t.json");
    fs::write(d.join("a.png"), png_bytes(1920, 1080)).unwrap();
    fs::write(d.join("edge.png"), png_bytes(4096, 4096)).unwrap();
    fs::write(d.join("a.jpg"), jpeg_bytes(0xc0, 800, 600)).unwrap();
    fs::write(d.join("b.JPEG"), jpeg_bytes(0xc2, 640, 480)).unwrap();

    let png = read_background(&theme, "a.png").unwrap();
    assert_eq!(
        (png.kind, png.width, png.height),
        (ImageKind::Png, 1920, 1080)
    );
    assert_eq!(png.bytes, png_bytes(1920, 1080));
    let jpg = read_background(&theme, "a.jpg").unwrap();
    assert_eq!(
        (jpg.kind, jpg.width, jpg.height),
        (ImageKind::Jpeg, 800, 600)
    );
    let prog = read_background(&theme, "b.JPEG").unwrap();
    assert_eq!((prog.width, prog.height), (640, 480));
    assert!(read_background(&theme, "edge.png").is_ok());
}

#[test]
fn read_background_refuses_what_it_should() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let theme = d.join("t.json");
    fs::write(d.join("wide.png"), png_bytes(5000, 100)).unwrap();
    fs::write(d.join("tall.jpg"), jpeg_bytes(0xc0, 100, 4097)).unwrap();
    fs::write(d.join("zero.png"), png_bytes(0, 10)).unwrap();
    fs::write(
        d.join("svg.png"),
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"/>"#,
    )
    .unwrap();
    fs::write(d.join("gif.png"), b"GIF89a\x01\x00\x01\x00").unwrap();
    fs::write(d.join("short.png"), b"\x89PNG\r\n\x1a\n").unwrap();
    // A JPEG whose frame header never comes, and one cut short.
    fs::write(d.join("noframe.jpg"), [0xff, 0xd8, 0xff, 0xda, 0, 2]).unwrap();
    fs::write(d.join("cut.jpg"), [0xff, 0xd8, 0xff, 0xc0, 0, 17, 8]).unwrap();
    // DHT (0xC4) is not a frame header.
    let mut dht = vec![0xff, 0xd8, 0xff, 0xc4, 0x00, 0x04, 0, 0];
    dht.extend_from_slice(&[0xff, 0xd9]);
    fs::write(d.join("dht.jpg"), dht).unwrap();
    fs::write(d.join("real.png"), png_bytes(10, 10)).unwrap();
    symlink(d.join("real.png"), d.join("link.png")).unwrap();
    let big = fs::File::create(d.join("big.png")).unwrap();
    big.set_len(MAX_IMAGE_BYTES + 1).unwrap();

    for (rel, fault) in [
        ("wide.png", "5000 by 100"),
        ("tall.jpg", "100 by 4097"),
        ("zero.png", "no pixels"),
        ("svg.png", "not a PNG or JPEG"),
        ("gif.png", "not a PNG or JPEG"),
        ("short.png", "size cannot be read"),
        ("noframe.jpg", "size cannot be read"),
        ("cut.jpg", "size cannot be read"),
        ("dht.jpg", "size cannot be read"),
        ("link.png", "symbolic link"),
        ("big.png", "8 MB"),
        ("../x.png", ".."),
        ("missing.png", "cannot be read"),
    ] {
        let err = read_background(&theme, rel).unwrap_err();
        assert!(err.contains(fault), "{rel}: {err}");
    }
}

#[test]
fn read_background_does_not_block_on_a_named_pipe() {
    let dir = tempfile::tempdir().unwrap();
    let status = std::process::Command::new("mkfifo")
        .arg(dir.path().join("p.png"))
        .status()
        .unwrap();
    assert!(status.success());
    let err = read_background(&dir.path().join("t.json"), "p.png").unwrap_err();
    assert!(err.contains("not a regular file"), "{err}");
}

// ---------------------------------------------------------------------
// load_file and the pack.toml format check
// ---------------------------------------------------------------------

#[test]
fn load_file_reads_a_user_file_like_scan() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("mine.json");
    write_theme(&file, &["One", "Two"]);
    let l = load_file(&file, Source::User);
    assert_eq!(l.path, file);
    let names: Vec<_> = l.record.unwrap().into_iter().map(|t| t.name).collect();
    assert_eq!(names, ["One", "Two"]);
    assert_eq!(
        scan(dir.path())[0].record.as_ref().unwrap().len(),
        2,
        "scan agrees"
    );

    fs::write(&file, "{").unwrap();
    assert!(load_file(&file, Source::User)
        .record
        .unwrap_err()
        .starts_with("mine.json: not valid JSON"));
    let big = fs::File::create(&file).unwrap();
    big.set_len(MAX_USER_FILE_BYTES + 1).unwrap();
    assert!(load_file(&file, Source::User)
        .record
        .unwrap_err()
        .contains("too large"));
    fs::remove_file(&file).unwrap();
    assert!(load_file(&file, Source::User).record.is_err());
    // A symbolic link is refused.
    write_theme(&dir.path().join("real.json"), &["R"]);
    symlink(dir.path().join("real.json"), &file).unwrap();
    assert!(load_file(&file, Source::User)
        .record
        .unwrap_err()
        .contains("symbolic link"));
}

#[test]
fn load_file_reads_a_pack_file_strictly() {
    let dir = tempfile::tempdir().unwrap();
    let d = pack_dir(dir.path(), "harbour");
    let file = d.join("theme.json");
    fs::write(&file, PACK_THEME).unwrap();
    let l = load_file(&file, Source::Pack);
    assert_eq!(l.path, file);
    assert_eq!(l.record.unwrap()[0].name, "Pack Dark");

    // The strict check applies: an unknown field is refused.
    fs::write(&file, r#"{"name":"P","author":"A","themes":[],"extra":1}"#).unwrap();
    assert!(load_file(&file, Source::Pack)
        .record
        .unwrap_err()
        .starts_with("harbour/theme.json: "));

    // A pack directory that is a symbolic link is refused.
    let link = dir.path().join("linked");
    symlink(&d, &link).unwrap();
    assert!(load_file(&link.join("theme.json"), Source::Pack)
        .record
        .unwrap_err()
        .contains("symbolic link"));

    // So is a pack with no desktop theme, as the caller named it.
    fs::remove_file(d.join("pack.toml")).unwrap();
    assert!(load_file(&file, Source::Pack)
        .record
        .unwrap_err()
        .contains("no desktop theme"));
}

#[test]
fn a_pack_made_for_another_format_is_listed_with_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let d = pack_dir(dir.path(), "future");
    fs::write(d.join("theme.json"), PACK_THEME).unwrap();
    fs::write(d.join("pack.toml"), "desktop_theme_format = 2\n").unwrap();
    for listed in [
        scan_packs(dir.path()).remove(0),
        load_file(&d.join("theme.json"), Source::Pack),
    ] {
        assert_eq!(listed.path, d.join("theme.json"));
        assert_eq!(
            listed.record.unwrap_err(),
            "future: made for desktop theme format 2; this Hippo reads 1"
        );
    }
    // A value that is not a whole number, and a file that is not TOML.
    fs::write(d.join("pack.toml"), "desktop_theme_format = \"1\"\n").unwrap();
    assert!(scan_packs(dir.path())[0]
        .record
        .as_ref()
        .unwrap_err()
        .contains("not a whole number"));
    fs::write(d.join("pack.toml"), "desktop_theme_format = = 1").unwrap();
    assert!(scan_packs(dir.path())[0]
        .record
        .as_ref()
        .unwrap_err()
        .contains("not valid TOML"));
}

#[test]
fn a_pack_without_the_key_or_without_pack_toml_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    // The key is missing: another kind of pack, with a theme.json by chance.
    let nokey = pack_dir(d, "nokey");
    fs::write(nokey.join("pack.toml"), "name = \"x\"\n").unwrap();
    fs::write(nokey.join("theme.json"), PACK_THEME).unwrap();
    // No pack.toml at all.
    fs::create_dir_all(d.join("nomanifest")).unwrap();
    fs::write(d.join("nomanifest/theme.json"), PACK_THEME).unwrap();
    // One that counts.
    let good = pack_dir(d, "good");
    fs::write(good.join("theme.json"), PACK_THEME).unwrap();

    let listed = scan_packs(d);
    assert_eq!(listed.len(), 1);
    assert!(listed[0].path.ends_with("good/theme.json"));
    assert!(listed[0].record.is_ok());
}

#[test]
fn a_pack_toml_that_is_a_link_or_not_a_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let linked = pack_dir(d, "linked");
    fs::write(linked.join("theme.json"), PACK_THEME).unwrap();
    fs::remove_file(linked.join("pack.toml")).unwrap();
    fs::write(d.join("elsewhere.toml"), PACK_TOML).unwrap();
    symlink(d.join("elsewhere.toml"), linked.join("pack.toml")).unwrap();

    let pipe = pack_dir(d, "pipe");
    fs::write(pipe.join("theme.json"), PACK_THEME).unwrap();
    fs::remove_file(pipe.join("pack.toml")).unwrap();
    let status = std::process::Command::new("mkfifo")
        .arg(pipe.join("pack.toml"))
        .status()
        .unwrap();
    assert!(status.success());

    let dirpack = pack_dir(d, "dirpack");
    fs::write(dirpack.join("theme.json"), PACK_THEME).unwrap();
    fs::remove_file(dirpack.join("pack.toml")).unwrap();
    fs::create_dir(dirpack.join("pack.toml")).unwrap();

    let listed = scan_packs(d);
    assert_eq!(listed.len(), 3);
    for l in &listed {
        let err = l.record.as_ref().unwrap_err();
        assert!(err.contains("pack.toml"), "{err}");
    }
    assert!(listed[1]
        .record
        .as_ref()
        .unwrap_err()
        .contains("symbolic link"));
    assert!(listed[0]
        .record
        .as_ref()
        .unwrap_err()
        .contains("not a regular file"));
    assert!(listed[2]
        .record
        .as_ref()
        .unwrap_err()
        .contains("not a regular file"));
}
