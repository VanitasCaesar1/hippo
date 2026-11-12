//! The themes the window can wear: the catalogue over the built-in themes,
//! `$HIPPOCAMP_HOME/themes/` and the workspace's theme packs, the theme in use
//! (from the saved preference, or System), and the once-a-second look at the
//! file of the theme in use.
//!
//! Everything here is plain data and file reads, so that it is tested without
//! a window: [`probe`] is what the background executor runs each second and
//! [`ThemeState::apply_probe`] what the window then does with its answer.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use gpui::{Image, ImageFormat, WindowAppearance};
use hippocamp_core::records::Listed;
use hippocamp_core::theme::{self, CatalogEntry, ImageKind, Source, Theme};

use crate::prefs;
use crate::ui::theme::ActiveTheme;

/// The size and modification time of a file, `None` when it is not there.
pub type Stamp = Option<(u64, Option<SystemTime>)>;

pub fn stamp(path: &Path) -> Stamp {
    fs::symlink_metadata(path)
        .ok()
        .map(|m| (m.len(), m.modified().ok()))
}

/// What the status bar says about the theme, when there is something to say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// One short line.
    pub text: String,
    /// The whole story, for a tooltip.
    pub detail: String,
    /// A real problem (the warning colour), as against a plain remark.
    pub problem: bool,
}

impl Notice {
    fn problem(text: impl Into<String>, detail: impl Into<String>) -> Notice {
        Notice {
            text: text.into(),
            detail: detail.into(),
            problem: true,
        }
    }
}

/// A theme id taken apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeId {
    System,
    Builtin(String),
    User { file: String, name: String },
    Pack { pack: String, name: String },
}

/// `system`, `builtin:<name>`, `user:<file name>/<name>`, `pack:<pack>/<name>`.
pub fn parse_id(id: &str) -> Option<ThemeId> {
    if id == prefs::SYSTEM {
        return Some(ThemeId::System);
    }
    if let Some(name) = id.strip_prefix("builtin:") {
        return Some(ThemeId::Builtin(name.to_string()));
    }
    let (user, rest) = match id.strip_prefix("user:") {
        Some(rest) => (true, rest),
        None => (false, id.strip_prefix("pack:")?),
    };
    let (owner, name) = rest.split_once('/')?;
    if owner.is_empty() || owner == "." || owner == ".." {
        return None;
    }
    let (owner, name) = (owner.to_string(), name.to_string());
    Some(if user {
        ThemeId::User { file: owner, name }
    } else {
        ThemeId::Pack { pack: owner, name }
    })
}

/// A name from a theme file as the window shows it: control and bidirectional
/// characters written out, and a long name ended with an ellipsis.
pub fn shown(text: &str) -> String {
    let text = hippocamp_view::text::visible(text);
    if text.chars().count() > 64 {
        let cut: String = text.chars().take(63).collect();
        format!("{cut}…")
    } else {
        text
    }
}

/// A theme's background image, ready for GPUI.
#[derive(Clone)]
pub struct Backdrop {
    pub image: Arc<Image>,
    pub opacity: f32,
}

/// The theme as the window applies it: its font family checked against the
/// installed fonts (a missing one becomes the system font, with a warning)
/// and the focus floor applied. An empty font list means the list could not be
/// read, so no family is judged missing.
pub fn prepare(mut theme: Theme, fonts: &HashSet<String>) -> Theme {
    if let Some(family) = theme.font_family.clone()
        && !fonts.is_empty()
        && !fonts.contains(&family)
    {
        theme.warnings.push(format!(
            "the font \"{family}\" is not installed: using the system font"
        ));
        theme.font_family = None;
    }
    theme::apply_focus_floor(&mut theme);
    theme
}

/// The background image of `theme`, read by `hippocamp-core` (which does every
/// path, size and format check) and handed to GPUI as image data.
fn read_backdrop(theme: &Theme, theme_file: &Path) -> Result<Option<Backdrop>, String> {
    let Some(bg) = &theme.background else {
        return Ok(None);
    };
    let dir = theme_file.parent().unwrap_or_else(|| Path::new("."));
    let rel = bg
        .image
        .strip_prefix(dir)
        .ok()
        .and_then(Path::to_str)
        .ok_or_else(|| "the image path is outside the theme's directory".to_string())?;
    let read = theme::read_background(theme_file, rel)?;
    let format = match read.kind {
        ImageKind::Png => ImageFormat::Png,
        ImageKind::Jpeg => ImageFormat::Jpeg,
    };
    Ok(Some(Backdrop {
        image: Arc::new(Image::from_bytes(format, read.bytes)),
        opacity: bg.opacity,
    }))
}

/// The file of the theme in use, which the poll looks at.
#[derive(Debug, Clone)]
struct Watch {
    path: PathBuf,
    source: Source,
    name: String,
    stamp: Stamp,
}

impl Watch {
    fn label(&self) -> String {
        shown(&self.path.file_name().map_or_else(
            || self.path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        ))
    }
}

/// What one look at the watched file asks the background executor to do.
pub struct ProbeRequest {
    id: String,
    watch: Watch,
    fonts: Arc<HashSet<String>>,
}

/// What a look at the file found.
pub enum Outcome {
    Unchanged,
    Reloaded {
        theme: Theme,
        backdrop: Result<Option<Backdrop>, String>,
    },
    /// The file stopped parsing, or no longer holds the theme.
    Failed {
        text: String,
        detail: String,
    },
}

pub struct Probe {
    id: String,
    stamp: Stamp,
    outcome: Outcome,
}

impl Probe {
    /// The file had not changed: nothing to redraw.
    pub fn unchanged(&self) -> bool {
        matches!(self.outcome, Outcome::Unchanged)
    }
}

/// One look at the watched file: its size and modification time, and when
/// either changed, the file read again with `hippocamp-core`'s reader.
pub fn probe(request: ProbeRequest) -> Probe {
    let ProbeRequest { id, watch, fonts } = request;
    let now = stamp(&watch.path);
    let label = watch.label();
    let outcome = if now == watch.stamp {
        Outcome::Unchanged
    } else if now.is_none() {
        Outcome::Failed {
            text: format!("{label} is gone: keeping the last good version"),
            detail: format!("{} is not there now", watch.path.display()),
        }
    } else {
        let listed = theme::load_file(&watch.path, watch.source);
        match listed.record {
            Err(e) => Outcome::Failed {
                text: format!("{label} no longer parses: keeping the last good version"),
                detail: shown(&e),
            },
            Ok(themes) => match themes.into_iter().find(|t| t.name == watch.name) {
                None => Outcome::Failed {
                    text: format!(
                        "{label} no longer has {}: keeping the last good version",
                        shown(&watch.name)
                    ),
                    detail: format!("{label} holds no theme named {}", shown(&watch.name)),
                },
                Some(theme) => {
                    let theme = prepare(theme, &fonts);
                    let backdrop = read_backdrop(&theme, &watch.path);
                    Outcome::Reloaded { theme, backdrop }
                }
            },
        }
    };
    Probe {
        id,
        stamp: now,
        outcome,
    }
}

/// The themes found in the two directories, listed again.
pub struct Scanned {
    pub user: Vec<Listed<Vec<Theme>>>,
    pub packs: Vec<Listed<Vec<Theme>>>,
    /// Why the user's directory could not be listed, when it could not.
    pub user_error: Option<String>,
}

/// Reads both theme directories. Reads only.
pub fn scan(themes_dir: &Path, packs_dir: Option<&Path>) -> Scanned {
    let user_error = match fs::read_dir(themes_dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Some(format!("cannot read {}: {e}", themes_dir.display()))
        }
        _ => None,
    };
    Scanned {
        user: theme::scan(themes_dir),
        packs: packs_dir.map(theme::scan_packs).unwrap_or_default(),
        user_error,
    }
}

/// The theme state of the window.
pub struct ThemeState {
    themes_dir: PathBuf,
    packs_dir: Option<PathBuf>,
    prefs_path: PathBuf,
    fonts: Arc<HashSet<String>>,
    catalog: Vec<CatalogEntry>,
    user_error: Option<String>,
    /// The id of the theme in use.
    id: String,
    /// The theme in use: the last good version of it.
    committed: ActiveTheme,
    notice: Option<Notice>,
    watch: Option<Watch>,
    backdrop: Option<Backdrop>,
    /// The picker is showing another theme than the one in use.
    pub previewing: bool,
}

impl ThemeState {
    /// `home` is `$HIPPOCAMP_HOME`; `workspace` is the configured workspace, when
    /// known, whose `.hippocamp/packs` holds theme packs.
    pub fn new(
        home: &Path,
        workspace: Option<&Path>,
        prefs_path: PathBuf,
        fonts: HashSet<String>,
        appearance: WindowAppearance,
    ) -> ThemeState {
        ThemeState {
            themes_dir: home.join("themes"),
            packs_dir: workspace.map(packs_dir_of),
            prefs_path,
            fonts: Arc::new(fonts),
            catalog: Vec::new(),
            user_error: None,
            id: prefs::SYSTEM.to_string(),
            committed: ActiveTheme::system(appearance),
            notice: None,
            watch: None,
            backdrop: None,
            previewing: false,
        }
    }

    pub fn themes_dir(&self) -> &Path {
        &self.themes_dir
    }

    pub fn packs_dir(&self) -> Option<&Path> {
        self.packs_dir.as_deref()
    }

    /// The workspace changed (a reload read another configuration).
    pub fn set_workspace(&mut self, workspace: Option<&Path>) {
        self.packs_dir = workspace.map(packs_dir_of);
    }

    pub fn catalog(&self) -> &[CatalogEntry] {
        &self.catalog
    }

    pub fn user_error(&self) -> Option<&str> {
        self.user_error.as_deref()
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn committed(&self) -> &ActiveTheme {
        &self.committed
    }

    pub fn notice(&self) -> Option<&Notice> {
        self.notice.as_ref()
    }

    pub fn set_notice(&mut self, notice: Option<Notice>) {
        self.notice = notice;
    }

    /// The image to draw in the centre pane: the theme in use's, never while
    /// another theme is only being previewed.
    pub fn backdrop(&self) -> Option<&Backdrop> {
        if self.previewing {
            None
        } else {
            self.backdrop.as_ref()
        }
    }

    /// Lists the two directories again, here and now.
    pub fn rescan(&mut self) {
        let scanned = scan(&self.themes_dir, self.packs_dir.as_deref());
        self.set_scanned(scanned);
    }

    /// Takes a fresh listing: builds the catalogue, with each theme prepared as
    /// it will be applied, so that the picker shows the warnings that apply.
    pub fn set_scanned(&mut self, scanned: Scanned) {
        let mut catalog = theme::catalog(&scanned.user, &scanned.packs);
        for entry in &mut catalog {
            if let Ok(t) = &entry.theme {
                entry.theme = Ok(prepare(t.clone(), &self.fonts));
            }
        }
        self.catalog = catalog;
        self.user_error = scanned.user_error;
    }

    /// The first usable entry with this id (a duplicate shares its first
    /// namesake's id and is never the one).
    fn find(&self, id: &str) -> Option<&CatalogEntry> {
        self.catalog.iter().find(|e| e.id == id && !e.duplicate)
    }

    /// The theme with this id, ready to apply, or why it cannot be.
    pub fn resolve(&self, id: &str, appearance: WindowAppearance) -> Result<ActiveTheme, Notice> {
        if id == prefs::SYSTEM {
            return Ok(ActiveTheme::system(appearance));
        }
        let entry = self.find(id).or_else(|| self.unusable_file_of(id));
        let Some(entry) = entry else {
            return Err(Notice::problem(
                "Saved theme is gone: using System",
                format!("no theme file now holds {}", shown(id)),
            ));
        };
        match &entry.theme {
            Ok(theme) => Ok(ActiveTheme {
                theme: theme.clone(),
                system: false,
            }),
            Err(e) => Err(Notice::problem(
                format!("{} does not parse: using System", shown(&entry.name)),
                shown(e),
            )),
        }
    }

    /// The entry of a file that does not load, when `id` names a theme in
    /// that file: such a file is listed once, under its own name, not under
    /// the names of the themes it would hold.
    fn unusable_file_of(&self, id: &str) -> Option<&CatalogEntry> {
        let owner = match parse_id(id)? {
            ThemeId::User { file, .. } => format!("user:{file}/"),
            ThemeId::Pack { pack, .. } => format!("pack:{pack}/"),
            ThemeId::System | ThemeId::Builtin(_) => return None,
        };
        self.catalog
            .iter()
            .find(|e| e.theme.is_err() && e.id.starts_with(&owner))
    }

    /// The file a theme id lives in.
    fn watch_for(&self, id: &str) -> Option<Watch> {
        let (path, source, name) = match parse_id(id)? {
            ThemeId::User { file, name } => (self.themes_dir.join(file), Source::User, name),
            ThemeId::Pack { pack, name } => (
                self.packs_dir.as_ref()?.join(pack).join("theme.json"),
                Source::Pack,
                name,
            ),
            ThemeId::System | ThemeId::Builtin(_) => return None,
        };
        let stamp = stamp(&path);
        Some(Watch {
            path,
            source,
            name,
            stamp,
        })
    }

    /// Makes `active` the theme in use, under `id`: watches its file and reads
    /// its background image.
    fn set_committed(&mut self, id: &str, active: ActiveTheme) {
        self.watch = self.watch_for(id);
        self.backdrop = None;
        if let Some(watch) = &self.watch {
            match read_backdrop(&active.theme, &watch.path) {
                Ok(b) => self.backdrop = b,
                Err(e) => self.notice = Some(background_notice(&e)),
            }
        }
        self.id = id.to_string();
        self.committed = active;
        self.previewing = false;
    }

    /// Chooses the theme at launch: System when Option is held, else the saved
    /// one, else System, with a notice when the saved one cannot be used.
    /// Lists the directories first. Returns the theme to apply.
    pub fn launch(&mut self, option_held: bool, appearance: WindowAppearance) -> ActiveTheme {
        self.rescan();
        self.notice = None;
        let system = ActiveTheme::system(appearance);
        if option_held {
            self.set_committed(prefs::SYSTEM, system.clone());
            self.notice = Some(Notice {
                text: "Option held: System for this session".into(),
                detail: "Hold Option while Hippocamp starts to use System for that session; the saved theme is unchanged".into(),
                problem: false,
            });
            return system;
        }
        let saved = match prefs::read(&self.prefs_path) {
            Ok(None) => prefs::SYSTEM.to_string(),
            Ok(Some(p)) => p.theme,
            Err(e) => {
                self.set_committed(prefs::SYSTEM, system.clone());
                self.notice = Some(Notice::problem(
                    "Preferences unreadable: using System",
                    shown(&e),
                ));
                return system;
            }
        };
        match self.resolve(&saved, appearance) {
            Ok(active) => {
                self.set_committed(&saved, active);
                self.committed.clone()
            }
            Err(notice) => {
                self.set_committed(prefs::SYSTEM, system.clone());
                self.notice = Some(notice);
                system
            }
        }
    }

    /// The theme of `id` to show while the picker is open; the theme in use
    /// stays as it is.
    pub fn preview(&mut self, id: &str, appearance: WindowAppearance) -> Option<ActiveTheme> {
        let active = self.resolve(id, appearance).ok()?;
        self.previewing = true;
        Some(active)
    }

    /// Keeps the theme of `id` and saves it. A failed save leaves the theme
    /// in use for the session and says why in the status bar. Returns the
    /// theme to apply.
    pub fn commit(
        &mut self,
        id: &str,
        appearance: WindowAppearance,
    ) -> Result<ActiveTheme, Notice> {
        let active = self.resolve(id, appearance)?;
        self.set_committed(id, active.clone());
        self.notice = None;
        self.save_choice();
        Ok(active)
    }

    /// "Theme: Use System": System, saved.
    pub fn use_system(&mut self, appearance: WindowAppearance) -> ActiveTheme {
        let system = ActiveTheme::system(appearance);
        self.set_committed(prefs::SYSTEM, system.clone());
        self.notice = None;
        self.save_choice();
        system
    }

    /// The preferences file.
    pub fn prefs_path(&self) -> &Path {
        &self.prefs_path
    }

    fn save_choice(&mut self) {
        // The pins are kept as they are in the file.
        let id = self.id.clone();
        if let Err(e) = prefs::update(&self.prefs_path, |p| p.theme = id) {
            self.notice = Some(Notice::problem(
                "Could not save the theme: kept for this session",
                shown(&format!("{}: {e}", self.prefs_path.display())),
            ));
        }
    }

    /// The picker was dismissed: the theme in use before it opened, found
    /// again by its id in the current list; when it is no longer there, the
    /// last good version of it. Returns the theme to apply.
    pub fn restore(&mut self, before: &str, appearance: WindowAppearance) -> ActiveTheme {
        match self.resolve(before, appearance) {
            Ok(active) if before == self.id => {
                self.set_committed(before, active);
            }
            _ => self.previewing = false,
        }
        if self.committed.system {
            self.committed = ActiveTheme::system(appearance);
        }
        self.committed.clone()
    }

    /// The system's light or dark appearance changed.
    pub fn follow_appearance(&mut self, appearance: WindowAppearance) {
        if self.committed.system {
            self.committed = ActiveTheme::system(appearance);
        }
    }

    // ---- the once-a-second look ------------------------------------------

    /// What to look at, or `None` when the theme in use has no file (System
    /// and the built-in themes).
    pub fn probe_request(&self) -> Option<ProbeRequest> {
        Some(ProbeRequest {
            id: self.id.clone(),
            watch: self.watch.clone()?,
            fonts: self.fonts.clone(),
        })
    }

    /// What the window does with a look's answer. Returns the theme now in
    /// use when it changed. An answer about a theme no longer in use is
    /// dropped.
    pub fn apply_probe(&mut self, probe: Probe) -> Option<ActiveTheme> {
        if probe.id != self.id {
            return None;
        }
        self.watch.as_mut()?.stamp = probe.stamp;
        match probe.outcome {
            Outcome::Unchanged => None,
            Outcome::Failed { text, detail } => {
                self.notice = Some(Notice::problem(text, detail));
                None
            }
            Outcome::Reloaded { theme, backdrop } => {
                self.notice = None;
                self.backdrop = None;
                match backdrop {
                    Ok(b) => self.backdrop = b,
                    Err(e) => self.notice = Some(background_notice(&e)),
                }
                self.committed = ActiveTheme {
                    theme,
                    system: false,
                };
                Some(self.committed.clone())
            }
        }
    }

    /// One look, now, on this thread (the tests drive the poll by hand).
    #[cfg(test)]
    pub fn poll_now(&mut self) -> Option<ActiveTheme> {
        let probe = probe(self.probe_request()?);
        self.apply_probe(probe)
    }
}

fn background_notice(error: &str) -> Notice {
    Notice {
        text: "Background image not shown".into(),
        detail: shown(error),
        problem: false,
    }
}

fn packs_dir_of(workspace: &Path) -> PathBuf {
    workspace.join(".hippocamp").join("packs")
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // tests write theme files into a temporary home
mod tests {
    use super::*;
    use crate::prefs::Prefs;

    const DARK: WindowAppearance = WindowAppearance::Dark;

    fn family(name: &str, text: &str, extra: &str) -> String {
        format!(
            r##"{{"name":"Fam","author":"t","themes":[{{"name":"{name}","appearance":"dark",
            "style":{{"background":"#101010ff","text":"{text}","border.focused":"#80c0ffff"}}{extra}}}]}}"##
        )
    }

    struct Home {
        _dir: tempfile::TempDir,
        home: PathBuf,
        prefs: PathBuf,
    }

    fn home() -> Home {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        fs::create_dir_all(home.join("themes")).unwrap();
        let prefs = dir.path().join("prefs/preferences.json");
        Home {
            _dir: dir,
            home,
            prefs,
        }
    }

    fn state(h: &Home, fonts: &[&str]) -> ThemeState {
        ThemeState::new(
            &h.home,
            None,
            h.prefs.clone(),
            fonts.iter().map(|s| s.to_string()).collect(),
            DARK,
        )
    }

    fn write_theme(h: &Home, file: &str, name: &str, text: &str) {
        write_padded(h, file, name, text, 0);
    }

    /// `pad` trailing spaces make each rewrite a different size, so a changed
    /// file is seen whatever the file system's clock does.
    fn write_padded(h: &Home, file: &str, name: &str, text: &str, pad: usize) {
        let body = family(name, text, "") + &" ".repeat(pad);
        fs::write(h.home.join("themes").join(file), body).unwrap();
    }

    #[test]
    fn ids_come_apart_in_the_four_shapes() {
        assert_eq!(parse_id("system"), Some(ThemeId::System));
        assert_eq!(
            parse_id("builtin:Hippocamp Dark"),
            Some(ThemeId::Builtin("Hippocamp Dark".into()))
        );
        assert_eq!(
            parse_id("user:a.json/Some/Name"),
            Some(ThemeId::User {
                file: "a.json".into(),
                name: "Some/Name".into()
            })
        );
        assert_eq!(
            parse_id("pack:p/N"),
            Some(ThemeId::Pack {
                pack: "p".into(),
                name: "N".into()
            })
        );
        assert_eq!(parse_id("user:../x"), None);
        assert_eq!(parse_id("nonsense"), None);
    }

    #[test]
    fn no_preferences_file_is_system_without_a_notice() {
        let h = home();
        let mut s = state(&h, &[]);
        let active = s.launch(false, DARK);
        assert!(active.system);
        assert_eq!(s.id(), "system");
        assert_eq!(s.notice(), None);
    }

    #[test]
    fn the_saved_theme_is_used_at_launch() {
        let h = home();
        write_theme(&h, "harbour.json", "Harbour", "#eeeeeeff");
        prefs::save(
            &h.prefs,
            &Prefs {
                theme: "user:harbour.json/Harbour".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let mut s = state(&h, &[]);
        let active = s.launch(false, DARK);
        assert_eq!(active.theme.name, "Harbour");
        assert!(!active.system);
        assert_eq!(s.notice(), None);
    }

    #[test]
    fn a_saved_id_that_resolves_to_no_theme_gives_system_and_a_notice() {
        let h = home();
        prefs::save(
            &h.prefs,
            &Prefs {
                theme: "user:gone.json/Gone".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let mut s = state(&h, &[]);
        let active = s.launch(false, DARK);
        assert!(active.system);
        let n = s.notice().expect("a notice");
        assert!(n.problem && n.text.contains("gone"), "{n:?}");
    }

    #[test]
    fn a_saved_file_that_does_not_parse_names_the_file_and_its_error() {
        let h = home();
        fs::write(h.home.join("themes/bad.json"), "{ nope").unwrap();
        prefs::save(
            &h.prefs,
            &Prefs {
                theme: "user:bad.json/Was Fine Once".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let mut s = state(&h, &[]);
        assert!(s.launch(false, DARK).system);
        let n = s.notice().unwrap();
        assert!(n.text.contains("bad.json"), "{n:?}");
        assert!(n.detail.contains("not valid JSON"), "{n:?}");
    }

    #[test]
    fn broken_preferences_give_system_and_a_notice_and_option_gives_system_quietly() {
        let h = home();
        write_theme(&h, "harbour.json", "Harbour", "#eeeeeeff");
        fs::create_dir_all(h.prefs.parent().unwrap()).unwrap();
        fs::write(&h.prefs, "{ broken").unwrap();
        let mut s = state(&h, &[]);
        assert!(s.launch(false, DARK).system);
        assert!(s.notice().unwrap().problem);

        prefs::save(
            &h.prefs,
            &Prefs {
                theme: "user:harbour.json/Harbour".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let mut s = state(&h, &[]);
        assert!(s.launch(true, DARK).system);
        let n = s.notice().unwrap();
        assert!(!n.problem, "holding Option is not a problem");
        assert_eq!(
            prefs::load(&h.prefs).theme,
            "user:harbour.json/Harbour",
            "the saved theme is untouched"
        );
    }

    #[test]
    fn a_changed_file_is_applied_and_one_that_stops_parsing_keeps_the_last_good_version() {
        let h = home();
        write_theme(&h, "harbour.json", "Harbour", "#eeeeeeff");
        let mut s = state(&h, &[]);
        s.rescan();
        s.commit("user:harbour.json/Harbour", DARK).unwrap();
        assert_eq!(s.poll_now().map(|a| a.theme.name), None, "nothing changed");

        // A different length, so the stamp changes whatever the clock does.
        write_padded(&h, "harbour.json", "Harbour", "#ff0000ff", 1);
        let a = s.poll_now().expect("the new colours apply");
        assert_eq!(a.theme.colors.text.0, 0xff0000ff);
        assert_eq!(s.notice(), None);

        fs::write(h.home.join("themes/harbour.json"), "{ broken").unwrap();
        assert!(s.poll_now().is_none(), "no change is applied");
        let n = s.notice().unwrap();
        assert!(n.problem && n.text.contains("last good"), "{n:?}");
        assert_eq!(s.committed().theme.colors.text.0, 0xff0000ff);
        assert_eq!(prefs::load(&h.prefs).theme, "user:harbour.json/Harbour");

        // Fixed again: applied, and the notice goes.
        write_padded(&h, "harbour.json", "Harbour", "#00ff00ff", 2);
        let a = s.poll_now().unwrap();
        assert_eq!(a.theme.colors.text.0, 0x00ff00ff);
        assert_eq!(s.notice(), None);

        // The file goes, then comes back holding another name.
        fs::remove_file(h.home.join("themes/harbour.json")).unwrap();
        assert!(s.poll_now().is_none());
        assert!(s.notice().unwrap().text.contains("is gone"));
        write_padded(&h, "harbour.json", "Other", "#00ff00ff", 3);
        assert!(s.poll_now().is_none());
        assert!(s.notice().unwrap().text.contains("no longer has Harbour"));
    }

    #[test]
    fn a_missing_font_family_falls_back_to_the_system_font_with_a_warning() {
        let h = home();
        fs::write(
            h.home.join("themes/f.json"),
            family(
                "Fonty",
                "#eeeeeeff",
                r#","hippocamp":{"font_family":"Nowhere Sans"}"#,
            ),
        )
        .unwrap();
        fs::write(
            h.home.join("themes/g.json"),
            family(
                "Fine",
                "#eeeeeeff",
                r#","hippocamp":{"font_family":"IBM Plex Sans"}"#,
            ),
        )
        .unwrap();
        let mut s = state(&h, &["IBM Plex Sans", "Helvetica"]);
        s.rescan();
        let fonty = s.commit("user:f.json/Fonty", DARK).unwrap();
        assert_eq!(fonty.theme.font_family, None);
        assert!(
            fonty
                .theme
                .warnings
                .iter()
                .any(|w| w.contains("Nowhere Sans")),
            "{:?}",
            fonty.theme.warnings
        );
        let listed = s.catalog().iter().find(|e| e.name == "Fonty").unwrap();
        assert!(
            listed
                .theme
                .as_ref()
                .unwrap()
                .warnings
                .iter()
                .any(|w| w.contains("not installed"))
        );
        let fine = s.commit("user:g.json/Fine", DARK).unwrap();
        assert_eq!(fine.theme.font_family.as_deref(), Some("IBM Plex Sans"));
        // With no font list at all, nothing is judged missing.
        let t = prepare(fonty.theme.clone(), &HashSet::new());
        assert!(t.warnings.len() >= fonty.theme.warnings.len());
    }

    #[test]
    fn a_failed_save_keeps_the_theme_for_the_session_and_says_so() {
        let h = home();
        write_theme(&h, "harbour.json", "Harbour", "#eeeeeeff");
        let blocker = h._dir.path().join("blocked");
        fs::write(&blocker, "x").unwrap();
        let mut s = ThemeState::new(
            &h.home,
            None,
            blocker.join("preferences.json"),
            HashSet::new(),
            DARK,
        );
        s.rescan();
        let a = s.commit("user:harbour.json/Harbour", DARK).unwrap();
        assert_eq!(a.theme.name, "Harbour");
        assert_eq!(s.committed().theme.name, "Harbour", "kept for the session");
        let n = s.notice().unwrap();
        assert!(n.problem && n.text.contains("Could not save"), "{n:?}");
    }

    #[test]
    fn use_system_switches_and_saves() {
        let h = home();
        write_theme(&h, "harbour.json", "Harbour", "#eeeeeeff");
        let mut s = state(&h, &[]);
        s.rescan();
        s.commit("user:harbour.json/Harbour", DARK).unwrap();
        assert_eq!(prefs::load(&h.prefs).theme, "user:harbour.json/Harbour");
        let a = s.use_system(WindowAppearance::Light);
        assert!(a.system);
        assert_eq!(a.theme.name, "Hippocamp Light");
        assert_eq!(prefs::load(&h.prefs).theme, "system");
        assert_eq!(
            s.poll_now().map(|_| ()),
            None,
            "System has no file to watch"
        );
    }

    #[test]
    fn the_catalogue_lists_a_pack_and_refuses_a_bad_one() {
        let h = home();
        let ws = h._dir.path().join("ws");
        let pack = ws.join(".hippocamp/packs/seaside");
        fs::create_dir_all(&pack).unwrap();
        fs::write(pack.join("pack.toml"), "desktop_theme_format = 1\n").unwrap();
        fs::write(
            pack.join("theme.json"),
            r##"{"name":"Seaside","author":"a","themes":[{"name":"Seaside Dark","appearance":"dark","style":{"background":"#101010ff"}}]}"##,
        )
        .unwrap();
        let bad = ws.join(".hippocamp/packs/bad");
        fs::create_dir_all(&bad).unwrap();
        fs::write(bad.join("pack.toml"), "desktop_theme_format = 1\n").unwrap();
        fs::write(bad.join("theme.json"), "{}").unwrap();
        let mut s = ThemeState::new(&h.home, Some(&ws), h.prefs.clone(), HashSet::new(), DARK);
        s.rescan();
        let ok = s
            .catalog()
            .iter()
            .find(|e| e.id == "pack:seaside/Seaside Dark")
            .unwrap();
        assert!(ok.theme.is_ok());
        assert!(
            s.catalog()
                .iter()
                .any(|e| e.id.starts_with("pack:bad/") && e.theme.is_err())
        );
        assert!(s.resolve("pack:bad/theme.json", DARK).is_err());
    }

    #[test]
    fn names_are_shown_safely_and_briefly() {
        assert_eq!(shown("plain"), "plain");
        assert_eq!(shown("a\u{202e}b"), "a\\u{202E}b");
        let long = "x".repeat(100);
        assert_eq!(shown(&long).chars().count(), 64);
        assert!(shown(&long).ends_with('…'));
    }
}
