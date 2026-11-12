//! Memory: every note as a tile of a treemap grouped by folder, and a note's
//! stats.
//!
//! Five modes, each setting a tile's size and colour: reads
//! (sessions that read, discussed or edited the note in 30 days, from the
//! rollup), injected and ignored (start blocks that carried it in 7 days,
//! from the x-ray's fleet stats), the file's size, and edits (sessions that
//! edited it plus dreaming receipts in 30 days); a dot marks a review that
//! is overdue. [`squarify`] lays out the tiles; it is pure, so it is tested
//! here and painted by the app.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use hippocamp_core::config::Workspace;
use hippocamp_core::dream::History;
use hippocamp_core::notes::{self, LintFinding};
use hippocamp_core::rollup::Rollup;
use hippocamp_core::xray::{self, PartKind, Tip};
use time::{Date, Duration, OffsetDateTime, UtcOffset};

use crate::brain::Brain;

/// The window the counts cover.
pub const DAYS: i64 = 30;

/// The rollup's kind for a session that edited a note.
const EDIT: &str = hippocamp_core::dream::observed::Kind::Edit.as_str();

/// The window the start-block counts cover: the x-ray's fleet week.
pub const BLOCK_DAYS: u32 = 7;

/// What a tile's size and colour stand for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SizeBy {
    #[default]
    Reads,
    Injected,
    Ignored,
    Size,
    Edits,
}

impl SizeBy {
    pub const ALL: [SizeBy; 5] = [
        SizeBy::Reads,
        SizeBy::Injected,
        SizeBy::Ignored,
        SizeBy::Size,
        SizeBy::Edits,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SizeBy::Reads => "Reads",
            SizeBy::Injected => "Injected",
            SizeBy::Ignored => "Ignored",
            SizeBy::Size => "Size",
            SizeBy::Edits => "Edits",
        }
    }

    /// The toggle's word below 900 points: never a bare digit.
    pub fn short(self) -> &'static str {
        match self {
            SizeBy::Reads => "Reads",
            SizeBy::Injected => "Inj",
            SizeBy::Ignored => "Ign",
            SizeBy::Size => "Size",
            SizeBy::Edits => "Edit",
        }
    }

    /// The legend's words for the size.
    pub fn means(self) -> &'static str {
        match self {
            SizeBy::Reads => "tile = sessions that read it · 30 days",
            SizeBy::Injected | SizeBy::Ignored => "tile = start blocks · 7 days",
            SizeBy::Size => "tile = file size",
            SizeBy::Edits => "tile = edits · 30 days",
        }
    }

    /// Only notes that reached a start block get a tile.
    pub fn injected_only(self) -> bool {
        matches!(self, SizeBy::Injected | SizeBy::Ignored)
    }

    /// The colours this mode uses, in the legend's order.
    pub fn shades(self) -> &'static [Shade] {
        match self {
            SizeBy::Reads | SizeBy::Edits => &[
                Shade::Fresh(Fresh::Week),
                Shade::Fresh(Fresh::Month),
                Shade::Fresh(Fresh::Older),
            ],
            SizeBy::Injected => &[Shade::Required, Shade::Relevant],
            SizeBy::Ignored => &[Shade::Opened, Shade::Half, Shade::Ignored],
            SizeBy::Size => &[Shade::ManyReads, Shade::FewReads, Shade::Unread],
        }
    }
}

/// What a tile's colour says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shade {
    Fresh(Fresh),
    /// Injected mode: carried as a required note.
    Required,
    /// Injected mode: carried as a relevant pick.
    Relevant,
    /// Ignored mode: never opened in under half the blocks that carried it.
    Opened,
    /// Ignored in a third to two thirds of the blocks that carried it.
    Half,
    /// Ignored in two thirds or more (the tune tip's rule).
    Ignored,
    /// Size mode: read in 10 or more sessions in 30 days.
    ManyReads,
    /// In 1 to 9.
    FewReads,
    /// In none.
    Unread,
}

/// How ignored a note is (one rule everywhere): 0 when it was
/// never opened or cited in under a third of the `carried` blocks, 1 from
/// a third, 2 from two thirds (the tune tip's rule).
pub fn ignored_level(ignored: u32, carried: u32) -> u8 {
    if carried == 0 || ignored * 3 < carried {
        0
    } else if ignored * 3 < carried * 2 {
        1
    } else {
        2
    }
}

impl Shade {
    pub fn label(self) -> &'static str {
        match self {
            Shade::Fresh(f) => f.label(),
            Shade::Required => "required",
            Shade::Relevant => "relevant pick",
            Shade::Opened => "mostly opened",
            Shade::Half => "ignored in 1 of 3+",
            Shade::Ignored => "ignored in 2 of 3+",
            Shade::ManyReads => "read 10+ times",
            Shade::FewReads => "read 1-9 times",
            Shade::Unread => "not read",
        }
    }
}

/// How recently a note's file changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Fresh {
    /// In the last 7 days, today included.
    Week,
    Month,
    Older,
}

impl Fresh {
    pub const ALL: [Fresh; 3] = [Fresh::Week, Fresh::Month, Fresh::Older];

    pub fn label(self) -> &'static str {
        match self {
            Fresh::Week => "changed this week",
            Fresh::Month => "this month",
            Fresh::Older => "older",
        }
    }

    pub fn of(modified: Option<OffsetDateTime>, now: OffsetDateTime, offset: UtcOffset) -> Fresh {
        let Some(m) = modified else {
            return Fresh::Older;
        };
        let today = now.to_offset(offset).date();
        let day = m.to_offset(offset).date();
        let age = (today - day).whole_days();
        if age < 7 {
            Fresh::Week
        } else if age < DAYS {
            Fresh::Month
        } else {
            Fresh::Older
        }
    }
}

/// One note on the map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tile {
    /// Relative to the workspace.
    pub path: PathBuf,
    pub title: String,
    /// The folder below the notes directory; `root` for its top level.
    pub folder: String,
    pub bytes: u64,
    pub modified: Option<OffsetDateTime>,
    pub fresh: Fresh,
    /// Sessions that read, discussed or edited it in 30 days.
    pub reads: u32,
    /// Sessions that edited it, plus dreaming receipts, in 30 days.
    pub edits: u32,
    /// `review_after`, when it has passed.
    pub overdue: Option<Date>,
    /// A required note (or the entry note).
    pub required: bool,
    /// Start blocks that carried it in [`BLOCK_DAYS`] days, and of those
    /// the ones whose session read it, cited it, or neither (the x-ray's
    /// fleet stats).
    pub injected: u32,
    pub read: u32,
    pub cited: u32,
    pub ignored: u32,
    /// How it was carried: required wins over relevant.
    pub carried_as: Option<PartKind>,
    /// The bytes its line takes in a start block.
    pub line_bytes: usize,
}

impl Tile {
    /// The number the size stands for.
    pub fn value(&self, by: SizeBy) -> u64 {
        match by {
            SizeBy::Reads => u64::from(self.reads),
            SizeBy::Injected | SizeBy::Ignored => u64::from(self.injected),
            SizeBy::Size => self.bytes,
            SizeBy::Edits => u64::from(self.edits),
        }
    }

    /// The value as the tile prints it.
    pub fn value_label(&self, by: SizeBy) -> String {
        match by {
            SizeBy::Size => human(self.bytes),
            SizeBy::Ignored if self.injected > 0 => {
                format!("{} · never opened in {}", self.injected, self.ignored)
            }
            _ => self.value(by).to_string(),
        }
    }

    /// The share of the blocks that carried it in which it was never
    /// opened or cited, 0 to 1; `None` when none carried it.
    pub fn ignored_share(&self) -> Option<f32> {
        (self.injected > 0).then(|| self.ignored as f32 / self.injected as f32)
    }

    /// Ignored in at least two of three blocks that carried it: the tune
    /// tip's rule.
    pub fn mostly_ignored(&self) -> bool {
        ignored_level(self.ignored, self.injected) == 2
    }

    /// The tile's colour in mode `by`.
    pub fn shade(&self, by: SizeBy) -> Shade {
        match by {
            SizeBy::Reads | SizeBy::Edits => Shade::Fresh(self.fresh),
            SizeBy::Injected => match self.carried_as {
                Some(PartKind::Required) => Shade::Required,
                _ => Shade::Relevant,
            },
            // The tune tip's thirds, the one rule everywhere.
            SizeBy::Ignored => match ignored_level(self.ignored, self.injected) {
                2 => Shade::Ignored,
                1 => Shade::Half,
                _ => Shade::Opened,
            },
            SizeBy::Size => match self.reads {
                0 => Shade::Unread,
                1..=9 => Shade::FewReads,
                _ => Shade::ManyReads,
            },
        }
    }
}

/// What one start block costs, from the x-ray's fleet week.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Cost {
    /// Sessions with a recorded block in the week.
    pub sessions: u32,
    pub avg_block: usize,
    /// Percent of the blocks' bytes nobody opened or cited.
    pub ignored_percent: u32,
}

/// `4.0 KB`, `812 B`.
pub fn human(bytes: u64) -> String {
    if bytes < 1000 {
        format!("{bytes} B")
    } else if bytes < 1_000_000 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    }
}

/// Every note, and the map's header numbers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Map {
    pub tiles: Vec<Tile>,
    /// Notes changed in the last 7 days.
    pub changed_week: usize,
    pub overdue: usize,
    /// The block's cost; `None` when no start block was recorded.
    pub cost: Option<Cost>,
    /// Required notes configured (the entry note not counted).
    pub required: usize,
    /// The fleet's tune tips, for the side panel.
    pub tips: Vec<Tip>,
}

impl Map {
    /// The folders with their tiles' indices, largest total first, each
    /// group's tiles largest first.
    pub fn groups(&self, by: SizeBy) -> Vec<(String, Vec<usize>)> {
        let mut groups: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for (i, t) in self.tiles.iter().enumerate() {
            if by.injected_only() && t.injected == 0 {
                continue;
            }
            groups.entry(t.folder.as_str()).or_default().push(i);
        }
        let weight = |i: &usize| self.weight(*i, by);
        let mut out: Vec<(String, Vec<usize>)> = groups
            .into_iter()
            .map(|(name, mut ix)| {
                ix.sort_by(|a, b| weight(b).total_cmp(&weight(a)).then(a.cmp(b)));
                (name.to_owned(), ix)
            })
            .collect();
        out.sort_by(|a, b| {
            let sa: f64 = a.1.iter().map(weight).sum();
            let sb: f64 = b.1.iter().map(weight).sum();
            sb.total_cmp(&sa).then(a.0.cmp(&b.0))
        });
        out
    }

    /// A tile's area weight: its value, with a floor of 2 % of the largest
    /// (and at least 1) so every note is on the map.
    pub fn weight(&self, i: usize, by: SizeBy) -> f64 {
        let max = self
            .tiles
            .iter()
            .map(|t| t.value(by))
            .max()
            .unwrap_or(0)
            .max(1) as f64;
        (self.tiles[i].value(by) as f64).max(max * 0.02).max(1.0)
    }

    /// Notes no start block carried in the week: collapsed into one block
    /// in the injected modes.
    pub fn never_injected(&self) -> usize {
        self.tiles.iter().filter(|t| t.injected == 0).count()
    }

    /// Required notes ignored in at least two of three blocks.
    pub fn mostly_ignored_required(&self) -> usize {
        self.tiles
            .iter()
            .filter(|t| t.carried_as == Some(PartKind::Required) && t.mostly_ignored())
            .count()
    }

    /// The tune tip for the note at `path`, when the fleet stats have one.
    pub fn tip(&self, path: &Path) -> Option<&Tip> {
        self.tips.iter().find(|t| t.note == path)
    }

    /// The tiles sorted for the list: by the value, then title.
    pub fn sorted(&self, by: SizeBy) -> Vec<usize> {
        let mut ix: Vec<usize> = (0..self.tiles.len()).collect();
        ix.sort_by(|a, b| {
            let (ta, tb) = (&self.tiles[*a], &self.tiles[*b]);
            tb.value(by)
                .cmp(&ta.value(by))
                .then_with(|| ta.title.cmp(&tb.title))
        });
        ix
    }
}

/// What [`load`] reads.
pub struct Inputs<'a> {
    pub ws: &'a Workspace,
    pub brain: &'a Brain,
    pub history: Option<&'a History>,
    pub rollup: Option<&'a Rollup>,
    /// The x-ray's fleet stats over [`BLOCK_DAYS`] days.
    pub fleet: Option<&'a xray::Fleet>,
    pub now: OffsetDateTime,
    pub offset: UtcOffset,
}

/// The x-ray's fleet stats over [`BLOCK_DAYS`] days, for [`Inputs::fleet`].
pub fn fleet(ws: &Workspace, rollup: &Rollup, now: OffsetDateTime) -> Option<xray::Fleet> {
    xray::fleet(Some(ws), rollup, BLOCK_DAYS, now).ok()
}

/// The map of every note the scan holds.
pub fn load(inp: &Inputs<'_>) -> Map {
    let ws = inp.ws;
    let since = inp.now - Duration::days(DAYS);
    let notes_dir = notes::normalise(&ws.notes_dir);
    let mut reads: HashMap<PathBuf, u32> = HashMap::new();
    let mut edits: HashMap<PathBuf, u32> = HashMap::new();
    if let Some(r) = inp.rollup {
        for seen in r
            .notes_in(since, inp.now + Duration::minutes(1))
            .unwrap_or_default()
        {
            let p = notes::normalise(&seen.note);
            *reads.entry(p.clone()).or_default() += 1;
            // The rollup writes `edit` (`dream::observed::Kind::as_str`);
            // comparing `edited` would count no session edit.
            if seen.kind == EDIT {
                *edits.entry(p).or_default() += 1;
            }
        }
    }
    let uses: HashMap<PathBuf, &xray::NoteUse> = inp
        .fleet
        .map(|f| {
            f.notes
                .iter()
                .map(|n| (notes::normalise(&n.note), n))
                .collect()
        })
        .unwrap_or_default();
    if let Some(h) = inp.history {
        for c in &h.changes {
            let Ok(p) = &c.proposal.record else { continue };
            let applied = c
                .receipts
                .iter()
                .any(|r| r.applied.is_some_and(|t| t >= since));
            if applied {
                *edits.entry(notes::normalise(&p.note)).or_default() += 1;
            }
        }
    }
    let overdue: HashMap<&Path, Date> = inp
        .brain
        .findings
        .iter()
        .filter_map(|(p, f)| match f {
            LintFinding::Overdue { review_after, .. } => Some((p.as_path(), *review_after)),
            _ => None,
        })
        .collect();
    let required: Vec<PathBuf> = ws
        .required_notes
        .iter()
        .chain(std::iter::once(&ws.entry_note))
        .map(|p| notes::normalise(p))
        .collect();
    let mut map = Map {
        required: ws.required_notes.len(),
        cost: inp.fleet.and_then(|f| {
            Some(Cost {
                sessions: f.recorded,
                avg_block: f.avg_block()?,
                ignored_percent: f.ignored_percent()?,
            })
        }),
        tips: inp.fleet.map(|f| f.tips.clone()).unwrap_or_default(),
        ..Map::default()
    };
    for row in &inp.brain.rows {
        let meta = std::fs::metadata(ws.root.join(&row.path)).ok();
        let modified = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .map(OffsetDateTime::from);
        let below = row.path.strip_prefix(&notes_dir).unwrap_or(&row.path);
        let folder = match below.components().count() {
            0 | 1 => "root".to_owned(),
            _ => below
                .components()
                .next()
                .map(|c| crate::text::visible(&c.as_os_str().to_string_lossy()))
                .unwrap_or_else(|| "root".to_owned()),
        };
        let fresh = Fresh::of(modified, inp.now, inp.offset);
        if fresh == Fresh::Week {
            map.changed_week += 1;
        }
        let od = overdue.get(row.path.as_path()).copied();
        if od.is_some() {
            map.overdue += 1;
        }
        map.tiles.push(Tile {
            path: row.path.clone(),
            title: row.title.clone(),
            folder,
            bytes: meta.map(|m| m.len()).unwrap_or(0),
            modified,
            fresh,
            reads: reads.get(&row.path).copied().unwrap_or(0),
            edits: edits.get(&row.path).copied().unwrap_or(0),
            overdue: od,
            required: required.contains(&row.path),
            injected: uses.get(&row.path).map_or(0, |u| u.sessions),
            read: uses.get(&row.path).map_or(0, |u| u.read),
            cited: uses.get(&row.path).map_or(0, |u| u.cited),
            ignored: uses.get(&row.path).map_or(0, |u| u.ignored),
            carried_as: uses.get(&row.path).and_then(|u| u.kind),
            line_bytes: uses.get(&row.path).map_or(0, |u| u.bytes),
        });
    }
    map
}

// ---------------------------------------------------------------------------
// Layout

/// A rectangle in points.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    /// Shrunk by `d` on every side, never below zero size.
    pub fn inset(&self, d: f32) -> Rect {
        Rect {
            x: self.x + d,
            y: self.y + d,
            w: (self.w - 2.0 * d).max(0.0),
            h: (self.h - 2.0 * d).max(0.0),
        }
    }
}

/// The squarified treemap (Bruls, Huizing and van Wijk): `weights`, sorted
/// largest first by the caller, fill `area` in rows along its shorter side,
/// each row kept while it improves the worst aspect ratio. Returns one
/// rectangle per weight, in the same order.
pub fn squarify(weights: &[f64], area: Rect) -> Vec<Rect> {
    let mut out = vec![Rect::default(); weights.len()];
    let total: f64 = weights.iter().sum();
    if weights.is_empty() || total <= 0.0 || area.w <= 0.0 || area.h <= 0.0 {
        return out;
    }
    let scale = f64::from(area.w) * f64::from(area.h) / total;
    let areas: Vec<f64> = weights.iter().map(|w| w * scale).collect();
    let mut free = area;
    let mut i = 0;
    while i < areas.len() {
        let side = f64::from(free.w.min(free.h));
        let mut j = i + 1;
        let mut best = worst(&areas[i..j], side);
        while j < areas.len() {
            let next = worst(&areas[i..=j], side);
            if next > best {
                break;
            }
            best = next;
            j += 1;
        }
        let row: f64 = areas[i..j].iter().sum();
        if free.w >= free.h {
            // A column on the left.
            let w = if free.h > 0.0 {
                (row / f64::from(free.h)) as f32
            } else {
                0.0
            };
            let w = if j == areas.len() {
                free.w
            } else {
                w.min(free.w)
            };
            let mut y = free.y;
            for k in i..j {
                let h = if row > 0.0 {
                    (areas[k] / row) as f32 * free.h
                } else {
                    0.0
                };
                out[k] = Rect::new(free.x, y, w, h);
                y += h;
            }
            free = Rect::new(free.x + w, free.y, free.w - w, free.h);
        } else {
            // A row on top.
            let h = if free.w > 0.0 {
                (row / f64::from(free.w)) as f32
            } else {
                0.0
            };
            let h = if j == areas.len() {
                free.h
            } else {
                h.min(free.h)
            };
            let mut x = free.x;
            for k in i..j {
                let w = if row > 0.0 {
                    (areas[k] / row) as f32 * free.w
                } else {
                    0.0
                };
                out[k] = Rect::new(x, free.y, w, h);
                x += w;
            }
            free = Rect::new(free.x, free.y + h, free.w, free.h - h);
        }
        i = j;
    }
    out
}

/// The worst aspect ratio of a row of `areas` laid along `side`.
fn worst(areas: &[f64], side: f64) -> f64 {
    let sum: f64 = areas.iter().sum();
    if sum <= 0.0 || side <= 0.0 {
        return f64::INFINITY;
    }
    let max = areas.iter().cloned().fold(f64::MIN, f64::max);
    let min = areas.iter().cloned().fold(f64::MAX, f64::min);
    let s2 = side * side;
    let r1 = s2 * max / (sum * sum);
    let r2 = (sum * sum) / (s2 * min.max(1e-9));
    r1.max(r2)
}

/// A laid-out map: each group's frame (its label on top) and each tile.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Laid {
    /// Group name, its whole frame, and whether its label fits.
    pub groups: Vec<(String, Rect, bool)>,
    /// Tile index and its rectangle (gaps already taken off).
    pub tiles: Vec<(usize, Rect)>,
    /// In the injected modes: the block standing for the notes no start
    /// block carried, and how many there are.
    pub rest: Option<(Rect, usize)>,
}

/// Lays `map` out in `area`: groups first, by their total weight, then the
/// tiles inside each group under a `label` points tall label. `gap` is the
/// space between tiles.
pub fn layout(map: &Map, by: SizeBy, area: Rect, label: f32, gap: f32) -> Laid {
    let groups = map.groups(by);
    let mut weights: Vec<f64> = groups
        .iter()
        .map(|(_, ix)| ix.iter().map(|i| map.weight(*i, by)).sum())
        .collect();
    // The notes no block carried: one muted block, a twelfth of the map at
    // least so its words fit, never more than a quarter.
    let rest = (by.injected_only() && map.never_injected() > 0).then(|| map.never_injected());
    if rest.is_some() {
        let total: f64 = weights.iter().sum();
        let w = if total > 0.0 { total / 11.0 } else { 1.0 };
        weights.push(w);
    }
    let mut frames = squarify(&weights, area);
    let mut laid = Laid::default();
    if let Some(n) = rest {
        let frame = frames.pop().unwrap_or_default().inset(gap * 1.5);
        laid.rest = Some((frame, n));
    }
    for ((name, ix), frame) in groups.into_iter().zip(frames) {
        let frame = frame.inset(gap * 1.5);
        let fits = frame.h >= label * 2.5 && frame.w >= 36.0;
        let inner = if fits {
            Rect::new(
                frame.x,
                frame.y + label,
                frame.w,
                (frame.h - label).max(0.0),
            )
        } else {
            frame
        };
        let w: Vec<f64> = ix.iter().map(|i| map.weight(*i, by)).collect();
        for (i, r) in ix.iter().zip(squarify(&w, inner)) {
            laid.tiles.push((*i, r.inset(gap / 2.0)));
        }
        laid.groups.push((name, frame, fits));
    }
    laid
}

// ---------------------------------------------------------------------------
// A note's stats

/// What the note page's stat cards show.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NoteStats {
    /// Sessions that read, discussed or edited it per day, oldest first,
    /// the last [`DAYS`] days ending today.
    pub per_day: Vec<u32>,
    pub reads: u32,
    pub edits: u32,
    /// Sessions by profile, most first.
    pub by_profile: Vec<(String, u32)>,
    /// Sessions whose recorded start block carried it (the x-ray's parts),
    /// and how many of those never opened it.
    pub injected: u32,
    pub ignored: u32,
}

/// The stats of the note at `path` (relative to the workspace).
pub fn note_stats(
    rollup: &Rollup,
    path: &Path,
    now: OffsetDateTime,
    offset: UtcOffset,
) -> NoteStats {
    let path = notes::normalise(path);
    let since = now - Duration::days(DAYS);
    let today = now.to_offset(offset).date();
    let mut s = NoteStats {
        per_day: vec![0; DAYS as usize],
        ..Default::default()
    };
    let mut profiles: BTreeMap<String, u32> = BTreeMap::new();
    for seen in rollup.sessions_of_note(&path).unwrap_or_default() {
        let Some(t) = hippocamp_core::rollup::parse_ts(&seen.ts) else {
            continue;
        };
        if t < since {
            continue;
        }
        s.reads += 1;
        if seen.kind == EDIT {
            s.edits += 1;
        }
        let age = (today - t.to_offset(offset).date()).whole_days();
        if (0..DAYS).contains(&age) {
            s.per_day[(DAYS - 1 - age) as usize] += 1;
        }
        let profile = rollup
            .session(&seen.session_id)
            .ok()
            .flatten()
            .and_then(|f| {
                // A sub-agent counts for its parent's profile.
                f.profile()
            })
            .unwrap_or_else(|| "?".to_owned());
        *profiles.entry(profile).or_default() += 1;
    }
    let mut by: Vec<(String, u32)> = profiles.into_iter().collect();
    by.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    s.by_profile = by;
    if let Ok((injected, ignored)) = rollup.note_injections(&path) {
        s.injected = injected;
        s.ignored = ignored;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(rs: &[Rect]) -> f32 {
        rs.iter().map(|r| r.w * r.h).sum()
    }

    #[test]
    fn squarify_fills_the_area_in_proportion_without_overlap() {
        let w = [6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0];
        let a = Rect::new(0.0, 0.0, 600.0, 400.0);
        let rs = squarify(&w, a);
        assert!((area(&rs) - 240_000.0).abs() < 1.0, "{}", area(&rs));
        for (r, w) in rs.iter().zip(w) {
            let share = r.w * r.h / 240_000.0;
            assert!((share - (w / 24.0) as f32).abs() < 0.001, "{r:?}");
            assert!(r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= 600.01 && r.y + r.h <= 400.01);
        }
        for i in 0..rs.len() {
            for j in i + 1..rs.len() {
                let (a, b) = (rs[i], rs[j]);
                let ox = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
                let oy = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
                assert!(ox <= 0.01 || oy <= 0.01, "{a:?} overlaps {b:?}");
            }
        }
        // The squarified rows keep tiles near square.
        let worst = rs
            .iter()
            .map(|r| (r.w / r.h).max(r.h / r.w))
            .fold(0.0, f32::max);
        assert!(worst < 4.0, "{worst}");
    }

    #[test]
    fn squarify_handles_nothing_and_one() {
        assert!(squarify(&[], Rect::new(0.0, 0.0, 10.0, 10.0)).is_empty());
        let one = squarify(&[3.0], Rect::new(5.0, 5.0, 10.0, 20.0));
        assert_eq!(one, [Rect::new(5.0, 5.0, 10.0, 20.0)]);
        let flat = squarify(&[1.0, 1.0], Rect::new(0.0, 0.0, 0.0, 10.0));
        assert_eq!(flat.len(), 2);
    }

    fn tile(folder: &str, title: &str, reads: u32, bytes: u64) -> Tile {
        Tile {
            path: PathBuf::from(format!("notes/{folder}/{title}.md")),
            title: title.into(),
            folder: folder.into(),
            bytes,
            modified: None,
            fresh: Fresh::Older,
            reads,
            edits: 0,
            overdue: None,
            required: false,
            injected: 0,
            read: 0,
            cited: 0,
            ignored: 0,
            carried_as: None,
            line_bytes: 0,
        }
    }

    #[test]
    fn injected_modes_tile_only_carried_notes_and_collapse_the_rest() {
        let mut a = tile("Gov", "Protocol", 40, 900);
        a.injected = 43;
        a.ignored = 35;
        a.carried_as = Some(PartKind::Required);
        let mut b = tile("Ops", "Runbook", 17, 500);
        b.injected = 10;
        b.ignored = 6;
        b.read = 4;
        b.carried_as = Some(PartKind::Relevant);
        let map = Map {
            tiles: vec![a, b, tile("Ops", "Log", 3, 9000), tile("Gov", "Old", 0, 50)],
            ..Default::default()
        };
        let g = map.groups(SizeBy::Ignored);
        assert_eq!(g.iter().map(|(_, ix)| ix.len()).sum::<usize>(), 2);
        assert_eq!(map.never_injected(), 2);
        let laid = layout(
            &map,
            SizeBy::Ignored,
            Rect::new(0.0, 0.0, 600.0, 400.0),
            16.0,
            2.0,
        );
        assert_eq!(laid.tiles.len(), 2);
        let (rest, n) = laid.rest.expect("a never-injected block");
        assert_eq!(n, 2);
        assert!(rest.w > 0.0 && rest.h > 0.0);
        assert!(layout(
            &map,
            SizeBy::Reads,
            Rect::new(0.0, 0.0, 600.0, 400.0),
            16.0,
            2.0
        )
        .rest
        .is_none());
        assert_eq!(map.tiles[0].shade(SizeBy::Ignored), Shade::Ignored);
        assert_eq!(map.tiles[1].shade(SizeBy::Ignored), Shade::Half);
        assert_eq!(map.tiles[0].shade(SizeBy::Injected), Shade::Required);
        assert_eq!(map.tiles[1].shade(SizeBy::Injected), Shade::Relevant);
        assert_eq!(map.tiles[2].shade(SizeBy::Size), Shade::FewReads);
        assert_eq!(map.tiles[3].shade(SizeBy::Size), Shade::Unread);
        assert_eq!(map.mostly_ignored_required(), 1);
        assert_eq!(
            map.tiles[0].value_label(SizeBy::Ignored),
            "43 · never opened in 35"
        );
    }

    #[test]
    fn groups_sort_by_weight_and_every_note_gets_a_tile() {
        let map = Map {
            tiles: vec![
                tile("Ops", "Runbook", 80, 100),
                tile("Ops", "Log", 0, 9000),
                tile("Gov", "Protocol", 97, 50),
                tile("Gov", "Questions", 3, 50),
            ],
            ..Default::default()
        };
        let g = map.groups(SizeBy::Reads);
        assert_eq!(g[0].0, "Gov");
        assert_eq!(g[0].1, [2, 3]);
        assert_eq!(map.groups(SizeBy::Size)[0].0, "Ops");
        // An unread note still has a floor.
        assert!(map.weight(1, SizeBy::Reads) >= 1.9);
        let laid = layout(
            &map,
            SizeBy::Reads,
            Rect::new(0.0, 0.0, 760.0, 420.0),
            16.0,
            2.0,
        );
        assert_eq!(laid.tiles.len(), 4);
        assert!(laid.tiles.iter().all(|(_, r)| r.w > 0.0 && r.h > 0.0));
        assert_eq!(map.sorted(SizeBy::Reads), [2, 0, 3, 1]);
    }

    #[test]
    fn freshness_counts_local_days() {
        let now = time::macros::datetime!(2026-11-10 06:00 UTC);
        let off = UtcOffset::from_hms(9, 30, 0).unwrap();
        assert_eq!(
            Fresh::of(Some(now - Duration::hours(2)), now, off),
            Fresh::Week
        );
        assert_eq!(
            Fresh::of(Some(now - Duration::days(3)), now, off),
            Fresh::Week
        );
        assert_eq!(
            Fresh::of(Some(now - Duration::days(20)), now, off),
            Fresh::Month
        );
        assert_eq!(
            Fresh::of(Some(now - Duration::days(40)), now, off),
            Fresh::Older
        );
        assert_eq!(Fresh::of(None, now, off), Fresh::Older);
    }
}
