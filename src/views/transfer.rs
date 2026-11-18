//! Transfer UI: progress rows (pill, card or photo stack), the drive-to-drive strip, conflict and delete prompts,
//! flying items, the Recycle Bin pop-up and the undo toast. Everything is drawn with Direct2D while something moves.
use crate::app::App;
use crate::draw::IconSize;
use crate::layout::Rect;
use crate::motion::{flight_point, particle_x, shake, spring};
pub use crate::settings::PanelStyle;
use crate::shell;
use crate::theme::Color;
use crate::transfer::{Choice, Op, Progress};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;
use windows_numerics::{Matrix3x2, Vector2};

const ROW_W: f32 = 440.0;
const GAP: f32 = 10.0;
/// How long a finished row stays before it shrinks away (seconds).
const DONE_HOLD: f32 = 1.4;
const DONE_FADE: f32 = 0.4;
const TOAST_SECS: f32 = 5.0;

pub struct Row {
    pub id: u64,
    pub op: Op,
    pub dest_name: String,
    pub progress: Progress,
    pub current: String,
    pub started: Instant,
    pub failed: Vec<(PathBuf, String)>,
    pub done: Option<(Instant, bool)>,
    pub cancel: Arc<AtomicBool>,
    /// Source and destination drive names when the copy crosses drives.
    pub drives: Option<(String, String)>,
    pub items: Vec<PathBuf>,
    pub dest: Option<PathBuf>,
    pub shake_at: Option<Instant>,
    /// When the "current file" last changed (photo stack flips).
    pub flipped_at: Option<Instant>,
}

impl Row {
    fn verb(&self, done: bool) -> &'static str {
        match (self.op, done) {
            (Op::Copy, false) => "Copying",
            (Op::Copy, true) => "Copied",
            (Op::Move, false) => "Moving",
            (Op::Move, true) => "Moved",
            (Op::Recycle, false) => "Moving to Recycle Bin",
            (Op::Recycle, true) => "Moved to Recycle Bin",
            (Op::Delete, false) => "Deleting",
            (Op::Delete, true) => "Deleted",
        }
    }

    pub fn title(&self) -> String {
        let n = self.progress.items;
        let things = if n == 1 { "1 item".to_string() } else { format!("{n} items") };
        match (self.done, self.op) {
            (Some((_, true)), _) => "Cancelled".into(),
            (Some(_), Op::Copy | Op::Move) => format!("{} {things} to {}", self.verb(true), self.dest_name),
            (Some(_), _) => format!("{} {things}", self.verb(true)),
            (None, Op::Copy | Op::Move) => format!("{} to {}", self.verb(false), self.dest_name),
            (None, _) => self.verb(false).to_string(),
        }
    }

    /// Fully gone after the hold and fade, unless failures wait for Retry/Skip.
    pub fn expired(&self, now: Instant) -> bool {
        self.failed.is_empty() && self.done.is_some_and(|(t, _)| now.duration_since(t).as_secs_f32() > DONE_HOLD + DONE_FADE)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PromptButton {
    ApplyAll,
    Skip,
    KeepBoth,
    Replace,
    Cancel,
    Delete,
}

pub enum Prompt {
    Conflict { op: Op, items: Vec<PathBuf>, dest: PathBuf, conflicts: Vec<PathBuf>, idx: usize, answers: HashMap<PathBuf, Choice>, apply_all: bool, shown: Instant },
    ConfirmDelete { items: Vec<PathBuf>, shown: Instant },
}

pub struct Flight {
    pub path: PathBuf,
    pub is_dir: bool,
    pub from: (f32, f32),
    pub to: (f32, f32),
    pub start: Instant,
    pub delay: f32,
    pub dur: f32,
}

impl Flight {
    fn t(&self, now: Instant) -> f32 {
        (now.duration_since(self.start).as_secs_f32() - self.delay) / self.dur
    }
}

pub struct Bin {
    pub shown: Instant,
    /// Set when every item has dropped in; the lid closes from then.
    pub landed: Option<Instant>,
}

pub struct Toast {
    pub text: String,
    pub shown: Instant,
    /// Items the Undo button restores from the Recycle Bin.
    pub undo: Vec<PathBuf>,
}

#[derive(Default)]
pub struct Transfers {
    pub rows: Vec<Row>,
    pub prompt: Option<Prompt>,
    pub flights: Vec<Flight>,
    pub bin: Option<Bin>,
    pub toast: Option<Toast>,
    pub next_id: u64,
    /// Drive strip collapsed by a click on the panel.
    pub strip_hidden: bool,
}

impl Transfers {
    /// True while anything is animating or running (keeps the 16 ms timer alive).
    pub fn active(&self) -> bool {
        !self.rows.is_empty() || !self.flights.is_empty() || self.bin.is_some() || self.toast.is_some() || self.prompt.is_some()
    }

    /// Drops what has finished animating.
    /// Indexes of rows drawn as panels, oldest first. A Recycle Bin move shows the bin instead, unless it failed.
    pub fn shown(&self) -> Vec<usize> {
        (0..self.rows.len()).filter(|&i| self.rows[i].op != Op::Recycle || !self.rows[i].failed.is_empty()).collect()
    }

    pub fn cleanup(&mut self, now: Instant) {
        self.rows.retain(|r| !r.expired(now));
        self.flights.retain(|f| f.t(now) < 1.0);
        if self.bin.as_ref().is_some_and(|b| b.landed.is_some_and(|l| now.duration_since(l).as_secs_f32() > 1.1)) {
            self.bin = None;
        }
        if self.toast.as_ref().is_some_and(|t| now.duration_since(t.shown).as_secs_f32() > TOAST_SECS) {
            self.toast = None;
        }
    }
}

fn row_h(style: PanelStyle) -> f32 {
    match style {
        PanelStyle::Pill => 58.0,
        PanelStyle::Card => 82.0,
        PanelStyle::Stack => 92.0,
    }
}

/// Rects of `n` rows, newest at the bottom-center of the content, older rows stacked above.
pub fn row_rects(content: Rect, n: usize, style: PanelStyle) -> Vec<Rect> {
    let (w, h) = (ROW_W.min(content.w - 32.0), row_h(style));
    let x = content.x + (content.w - w) / 2.0;
    (0..n).map(|i| Rect::new(x, content.bottom() - 16.0 - h - i as f32 * (h + GAP), w, h)).collect()
}

/// (cancel ×, Retry, Skip) inside a row.
pub fn row_buttons(row: Rect) -> (Rect, Rect, Rect) {
    let cancel = Rect::new(row.right() - 40.0, row.y + 14.0, 28.0, 28.0);
    let retry = Rect::new(row.right() - 84.0, row.bottom() - 36.0, 72.0, 26.0);
    let skip = Rect::new(retry.x - 72.0, retry.y, 64.0, 26.0);
    (cancel, retry, skip)
}

/// The prompt card and its buttons (conflict card when `conflict`, else the permanent-delete confirmation).
pub fn prompt_layout(content: Rect, conflict: bool) -> (Rect, Vec<(PromptButton, Rect)>) {
    let (w, h) = if conflict { (480.0f32.min(content.w - 32.0), 186.0) } else { (420.0f32.min(content.w - 32.0), 150.0) };
    let card = Rect::new(content.x + (content.w - w) / 2.0, content.bottom() - h - 24.0, w, h);
    let by = card.bottom() - 46.0;
    let mut b = Vec::new();
    if conflict {
        b.push((PromptButton::ApplyAll, Rect::new(card.x + 16.0, by, 190.0, 30.0)));
        b.push((PromptButton::Skip, Rect::new(card.right() - 16.0 - 82.0 * 3.0 - 16.0, by, 82.0, 30.0)));
        b.push((PromptButton::KeepBoth, Rect::new(card.right() - 16.0 - 82.0 * 2.0 - 8.0, by, 82.0, 30.0)));
        b.push((PromptButton::Replace, Rect::new(card.right() - 16.0 - 82.0, by, 82.0, 30.0)));
    } else {
        b.push((PromptButton::Cancel, Rect::new(card.right() - 16.0 - 90.0 * 2.0 - 8.0, by, 90.0, 30.0)));
        b.push((PromptButton::Delete, Rect::new(card.right() - 16.0 - 90.0, by, 90.0, 30.0)));
    }
    (card, b)
}

/// Where the Recycle Bin pops up: the bottom-right corner of the content area.
pub fn bin_rect(content: Rect) -> Rect {
    Rect::new(content.right() - 112.0, content.bottom() - 132.0, 80.0, 100.0)
}

/// Mouth of the bin, where flying items land.
pub fn bin_mouth(content: Rect) -> (f32, f32) {
    let b = bin_rect(content);
    (b.x + b.w / 2.0, b.y + 30.0)
}

pub fn toast_rect(content: Rect) -> (Rect, Rect) {
    let r = Rect::new(content.x + (content.w - 380.0) / 2.0, content.bottom() - 60.0, 380.0, 40.0);
    (r, Rect::new(r.right() - 76.0, r.y + 6.0, 68.0, 28.0))
}

fn strip_rect(content: Rect, top_row: Rect) -> Rect {
    let w = ROW_W.min(content.w - 32.0);
    Rect::new(content.x + (content.w - w) / 2.0, top_row.y - 96.0, w, 86.0)
}

fn secs(now: Instant, t: Instant) -> f32 {
    now.duration_since(t).as_secs_f32()
}

fn card_bg(app: &App, r: Rect, radius: f32, alpha: f32) {
    let t = &app.theme;
    app.gfx.fill_round(Rect::new(r.x - 1.0, r.y - 1.0, r.w + 2.0, r.h + 2.0), radius + 1.0, t.separator.fade(alpha));
    app.gfx.fill_round(r, radius, t.content_bg.fade(alpha));
}

/// Progress ring with a check mark that draws itself when done.
fn ring(app: &App, cx: f32, cy: f32, frac: f32, done: Option<f32>, alpha: f32) {
    let t = &app.theme;
    let ok = Color::rgb(0x1F9D55);
    app.gfx.arc(cx, cy, 14.0, 1.0, 4.0, t.separator.fade(alpha));
    app.gfx.arc(cx, cy, 14.0, frac, 4.0, if done.is_some() { ok } else { t.accent }.fade(alpha));
    if let Some(k) = done {
        let k = (k / 0.45).clamp(0.0, 1.0);
        let (p0, p1, p2) = ((cx - 6.0, cy + 0.5), (cx - 1.5, cy + 5.0), (cx + 7.0, cy - 5.0));
        let a = (k * 2.0).min(1.0);
        app.gfx.stroke(p0.0, p0.1, p0.0 + (p1.0 - p0.0) * a, p0.1 + (p1.1 - p0.1) * a, 3.0, ok.fade(alpha));
        if k > 0.5 {
            let b = (k - 0.5) * 2.0;
            app.gfx.stroke(p1.0, p1.1, p1.0 + (p2.0 - p1.0) * b, p1.1 + (p2.1 - p1.1) * b, 3.0, ok.fade(alpha));
        }
    }
}

fn item_icon(app: &mut App, path: &std::path::Path, r: Rect, alpha: f32) {
    let is_dir = path.is_dir();
    if is_dir {
        crate::views::grid::paint_folder(app, r);
        return;
    }
    let e = crate::listing::Entry { name: path.file_name().unwrap_or_default().to_os_string(), size: 0, modified: 0, created: 0, attrs: 0x20, rel_dir: None };
    let idx = shell::icon_index(&e, &mut app.icon_idx);
    if let Some(b) = app.gfx.icon(idx, IconSize::Jumbo) {
        app.gfx.bitmap_alpha(&b, r, alpha);
    }
}

fn paint_row(app: &mut App, i: usize, r0: Rect, now: Instant) {
    let t = app.theme;
    let style = app.settings.panel;
    let row = &app.xfer.rows[i];
    // Finished rows hold, then shrink toward the bottom and fade.
    let (alpha, r) = match row.done {
        Some((at, _)) if row.failed.is_empty() => {
            let k = ((secs(now, at) - DONE_HOLD) / DONE_FADE).clamp(0.0, 1.0);
            let s = 1.0 - 0.35 * k;
            (1.0 - k, Rect::new(r0.x + r0.w * (1.0 - s) / 2.0, r0.y + r0.h * (1.0 - s) + 8.0 * k, r0.w * s, r0.h * s))
        }
        _ => (1.0, r0),
    };
    let dx = row.shake_at.map_or(0.0, |s| shake(secs(now, s) / 0.45));
    let r = Rect::new(r.x + dx, r.y, r.w, r.h);
    let failed = !row.failed.is_empty();
    let frac = row.progress.fraction as f32;
    let done_k = row.done.filter(|d| !d.1).map(|(at, _)| secs(now, at));
    let title = if failed { if row.failed.len() == 1 { "Couldn’t finish 1 item".to_string() } else { format!("Couldn’t finish {} items", row.failed.len()) } } else { row.title() };
    let sub = if failed {
        let (p, m) = &row.failed[0];
        format!("“{}” — {}", p.file_name().unwrap_or_default().to_string_lossy(), m.trim())
    } else if row.done.is_some() {
        row.progress.line().split(" · ").take(2).collect::<Vec<_>>().join(" · ")
    } else {
        row.progress.line()
    };
    let current = row.current.clone();
    let (stack_items, flip_k) = (row.items.iter().take(3).cloned().collect::<Vec<_>>(), row.flipped_at.map(|f| secs(now, f) / 0.35));
    let radius = if style == PanelStyle::Pill { r.h / 2.0 } else { 14.0 };
    card_bg(app, r, radius, alpha);
    if failed {
        app.gfx.fill_round(r, radius, Color::rgb(0xD7372B).fade(0.10 * alpha));
    }
    let (cancel, retry, skip) = row_buttons(r);
    let text_x = match style {
        PanelStyle::Pill => {
            if failed {
                app.gfx.arc(r.x + 30.0, r.y + r.h / 2.0, 14.0, 1.0, 4.0, Color::rgb(0xD7372B).fade(alpha));
                app.gfx.stroke(r.x + 30.0, r.y + r.h / 2.0 - 7.0, r.x + 30.0, r.y + r.h / 2.0 + 2.0, 3.0, Color::rgb(0xD7372B).fade(alpha));
                app.gfx.stroke(r.x + 30.0, r.y + r.h / 2.0 + 7.0, r.x + 30.0, r.y + r.h / 2.0 + 7.0, 3.5, Color::rgb(0xD7372B).fade(alpha));
            } else {
                ring(app, r.x + 30.0, r.y + r.h / 2.0, if row_done(app, i) { 1.0 } else { frac }, done_k, alpha);
            }
            r.x + 56.0
        }
        PanelStyle::Card => r.x + 16.0,
        PanelStyle::Stack => {
            // Three item icons, offset like a small stack; the top one lifts away when the current file changes.
            for (k, p) in stack_items.iter().enumerate().rev() {
                let mut ir = Rect::new(r.x + 14.0 + k as f32 * 6.0, r.y + 14.0 + k as f32 * 3.0, 40.0, 40.0);
                if k == 0
                    && let Some(fk) = flip_k.filter(|f| *f < 1.0)
                {
                    ir.y -= 14.0 * fk;
                }
                item_icon(app, p, ir, alpha * if k == 0 { 1.0 - flip_k.filter(|f| *f < 1.0).unwrap_or(0.0) * 0.6 } else { 0.8 });
            }
            r.x + 76.0
        }
    };
    let text_w = r.right() - text_x - 48.0;
    let g = &app.gfx;
    let fg = if failed { Color::rgb(0xD7372B) } else { t.text };
    let name = if style == PanelStyle::Pill && row_done(app, i) || failed || current.is_empty() { title.clone() } else { current.clone() };
    g.text(&name, Rect::new(text_x, r.y + 8.0, text_w, 20.0), &g.fonts.title, fg.fade(alpha));
    match style {
        PanelStyle::Pill => {
            g.text(&sub, Rect::new(text_x, r.y + 28.0, text_w, 20.0), &g.fonts.small, t.text_dim.fade(alpha));
        }
        PanelStyle::Card | PanelStyle::Stack => {
            if !failed && style == PanelStyle::Card && !current.is_empty() && row_done(app, i) {
                // Finished: the title already says it all.
            }
            let bar = Rect::new(text_x, r.y + 36.0, r.right() - text_x - 52.0, 8.0);
            g.fill_round(bar, 4.0, t.separator.fade(alpha));
            let fill = Rect::new(bar.x, bar.y, bar.w * if row_done(app, i) { 1.0 } else { frac }, bar.h);
            let col = if failed { Color::rgb(0xD7372B) } else if done_k.is_some() { Color::rgb(0x1F9D55) } else { t.accent };
            g.fill_round(fill, 4.0, col.fade(alpha));
            if done_k.is_none() && !failed && fill.w > 4.0 {
                // Shimmer sweeping across the filled part.
                g.push_clip(fill);
                let sx = fill.x - 60.0 + (secs(now, app.xfer.rows[i].started) * 0.8).fract() * (fill.w + 60.0);
                g.fill_rect(Rect::new(sx, fill.y, 60.0, fill.h), Color::rgb(0xFFFFFF).fade(0.35 * alpha));
                g.pop_clip();
            }
            g.text(&sub, Rect::new(text_x, r.y + 50.0, r.right() - text_x - 16.0, 20.0), &g.fonts.small, t.text_dim.fade(alpha));
        }
    }
    if failed {
        for (b, label, primary) in [(skip, "Skip", false), (retry, "Retry", true)] {
            g.fill_round(b, 7.0, if primary { t.accent } else { t.field_bg });
            g.text(label, Rect::new(b.x, b.y + 5.0, b.w, b.h - 5.0), &g.fonts.cell, if primary { t.selection_text } else { t.text });
        }
    } else if app.xfer.rows[i].done.is_none() {
        g.fill_round(cancel, 14.0, t.field_bg.fade(alpha));
        g.glyph(crate::draw::Glyph::Close, Rect::new(cancel.x + 6.0, cancel.y + 6.0, 16.0, 16.0), t.text_dim.fade(alpha));
    }
}

fn row_done(app: &App, i: usize) -> bool {
    app.xfer.rows[i].done.is_some_and(|d| !d.1)
}

fn paint_strip(app: &App, r: Rect, row: &Row, now: Instant) {
    let t = &app.theme;
    let Some((from, to)) = &row.drives else { return };
    card_bg(app, r, 14.0, 1.0);
    let g = &app.gfx;
    let drive = |x: f32, label: &str| {
        let b = Rect::new(x, r.y + 14.0, 56.0, 34.0);
        g.fill_round(b, 8.0, t.field_bg);
        g.glyph(crate::draw::Glyph::Drive, Rect::new(b.x + 18.0, b.y + 7.0, 20.0, 20.0), t.text);
        g.text(label, Rect::new(x - 22.0, r.y + 52.0, 100.0, 18.0), &g.fonts.cell, t.text_dim);
    };
    drive(r.x + 22.0, from);
    drive(r.right() - 78.0, to);
    let (x0, x1, cy) = (r.x + 88.0, r.right() - 88.0, r.y + 31.0);
    let speed = row.progress.speed();
    let time = secs(now, row.started);
    for k in 0..36 {
        let seed = (k as f32 * 0.618_034).fract();
        let p = particle_x(seed, time, speed);
        let wave = ((k as f32 * 1.7).sin()) * 10.0 * (p * std::f32::consts::PI).sin();
        let a = (p * std::f32::consts::PI).sin() * 0.9;
        let s = 2.0 + (k % 3) as f32;
        g.fill_round(Rect::new(x0 + (x1 - x0) * p - s / 2.0, cy + wave - s / 2.0, s, s), s / 2.0, t.accent.fade(a));
    }
    let label = format!("{}/s", crate::format::format_size(speed as u64));
    g.text(&label, Rect::new(r.x, r.bottom() - 22.0, r.w, 18.0), &g.fonts.cell, t.text_dim);
}

fn paint_prompt(app: &mut App, now: Instant) {
    let t = app.theme;
    let content = app.layout.content;
    let (conflict, shown) = match &app.xfer.prompt {
        Some(Prompt::Conflict { shown, .. }) => (true, *shown),
        Some(Prompt::ConfirmDelete { shown, .. }) => (false, *shown),
        None => return,
    };
    let k = spring(secs(now, shown) / 0.6);
    let (card0, buttons) = prompt_layout(content, conflict);
    let dy = (1.0 - k) * 60.0;
    let alpha = k.clamp(0.0, 1.0);
    let card = Rect::new(card0.x, card0.y + dy, card0.w, card0.h);
    app.gfx.fill_rect(content, Color::rgb(0x000000).fade(0.18 * alpha));
    card_bg(app, card, 14.0, alpha);
    match &app.xfer.prompt {
        Some(Prompt::Conflict { conflicts, idx, dest, apply_all, .. }) => {
            let (conflicts, idx, dest, apply_all) = (conflicts.clone(), *idx, dest.clone(), *apply_all);
            let src = conflicts[idx].clone();
            let dst = dest.join(src.file_name().unwrap_or_default());
            let left = conflicts.len() - idx - 1;
            let name = src.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let folder = crate::app::folder_title(&dest, &app.sidebar.drives);
            
            let g = &app.gfx;
            g.text(&format!("“{name}” already exists in {folder}"), Rect::new(card.x + 16.0, card.y + 12.0, card.w - 32.0, 22.0), &g.fonts.title, t.text.fade(alpha));
            let half = (card.w - 44.0) / 2.0;
            for (k2, (p, label)) in [(src.clone(), "Copying"), (dst.clone(), "Already there")].into_iter().enumerate() {
                let bx = Rect::new(card.x + 16.0 + k2 as f32 * (half + 12.0), card.y + 44.0, half, 76.0);
                let meta = std::fs::metadata(&p).ok();
                let size = meta.as_ref().map(|m| crate::format::format_size(m.len())).unwrap_or_default();
                let when = meta.as_ref().and_then(|m| m.modified().ok()).map(|_| {
                    use std::os::windows::fs::MetadataExt;
                    crate::format::format_modified(shell::to_local(meta.as_ref().map_or(0, |m| m.last_write_time())), shell::now_local())
                });
                app.gfx.fill_round(bx, 10.0, t.field_bg.fade(alpha));
                let thumb = app.thumb_or_request(p.clone(), 64.0);
                let ir = Rect::new(bx.x + 10.0, bx.y + 10.0, 56.0, 56.0);
                match thumb {
                    Some(b) => app.gfx.bitmap_fit(&b, ir),
                    None => item_icon(app, &p, ir, alpha),
                }
                let g = &app.gfx;
                g.text(label, Rect::new(bx.x + 76.0, bx.y + 12.0, bx.w - 84.0, 20.0), &g.fonts.title, t.text.fade(alpha));
                g.text(&size, Rect::new(bx.x + 76.0, bx.y + 34.0, bx.w - 84.0, 18.0), &g.fonts.small, t.text_dim.fade(alpha));
                g.text(&when.unwrap_or_default(), Rect::new(bx.x + 76.0, bx.y + 52.0, bx.w - 84.0, 18.0), &g.fonts.small, t.text_dim.fade(alpha));
            }
            let g = &app.gfx;
            for (b, r0) in &buttons {
                let r = Rect::new(r0.x, r0.y + dy, r0.w, r0.h);
                match b {
                    PromptButton::ApplyAll => {
                        if left > 0 {
                            let bx = Rect::new(r.x, r.y + 7.0, 16.0, 16.0);
                            g.fill_round(bx, 4.0, if apply_all { t.accent } else { t.field_bg }.fade(alpha));
                            if apply_all {
                                g.stroke(bx.x + 4.0, bx.y + 8.0, bx.x + 7.0, bx.y + 11.0, 2.0, t.selection_text);
                                g.stroke(bx.x + 7.0, bx.y + 11.0, bx.x + 12.0, bx.y + 5.0, 2.0, t.selection_text);
                            }
                            g.text(&format!("Do this for the rest ({left})"), Rect::new(r.x + 22.0, r.y + 5.0, r.w - 22.0, 20.0), &g.fonts.small, t.text.fade(alpha));
                        }
                    }
                    other => {
                        let primary = *other == PromptButton::Replace;
                        let label = match other {
                            PromptButton::Skip => "Skip",
                            PromptButton::KeepBoth => "Keep both",
                            _ => "Replace",
                        };
                        g.fill_round(r, 8.0, if primary { t.accent } else { t.field_bg }.fade(alpha));
                        g.text(label, Rect::new(r.x, r.y + 7.0, r.w, r.h - 7.0), &g.fonts.cell, if primary { t.selection_text } else { t.text }.fade(alpha));
                    }
                }
            }
        }
        Some(Prompt::ConfirmDelete { items, .. }) => {
            let n = items.len();
            let what = if n == 1 { format!("“{}”", items[0].file_name().unwrap_or_default().to_string_lossy()) } else { format!("{n} items") };
            let g = &app.gfx;
            g.text(&format!("Permanently delete {what}?"), Rect::new(card.x + 16.0, card.y + 16.0, card.w - 32.0, 22.0), &g.fonts.title, t.text.fade(alpha));
            g.text("This can’t be undone. They won’t go to the Recycle Bin.", Rect::new(card.x + 16.0, card.y + 44.0, card.w - 32.0, 20.0), &g.fonts.body, t.text_dim.fade(alpha));
            for (b, r0) in &buttons {
                let r = Rect::new(r0.x, r0.y + dy, r0.w, r0.h);
                let danger = *b == PromptButton::Delete;
                g.fill_round(r, 8.0, if danger { Color::rgb(0xD7372B) } else { t.field_bg }.fade(alpha));
                g.text(if danger { "Delete" } else { "Cancel" }, Rect::new(r.x, r.y + 7.0, r.w, r.h - 7.0), &g.fonts.cell, if danger { Color::rgb(0xFFFFFF) } else { t.text }.fade(alpha));
            }
        }
        None => {}
    }
}

/// The Recycle Bin: pops up, opens its lid while items fly in, closes with a wobble, then sinks away.
fn paint_bin(app: &App, now: Instant) {
    let Some(bin) = &app.xfer.bin else { return };
    let t = &app.theme;
    let b = bin_rect(app.layout.content);
    let since = secs(now, bin.shown);
    let pop = spring(since / 0.5);
    let leave = bin.landed.map_or(0.0, |l| ((secs(now, l) - 0.7) / 0.35).clamp(0.0, 1.0));
    let scale = pop * (1.0 - leave);
    if scale <= 0.01 {
        return;
    }
    let close = bin.landed.map_or(0.0, |l| (secs(now, l) / 0.22).clamp(0.0, 1.0));
    let open = (since / 0.25).clamp(0.0, 1.0) * (1.0 - close);
    let wobble = bin.landed.map_or(0.0, |l| shake(secs(now, l) / 0.5) * 0.6);
    let center = Vector2 { X: b.x + b.w / 2.0, Y: b.bottom() };
    let base = Matrix3x2::scale_around(scale, scale, center) * Matrix3x2::rotation_around(wobble, center);
    let g = &app.gfx;
    g.set_transform(base);
    let (metal, dark, light) = if t.dark {
        (Color::rgb(0x9A9EA6), Color::rgb(0x6A6E76), Color::rgb(0xC4C8CF))
    } else {
        (Color::rgb(0xC2C7CF), Color::rgb(0x8E949E), Color::rgb(0xE3E6EA))
    };
    // Shadow, then a body that narrows toward the bottom.
    g.fill_round(Rect::new(b.x + 10.0, b.bottom() - 7.0, b.w - 20.0, 9.0), 4.5, Color::rgb(0x000000).fade(0.22));
    let (top, bot) = (b.y + 26.0, b.bottom() - 2.0);
    g.fill_poly(&[(b.x + 6.0, top), (b.right() - 6.0, top), (b.right() - 14.0, bot), (b.x + 14.0, bot)], metal);
    g.fill_poly(&[(b.x + 6.0, top), (b.x + 20.0, top), (b.x + 24.0, bot), (b.x + 14.0, bot)], light.fade(0.55));
    for k in 1..4 {
        let f = k as f32 / 4.0;
        let (xt, xb) = (b.x + 6.0 + (b.w - 12.0) * f, b.x + 14.0 + (b.w - 28.0) * f);
        g.stroke(xt, top + 12.0, xb, bot - 9.0, 3.0, dark);
    }
    // Dark mouth shows while the lid is up.
    if open > 0.05 {
        g.fill_round(Rect::new(b.x + 6.0, top - 5.0, b.w - 12.0, 10.0), 5.0, Color::rgb(0x202226).fade(open));
    }
    // Lid hinged at its right end; swings up and back.
    let hinge = Vector2 { X: b.right() - 3.0, Y: top - 2.0 };
    g.set_transform(Matrix3x2::rotation_around(78.0 * open, hinge) * base);
    g.fill_round(Rect::new(b.x + 1.0, top - 10.0, b.w - 2.0, 10.0), 5.0, metal);
    g.fill_round(Rect::new(b.x + 1.0, top - 10.0, b.w - 2.0, 4.0), 2.0, light.fade(0.6));
    g.fill_round(Rect::new(b.x + b.w / 2.0 - 11.0, top - 16.0, 22.0, 7.0), 3.5, dark);
    g.reset_transform();
}

fn paint_flights(app: &mut App, now: Instant) {
    let flights: Vec<(PathBuf, bool, f32, f32, f32, f32)> = app
        .xfer
        .flights
        .iter()
        .filter_map(|f| {
            let t = f.t(now);
            (0.0..1.0).contains(&t).then(|| {
                let (x, y, s) = flight_point(f.from, f.to, ease_out(t));
                (f.path.clone(), f.is_dir, x, y, s, if t > 0.85 { (1.0 - t) / 0.15 } else { 1.0 })
            })
        })
        .collect();
    for (path, is_dir, x, y, s, a) in flights {
        let size = 48.0 * s;
        let r = Rect::new(x - size / 2.0, y - size / 2.0, size, size);
        app.gfx.fill_round(Rect::new(r.x + 3.0, r.y + 5.0, r.w, r.h), 8.0, Color::rgb(0x000000).fade(0.12 * a));
        if is_dir {
            crate::views::grid::paint_folder(app, r);
        } else {
            item_icon(app, &path, r, a);
        }
    }
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

fn paint_toast(app: &App, now: Instant) {
    let Some(toast) = &app.xfer.toast else { return };
    let t = &app.theme;
    let (r0, undo) = toast_rect(app.layout.content);
    let k = spring(secs(now, toast.shown) / 0.5);
    let out = ((secs(now, toast.shown) - (TOAST_SECS - 0.3)) / 0.3).clamp(0.0, 1.0);
    let dy = (1.0 - k) * 30.0 + out * 20.0;
    let a = (1.0 - out) * k.min(1.0);
    let r = Rect::new(r0.x, r0.y + dy, r0.w, r0.h);
    let g = &app.gfx;
    g.fill_round(r, r.h / 2.0, t.text.fade(0.92 * a));
    g.text(&toast.text, Rect::new(r.x + 18.0, r.y, r.w - 110.0, r.h), &g.fonts.body, t.content_bg.fade(a));
    if !toast.undo.is_empty() {
        let u = Rect::new(undo.x, undo.y + dy, undo.w, undo.h);
        g.fill_round(u, 14.0, t.content_bg.fade(0.18 * a));
        g.text("Undo", Rect::new(u.x, u.y + 6.0, u.w, u.h - 6.0), &g.fonts.cell, t.content_bg.fade(a));
    }
}

pub fn paint(app: &mut App) {
    let now = Instant::now();
    let content = app.layout.content;
    let shown = app.xfer.shown();
    let rects = row_rects(content, shown.len(), app.settings.panel);
    // Newest row sits lowest: rows are stored oldest first.
    for (slot, &i) in shown.iter().rev().enumerate() {
        paint_row(app, i, rects[slot], now);
    }
    if let Some(top) = rects.last().copied()
        && !app.xfer.strip_hidden
        && let Some(row) = app.xfer.rows.iter().rev().find(|r| r.done.is_none() && r.drives.is_some())
    {
        paint_strip(app, strip_rect(content, top), row, now);
    }
    paint_bin(app, now);
    paint_flights(app, now);
    paint_toast(app, now);
    paint_prompt(app, now);
}

/// What a click in the transfer UI hit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    Cancel(u64),
    Retry(u64),
    Skip(u64),
    Row,
    Prompt(PromptButton),
    PromptCard,
    Undo,
}

pub fn hit(app: &App, x: f32, y: f32) -> Option<Hit> {
    let content = app.layout.content;
    if let Some(p) = &app.xfer.prompt {
        let (_, buttons) = prompt_layout(content, matches!(p, Prompt::Conflict { .. }));
        return Some(buttons.into_iter().find(|b| b.1.contains(x, y)).map_or(Hit::PromptCard, |b| Hit::Prompt(b.0)));
    }
    if let Some(toast) = &app.xfer.toast {
        let (r, undo) = toast_rect(content);
        if !toast.undo.is_empty() && undo.contains(x, y) {
            return Some(Hit::Undo);
        }
        if r.contains(x, y) {
            return Some(Hit::Row);
        }
    }
    let shown = app.xfer.shown();
    let rects = row_rects(content, shown.len(), app.settings.panel);
    for (slot, &i) in shown.iter().rev().enumerate() {
        let (r, row) = (rects[slot], &app.xfer.rows[i]);
        if !r.contains(x, y) {
            continue;
        }
        let (cancel, retry, skip) = row_buttons(r);
        return Some(if !row.failed.is_empty() && retry.contains(x, y) {
            Hit::Retry(row.id)
        } else if !row.failed.is_empty() && skip.contains(x, y) {
            Hit::Skip(row.id)
        } else if row.done.is_none() && cancel.contains(x, y) {
            Hit::Cancel(row.id)
        } else {
            Hit::Row
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Rect;

    fn content() -> Rect {
        Rect::new(200.0, 64.0, 900.0, 636.0)
    }

    #[test]
    fn rows_stack_up_from_the_bottom_center() {
        let r = row_rects(content(), 3, PanelStyle::Pill);
        assert_eq!(r.len(), 3);
        assert!((r[0].x + r[0].w / 2.0 - (content().x + content().w / 2.0)).abs() < 0.5, "centered");
        assert!(r[0].bottom() <= content().bottom() - 12.0);
        assert!(r[1].bottom() <= r[0].y && r[2].bottom() <= r[1].y, "newer rows above older");
        assert!(row_rects(content(), 1, PanelStyle::Card)[0].h > row_rects(content(), 1, PanelStyle::Pill)[0].h);
    }

    #[test]
    fn row_buttons_sit_inside_their_row() {
        let row = row_rects(content(), 1, PanelStyle::Card)[0];
        let (cancel, retry, skip) = row_buttons(row);
        for b in [cancel, retry, skip] {
            assert!(b.x >= row.x && b.right() <= row.right() && b.y >= row.y && b.bottom() <= row.bottom(), "{b:?} in {row:?}");
        }
        assert!(skip.right() <= retry.x, "Skip then Retry");
    }

    #[test]
    fn prompt_buttons_fit_and_bin_sits_bottom_right() {
        let (card, buttons) = prompt_layout(content(), true);
        assert_eq!(buttons.len(), 4, "checkbox, Skip, Keep both, Replace");
        for b in &buttons {
            assert!(b.1.x >= card.x && b.1.right() <= card.right() && b.1.bottom() <= card.bottom());
        }
        let (card, buttons) = prompt_layout(content(), false);
        assert_eq!(buttons.iter().map(|b| b.0).collect::<Vec<_>>(), [PromptButton::Cancel, PromptButton::Delete]);
        assert!(card.bottom() <= content().bottom());
        let bin = bin_rect(content());
        assert!(bin.right() <= content().right() && bin.bottom() <= content().bottom() && bin.x > content().x + content().w / 2.0);
    }
}
