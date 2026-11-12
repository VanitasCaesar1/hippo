//! Brief a new agent: the Ask bar's Brief mode. A task becomes a start
//! block (`hippo_core::brief`): required and relevant notes, past sessions
//! as short digests, kept rules; each part can be turned off. Copy, Start
//! claude here (Terminal, the block as the first prompt read from a private
//! file) and Send to T3 Code (its `t3code://new` draft link).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use gpui::{
    ClipboardItem, Context, FocusHandle, Focusable, FontWeight, Role, SharedString, Window, div,
    prelude::*, px,
};
use hippo_core::brief::{self, Brief, Group};
use hippo_core::config::{Config, Workspace};
use hippo_view::briefing;

use super::Shell;
use super::kit::{self, Kind};
use super::theme::Look;
use crate::state::{Section, Tab};

/// Brief's state.
pub struct BriefUi {
    /// The Ask bar is in Brief mode.
    pub mode: bool,
    pub task: String,
    pub result: Option<Result<Brief, String>>,
    pub building: bool,
    /// Part ids turned off.
    pub off: HashSet<String>,
    pub selected: usize,
    /// The profile to start on; `None` is Use next.
    pub profile: Option<String>,
    pub repo: Option<PathBuf>,
    /// Which menu is open: `profile` or `repo`.
    pub menu: Option<&'static str>,
    /// What the last Copy, Start or Send did.
    pub said: Option<Result<String, String>>,
    pub focus: FocusHandle,
    pub seq: u64,
    /// Below 1000 points: the block instead of the parts.
    pub show_block: bool,
    /// The person chose the repository; a brief's guess leaves it.
    pub repo_picked: bool,
}

impl BriefUi {
    pub fn new(focus: FocusHandle) -> BriefUi {
        BriefUi {
            mode: false,
            task: String::new(),
            result: None,
            building: false,
            off: HashSet::new(),
            selected: 0,
            profile: None,
            repo: None,
            menu: None,
            said: None,
            focus,
            seq: 0,
            repo_picked: false,
            show_block: false,
        }
    }

    pub(crate) fn brief(&self) -> Option<&Brief> {
        self.result.as_ref().and_then(|r| r.as_ref().ok())
    }

    /// The block with the parts on.
    pub fn block(&self) -> Option<String> {
        let b = self.brief()?;
        Some(brief::assemble(b, |p| !self.off.contains(&p.id)))
    }
}

/// Whether `codex` is on the PATH the app runs with, or in the usual
/// places a shell's PATH would find it.
pub fn codex_installed() -> bool {
    static FOUND: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FOUND.get_or_init(|| {
        let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        if let Some(h) = hippo_core::config::user_home() {
            dirs.push(h.join(".local/bin"));
            dirs.push(h.join(".cargo/bin"));
        }
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
        dirs.iter().any(|d| d.join("codex").is_file())
    })
}

/// A profile's meter as the menu says it: `5h 92% · 7d 23%`.
fn meter_words(h: &hippo_core::usage::Headroom) -> String {
    let pct = |f: f64| (f * 100.).round() as i64;
    match (h.five, h.seven) {
        (Some(f), Some(s)) => format!("5h {}% · 7d {}%", pct(f.frac), pct(s.frac)),
        (Some(f), None) => format!("5h {}%", pct(f.frac)),
        (None, Some(s)) => format!("7d {}%", pct(s.frac)),
        (None, None) => h
            .left()
            .map(|l| format!("{}% left", pct(l)))
            .unwrap_or_default(),
    }
}

/// An ISO day in a reason (`2026-11-09`) as the app writes days:
/// the weekday within a week, else `9 Nov`.
fn app_dates(text: &str) -> String {
    let today = time::OffsetDateTime::now_utc()
        .to_offset(super::status::local_offset())
        .date();
    app_dates_at(text, today)
}

fn app_dates_at(text: &str, today: time::Date) -> String {
    let b = text.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < text.len() {
        if i + 10 <= text.len()
            && b[i..i + 10].iter().enumerate().all(|(k, c)| {
                if k == 4 || k == 7 {
                    *c == b'-'
                } else {
                    c.is_ascii_digit()
                }
            })
            && let Ok(d) = time::Date::parse(
                &text[i..i + 10],
                time::macros::format_description!("[year]-[month]-[day]"),
            )
        {
            out.push_str(&if (today - d).whole_days().abs() < 7 {
                d.weekday().to_string()[..3].to_owned()
            } else {
                format!("{} {}", d.day(), &d.month().to_string()[..3])
            });
            i += 10;
            continue;
        }
        let c = text[i..].chars().next().expect("in bounds");
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// The required part for the entry note itself, when the entry part `id`
/// has one (or the entry part, given that required part): Brief shows them
/// as one row.
fn entry_twin(b: &Brief, id: &str) -> Option<String> {
    let entry = b.parts.iter().find(|p| p.id == "entry")?;
    let twin = b
        .parts
        .iter()
        .find(|p| p.group == Group::Required && p.id != "entry" && p.note == entry.note)?;
    if id == "entry" {
        Some(twin.id.clone())
    } else if id == twin.id {
        Some("entry".to_owned())
    } else {
        None
    }
}

/// Whether T3 Code is installed (it registers `t3code://`).
pub fn t3_installed() -> bool {
    t3_app().is_some()
}

/// T3 Code's application bundle, in `/Applications` or `~/Applications`
/// (`T3 Code.app`, `T3 Code (Nightly).app`, …).
pub fn t3_app() -> Option<PathBuf> {
    let mut dirs = vec![PathBuf::from("/Applications")];
    if let Some(h) = hippo_core::config::user_home() {
        dirs.push(h.join("Applications"));
    }
    dirs.iter().find_map(|d| {
        let mut found: Vec<PathBuf> = std::fs::read_dir(d)
            .ok()?
            .flatten()
            .filter(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                n.starts_with("T3 Code") && n.ends_with(".app")
            })
            .map(|e| e.path())
            .collect();
        found.sort();
        found.into_iter().next()
    })
}

/// Writes the block to a private temporary file (0600) and the one-shot
/// script beside it (0700), in a new directory of their own (0700), and
/// returns the script's path. The one write the app makes outside its
/// preferences file: it writes
/// into the system's temporary directory, never into hippo's or an
/// agent's files; the script deletes both when it runs.
#[allow(clippy::disallowed_methods)]
fn write_start_files(
    agent: briefing::Agent,
    block: &str,
    repo: &Path,
    config_dir: Option<&Path>,
) -> std::io::Result<PathBuf> {
    use std::io::Write as _;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
    let base = std::env::temp_dir().join(format!("hippo-brief-{}", ulid::Ulid::generate()));
    std::fs::DirBuilder::new().mode(0o700).create(&base)?;
    let file = base.join("brief.txt");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&file)?;
    f.write_all(block.as_bytes())?;
    let script = base.join(format!("start-{}.command", agent.program()));
    std::fs::write(
        &script,
        briefing::start_script_for(agent, repo, &file, config_dir),
    )?;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))?;
    Ok(script)
}

/// The repository most of a brief's checked past sessions worked in, from
/// the rollup, read-only: the project named most often, and a
/// folder for it. A session run from a parent folder (T3 Code's project
/// root) still names the repository it worked in as its project, so the
/// folder is the one named like the project: a session's working folder,
/// else a sibling of the workspace or of a session's folder.
pub fn repo_of_sessions(home: &Path, b: &Brief) -> Option<PathBuf> {
    let rollup = hippo_core::rollup::Rollup::open(home).ok().flatten()?;
    let user = hippo_core::config::user_home();
    let mut projects: Vec<(String, usize)> = Vec::new();
    let mut cwds: Vec<PathBuf> = Vec::new();
    for p in b
        .parts
        .iter()
        .filter(|p| p.group == Group::Session && !p.weak)
    {
        let Some(f) = p
            .session
            .as_deref()
            .and_then(|id| rollup.session(id).ok().flatten())
        else {
            continue;
        };
        if let Some(cwd) = hippo_core::session_meta::origin(&f.id, &f.tool, &f.transcript_path).cwd
            && Some(&cwd) != user.as_ref()
            && cwd.is_dir()
            && !cwds.contains(&cwd)
        {
            cwds.push(cwd);
        }
        if let Some(name) = f.project.clone() {
            match projects.iter_mut().find(|(n, _)| *n == name) {
                Some((_, k)) => *k += 1,
                None => projects.push((name, 1)),
            }
        }
    }
    // The most named; on a tie, the first (the best-ranked session).
    let best = projects.iter().map(|(_, n)| *n).max();
    let project = best.and_then(|b| {
        projects
            .iter()
            .find(|(_, n)| *n == b)
            .map(|(p, _)| p.clone())
    });
    let Some(project) = project else {
        return cwds.into_iter().next();
    };
    let named = |p: &Path| p.file_name().is_some_and(|n| n == project.as_str());
    if let Some(c) = cwds.iter().find(|c| named(c)) {
        return Some(c.clone());
    }
    let mut parents: Vec<PathBuf> = cwds
        .iter()
        .filter_map(|c| c.parent().map(Path::to_path_buf))
        .collect();
    if let Some(ws) = Config::load_in(home)
        .ok()
        .and_then(|c| Workspace::load_optional(&c).ok().flatten())
    {
        parents.extend(ws.root.parent().map(Path::to_path_buf));
    }
    parents
        .into_iter()
        .map(|p| p.join(&project))
        .find(|p| p.is_dir())
        .or_else(|| cwds.into_iter().next())
}

/// `owner/name` of the repository's GitHub `origin`, if it has one.
fn github_of(repo: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| briefing::github_slug(&String::from_utf8_lossy(&out.stdout)))
        .flatten()
}

impl Shell {
    /// Puts the Ask bar in Brief mode and focuses it (Manage's Preview for
    /// a task, ⌘B in the bar, the Brief segment).
    pub(crate) fn open_brief_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.brief.mode = true;
        self.state.tabs.select(Section::Home);
        let focus = self.ask_field.focus_handle(cx);
        window.focus(&focus, cx);
        self.after_state_change(window, cx);
    }

    pub(crate) fn set_brief_mode(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.brief.mode = on;
        let focus = self.ask_field.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    /// The repositories Brief offers: hippo's workspace, then today's
    /// sessions' working directories, newest first.
    fn brief_repos(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        if let Some(w) = self.loaded.as_ref().and_then(|l| l.setup.workspace()) {
            out.push(w.root.clone());
        }
        let home = hippo_core::config::user_home();
        if let Some(f) = self.loaded.as_ref().and_then(|l| l.fleet.as_ref()) {
            let mut rows: Vec<_> = f.rows.iter().collect();
            rows.sort_by_key(|r| std::cmp::Reverse(r.last));
            for r in rows {
                if let Some(c) = &r.cwd
                    && Some(c) != home.as_ref()
                    && !out.contains(c)
                    && c.is_dir()
                {
                    out.push(c.clone());
                }
            }
        }
        out
    }

    /// Builds the brief for the Ask bar's text, in the background.
    pub(crate) fn submit_brief(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let task = self.ask_field.read(cx).text().trim().to_owned();
        if task.is_empty() {
            return;
        }
        let Some(loaded) = self.loaded.as_ref() else {
            return;
        };
        let (Some(cfg), Some(ws)) = (
            loaded.setup.config().cloned(),
            loaded.setup.workspace().cloned(),
        ) else {
            self.brief.result = Some(Err(
                "Brief needs a notes folder: the step-two sheet gives your agents a memory.".into(),
            ));
            self.state.tabs.select(Section::Home);
            self.state.tabs.open(Tab::Brief);
            cx.notify();
            return;
        };
        self.brief.seq += 1;
        let seq = self.brief.seq;
        self.brief.task = task.clone();
        self.brief.building = true;
        self.brief.result = None;
        self.brief.off.clear();
        self.brief.selected = 0;
        self.brief.said = None;
        if self.brief.repo.is_none() {
            self.brief.repo = self.brief_repos().into_iter().next();
        }
        if self.state.tabs.section() != Section::Home {
            self.state.tabs.select(Section::Home);
        }
        self.state.tabs.open(Tab::Brief);
        cx.spawn(async move |this, cx| {
            let (result, guess) = cx
                .background_executor()
                .spawn(async move {
                    let r = brief::run(&cfg, &ws, &task).map_err(|e| e.to_string());
                    let guess = r.as_ref().ok().and_then(|b| repo_of_sessions(&cfg.home, b));
                    (r, guess)
                })
                .await;
            this.update(cx, |this, cx| {
                if this.brief.seq != seq {
                    return;
                }
                // The repository most of the checked past sessions
                // ran in, unless the person chose one.
                if let Some(g) = guess.filter(|_| !this.brief.repo_picked) {
                    this.brief.repo = Some(g);
                }
                this.brief.building = false;
                // A weak relevant pick starts off.
                if let Ok(b) = &result {
                    this.brief.off = b
                        .parts
                        .iter()
                        .filter(|p| p.weak)
                        .map(|p| p.id.clone())
                        .collect();
                }
                this.brief.result = Some(result);
                cx.notify();
            })
            .ok();
        })
        .detach();
        window.focus(&self.brief.focus, cx);
        cx.notify();
    }

    /// The profile a start uses and its config dir: the chosen one, else
    /// Use next, else the default.
    fn brief_profile(&self) -> (String, Option<PathBuf>) {
        let fleet = self.loaded.as_ref().and_then(|l| l.fleet.as_ref());
        let name = self
            .brief
            .profile
            .clone()
            .or_else(|| fleet.and_then(|f| f.use_next.clone()))
            .unwrap_or_else(|| "default".into());
        let dir = fleet
            .and_then(|f| f.headroom.iter().find(|h| h.profile == name))
            .and_then(|h| h.dir.clone());
        // The default profile runs without CLAUDE_CONFIG_DIR.
        let dir =
            dir.filter(|d| hippo_core::config::user_home().is_none_or(|h| *d != h.join(".claude")));
        (name, dir)
    }

    pub(crate) fn brief_copy(&mut self, cx: &mut Context<Self>) {
        if let Some(b) = self.brief.block() {
            cx.write_to_clipboard(ClipboardItem::new_string(b.clone()));
            self.brief.said = Some(Ok(format!(
                "copied the brief ({})",
                hippo_view::memory::human(b.len() as u64)
            )));
            cx.notify();
        }
    }

    /// Start claude here (or Start for Codex): a private file with the
    /// block and a one-shot script, opened in Terminal.
    fn brief_start(&mut self, agent: briefing::Agent, cx: &mut Context<Self>) {
        let (Some(block), Some(repo)) = (self.brief.block(), self.brief.repo.clone()) else {
            return;
        };
        let dir = match agent {
            briefing::Agent::Claude => self.brief_profile().1,
            briefing::Agent::Codex => None,
        };
        let term = self.term();
        let r = write_start_files(agent, &block, &repo, dir.as_deref()).and_then(|script| {
            crate::launch::open_script(&term, &script).map_err(std::io::Error::other)?;
            Ok(format!(
                "started in {}: {}",
                term.name(),
                briefing::start_line_for(agent, &repo, dir.as_deref())
            ))
        });
        self.brief.said = Some(r.map_err(|e| format!("could not start {}: {e}", agent.program())));
        cx.notify();
    }

    fn brief_send_t3(&mut self, cx: &mut Context<Self>) {
        let Some(block) = self.brief.block() else {
            return;
        };
        let slug = self.brief.repo.as_deref().and_then(github_of);
        let Some(link) = briefing::t3_link(&block, slug.as_deref()) else {
            self.brief.said = Some(Err(format!(
                "over T3 Code's {}-character link: turn parts off or Copy",
                briefing::T3_PROMPT_LIMIT
            )));
            cx.notify();
            return;
        };
        let r = std::process::Command::new("/usr/bin/open")
            .arg(&link)
            .status();
        self.brief.said = Some(match r {
            Ok(s) if s.success() => Ok(format!(
                "sent to T3 Code as a draft{}: nothing runs until you press Send there",
                slug.map(|s| format!(" for {s}")).unwrap_or_default()
            )),
            Ok(_) => Err("open did not accept the t3code:// link".into()),
            Err(e) => Err(format!("cannot run open: {e}")),
        });
        cx.notify();
    }

    fn brief_toggle(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(id) = self
            .brief
            .brief()
            .and_then(|b| b.parts.get(i))
            .map(|p| p.id.clone())
        else {
            return;
        };
        let on = !self.brief.off.remove(&id);
        if on {
            self.brief.off.insert(id.clone());
        }
        // The entry note's own required row is merged into the entry row:
        // they turn on and off together.
        if let Some(twin) = self.brief.brief().and_then(|b| entry_twin(b, &id)) {
            if on {
                self.brief.off.insert(twin);
            } else {
                self.brief.off.remove(&twin);
            }
        }
        self.brief.selected = i;
        cx.notify();
    }

    pub(super) fn brief_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let n = self.brief.brief().map_or(0, |b| b.parts.len());
        match key {
            "down" | "j" if n > 0 => self.brief.selected = (self.brief.selected + 1).min(n - 1),
            "up" | "k" => self.brief.selected = self.brief.selected.saturating_sub(1),
            "space" => self.brief_toggle(self.brief.selected, cx),
            _ => return false,
        }
        cx.notify();
        true
    }

    pub(super) fn render_brief_tab(
        &mut self,
        look: &Look,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let width = window.viewport_size().width;
        let side = width >= px(1000.);
        // The composer at the bottom, as on Home.
        let bar = self.render_composer(false, look, window, cx);
        let body: gpui::AnyElement = match &self.brief.result {
            None => div()
                .p(look.px(16.))
                .text_color(look.c.text_muted)
                .child(if self.brief.building {
                    "Assembling the brief: the notes for the task, past sessions, kept rules…"
                } else {
                    "Describe a task in the bar and press ↩."
                })
                .into_any_element(),
            Some(Err(e)) => div()
                .p(look.px(16.))
                .text_color(look.c.error)
                .child(e.clone())
                .into_any_element(),
            Some(Ok(b)) => {
                let b = b.clone();
                let parts = self.brief_parts(&b, look, cx);
                let block = self.brief.block().unwrap_or_default();
                let preview = div()
                    .id("brief-block")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .min_h(look.px(160.))
                    .gap(look.px(8.))
                    .child(
                        div()
                            .flex()
                            .gap(look.px(8.))
                            .child(kit::label(
                                look,
                                "THE BLOCK THE AGENT SEES",
                                look.c.text_muted,
                            ))
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_size(look.text(11.5))
                                    .text_color(kit::dim(look))
                                    .child("its first prompt"),
                            ),
                    )
                    .child(
                        div()
                            .id("brief-block-text")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .p(look.px(12.))
                            .rounded(look.px(8.))
                            .border(look.fixed(1.))
                            .border_color(look.c.border_variant)
                            .bg(look.c.window)
                            .font_family("Menlo")
                            .text_size(look.text(11.5))
                            .line_height(look.px(17.))
                            .children(block.lines().map(|l| {
                                let head =
                                    l.ends_with(':') && !l.starts_with(' ') && !l.starts_with('-')
                                        || l.starts_with("# hippo brief");
                                div()
                                    .min_h(look.px(17.))
                                    .when(head, |d| d.text_color(look.c.text_accent))
                                    .child(l.to_owned())
                            })),
                    );
                if side {
                    div()
                        .flex()
                        .flex_1()
                        .min_h_0()
                        .gap(look.px(16.))
                        .child(
                            div()
                                .id("brief-parts-scroll")
                                .w(gpui::relative(0.47))
                                .flex_shrink_0()
                                .overflow_y_scroll()
                                .child(parts),
                        )
                        .child(preview)
                        .into_any_element()
                } else {
                    // Below 1000 points: Parts | Block over one pane.
                    let show_block = self.brief.show_block;
                    let seg = |id: &'static str, label: &'static str, on: bool, block: bool| {
                        div()
                            .id(id)
                            .role(Role::Tab)
                            .aria_selected(on)
                            .px(look.px(12.))
                            .h(look.px(24.))
                            .flex()
                            .items_center()
                            .rounded(look.px(5.))
                            .cursor_pointer()
                            .when(on, |d| d.bg(look.c.selected).text_color(look.c.text))
                            .when(!on, |d| d.text_color(look.c.text_muted))
                            .child(label)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.brief.show_block = block;
                                cx.notify();
                            }))
                    };
                    let toggle = div()
                        .flex()
                        .flex_shrink_0()
                        .p(look.px(2.))
                        .rounded(look.px(7.))
                        .border(look.fixed(1.))
                        .border_color(look.c.border)
                        .child(seg("brief-show-parts", "Parts", !show_block, false))
                        .child(seg("brief-show-block", "Block", show_block, true));
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .gap(look.px(10.))
                        .child(div().flex().child(toggle))
                        .child(if show_block {
                            div()
                                .flex()
                                .flex_1()
                                .min_h_0()
                                .child(preview)
                                .into_any_element()
                        } else {
                            div()
                                .id("brief-stack")
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .child(parts)
                                .into_any_element()
                        })
                        .into_any_element()
                }
            }
        };
        let footer = self.brief_footer(look, cx);
        div()
            .id("brief")
            .key_context("Brief")
            .track_focus(&self.brief.focus)
            .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, _, cx| {
                let k = &e.keystroke;
                if k.modifiers.platform || k.modifiers.control || k.modifiers.alt {
                    return;
                }
                if this.brief_key(k.key.as_str(), cx) {
                    cx.stop_propagation();
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            .gap(look.px(12.))
            .px(look.px(16.))
            .pt(look.px(12.))
            .pb(look.px(12.))
            .text_size(look.text(13.))
            .child(body)
            .children(footer)
            .child(bar)
            .into_any_element()
    }

    fn brief_parts(&self, b: &Brief, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let on_bytes: usize = b
            .parts
            .iter()
            .filter(|p| !self.brief.off.contains(&p.id))
            .map(|p| p.bytes)
            .sum();
        let total = self.brief.block().map_or(on_bytes, |t| t.len());
        let over = total > b.budget;
        let group_color = |g: Group| match g {
            Group::Required => look.c.text_muted,
            Group::Relevant => look.c.text_accent,
            Group::Session => look.c.text_accent.opacity(0.6),
            Group::Rule => kit::ok(look),
        };
        // The meter: a segment per group, on parts only.
        let segs: Vec<(Group, usize)> = [
            Group::Required,
            Group::Relevant,
            Group::Session,
            Group::Rule,
        ]
        .iter()
        .map(|g| {
            (
                *g,
                b.parts
                    .iter()
                    .filter(|p| p.group == *g && !self.brief.off.contains(&p.id))
                    .map(|p| p.bytes)
                    .sum(),
            )
        })
        .collect();
        let scale = b.budget.max(total).max(1) as f32;
        let meter = div()
            .flex()
            .flex_1()
            .h(look.px(8.))
            .rounded(look.px(4.))
            .overflow_hidden()
            .bg(look.c.border_variant)
            .children(segs.iter().filter(|(_, n)| *n > 0).map(|(g, n)| {
                div()
                    .h_full()
                    .w(gpui::relative(*n as f32 / scale))
                    .bg(group_color(*g))
            }));
        let mut col = div()
            .flex()
            .flex_col()
            .gap(look.px(2.))
            .child(
                div()
                    .flex()
                    .gap(look.px(8.))
                    .child(kit::label(
                        look,
                        "CONTEXT HIPPO WILL HAND OVER",
                        look.c.text_muted,
                    ))
                    .child(div().flex_1()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(10.))
                    .py(look.px(6.))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if over { look.c.warning } else { look.c.text })
                            .child(hippo_view::memory::human(total as u64)),
                    )
                    .child(meter)
                    .child(
                        div()
                            .text_size(look.text(12.))
                            .text_color(if over { look.c.warning } else { kit::dim(look) })
                            .child(format!(
                                "of {}{}",
                                hippo_view::memory::human(b.budget as u64),
                                if over { " · over the budget" } else { "" }
                            )),
                    ),
            );
        let heads = [
            (Group::Required, "REQUIRED · EVERY AGENT GETS THESE"),
            (Group::Relevant, "RELEVANT NOTES · PICKED FOR THIS TASK"),
            (Group::Session, "PAST SESSIONS · HANDOFFS, NOT TRANSCRIPTS"),
            (Group::Rule, "RULES · KEPT LESSONS MATCHING THE TASK"),
        ];
        for (g, head) in heads {
            let idx: Vec<usize> = b
                .parts
                .iter()
                .enumerate()
                .filter(|(_, p)| p.group == g)
                .map(|(i, _)| i)
                .collect();
            let sum: usize = idx.iter().map(|i| b.parts[*i].bytes).sum();
            col = col.child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(8.))
                    .pt(look.px(10.))
                    .pb(look.px(2.))
                    .child(kit::label(look, head, look.c.text_muted))
                    .child(div().flex_1().h(look.fixed(1.)).bg(look.c.border_variant))
                    .child(
                        div()
                            .flex_shrink_0()
                            .w(look.px(62.))
                            .flex()
                            .justify_end()
                            .font_family("Menlo")
                            .text_size(look.text(11.5))
                            .text_color(kit::dim(look))
                            .child(if idx.is_empty() {
                                "—".to_owned()
                            } else {
                                hippo_view::memory::human(sum as u64)
                            }),
                    ),
            );
            let twin = entry_twin(b, "entry");
            for i in idx {
                let p = &b.parts[i];
                if twin.as_deref() == Some(p.id.as_str()) {
                    continue;
                }
                let on = !self.brief.off.contains(&p.id);
                let sel = self.brief.selected == i;
                let title = if p.id == "entry" {
                    let stem = p
                        .note
                        .as_ref()
                        .and_then(|n| n.file_stem())
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| p.title.clone());
                    format!("{stem} · entry note")
                } else {
                    p.title.clone()
                };
                let bytes = p.bytes
                    + twin
                        .as_ref()
                        .filter(|_| p.id == "entry")
                        .and_then(|t| b.parts.iter().find(|q| &q.id == t))
                        .map_or(0, |q| q.bytes);
                let reason = app_dates(&p.reason);
                let detail = match (p.group, p.note_bytes) {
                    (Group::Required, Some(nb)) if p.id == "entry" => format!(
                        "its opening and its line · note {}",
                        hippo_view::memory::human(nb)
                    ),
                    (Group::Required, Some(nb)) => {
                        format!("{reason} · note {}", hippo_view::memory::human(nb))
                    }
                    _ if p.weak => format!("weak match · {reason}"),
                    _ => reason,
                };
                col = col.child(
                    div()
                        .id(("brief-part", i))
                        .role(Role::CheckBox)
                        .aria_label(p.title.clone())
                        .aria_selected(on)
                        .flex()
                        .items_center()
                        .gap(look.px(10.))
                        .h(look.px(34.))
                        .px(look.px(6.))
                        .rounded(look.px(5.))
                        .cursor_pointer()
                        .when(sel, |d| d.bg(look.c.hover))
                        .hover(|h| h.bg(look.c.hover))
                        .child(
                            div()
                                .flex_shrink_0()
                                .size(look.px(16.))
                                .rounded(look.px(4.))
                                .border(look.fixed(1.5))
                                .border_color(if on {
                                    look.c.text_accent
                                } else {
                                    look.c.border
                                })
                                .when(on, |d| d.bg(look.c.text_accent))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(look.text(11.))
                                .text_color(look.c.window)
                                .child(if on { "✓" } else { "" }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .when(!on, |d| d.text_color(look.c.text_muted))
                                .child(title),
                        )
                        .child(
                            div()
                                .flex_shrink_1()
                                .max_w(look.px(240.))
                                .truncate()
                                .text_size(look.text(11.5))
                                .text_color(if p.weak {
                                    look.c.warning
                                } else {
                                    kit::dim(look)
                                })
                                .child(detail),
                        )
                        .child(
                            // Sizes: a 56-point right-aligned tabular column.
                            div()
                                .flex_shrink_0()
                                .w(look.px(56.))
                                .flex()
                                .justify_end()
                                .text_size(look.text(12.))
                                .font_family("Menlo")
                                .text_color(look.c.text_muted)
                                .child(hippo_view::memory::human(bytes as u64)),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.brief_toggle(i, cx))),
                );
            }
        }
        col.into_any_element()
    }

    /// A chip in the composer's foot: 26
    /// high, round, an outline, its lead, its label and a chevron.
    #[allow(clippy::too_many_arguments)]
    fn brief_chip(
        &self,
        look: &Look,
        id: &'static str,
        which: &'static str,
        lead: gpui::AnyElement,
        label: String,
        tail: Option<gpui::AnyElement>,
        tip: String,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let open = self.brief.menu == Some(which);
        div()
            .id(id)
            .role(Role::Button)
            .aria_label(label.clone())
            .aria_expanded(open)
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(look.px(6.))
            .h(look.px(26.))
            .pl(look.px(8.))
            .pr(look.px(7.))
            .rounded_full()
            .border(look.fixed(1.))
            .border_color(look.c.border_strong)
            .when(open, |d| d.bg(look.c.raised))
            .hover(|h| h.bg(look.c.hover))
            .cursor_pointer()
            .text_size(look.text(12.5))
            .text_color(look.c.text)
            .whitespace_nowrap()
            .child(lead)
            .child(div().max_w(look.px(140.)).truncate().child(label))
            .children(tail)
            .child(kit::icon(look, "chevron-down", 12., look.c.faint))
            .tooltip(super::tooltip::Tooltip::text(tip))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.brief.menu = if this.brief.menu == Some(which) {
                    None
                } else {
                    Some(which)
                };
                cx.notify();
            }))
            .into_any_element()
    }

    /// The repository and account chips in Brief's composer.
    pub(super) fn brief_chips(&self, look: &Look, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let fleet = self.loaded.as_ref().and_then(|l| l.fleet.as_ref());
        let (profile, _) = self.brief_profile();
        let use_next = self.brief.profile.is_none();
        let repo = self
            .brief
            .repo
            .as_ref()
            .and_then(|r| r.file_name())
            .map(|n| n.to_string_lossy().into_owned());
        let repo_tip = self.brief.repo.as_ref().map_or_else(
            || "No repository: pick one to start an agent in it".to_owned(),
            |r| format!("Starts in {}", super::manage_page::tilde(r)),
        );
        let repo_chip = self.brief_chip(
            look,
            "brief-repo",
            "repo",
            kit::icon(look, "folder", 14., look.c.text_muted).into_any_element(),
            repo.unwrap_or_else(|| "no repository".into()),
            None,
            repo_tip,
            cx,
        );
        let left = fleet
            .and_then(|f| f.room(&profile))
            .and_then(super::home_panes::tighter);
        let tail = left.as_ref().map(|l| {
            let guess = hippo_view::fleet::inferred(l);
            div()
                .flex()
                .items_center()
                .gap(look.px(5.))
                .child(kit::usage_ring(look, l.frac, guess, 14.))
                .child(
                    div()
                        .text_color(if guess {
                            look.c.faint
                        } else {
                            look.c.text_muted
                        })
                        .child(super::home::pct_left(l)),
                )
                .into_any_element()
        });
        let account_tip = format!(
            "{}{}",
            kit::profile_name(&profile),
            if use_next {
                ": the account to use next"
            } else {
                ""
            }
        );
        let account_chip = self.brief_chip(
            look,
            "brief-profile",
            "profile",
            kit::mark(look, Some(&profile), 18.).into_any_element(),
            kit::profile_name(&profile),
            tail,
            account_tip,
            cx,
        );
        vec![repo_chip, account_chip]
    }

    /// Brief's actions at the right of the composer's foot: Start in Claude Code and Start in Codex as ghost
    /// buttons, Send to T3 Code ⌘↵ the primary. `None` until a brief is
    /// built.
    pub(super) fn brief_actions(
        &self,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        self.brief.brief()?;
        let block_len = self.brief.block().map_or(0, |b| b.chars().count());
        let t3 = t3_installed();
        let t3_fits = block_len <= briefing::T3_PROMPT_LIMIT;
        let can_start = self.brief.repo.is_some();
        let start_tip = |what: &str| {
            if can_start {
                what.to_owned()
            } else {
                "Pick a repository first".to_owned()
            }
        };
        Some(
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(look.px(4.))
                .ml_auto()
                .child(
                    kit::button(
                        look,
                        "brief-start",
                        "Start in Claude Code",
                        if can_start {
                            Kind::Ghost
                        } else {
                            Kind::Disabled
                        },
                    )
                    .border_color(gpui::transparent_black())
                    .tooltip(super::tooltip::Tooltip::text(start_tip(
                        "Opens the terminal in the repository and runs claude with the brief as its first prompt",
                    )))
                    .when(can_start, |b| {
                        b.on_click(cx.listener(|this, _, _, cx| {
                            this.brief_start(briefing::Agent::Claude, cx)
                        }))
                    }),
                )
                .when(codex_installed(), |d| {
                    d.child(
                        kit::button(
                            look,
                            "brief-codex",
                            "Start in Codex",
                            if can_start {
                                Kind::Ghost
                            } else {
                                Kind::Disabled
                            },
                        )
                        .border_color(gpui::transparent_black())
                        .tooltip(super::tooltip::Tooltip::text(start_tip(
                            "Opens the terminal (Manage › Settings) in the repository and runs codex with the brief as its first prompt, read from a private file that is deleted first.",
                        )))
                        .when(can_start, |b| {
                            b.on_click(cx.listener(|this, _, _, cx| {
                                this.brief_start(briefing::Agent::Codex, cx)
                            }))
                        }),
                    )
                })
                .when(t3, |d| {
                    d.child(
                        kit::button_key(
                            look,
                            "brief-t3",
                            "Send to T3 Code",
                            "⌘↵",
                            if t3_fits {
                                Kind::Primary
                            } else {
                                Kind::Disabled
                            },
                        )
                        .when(t3_fits, |b| {
                            b.tooltip(super::tooltip::Tooltip::text(
                                "Opens a draft in T3 Code: nothing runs until you press Send there",
                            ))
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.brief_send_t3(cx)
                            }))
                        })
                        .when(!t3_fits, |b| {
                            b.tooltip(super::tooltip::Tooltip::text(format!(
                                "over T3 Code's {}-character link: turn parts off or Copy",
                                briefing::T3_PROMPT_LIMIT
                            )))
                        }),
                    )
                })
                .into_any_element(),
        )
    }

    /// ⌘↵ in Brief's composer: Send to T3 Code, when a brief is built, it
    /// fits the link and the window is engaged (the key guard's rule, as
    /// any key that acts outside the app).
    pub(crate) fn brief_send_key(&mut self, cx: &mut Context<Self>) {
        if self.brief.brief().is_none() || !t3_installed() {
            return;
        }
        if !crate::keyguard::allow(cx) {
            cx.notify();
            return;
        }
        self.brief_send_t3(cx);
    }

    /// Above the composer: the open chip's menu and what the last action
    /// said.
    fn brief_footer(&self, look: &Look, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let fleet = self.loaded.as_ref().and_then(|l| l.fleet.as_ref());
        let mut menu: Option<gpui::AnyElement> = None;
        if let Some(which) = self.brief.menu {
            let items: Vec<(String, Option<String>, Option<PathBuf>)> = if which == "profile" {
                let mut v = vec![("Use next".to_owned(), None, None)];
                if let Some(f) = fleet {
                    // The headroom strip's order and figure: what is
                    // left of the tighter window.
                    for h in hippo_view::fleet::ordered_rooms(&f.headroom)
                        .into_iter()
                        .filter(|h| !h.codex)
                    {
                        let words = meter_words(h);
                        let pct = if words.is_empty() {
                            String::new()
                        } else {
                            format!(" · {words}")
                        };
                        let signed = if f.signed_out.contains(&h.profile) {
                            " · sign-in failed"
                        } else {
                            ""
                        };
                        v.push((
                            format!("{}{pct}{signed}", kit::profile_name(&h.profile)),
                            Some(h.profile.clone()),
                            None,
                        ));
                    }
                }
                v
            } else {
                self.brief_repos()
                    .into_iter()
                    .map(|r| (super::manage_page::tilde(&r), None, Some(r)))
                    .collect()
            };
            let mut rows: Vec<gpui::AnyElement> = Vec::new();
            for (i, (label, p, r)) in items.into_iter().enumerate() {
                rows.push(
                    div()
                        .id(SharedString::from(format!("brief-menu-{which}-{i}")))
                        .role(Role::MenuItem)
                        .px(look.px(10.))
                        .py(look.px(5.))
                        .rounded(look.px(5.))
                        .text_size(look.text(12.5))
                        .cursor_pointer()
                        .hover(|h| h.bg(look.c.hover))
                        .child(label)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if which == "profile" {
                                this.brief.profile = p.clone();
                            } else {
                                this.brief.repo = r.clone();
                                this.brief.repo_picked = true;
                            }
                            this.brief.menu = None;
                            cx.notify();
                        }))
                        .into_any_element(),
                );
            }
            menu = Some(
                div()
                    .flex()
                    .flex_col()
                    .gap(look.px(1.))
                    .p(look.px(4.))
                    .max_w(look.px(420.))
                    .rounded(look.px(8.))
                    .border(look.fixed(1.))
                    .border_color(look.c.border)
                    .bg(look.c.elevated)
                    .children(rows)
                    .into_any_element(),
            );
        }
        let said = self.brief.said.clone().map(|r| {
            let (t, c) = match r {
                Ok(t) => (t, kit::dim(look)),
                Err(e) => (e, look.c.error),
            };
            div()
                .min_w_0()
                .truncate()
                .text_size(look.text(12.))
                .text_color(c)
                .font_family("Menlo")
                .child(t)
        });
        if menu.is_none() && said.is_none() {
            return None;
        }
        Some(
            div()
                .flex()
                .flex_col()
                .gap(look.px(8.))
                .flex_shrink_0()
                .children(menu)
                .children(said)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod step6_tests {
    use super::*;

    /// Start for Codex end to end, without Terminal: the script it writes,
    /// run with a stub `codex` that records its arguments, hands over the
    /// block verbatim in the repository and deletes both files.
    #[test]
    #[allow(clippy::disallowed_methods)]
    fn start_for_codex_runs_codex_with_the_block_as_its_prompt() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("a repo's dir");
        std::fs::create_dir(&repo).unwrap();
        let bin = tmp.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let log = tmp.path().join("argv");
        let stub = bin.join("codex");
        std::fs::write(
            &stub,
            format!(
                "#!/bin/sh\npwd > '{0}'\nprintf '%s\\n' \"$#\" \"$1\" >> '{0}'\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        let block = "# hippo brief · demo\ntask: say \"hi\" $HOME `x` $(y)";
        let script = write_start_files(
            briefing::Agent::Codex,
            block,
            &repo,
            Some(Path::new("/not/used")),
        )
        .unwrap();
        let text = std::fs::read_to_string(&script).unwrap();
        assert!(!text.contains("CLAUDE_CONFIG_DIR"), "{text}");
        let st = std::process::Command::new("/bin/sh")
            .arg(&script)
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .status()
            .unwrap();
        assert!(st.success());
        let got = std::fs::read_to_string(&log).unwrap();
        let mut lines = got.lines();
        assert!(lines.next().unwrap().ends_with("a repo's dir"), "{got}");
        assert_eq!(lines.next(), Some("1"), "one argument: {got}");
        assert_eq!(lines.collect::<Vec<_>>().join("\n"), block);
        assert!(!script.exists());
        assert!(!script.with_file_name("brief.txt").exists());
    }

    #[test]
    fn iso_days_in_reasons_read_as_the_app_writes_days() {
        let today = time::macros::date!(2026 - 11 - 10);
        assert_eq!(app_dates_at("work · 2026-11-09", today), "work · Mon");
        assert_eq!(app_dates_at("work · 2026-10-09", today), "work · 9 Oct");
        assert_eq!(app_dates("no date here"), "no date here");
        assert_eq!(app_dates("2026-13-40 stays"), "2026-13-40 stays");
    }
}
