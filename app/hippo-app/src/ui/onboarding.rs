//! First open: the Welcome, Found, Empty and Reading pages, the first
//! home's banner and the step-two sheet.
//! The words are `hippo_view::onboarding`'s; the
//! state and the commands are `crate::flow`'s.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{
    ClipboardItem, Context, FontWeight, HighlightStyle, Hsla, PathPromptOptions, Role,
    SharedString, StyledText, div, prelude::*, svg,
};
use hippo_view::onboarding::{self, STEPS};

use super::Shell;
use super::theme::Look;
use super::tooltip::Tooltip;
use crate::cli::Stream;
use crate::flow::{self, Job, Page, Purpose, Sheet};

/// The Ask step's suggested questions, from the newest sessions' topics.
fn first_questions(topics: &[String]) -> Vec<String> {
    let forms = [
        "what happened with “{}”?",
        "where did “{}” stop?",
        "what did we decide about “{}”?",
    ];
    topics
        .iter()
        .take(3)
        .zip(forms.iter())
        .map(|(t, f)| {
            let t = t.trim_end_matches(['.', '?', '!', ':', ';', ',']).trim();
            f.replace("{}", &t.to_lowercase())
        })
        .collect()
}

/// The serif face of the onboarding headlines.
const SERIF: &str = "Georgia";
/// The monospace face of commands.
const MONO: &str = "Menlo";

fn faint(look: &Look) -> Hsla {
    look.c.text_muted.opacity(0.8)
}

/// Text with some pieces in the text colour and medium weight, the rest in
/// the colour it is drawn in.
fn rich(parts: &[(&str, bool)], look: &Look) -> StyledText {
    let mut text = String::new();
    let mut highlights = Vec::new();
    for (piece, strong) in parts {
        let start = text.len();
        text.push_str(piece);
        if *strong {
            highlights.push((
                start..text.len(),
                HighlightStyle {
                    color: Some(look.c.text),
                    font_weight: Some(FontWeight::MEDIUM),
                    ..Default::default()
                },
            ));
        }
    }
    StyledText::new(text).with_highlights(highlights)
}

fn kicker(look: &Look, text: &str, accent: bool) -> gpui::Div {
    div()
        .text_size(look.text(11.))
        .text_color(if accent {
            look.c.text_accent
        } else {
            look.c.text_muted
        })
        .child(super::kit::sentence(&text.to_uppercase()))
}

fn headline(look: &Look, text: impl Into<SharedString>, size: f32) -> gpui::Div {
    div()
        .font_family(SERIF)
        .text_size(look.text(size))
        .line_height(look.px(size * 1.2))
        .text_color(look.c.text)
        .child(text.into())
}

/// A button: `primary` in the accent colour, else on the hover colour with a
/// hairline.
fn button(
    look: &Look,
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    primary: bool,
) -> gpui::Stateful<gpui::Div> {
    sized_button(look, id, label, primary, 30., 13.)
}

/// The page's main action: 40 points tall.
fn big_button(
    look: &Look,
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
) -> gpui::Stateful<gpui::Div> {
    sized_button(look, id, label, true, 40., 14.)
}

fn sized_button(
    look: &Look,
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    primary: bool,
    height: f32,
    text: f32,
) -> gpui::Stateful<gpui::Div> {
    let label: SharedString = label.into();
    let base = div()
        .id(id.into())
        .role(Role::Button)
        .aria_label(label.clone())
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .h(look.px(height))
        .px(look.px(height * 0.5))
        .rounded(look.px(6.))
        .border(look.fixed(1.))
        .text_size(look.text(text))
        .cursor_pointer();
    // `…  ↩`: the Return glyph as a small keycap, a keyboard hint.
    let base = match label.strip_suffix("  ↩") {
        Some(name) => base.gap(look.px(8.)).child(name.to_owned()).child(
            div()
                .px(look.px(4.))
                .rounded(look.px(3.))
                .border(look.fixed(1.))
                .border_color(gpui::black().opacity(0.25))
                .text_size(look.text(text - 2.))
                .opacity(0.75)
                .child("↩"),
        ),
        None => base.child(label.clone()),
    };
    if primary {
        base.bg(look.c.text_accent)
            .border_color(look.c.text_accent)
            .text_color(look.c.window)
            .font_weight(FontWeight::MEDIUM)
            .hover(|h| h.opacity(0.9))
    } else {
        base.bg(look.c.hover)
            .border_color(look.c.border)
            .text_color(look.c.text)
            .hover(|h| h.bg(look.c.selected))
    }
}

fn quiet_button(
    look: &Look,
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
) -> gpui::Stateful<gpui::Div> {
    let label: SharedString = label.into();
    div()
        .id(id.into())
        .role(Role::Button)
        .aria_label(label.clone())
        .px(look.px(6.))
        .text_size(look.text(12.))
        .text_color(look.c.text_muted)
        .cursor_pointer()
        .hover(|h| h.text_color(look.c.text))
        .child(label)
}

/// The steps along the foot of a page: one quiet, centred row, a dot and
/// a name per step, the current one emphasised. `on` is the current step
/// (0-based); on the Empty page, `nothing` marks the second step as not reached.
fn steps(look: &Look, on: usize, nothing: bool) -> gpui::Div {
    const NAMES: [&str; 4] = ["Find agents", "Connect", "Ask", "Give them a memory"];
    let _ = STEPS;
    div()
        .flex()
        .flex_wrap()
        .justify_center()
        .items_center()
        .gap_x(look.px(20.))
        .gap_y(look.px(4.))
        .text_size(look.text(12.))
        .children(NAMES.iter().enumerate().map(|(i, name)| {
            let current = i == on && !(nothing && i == 1);
            let done = i < on;
            let (dot, text) = if current {
                (look.c.text_accent, look.c.text)
            } else if done {
                (look.c.text_muted, look.c.text_muted)
            } else {
                (
                    look.c.text_muted.opacity(0.45),
                    look.c.text_muted.opacity(0.85),
                )
            };
            div()
                .flex()
                .items_center()
                .gap(look.px(7.))
                .text_color(text)
                .when(current, |d| d.font_weight(FontWeight::MEDIUM))
                .child(div().size(look.px(6.)).rounded_full().bg(dot))
                .child(*name)
        }))
}

impl Shell {
    /// The full-window onboarding page, when one shows.
    pub(crate) fn onboarding_page(&self) -> Option<Page> {
        self.flow.page
    }

    /// After a reload: a home with no `config.toml` starts onboarding, once.
    pub(crate) fn decide_onboarding(&mut self) {
        if self.flow.decided {
            return;
        }
        let Some(loaded) = self.loaded.as_ref() else {
            return;
        };
        self.flow.decided = true;
        let fresh = matches!(loaded.setup, hippo_view::setup::Setup::NotSetUp { .. })
            && !self.installation.home.join("config.toml").exists();
        if fresh {
            self.flow.page = Some(Page::Welcome);
        }
    }

    /// Looks for agent history: `hippo_core::detect` on a background thread,
    /// then the Found or Empty page.
    pub(crate) fn look_for_history(&mut self, cx: &mut Context<Self>) {
        if self.flow.looking {
            return;
        }
        self.flow.looking = true;
        let home = self.installation.home.clone();
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    let patterns = hippo_core::config::Config::load_in(&home)
                        .map(|c| c.transcript_roots)
                        .unwrap_or_else(|_| hippo_core::config::default_transcript_roots());
                    let user = hippo_core::config::user_home();
                    hippo_core::detect::detect(&patterns, &home, user.as_deref())
                })
                .await;
            this.update(cx, |this, cx| {
                let now = time::OffsetDateTime::now_utc();
                let offset = super::status::local_offset();
                this.flow.looking = false;
                if found.files() == 0 {
                    this.flow.looked = onboarding::empty(&found);
                    this.flow.page = Some(Page::Empty);
                } else {
                    this.flow.found = Some(onboarding::found(&found, now, offset));
                    this.flow.page = Some(Page::Found);
                }
                this.flow.detected = Some(found);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Runs `commands` for `purpose` and starts polling them.
    pub(crate) fn start_job(
        &mut self,
        purpose: Purpose,
        commands: Vec<Vec<String>>,
        cx: &mut Context<Self>,
    ) {
        if self
            .flow
            .job
            .as_ref()
            .is_some_and(|j| j.result.lock().map(|r| r.is_none()).unwrap_or(false))
        {
            return;
        }
        let stream = Arc::new(Mutex::new(Stream::default()));
        let result = Arc::new(Mutex::new(None));
        self.flow.job = Some(Job {
            purpose,
            stream: stream.clone(),
            result: result.clone(),
        });
        self.flow.error = None;
        let (binary, home, cwd) = (
            self.installation.binary.clone(),
            self.installation.home.clone(),
            flow::job_cwd(),
        );
        cx.background_executor()
            .spawn(async move {
                let r = flow::run_job(&binary, &home, &cwd, &commands, stream);
                if let Ok(mut slot) = result.lock() {
                    *slot = Some(r);
                }
            })
            .detach();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let more = this
                    .update(cx, |this, cx| this.poll_flow(cx))
                    .unwrap_or(false);
                if !more {
                    break;
                }
            }
        })
        .detach();
    }

    /// One poll of the running job; `false` once it has ended.
    fn poll_flow(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(purpose) = self.flow.job.as_ref().map(|j| j.purpose) else {
            return false;
        };
        let before = self.flow.log.len();
        let (refreshed, result) = self.flow.poll();
        if purpose == Purpose::StepTwo {
            let new: Vec<String> = self.flow.log[before.min(self.flow.log.len())..].to_vec();
            if let Some(sheet) = self.flow.sheet.as_mut() {
                sheet.output.extend(new);
            }
        }
        // `hippo init` has written config.toml: the shell can load it.
        if purpose == Purpose::Read
            && self
                .flow
                .log
                .iter()
                .any(|l| l.starts_with("$ hippo ingest"))
            && self.loaded.as_ref().is_some_and(|l| !l.ready())
        {
            self.reload(cx);
        }
        if refreshed {
            let stale = self
                .flow
                .so_far_at
                .is_none_or(|t| t.elapsed() > Duration::from_secs(2));
            if stale {
                self.flow.so_far_at = Some(Instant::now());
                self.refresh_so_far(cx);
            }
            let home_stale = self
                .flow
                .home_at
                .is_none_or(|t| t.elapsed() > Duration::from_secs(10));
            if self.flow.page.is_none() && home_stale {
                self.flow.home_at = Some(Instant::now());
                self.refresh_read(cx);
            }
        }
        cx.notify();
        let Some(result) = result else {
            return true;
        };
        match (purpose, result) {
            (Purpose::Read, Ok(())) => {
                self.flow.read_done = true;
                self.flow.banner = true;
                // The Ask step stays until Keep going: a question may be
                // open there.
                self.refresh_read(cx);
                self.reload(cx);
            }
            (Purpose::Read, Err(e)) => {
                // An ingest that read some of it still leaves a home to open.
                self.flow.error = Some(e);
                self.refresh_so_far(cx);
            }
            (Purpose::AddRoot, Ok(())) => {
                self.flow.job = None;
                self.look_for_history(cx);
            }
            (Purpose::AddRoot, Err(e)) => self.flow.error = Some(e),
            (Purpose::Lesson, r) => {
                let line = self
                    .last_output()
                    .or_else(|| self.flow.log.iter().find(|l| l.starts_with('$')).cloned())
                    .unwrap_or_default();
                self.inbox.result = Some((line, r));
                self.inbox.busy = None;
                self.reload(cx);
            }
            (Purpose::Propose, r) => {
                self.inbox.proposing = false;
                self.inbox.proposed = Some(match r {
                    Ok(()) => Ok(self
                        .last_output()
                        .unwrap_or_else(|| "sent to the Inbox".to_owned())),
                    Err(e) => Err(e),
                });
                self.reload(cx);
            }
            (Purpose::Miss, r) => {
                self.xray.miss_result = Some(match r {
                    Ok(()) => Ok(self
                        .last_output()
                        .unwrap_or_else(|| "sent to the Inbox".to_owned())),
                    Err(e) => Err(e),
                });
                self.reload(cx);
            }
            (Purpose::Tune, r) => {
                self.xray.tune_result = Some(match r {
                    Ok(()) => Ok(
                        "hippo.toml changed: the note is relevant-only from the next session"
                            .to_owned(),
                    ),
                    Err(e) => Err(e),
                });
                self.xray.confirm = None;
                self.xray.fleet = None;
                self.reload(cx);
            }
            (Purpose::Review, r) => {
                if let Some(line) = self.flow.log.iter().find(|l| l.starts_with('$')).cloned() {
                    self.flow.review = Some((line, r));
                }
                self.reload(cx);
            }
            (p @ (Purpose::Manage | Purpose::Doctor), r) => self.manage_done(p, r, cx),
            (Purpose::StepTwo, r) => {
                if let Some(sheet) = self.flow.sheet.as_mut() {
                    sheet.running = false;
                    match r {
                        Ok(()) => {
                            sheet.done = true;
                            self.flow.banner = false;
                        }
                        Err(e) => sheet.error = Some(e),
                    }
                }
                self.reload(cx);
            }
        }
        cx.notify();
        false
    }

    /// Reads the Reading page's numbers from the rollup, in the background.
    fn refresh_so_far(&mut self, cx: &mut Context<Self>) {
        let home = self.installation.home.clone();
        // On a first read every skipped file is one that could not be read
        // (nothing is unchanged yet). The index is not opened: the running
        // ingest holds it.
        let unread = self
            .flow
            .progress
            .as_ref()
            .map_or(0, |p| p.skipped as usize);
        cx.spawn(async move |this, cx| {
            let so_far = cx
                .background_executor()
                .spawn(async move {
                    let rollup = match hippo_core::rollup::Rollup::open(&home) {
                        Ok(Some(r)) => r,
                        Ok(None) => return None,
                        Err(e) => {
                            eprintln!("hippo-app: cannot read the rollup yet: {e}");
                            return None;
                        }
                    };
                    match onboarding::so_far(&rollup, unread, super::status::local_offset()) {
                        Ok(s) => Some(s),
                        Err(e) => {
                            eprintln!("hippo-app: cannot read the rollup yet: {e}");
                            None
                        }
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                if so_far.is_some() {
                    this.flow.so_far = so_far;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Reads everything again: what was read so far shows.
    fn refresh_read(&mut self, cx: &mut Context<Self>) {
        self.reload(cx);
    }

    /// Read: `init`, `ingest --progress`, `schedule install`.
    pub(crate) fn start_read(&mut self, cx: &mut Context<Self>) {
        self.flow.page = Some(Page::Reading);
        self.flow.read_started = Some(Instant::now());
        self.flow.progress = None;
        self.flow.log.clear();
        let commands = flow::read_commands(&self.flow.off);
        self.start_job(Purpose::Read, commands, cx);
    }

    /// Asks for a folder of transcripts and adds it with `hippo init
    /// --add-root`.
    pub(crate) fn add_transcript_folder(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Add".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            this.update(cx, |this, cx| {
                let args = vec![
                    "init".to_owned(),
                    "--add-root".to_owned(),
                    path.to_string_lossy().into_owned(),
                ];
                this.start_job(Purpose::AddRoot, vec![args], cx);
            })
            .ok();
        })
        .detach();
    }

    /// Leaves the Reading page for the home built so far.
    pub(crate) fn open_ready_so_far(&mut self, cx: &mut Context<Self>) {
        self.flow.page = None;
        self.flow.banner = true;
        self.refresh_read(cx);
        cx.notify();
    }

    /// Opens the step-two sheet, suggesting a notes folder from where the
    /// agents worked.
    pub(crate) fn open_step_two(&mut self, cx: &mut Context<Self>) {
        if self.flow.sheet.is_none() {
            self.flow.sheet = Some(Sheet::new());
        }
        let home = self.installation.home.clone();
        let have = self.flow.sheet.as_ref().is_some_and(|s| s.folder.is_some());
        if !have {
            cx.spawn(async move |this, cx| {
                let folder = cx
                    .background_executor()
                    .spawn(async move { suggest(&home) })
                    .await;
                this.update(cx, |this, cx| {
                    if let (Some(sheet), Some(folder)) = (this.flow.sheet.as_mut(), folder)
                        && sheet.folder.is_none()
                    {
                        sheet.folder = Some(folder);
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    pub(crate) fn close_step_two(&mut self, cx: &mut Context<Self>) {
        if self.flow.sheet.as_ref().is_some_and(|s| s.running) {
            return;
        }
        self.flow.sheet = None;
        cx.notify();
    }

    fn choose_notes_folder(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Use as notes folder".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let folder = cx
                .background_executor()
                .spawn(async move { onboarding::inspect_folder(&path) })
                .await;
            this.update(cx, |this, cx| {
                if let Some(sheet) = this.flow.sheet.as_mut() {
                    sheet.folder = Some(folder);
                    sheet.error = None;
                    sheet.done = false;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn run_step_two(&mut self, cx: &mut Context<Self>) {
        let Some(sheet) = self.flow.sheet.as_mut() else {
            return;
        };
        let Some(commands) = sheet.commands() else {
            return;
        };
        if sheet.running {
            return;
        }
        sheet.running = true;
        sheet.output.clear();
        sheet.error = None;
        self.flow.log.clear();
        self.start_job(Purpose::StepTwo, commands, cx);
    }

    /// Return presses the page's primary button (the one marked ↩) and
    /// Escape closes the step-two sheet. Returns whether the key was used.
    pub(crate) fn onboarding_key(
        &mut self,
        key: &str,
        field: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if field && self.flow.page == Some(Page::Reading) {
            return false;
        }
        // A page's Return acts only once the window has settled;
        // these pages hold no list to engage first.
        if key == "enter" && (self.flow.sheet.is_some() || self.flow.page.is_some()) {
            let g = cx.default_global::<crate::keyguard::KeyGuard>();
            if let Err(r) = g.check_page(std::time::Instant::now()) {
                g.refused = Some(r);
                cx.notify();
                return true;
            }
        }
        if self.flow.sheet.is_some() {
            match key {
                "escape" => self.close_step_two(cx),
                "enter" => {
                    if self.flow.sheet.as_ref().is_some_and(|s| s.done) {
                        self.close_step_two(cx)
                    } else {
                        self.run_step_two(cx)
                    }
                }
                _ => return false,
            }
            return true;
        }
        if key != "enter" {
            return false;
        }
        match self.flow.page {
            Some(Page::Welcome) => self.look_for_history(cx),
            Some(Page::Found) => self.start_read(cx),
            Some(Page::Reading)
                if self.flow.read_done
                    || self.flow.so_far.as_ref().is_some_and(|s| s.threads > 0) =>
            {
                self.open_ready_so_far(cx)
            }
            _ => return false,
        }
        true
    }

    // ---------------------------------------------------------------------
    // Drawing. A page is a column with a scrolling body and a footer; every
    // text that shares a row shrinks (`min_w_0`) and wraps or truncates;
    // rows of buttons wrap; nothing is placed absolutely but the sheet's
    // full-window overlay.

    /// The page `page`, filling the window below the title bar: a scrolling
    /// area whose content is one centred column, balanced a little above the
    /// middle when it is shorter than the window, and the step row under it.
    pub(super) fn render_onboarding(
        &mut self,
        page: Page,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let (body, step, nothing, max) = match page {
            Page::Welcome => (self.render_welcome(look, cx), 0, false, 680.),
            Page::Found => (self.render_found(look, cx), 1, false, 900.),
            Page::Empty => (self.render_empty(look, cx), 1, true, 680.),
            Page::Reading => (self.render_reading(look, cx), 2, false, 1180.),
        };
        // Spacers above and below the column: 2 to 3, so the column sits
        // at the optical centre.
        let edge = if self.flow.compact { 12. } else { 24. };
        let spacer = |grow: f32| {
            let mut d = div().flex_shrink_0().min_h(look.px(edge));
            d.style().flex_grow = Some(grow);
            d
        };
        div()
            .id("onboarding")
            .role(Role::Region)
            .aria_label("Welcome to hippo")
            .flex_1()
            .flex()
            .flex_col()
            .min_h_0()
            .min_w_0()
            .bg(look.c.window)
            .child(
                div()
                    .id("onboarding-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .min_h_full()
                            .w_full()
                            .px(look.px(40.))
                            .text_size(look.text(15.))
                            .text_color(look.c.text)
                            .child(spacer(2.))
                            .child(Self::column(look, max).child(body))
                            .child(spacer(3.)),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .h(look.px(44.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .px(look.px(24.))
                    .border_t(look.fixed(1.))
                    .border_color(look.c.border_variant.opacity(0.6))
                    .child(steps(look, step, nothing)),
            )
            .into_any_element()
    }

    /// The body column of a page: as wide as the window allows, up to `max`.
    fn column(look: &Look, max: f32) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .max_w(look.px(max))
    }

    /// A row of buttons and a note, wrapping at narrow widths.
    fn actions(look: &Look) -> gpui::Div {
        div().flex().flex_wrap().items_center().gap(look.px(12.))
    }

    fn render_welcome(&mut self, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let lines = onboarding::JOBS.iter().enumerate().map(|(i, (k, v))| {
            div()
                .flex()
                .gap(look.px(20.))
                .py(look.px(if self.flow.compact { 8. } else { 12. }))
                .border_t(look.fixed(1.))
                .border_color(look.c.border_variant)
                .when(i == 2, |d| d.border_b(look.fixed(1.)))
                .child(
                    div()
                        .w(look.px(112.))
                        .flex_shrink_0()
                        .font_weight(FontWeight::MEDIUM)
                        .child(*k),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .line_height(look.px(23.))
                        .text_color(look.c.text_muted)
                        .child(*v),
                )
        });
        let looking = self.flow.looking;
        let k = if self.flow.compact { 0.6 } else { 1.0 };
        div()
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .child(
                svg()
                    .path("hippo-mark.svg")
                    .w(look.px(56. * (0.4 + 0.6 * k)))
                    .h(look.px(46. * (0.4 + 0.6 * k)))
                    .text_color(look.c.text),
            )
            .child(
                headline(
                    look,
                    onboarding::HEADLINE,
                    if self.flow.compact { 34. } else { 42. },
                )
                .mt(look.px(24. * k)),
            )
            .child(
                div()
                    .mt(look.px(12.))
                    .text_size(look.text(17.))
                    .line_height(look.px(26.))
                    .text_color(look.c.text_muted)
                    .child(onboarding::LINE),
            )
            .child(div().mt(look.px(28. * k)).children(lines))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(look.px(10.))
                    .mt(look.px(32. * k))
                    .child(
                        big_button(
                            look,
                            "look-for-history",
                            if looking {
                                "Looking…"
                            } else {
                                "Look for agent history on this Mac  ↩"
                            },
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.look_for_history(cx))),
                    )
                    .child(
                        div()
                            .text_size(look.text(13.))
                            .text_color(faint(look))
                            .child("Read-only: hippo changes nothing in your agents' folders."),
                    ),
            )
            .child(
                div()
                    .mt(look.px(4.))
                    .text_size(look.text(13.))
                    .text_color(faint(look))
                    .child(rich(
                        &[
                            ("Works with ", false),
                            ("Claude Code", true),
                            (" and ", false),
                            ("Codex", true),
                            (" today. OpenCode and Cursor are planned.", false),
                        ],
                        look,
                    )),
            )
            .into_any_element()
    }

    /// A command the page will run, as the CLI prints it, with Copy. The
    /// command keeps its width until the box is too narrow, then truncates;
    /// the comment takes what is left. The full line is the tooltip.
    fn command_box(
        &self,
        look: &Look,
        id: String,
        prefix: &str,
        args: &[String],
        comment: &str,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let line = onboarding::shell_line(args);
        let copied = self.copied.as_deref() == Some(line.as_str());
        div()
            .id(SharedString::from(format!("{id}-box")))
            .flex()
            .items_center()
            .gap(look.px(10.))
            .w_full()
            .min_w_0()
            .px(look.px(10.))
            .py(look.px(6.))
            .rounded(look.px(5.))
            .border(look.fixed(1.))
            .border_color(look.c.border_variant)
            .bg(look.c.dock)
            .font_family(MONO)
            .text_size(look.text(12.))
            .tooltip(Tooltip::text(if comment.is_empty() {
                line.clone()
            } else {
                format!("{line}  {comment}")
            }))
            .children((!prefix.is_empty()).then(|| {
                div()
                    .flex_shrink_0()
                    .text_color(faint(look))
                    .child(prefix.to_owned())
            }))
            .child(if comment.is_empty() {
                // Nothing beside it: the whole command shows, wrapping.
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(look.c.text)
                    .child(line.clone())
            } else {
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(look.c.text)
                    .child(line.clone())
            })
            .when(!comment.is_empty(), |d| {
                d.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(look.text(11.))
                        .text_color(faint(look))
                        .child(comment.to_owned()),
                )
            })
            .child(
                div()
                    .id(SharedString::from(id))
                    .role(Role::Button)
                    .aria_label(format!("Copy {line}"))
                    .flex_shrink_0()
                    .font_family(".SystemUIFont")
                    .px(look.px(6.))
                    .rounded(look.px(4.))
                    .border(look.fixed(1.))
                    .border_color(look.c.border)
                    .text_size(look.text(11.))
                    .text_color(look.c.text_muted)
                    .cursor_pointer()
                    .hover(|h| h.bg(look.c.hover))
                    .child(if copied { "copied" } else { "copy" })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(line.clone()));
                        this.copied = Some(line.clone());
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    fn render_found(&mut self, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(found) = self.flow.found.clone() else {
            return div().into_any_element();
        };
        // Source, then where (shrinks first), then three fixed columns.
        const SOURCE: f32 = 150.;
        const SESSIONS: f32 = 72.;
        const DISK: f32 = 84.;
        const LATEST: f32 = 150.;
        const READ: f32 = 44.;
        let row_pad = if self.flow.compact { 5. } else { 9. };
        let fixed = |w: f32| div().w(look.px(w)).flex_shrink_0().min_w_0().truncate();
        let right = |w: f32| fixed(w).flex().justify_end();
        let header = div()
            .flex()
            .gap(look.px(12.))
            .pb(look.px(6.))
            .border_b(look.fixed(1.))
            .border_color(look.c.border)
            .text_size(look.text(11.))
            .text_color(faint(look))
            .child(fixed(SOURCE).child("SOURCE"))
            .child(div().flex_1().min_w_0().truncate().child("WHERE"))
            .child(right(SESSIONS).child("SESSIONS"))
            .child(right(DISK).child("ON DISK"))
            .child(fixed(LATEST).child("LATEST"))
            .child(fixed(READ).child("READ"));
        let off = self.flow.off.clone();
        let row = |i: usize, r: &onboarding::FoundRow, strong: bool| {
            let switch = r.dir.clone().map(|dir| {
                let on = !off.contains(&dir);
                let e = cx.entity().downgrade();
                let d2 = dir.clone();
                super::controls::switch(
                    look,
                    ("connect-switch", i),
                    format!("Read {}", r.source),
                    on,
                    true,
                    std::rc::Rc::new(move |_, app: &mut gpui::App| {
                        let d3 = d2.clone();
                        e.update(app, |this, cx| {
                            if let Some(at) = this.flow.off.iter().position(|x| *x == d3) {
                                this.flow.off.remove(at);
                            } else {
                                this.flow.off.push(d3);
                            }
                            cx.notify();
                        })
                        .ok();
                    }),
                )
            });
            let colour = if r.empty {
                faint(look)
            } else if r.indent {
                look.c.text_muted
            } else {
                look.c.text
            };
            div()
                .flex()
                .items_baseline()
                .gap(look.px(12.))
                .py(look.px(row_pad))
                .border_b(look.fixed(1.))
                .border_color(look.c.border_variant)
                .text_color(colour)
                .child(
                    fixed(SOURCE)
                        .when(r.indent, |d| d.pl(look.px(16.)))
                        .when(strong, |d| d.font_weight(FontWeight::MEDIUM))
                        .child(r.source.clone()),
                )
                .child(
                    div()
                        .id(("found-where", i))
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_ellipsis_start()
                        .font_family(MONO)
                        .text_size(look.text(11.))
                        .text_color(faint(look))
                        .when(!r.place.is_empty(), |d| {
                            d.tooltip(Tooltip::text(r.place.clone()))
                        })
                        .child(r.place.clone()),
                )
                .child(right(SESSIONS).child(r.sessions.clone()))
                .child(right(DISK).child(r.disk.clone()))
                .child(
                    fixed(LATEST)
                        .text_color(if r.empty {
                            faint(look)
                        } else {
                            look.c.text_muted
                        })
                        .child(r.latest.clone()),
                )
                .child(
                    div()
                        .w(look.px(READ))
                        .flex_shrink_0()
                        .flex()
                        .justify_end()
                        .children(switch),
                )
        };
        let mut table = div()
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .mt(look.px(16.))
            .child(header);
        for (i, r) in found.rows.iter().enumerate() {
            table = table.child(row(i, r, !r.indent));
        }
        if let Some((names, _)) = &found.unsupported {
            let names = match names.rsplit_once(", ") {
                Some((a, b)) => format!("{a} and {b}"),
                None => names.clone(),
            };
            let verb = if names.contains(" and ") { "are" } else { "is" };
            table = table.child(
                div()
                    .py(look.px(row_pad))
                    .border_b(look.fixed(1.))
                    .border_color(look.c.border_variant)
                    .text_color(faint(look))
                    .child(format!(
                        "{names} {verb} installed too, not read yet: hippo has no adapter for them."
                    )),
            );
        }
        table = table.child(row(usize::MAX, &found.total, true).border_b(look.fixed(0.)));
        let commands = flow::read_commands(&self.flow.off);
        let home_text = tilde(&self.installation.home);
        let boxes: Vec<gpui::AnyElement> = commands
            .iter()
            .enumerate()
            .map(|(i, args)| {
                self.command_box(
                    look,
                    format!("copy-read-{i}"),
                    if i == 0 { "runs" } else { "then" },
                    args,
                    flow::read_comment(args),
                    cx,
                )
            })
            .collect();
        let error = self.flow.error.clone();
        let show = self.flow.show_commands;
        Self::column(look, 900.)
            .text_size(look.text(13.))
            .child(kicker(look, "Found on this Mac · connect", false))
            .child(
                headline(look, found.headline.clone(), if self.flow.compact { 24. } else { 28. })
                    .mt(look.px(10.)),
            )
            .child(table)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(look.px(12.))
                    .mt(look.px(24.))
                    .child(
                        big_button(
                            look,
                            "read-all",
                            if self.flow.off.is_empty() {
                                format!("Connect all {} sessions  ↩", found.total.sessions)
                            } else {
                                format!("Connect ({} switched off)  ↩", self.flow.off.len())
                            },
                        )
                            .on_click(cx.listener(|this, _, _, cx| this.start_read(cx))),
                    )
                    .child(
                        sized_button(look, "add-folder", "Add a folder…", false, 40., 14.)
                            .on_click(cx.listener(|this, _, _, cx| this.add_transcript_folder(cx))),
                    ),
            )
            .child(
                div()
                    .mt(look.px(10.))
                    .text_color(faint(look))
                    .child(found.estimate.clone()),
            )
            .children(error.map(|e| {
                div()
                    .mt(look.px(8.))
                    .text_color(look.c.error)
                    .child(e)
            }))
            .child(
                div()
                    .mt(look.px(18.))
                    .text_color(look.c.text_muted)
                    .child(rich(
                        &[
                            ("Reading is ", false),
                            ("read-only", true),
                            (
                                ": hippo opens the transcript files and never changes them. Its own files live in ",
                                false,
                            ),
                            (&home_text, true),
                            (". Text that looks like a key or token is redacted before it is stored.", false),
                        ],
                        look,
                    )),
            )
            .child(
                div()
                    .id("show-commands")
                    .role(Role::Button)
                    .aria_label("Show the commands it runs")
                    .mt(look.px(14.))
                    .text_color(look.c.text_accent)
                    .cursor_pointer()
                    .child(if show {
                        "Hide the commands it runs ▾"
                    } else {
                        "Show the commands it runs ▸"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.flow.show_commands = !this.flow.show_commands;
                        cx.notify();
                    })),
            )
            .when(show, |d| {
                d.child(div().flex().flex_col().gap(look.px(6.)).mt(look.px(10.)).children(boxes))
            })
            .into_any_element()
    }

    fn render_empty(&mut self, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let rows = self.flow.looked.iter().enumerate().map(|(i, r)| {
            div()
                .flex()
                .items_baseline()
                .gap(look.px(12.))
                .py(look.px(8.))
                .border_b(look.fixed(1.))
                .border_color(look.c.border_variant)
                .child(
                    div()
                        .w(look.px(190.))
                        .flex_shrink_0()
                        .child(r.source.clone()),
                )
                .child(
                    div()
                        .id(("looked", i))
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(MONO)
                        .text_size(look.text(11.))
                        .text_color(faint(look))
                        .tooltip(Tooltip::text(r.place.clone()))
                        .child(r.place.clone()),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_color(look.c.text_muted)
                        .child(r.state.clone()),
                )
        });
        let action =
            |id: &'static str, title: &'static str, text: &'static str, el: gpui::AnyElement| {
                div()
                    .id(id)
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(look.px(16.))
                    .py(look.px(10.))
                    .border_b(look.fixed(1.))
                    .border_color(look.c.border_variant)
                    .child(
                        div()
                            .flex_1()
                            .min_w(look.px(240.))
                            .child(div().font_weight(FontWeight::MEDIUM).child(title))
                            .child(
                                div()
                                    .text_size(look.text(12.))
                                    .text_color(look.c.text_muted)
                                    .child(text),
                            ),
                    )
                    .child(el)
            };
        let looking = self.flow.looking;
        Self::column(look, 680.)
            .text_size(look.text(14.))
            .child(kicker(look, "Found on this Mac", false))
            .child(headline(look, "No agent history here yet.", 28.).mt(look.px(12.)).mb(look.px(10.)))
            .child(
                div()
                    .text_size(look.text(15.))
                    .line_height(look.px(22.))
                    .text_color(look.c.text_muted)
                    .child("hippo builds its home page from what your agents did. Until a session has run, there is nothing to read."),
            )
            .child(
                kicker(look, "Where it looked", false)
                    .mt(look.px(24.))
                    .pb(look.px(6.))
                    .border_b(look.fixed(1.))
                    .border_color(look.c.border),
            )
            .children(rows)
            .child(div().h(look.px(22.)))
            .child(action(
                "again-row",
                "Run a session, then come back",
                "Start Claude Code or Codex anywhere, then look again: hippo shows what it finds here.",
                button(look, "check-again", if looking { "Looking…" } else { "Check again  ⌘R" }, false)
                    .on_click(cx.listener(|this, _, _, cx| this.look_for_history(cx)))
                    .into_any_element(),
            ))
            .child(action(
                "folder-row",
                "Point hippo at another folder",
                "Transcripts on another disk, or a copy from an old Mac. hippo init --add-root adds the folder to transcript_roots in its config.",
                button(look, "add-folder-empty", "Add a folder…", false)
                    .on_click(cx.listener(|this, _, _, cx| this.add_transcript_folder(cx)))
                    .into_any_element(),
            ))
            .child(action(
                "notes-row",
                "Start with the notes instead",
                "Pick or create a notes folder now; dreaming and the session-start briefing work from the first session onward.",
                button(look, "give-memory-empty", "Give your agents a memory", true)
                    .on_click(cx.listener(|this, _, _, cx| this.open_step_two(cx)))
                    .into_any_element(),
            ))
            .children(self.flow.error.clone().map(|e| {
                div()
                    .mt(look.px(8.))
                    .text_size(look.text(12.))
                    .text_color(look.c.error)
                    .child(e)
            }))
            .into_any_element()
    }

    /// The Ask step: the folders being read and the progress
    /// on the left; on the right, Ask with questions drawn from the newest
    /// sessions read so far, answered while the read goes on.
    fn render_reading(&mut self, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let user = hippo_core::config::user_home();
        let elapsed = self
            .flow
            .read_started
            .map(flow::elapsed)
            .unwrap_or(time::Duration::ZERO);
        let reading = self.flow.progress.as_ref().map(|p| {
            onboarding::reading(p, elapsed, super::status::local_offset(), user.as_deref())
        });
        let (fraction, count, size) = match &reading {
            Some(r) => (r.fraction, r.count.clone(), r.size.clone()),
            None => (0.0, "Starting…".to_owned(), String::new()),
        };
        let done = self.flow.read_done;
        let ready = self.flow.so_far.as_ref().is_some_and(|s| s.threads > 0) || done;
        let error = self.flow.error.clone();
        let wide = !self.flow.compact_width;

        // Left: what was found, and the read.
        let mut found_rows: Vec<gpui::AnyElement> = Vec::new();
        if let Some(found) = &self.flow.found {
            for (i, r) in found.rows.iter().enumerate() {
                if r.dir.is_none() || r.empty {
                    continue;
                }
                let off = r.dir.as_ref().is_some_and(|d| self.flow.off.contains(d));
                found_rows.push(
                    div()
                        .id(("asked-found", i))
                        .flex()
                        .items_center()
                        .gap(look.px(10.))
                        .py(look.px(7.))
                        .border_b(look.fixed(1.))
                        .border_color(look.c.border_variant)
                        .when(off, |d| d.opacity(0.45))
                        .child(super::kit::avatar(
                            look,
                            Some(if r.source == "Codex" {
                                "codex"
                            } else {
                                r.source.as_str()
                            }),
                            None,
                        ))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(look.text(13.))
                                        .child(r.source.clone()),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .font_family(MONO)
                                        .text_size(look.text(11.))
                                        .text_color(faint(look))
                                        .child(r.place.clone()),
                                ),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(look.text(12.5))
                                .text_color(look.c.text_muted)
                                .child(if off {
                                    "off".to_owned()
                                } else {
                                    r.sessions.clone()
                                }),
                        )
                        .into_any_element(),
                );
            }
        }
        let commands: Vec<String> = flow::read_commands(&self.flow.off)
            .iter()
            .map(|a| onboarding::shell_line(a))
            .collect();
        let left = div()
            .flex()
            .flex_col()
            .gap(look.px(4.))
            .p(look.px(16.))
            .rounded(look.px(10.))
            .border(look.fixed(1.))
            .border_color(look.c.border_variant)
            .bg(look.c.dock)
            .when(wide, |d| d.w(look.px(340.)).flex_shrink_0())
            .when(!wide, |d| d.w_full())
            .text_size(look.text(13.))
            .child(
                div()
                    .flex()
                    .child(kicker(look, "Found on this Mac", false))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(look.text(11.))
                            .text_color(faint(look))
                            .child("read-only"),
                    ),
            )
            .children(found_rows)
            .child(
                div()
                    .flex()
                    .mt(look.px(12.))
                    .child(div().flex_1().min_w_0().truncate().child(if done {
                        "Read".to_owned()
                    } else {
                        "Reading".to_owned()
                    }))
                    .child(div().text_color(look.c.text_muted).child(count.clone())),
            )
            .child(
                div()
                    .mt(look.px(4.))
                    .h(look.px(6.))
                    .w_full()
                    .rounded(look.px(3.))
                    .bg(look.c.border)
                    .child(
                        div()
                            .h_full()
                            .rounded(look.px(3.))
                            .bg(look.c.text_accent)
                            .w(gpui::relative(if done { 1.0 } else { fraction })),
                    ),
            )
            .child(
                div()
                    .mt(look.px(4.))
                    .text_size(look.text(11.5))
                    .text_color(faint(look))
                    .child(format!(
                        "newest first{} · answers get better as it reads",
                        if size.is_empty() {
                            String::new()
                        } else {
                            format!(" · {size}")
                        }
                    )),
            )
            .child(
                div()
                    .mt(look.px(10.))
                    .text_size(look.text(11.5))
                    .text_color(faint(look))
                    .child("hippo changed nothing in these folders. Commands it ran:"),
            )
            .children(commands.into_iter().map(|c| {
                div()
                    .font_family(MONO)
                    .text_size(look.text(11.))
                    .text_color(faint(look))
                    .truncate()
                    .child(c)
            }))
            .children(error.map(|e| {
                div()
                    .mt(look.px(8.))
                    .text_size(look.text(12.))
                    .text_color(look.c.error)
                    .child(e)
            }));

        // Right: Ask.
        let topics: Vec<String> = self
            .loaded
            .as_ref()
            .map(|l| l.suggestions.clone())
            .unwrap_or_default();
        let questions = first_questions(&topics);
        let field = self.ask_field.clone();
        let focus = gpui::Focusable::focus_handle(&field, cx);
        let ask_box = div()
            .id("first-ask")
            .key_context("AskField")
            .flex()
            .items_center()
            .gap(look.px(12.))
            .w_full()
            .h(look.px(52.))
            .px(look.px(16.))
            .rounded(look.px(12.))
            .border(look.fixed(1.5))
            .border_color(look.c.text_accent.opacity(0.75))
            .bg(look.c.elevated.opacity(0.7))
            .cursor_text()
            .on_click(move |_, window, cx| window.focus(&focus, cx))
            .child(super::kit::icon(look, "search", 18., look.c.text_accent))
            .child(div().flex_1().min_w_0().overflow_hidden().child(field))
            .child(super::kit::kbd(look, "↩"));
        let chips = div().flex().flex_wrap().gap(look.px(8.)).children(
            questions.into_iter().enumerate().map(|(i, q)| {
                let q2 = q.clone();
                super::kit::chip(look, ("first-q", i), q, false).on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.ask_field
                            .update(cx, |f, cx| f.set_text(&q2, window, cx));
                        this.submit_ask(false, window, cx);
                    },
                ))
            }),
        );
        // The larger of what the read says and what the progress counted:
        // `so_far` lags the progress while the index is written.
        let sessions_read = self
            .flow
            .so_far
            .as_ref()
            .map(|s| s.threads)
            .unwrap_or(0)
            .max(
                self.flow
                    .progress
                    .as_ref()
                    .map_or(0, |p| p.files_done as usize),
            );
        let answer = self.first_answer(sessions_read, look, cx);
        let right = div()
            .flex()
            .flex_col()
            .gap(look.px(12.))
            .flex_1()
            .min_w_0()
            .child(headline(look, "Ask your agents' past", if self.flow.compact { 24. } else { 30. }))
            .child(
                div()
                    .text_color(look.c.text_muted)
                    .text_size(look.text(14.))
                    .child(if topics.is_empty() {
                        "Everything your agents did is one question away. Questions from your last week appear here as the newest sessions are read."
                    } else {
                        "Everything your agents did is one question away. Try one of these, taken from your last week:"
                    }),
            )
            .child(ask_box)
            .child(chips)
            .children(answer)
            .child(
                Self::actions(look)
                    .mt(look.px(6.))
                    .child(
                        sized_button(look, "keep-going", "Keep going  ↩", true, 40., 14.)
                            .when(!ready, |b| b.opacity(0.5).cursor_default())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if ready {
                                    this.open_ready_so_far(cx);
                                }
                            })),
                    )
                    .child(
                        sized_button(look, "memory-now", "Give your agents a memory now", false, 40., 14.)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open_ready_so_far(cx);
                                this.open_step_two(cx);
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(look.px(200.))
                            .text_size(look.text(12.))
                            .text_color(faint(look))
                            .child("step 4 adds a notes folder, nightly dreaming and the session-start briefing"),
                    ),
            );
        let row = div()
            .flex()
            .w_full()
            .gap(look.px(28.))
            .items_start()
            .when(!wide, |d| d.flex_col());
        // Narrow: Ask first, the read under it.
        if wide {
            row.child(left).child(right).into_any_element()
        } else {
            row.child(right).child(left).into_any_element()
        }
    }

    /// The answer area on the Ask step: Ask's sources and its written answer,
    /// short.
    fn first_answer(
        &self,
        read: usize,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let a = &self.ask;
        if a.question.is_empty() || (a.result.is_none() && !a.searching) {
            return None;
        }
        let mut col = div()
            .flex()
            .flex_col()
            .gap(look.px(8.))
            .p(look.px(14.))
            .rounded(look.px(10.))
            .border(look.fixed(1.))
            .border_color(look.c.border_variant)
            .bg(look.c.elevated.opacity(0.5));
        match &a.result {
            None => col = col.child(div().text_color(look.c.text_muted).child("Searching…")),
            Some(Err(e)) => col = col.child(div().text_color(look.c.error).child(e.clone())),
            Some(Ok(asked)) => {
                let n = asked.sources.len();
                let read = read.max(asked.searched.sessions as usize);
                col = col.child(
                    div()
                        .text_size(look.text(12.))
                        .text_color(faint(look))
                        .child(if read > 0 {
                            format!(
                                "Answer · {n} source{} · from the {read} sessions read so far",
                                if n == 1 { "" } else { "s" }
                            )
                        } else {
                            format!("Answer · {n} source{}", if n == 1 { "" } else { "s" })
                        }),
                );
                match &a.answer {
                    Some(Ok((Some(ans), _))) => {
                        col = col.child(div().text_size(look.text(14.)).child(ans.text.clone()));
                    }
                    _ if a.answering => {
                        col = col.child(
                            div()
                                .text_color(look.c.text_muted)
                                .child("Writing the answer…"),
                        );
                    }
                    Some(Ok((None, why))) => {
                        col = col.child(
                            div()
                                .text_size(look.text(12.))
                                .text_color(faint(look))
                                .child(format!(
                                    "sources only: {}",
                                    why.clone().unwrap_or_else(|| "no written answer".into())
                                )),
                        );
                    }
                    Some(Err(e)) => {
                        col = col.child(
                            div()
                                .text_size(look.text(12.))
                                .text_color(faint(look))
                                .child(format!("sources only: {e}")),
                        );
                    }
                    _ => {}
                }
                for s in asked.sources.iter().take(4) {
                    let id = s.id.clone();
                    let path = s.path.clone();
                    col = col.child(
                        div()
                            .id(SharedString::from(format!("first-src-{}", s.n)))
                            .flex()
                            .items_center()
                            .gap(look.px(10.))
                            .p(look.px(8.))
                            .rounded(look.px(8.))
                            .border(look.fixed(1.))
                            .border_color(look.c.border_variant)
                            .child(
                                div()
                                    .w(look.px(18.))
                                    .text_color(look.c.text_accent)
                                    .child(s.n.to_string()),
                            )
                            .child(super::kit::avatar(look, s.profile.as_deref(), None))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .child(div().truncate().child(s.title.clone()))
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(look.text(12.))
                                            .text_color(faint(look))
                                            .child(s.why.clone()),
                                    ),
                            )
                            .child(
                                sized_button(
                                    look,
                                    SharedString::from(format!("first-open-{}", s.n)),
                                    "Open",
                                    false,
                                    28.,
                                    12.,
                                )
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        this.open_ready_so_far(cx);
                                        if let Some(id) = &id {
                                            this.state.tabs.open(crate::state::Tab::Session {
                                                id: id.clone(),
                                                ordinal: None,
                                            });
                                        } else if let Some(p) = &path {
                                            this.state
                                                .tabs
                                                .open(crate::state::Tab::Note(p.clone()));
                                        }
                                        this.after_state_change(window, cx);
                                    },
                                )),
                            ),
                    );
                }
            }
        }
        Some(col.into_any_element())
    }

    /// The first home's banner, above the home, while it applies.
    pub(super) fn render_first_banner(
        &self,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if !self.flow.banner || self.flow.page.is_some() {
            return None;
        }
        let history_only = self
            .loaded
            .as_ref()
            .is_some_and(|l| matches!(l.setup, hippo_view::setup::Setup::HistoryOnly { .. }));
        let total = self.flow.found.as_ref().map(|f| f.total.sessions.clone());
        let rest = match (&self.flow.progress, self.flow.reading()) {
            (Some(p), true) => format!(
                ", built from what is read so far ({} of {} sessions). It fills in as the rest arrives.",
                hippo_view::edition::thousands(p.files_done),
                hippo_view::edition::thousands(p.files_total)
            ),
            _ => format!(
                ", built from {} sessions on this Mac. hippo re-reads them every 10 minutes; nothing else has changed.",
                total.unwrap_or_else(|| "the".into())
            ),
        };
        Some(
            div()
                .flex()
                .flex_wrap()
                .flex_shrink_0()
                .items_center()
                .gap(look.px(12.))
                .mx(look.px(24.))
                .mt(look.px(12.))
                .px(look.px(12.))
                .py(look.px(9.))
                .rounded(look.px(6.))
                .border(look.fixed(1.))
                .border_color(look.c.text_accent.opacity(0.45))
                .bg(look.c.text_accent.opacity(0.10))
                .child(
                    div()
                        .flex_1()
                        .min_w(look.px(260.))
                        .text_color(look.c.text_muted)
                        .child(rich(&[("Your first home", true), (&rest, false)], look)),
                )
                .child(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap(look.px(8.))
                        .when(history_only, |d| {
                            d.child(
                                button(
                                    look,
                                    "banner-next",
                                    "Next: give your agents a memory  →",
                                    true,
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.open_step_two(cx))),
                            )
                        })
                        .child(
                            quiet_button(look, "banner-later", "Later").on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.flow.banner = false;
                                    cx.notify();
                                },
                            )),
                        ),
                )
                .into_any_element(),
        )
    }

    /// The step-two sheet over the window, when open: a header, a body that
    /// scrolls, and a footer with the buttons that never scrolls away.
    pub(super) fn render_step_two(
        &self,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let sheet = self.flow.sheet.as_ref()?;
        let number = |n: &'static str| {
            div()
                .flex()
                .flex_shrink_0()
                .justify_center()
                .items_center()
                .size(look.px(22.))
                .rounded_full()
                .border(look.fixed(1.))
                .border_color(look.c.border)
                .text_size(look.text(11.))
                .text_color(look.c.text_muted)
                .child(n)
        };
        let toggle = |id: &'static str, on: bool| {
            div()
                .id(id)
                .role(Role::Switch)
                .aria_label(if on { "on" } else { "off" })
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(look.px(6.))
                .cursor_pointer()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .w(look.px(30.))
                        .h(look.px(18.))
                        .px(look.px(2.))
                        .rounded_full()
                        .bg(if on {
                            look.c.text_accent
                        } else {
                            look.c.border
                        })
                        .when(on, |d| d.justify_end())
                        .child(div().size(look.px(14.)).rounded_full().bg(gpui::white())),
                )
                .child(
                    div()
                        .text_size(look.text(12.))
                        .text_color(look.c.text_muted)
                        .child(if on { "on" } else { "off" }),
                )
        };
        let section = |n: &'static str,
                       title: &'static str,
                       right: gpui::AnyElement,
                       body: Vec<gpui::AnyElement>| {
            div()
                .flex()
                .gap(look.px(12.))
                .py(look.px(14.))
                .border_t(look.fixed(1.))
                .border_color(look.c.border_variant)
                .child(number(n))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(look.px(8.))
                        .child(
                            div()
                                .flex()
                                .justify_between()
                                .items_center()
                                .gap(look.px(8.))
                                .child(
                                    div()
                                        .min_w_0()
                                        .text_size(look.text(14.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(title),
                                )
                                .child(right),
                        )
                        .children(body),
                )
        };
        let folder = sheet.folder.clone();
        let folder_text = folder
            .as_ref()
            .map(|f| f.path.display().to_string())
            .unwrap_or_else(|| "Choose a folder of Markdown notes".to_owned());
        let found_line: Vec<(String, bool)> = match &folder {
            None => vec![(
                "hippo looked where your agents worked and found no notes folder. Choose one you have, or make a folder with one note, START HERE.md, and choose it."
                    .into(),
                false,
            )],
            Some(f) if !f.exists => vec![("That folder does not exist.".into(), false)],
            Some(f) => match &f.entry_note {
                Some(e) => vec![
                    ("Found ".into(), false),
                    (format!("{} notes", hippo_view::edition::thousands(f.notes as u64)), true),
                    (
                        format!(" in {} folder{}. Entry note: ", f.folders, if f.folders == 1 { "" } else { "s" }),
                        false,
                    ),
                    (e.rsplit('/').next().unwrap_or(e).to_owned(), true),
                    (". It opens every briefing.".into(), false),
                ],
                None => vec![(
                    "No Markdown notes here yet. Add one, such as START HERE.md, then choose the folder again: it opens every briefing."
                        .into(),
                    false,
                )],
            },
        };
        let found_parts: Vec<(&str, bool)> =
            found_line.iter().map(|(t, s)| (t.as_str(), *s)).collect();
        let commands = sheet.commands();
        let init = commands.as_ref().map(|c| c[0].clone());
        let schedule = sheet
            .dreaming
            .then(|| vec!["schedule".to_owned(), "install".to_owned()]);
        let hooks = sheet.briefing.then(|| {
            let ws = folder
                .as_ref()
                .map(|f| f.workspace.display().to_string())
                .unwrap_or_else(|| "<workspace>".into());
            vec![
                "hooks".to_owned(),
                "install".to_owned(),
                "--workspace".to_owned(),
                ws,
            ]
        });
        let mut writes: Vec<&str> = vec!["~/.hippo/config.toml", "the workspace's .hippo/"];
        if sheet.dreaming {
            writes.push("two launchd jobs");
        }
        if sheet.briefing {
            writes.push("three hooks");
        }
        let writes_line = match writes.len() {
            2 => format!("Writes {} and {}.", writes[0], writes[1]),
            n => format!(
                "Writes {}, and {}.",
                writes[..n - 1].join(", "),
                writes[n - 1]
            ),
        };
        let can_run = commands.is_some() && !sheet.running && !sheet.done;
        let output: Vec<String> = {
            let mut o: Vec<String> = sheet.output.iter().rev().take(8).cloned().collect();
            o.reverse();
            o
        };
        let (dreaming, briefing) = (sheet.dreaming, sheet.briefing);
        let body = div()
            .id("step-two-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(look.px(20.))
            .pb(look.px(16.))
            .child(section(
                "1",
                "Notes folder",
                div()
                    .flex_shrink_0()
                    .px(look.px(5.))
                    .rounded(look.px(3.))
                    .bg(look.c.hover)
                    .text_size(look.text(11.))
                    .text_color(look.c.text_muted)
                    .child("required")
                    .into_any_element(),
                vec![
                    div()
                        .text_color(look.c.text_muted)
                        .child("A folder of Markdown notes you own. hippo keeps it true; you and your agents keep editing it.")
                        .into_any_element(),
                    div()
                        .flex()
                        .items_center()
                        .gap(look.px(10.))
                        .min_w_0()
                        .child(div().flex_shrink_0().text_color(look.c.text_muted).child("Folder"))
                        .child(
                            div()
                                .id("notes-folder")
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_ellipsis_start()
                                .px(look.px(8.))
                                .py(look.px(5.))
                                .rounded(look.px(5.))
                                .border(look.fixed(1.))
                                .border_color(look.c.border)
                                .bg(look.c.window)
                                .font_family(MONO)
                                .text_size(look.text(12.))
                                .text_color(if folder.is_some() { look.c.text } else { faint(look) })
                                .tooltip(Tooltip::text(folder_text.clone()))
                                .child(folder_text),
                        )
                        .child(
                            button(look, "choose-folder", "Change…", false)
                                .on_click(cx.listener(|this, _, _, cx| this.choose_notes_folder(cx))),
                        )
                        .into_any_element(),
                    div()
                        .text_size(look.text(12.))
                        .text_color(look.c.text_muted)
                        .child(rich(&found_parts, look))
                        .into_any_element(),
                ]
                .into_iter()
                .chain(init.map(|args| self.command_box(look, "copy-step-init".into(), "runs", &args, "", cx)))
                .collect(),
            ))
            .child(section(
                "2",
                "Nightly dreaming",
                toggle("toggle-dreaming", dreaming)
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(s) = this.flow.sheet.as_mut().filter(|s| !s.running) {
                            s.dreaming = !s.dreaming;
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
                vec![div()
                    .text_color(look.c.text_muted)
                    .child(rich(
                        &[
                            ("At ", false),
                            ("03:30", true),
                            (" each night, hippo checks the notes against the day's transcripts, fixes what is wrong and writes a report. A fresh-context reviewer signs every fix, and any fix can be undone from Home. Uses your claude CLI with no tools.", false),
                        ],
                        look,
                    ))
                    .into_any_element()]
                .into_iter()
                .chain(schedule.map(|args| {
                    self.command_box(
                        look,
                        "copy-step-schedule".into(),
                        "runs",
                        &args,
                        "",
                        cx,
                    )
                }))
                .collect(),
            ))
            .child(section(
                "3",
                "Session-start briefing",
                toggle("toggle-briefing", briefing)
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(s) = this.flow.sheet.as_mut().filter(|s| !s.running) {
                            s.briefing = !s.briefing;
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
                vec![div()
                    .text_color(look.c.text_muted)
                    .child(rich(
                        &[
                            ("Each new Claude Code session in this workspace starts with the notes its task needs, within ", false),
                            ("25 KB", true),
                            (". Three hooks in the workspace's .claude/settings.json; a hook never fails a session.", false),
                        ],
                        look,
                    ))
                    .into_any_element()]
                .into_iter()
                .chain(hooks.map(|args| self.command_box(look, "copy-step-hooks".into(), "runs", &args, "", cx)))
                .collect(),
            ))
            .children((!output.is_empty()).then(|| {
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .mb(look.px(10.))
                    .p(look.px(10.))
                    .rounded(look.px(5.))
                    .bg(look.c.dock)
                    .font_family(MONO)
                    .text_size(look.text(11.))
                    .text_color(look.c.text_muted)
                    .children(output.into_iter().map(|l| div().w_full().min_w_0().truncate().child(l)))
            }))
            .children(sheet.error.clone().map(|e| {
                div()
                    .mb(look.px(10.))
                    .text_size(look.text(12.))
                    .text_color(look.c.error)
                    .child(e)
            }));
        let footer = div()
            .flex()
            .flex_wrap()
            .flex_shrink_0()
            .items_center()
            .gap(look.px(12.))
            .px(look.px(20.))
            .py(look.px(14.))
            .border_t(look.fixed(1.))
            .border_color(look.c.border_variant)
            .child(if sheet.done {
                button(look, "step-two-close", "Done", true)
                    .on_click(cx.listener(|this, _, _, cx| this.close_step_two(cx)))
                    .into_any_element()
            } else {
                button(look, "step-two-run", if sheet.running { "Setting up…" } else { "Set up  ↩" }, true)
                    .when(!can_run, |b| b.opacity(0.5).cursor_default())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if can_run {
                            this.run_step_two(cx);
                        }
                    }))
                    .into_any_element()
            })
            .children((!sheet.done).then(|| {
                button(look, "step-two-later", "Not now", false)
                    .on_click(cx.listener(|this, _, _, cx| this.close_step_two(cx)))
            }))
            .child(
                div()
                    .flex_1()
                    .min_w(look.px(200.))
                    .text_size(look.text(11.))
                    .text_color(faint(look))
                    .child(if sheet.done {
                        "Done. Your agents have a memory: the next session starts with its briefing.".to_owned()
                    } else {
                        format!("{writes_line} Nothing else.")
                    }),
            );
        let panel = div()
            .id("step-two")
            .role(Role::Dialog)
            .aria_label("Give your agents a memory")
            .flex()
            .flex_col()
            .w(look.px(580.))
            .max_w(gpui::relative(0.92))
            .max_h(gpui::relative(0.92))
            .rounded(look.px(8.))
            .bg(look.c.elevated)
            .border(look.fixed(1.))
            .border_color(look.c.border)
            .text_size(look.text(13.))
            .text_color(look.c.text)
            .child(
                div()
                    .flex_shrink_0()
                    .px(look.px(20.))
                    .pt(look.px(20.))
                    .pb(look.px(12.))
                    .child(kicker(look, "Step two · optional", true))
                    .child(headline(look, "Give your agents a memory", 26.).mt(look.px(8.)))
                    .child(
                        div()
                            .mt(look.px(8.))
                            .text_color(look.c.text_muted)
                            .child("Three things, each one command. hippo shows the command before it runs and writes only what the command writes."),
                    ),
            )
            .child(body)
            .child(footer);
        Some(
            div()
                .id("step-two-overlay")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .justify_center()
                .items_center()
                .p(look.px(16.))
                .bg(gpui::black().opacity(0.45))
                .occlude()
                .child(panel)
                .into_any_element(),
        )
    }
}

/// `path` with the home directory written `~`.
fn tilde(path: &std::path::Path) -> String {
    let text = path.display().to_string();
    match hippo_core::config::user_home().and_then(|h| h.to_str().map(str::to_owned)) {
        Some(h) if text.starts_with(&h) => format!("~{}", &text[h.len()..]),
        _ => text,
    }
}

/// A notes folder to suggest: from the working directories of the newest
/// sessions in the rollup (read from each transcript's head). Reads only;
/// the index is not opened, so a running ingest does not hold it up.
fn suggest(home: &std::path::Path) -> Option<hippo_view::onboarding::NotesFolder> {
    let rollup = hippo_core::rollup::Rollup::open(home).ok().flatten()?;
    let now = time::OffsetDateTime::now_utc();
    let recent = rollup
        .sessions_in(now - time::Duration::days(14), now)
        .ok()?;
    let cwds: Vec<PathBuf> = recent
        .iter()
        .filter(|s| s.facts.parent.is_none())
        .take(80)
        .filter_map(|s| {
            hippo_core::sessions::origin(&s.facts.id, &s.facts.tool, &s.facts.transcript_path).cwd
        })
        .collect();
    let path = onboarding::suggest_folder(&cwds)?;
    Some(onboarding::inspect_folder(&path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_steps_read_in_order() {
        assert_eq!(STEPS[3], "4 Give your agents a memory (optional)");
    }

    #[test]
    fn first_questions_quote_the_topics_without_their_punctuation() {
        let q = super::first_questions(&[
            "Rename the sign helper.".into(),
            "Tidy the upload API".into(),
        ]);
        assert_eq!(
            q,
            [
                "what happened with “rename the sign helper”?",
                "where did “tidy the upload api” stop?"
            ]
        );
    }
}
