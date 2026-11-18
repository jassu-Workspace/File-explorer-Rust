use crate::app::App;
use crate::draw::IconSize;
use crate::layout::Rect;
pub use crate::listing::Group;
use crate::shell;
use crate::theme::{Color, ROW_H};

/// Left/right padding of the grid inside the content area.
pub const PAD: f32 = 12.0;
pub const ICON_MIN: f32 = 48.0;
pub const ICON_MAX: f32 = 128.0;
pub const ICON_DEFAULT: f32 = 64.0;
/// Height of a group's section header ("Today", "PDF Document", ...).
pub const HEADER_H: f32 = 28.0;
const NAME_H: f32 = 32.0; // two lines of 12 px text

pub fn clamp_icon(size: f32) -> f32 {
    size.clamp(ICON_MIN, ICON_MAX)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// Item geometry for the icon view (many columns) and the list view (one column), with optional
/// group sections. Items are indexes into the sorted entries; each group starts a new row under its header.
#[derive(Clone, Debug)]
pub struct Grid {
    pub cols: usize,
    pub cell_w: f32,
    pub cell_h: f32,
    pub icon: f32,
    pad: f32,
    /// Groups and the y of each group's header; empty means one group without a header.
    groups: Vec<(Group, f32)>,
}

impl Grid {
    pub fn new(width: f32, icon: f32) -> Grid {
        let min_w = icon + 40.0;
        let usable = (width - 2.0 * PAD).max(min_w);
        let cols = ((usable / min_w).floor() as usize).max(1);
        Grid { cols, cell_w: usable / cols as f32, cell_h: icon + 54.0, icon, pad: PAD, groups: Vec::new() }
    }

    /// One row per item, full width (list view).
    pub fn list(width: f32) -> Grid {
        Grid { cols: 1, cell_w: width, cell_h: ROW_H, icon: 16.0, pad: 0.0, groups: Vec::new() }
    }

    pub fn grouped(mut self, groups: Vec<Group>) -> Grid {
        let mut y = 0.0;
        self.groups = groups
            .into_iter()
            .map(|g| {
                let top = y;
                y += HEADER_H + self.rows(g.range.len()) as f32 * self.cell_h;
                (g, top)
            })
            .collect();
        self
    }

    fn rows(&self, len: usize) -> usize {
        len.div_ceil(self.cols)
    }

    /// (first index, item count, y of the first row) for each group; one unlabeled group when ungrouped.
    fn sections(&self, n: usize) -> Vec<(usize, usize, f32)> {
        if self.groups.is_empty() {
            return vec![(0, n, 0.0)];
        }
        self.groups.iter().map(|(g, top)| (g.range.start, g.range.len(), top + HEADER_H)).collect()
    }

    pub fn headers(&self, _n: usize) -> Vec<(String, f32)> {
        self.groups.iter().map(|(g, top)| (g.label.clone(), *top)).collect()
    }

    pub fn content_h(&self, n: usize) -> f32 {
        self.sections(n).last().map_or(0.0, |&(_, len, y)| y + self.rows(len) as f32 * self.cell_h)
    }

    /// Cell rect relative to the grid origin (content top-left), before scrolling.
    pub fn cell(&self, i: usize) -> Rect {
        let (start, _, y) = self.sections(i + 1).into_iter().rev().find(|s| s.0 <= i).unwrap_or((0, 0, 0.0));
        let k = i - start;
        Rect::new(self.pad + (k % self.cols) as f32 * self.cell_w, y + (k / self.cols) as f32 * self.cell_h, self.cell_w, self.cell_h)
    }

    /// Item under a point given relative to the content top-left.
    pub fn cell_at(&self, x: f32, y: f32, scroll: f32, n: usize) -> Option<usize> {
        if x < self.pad || y < 0.0 {
            return None;
        }
        let y = y + scroll;
        let (start, len, top) = self.sections(n).into_iter().rev().find(|s| s.2 - HEADER_H <= y || s.0 == 0)?;
        if y < top {
            return None; // on a header
        }
        let col = ((x - self.pad) / self.cell_w) as usize;
        let k = ((y - top) / self.cell_h) as usize * self.cols + col;
        (col < self.cols && k < len).then_some(start + k)
    }

    /// Items with any part inside the vertical window [scroll, scroll + viewport_h).
    pub fn visible(&self, scroll: f32, viewport_h: f32, n: usize) -> Vec<usize> {
        let (lo, hi) = (scroll.max(0.0), scroll.max(0.0) + viewport_h);
        let mut out = Vec::new();
        for (start, len, top) in self.sections(n) {
            let bottom = top + self.rows(len) as f32 * self.cell_h;
            if bottom <= lo || top >= hi {
                continue;
            }
            let r0 = ((lo - top).max(0.0) / self.cell_h) as usize;
            let r1 = (((hi - top) / self.cell_h).ceil() as usize).min(self.rows(len));
            out.extend((r0 * self.cols..(r1 * self.cols).min(len)).map(|k| start + k));
        }
        out
    }

    /// Items whose cell intersects `r` (grid coordinates, scroll included).
    pub fn cells_in(&self, r: Rect, n: usize) -> Vec<usize> {
        self.visible(r.y, r.h.max(0.01), n)
            .into_iter()
            .filter(|&i| {
                let c = self.cell(i);
                c.x < r.right() && r.x < c.right() && c.y < r.bottom() && r.y < c.bottom()
            })
            .collect()
    }

    /// Next focus for an arrow key. Up/Down keep the column, clamped to shorter rows; they cross group boundaries.
    pub fn step(&self, cur: Option<usize>, d: Dir, n: usize) -> usize {
        let Some(c) = cur else { return 0 };
        if n == 0 {
            return 0;
        }
        // Every visual row as (first index, length).
        let rows: Vec<(usize, usize)> = self
            .sections(n)
            .into_iter()
            .flat_map(|(start, len, _)| (0..self.rows(len)).map(move |r| (start + r * self.cols, (len - r * self.cols).min(self.cols))))
            .collect();
        let Some(ri) = rows.iter().position(|&(s, l)| c >= s && c < s + l) else { return c.min(n - 1) };
        let col = c - rows[ri].0;
        let to_row = |r: usize| rows[r].0 + col.min(rows[r].1 - 1);
        match d {
            Dir::Left => c.saturating_sub(1),
            Dir::Right => (c + 1).min(n - 1),
            Dir::Up if ri > 0 => to_row(ri - 1),
            Dir::Down if ri + 1 < rows.len() => to_row(ri + 1),
            _ => c,
        }
    }

    /// Scroll offset that keeps item `i` fully visible.
    pub fn scroll_to_show(&self, i: usize, scroll: f32, viewport_h: f32) -> f32 {
        let c = self.cell(i);
        if c.y < scroll {
            c.y
        } else if c.bottom() > scroll + viewport_h {
            c.bottom() - viewport_h
        } else {
            scroll
        }
    }
}

/// Finder-style overlapping tag dots starting at `x`, vertically centered on `cy`.
pub fn paint_tag_dots(app: &App, tags: &[crate::tags::Tag], x: f32, cy: f32) {
    for (k, tag) in tags.iter().enumerate() {
        let r = Rect::new(x + k as f32 * 6.0, cy - 4.5, 9.0, 9.0);
        app.gfx.fill_round(Rect::new(r.x - 1.0, r.y - 1.0, r.w + 2.0, r.h + 2.0), 5.5, app.theme.content_bg);
        app.gfx.fill_round(r, 4.5, Color::rgb(tag.rgb()));
    }
}

/// Section header: label on the left and a hairline under it.
pub fn paint_header(app: &App, label: &str, r: Rect) {
    let t = app.theme;
    app.gfx.text(label, Rect::new(r.x + 12.0, r.y + 2.0, r.w - 24.0, r.h - 4.0), &app.gfx.fonts.title, t.text);
    app.gfx.line(r.x + 12.0, r.bottom() - 0.5, r.right() - 12.0, r.bottom() - 0.5, t.separator);
}

/// Finder-style folder: a darker back with the tab, a lighter front panel.
pub fn paint_folder(app: &App, r: Rect) {
    let (back, front) = if app.theme.dark { (Color::rgb(0x3D8FD9), Color::rgb(0x5AA9EE)) } else { (Color::rgb(0x5AA8EC), Color::rgb(0x84C1F5)) };
    let g = &app.gfx;
    // Proportions match a 16-unit folder glyph scaled into the icon box.
    let u = r.w / 16.0;
    g.fill_round(Rect::new(r.x + 1.5 * u, r.y + 3.0 * u, 5.5 * u, 2.5 * u), 1.0 * u, back);
    g.fill_round(Rect::new(r.x + 1.5 * u, r.y + 4.0 * u, 13.0 * u, 9.5 * u), 1.2 * u, back);
    g.fill_round(Rect::new(r.x + 1.5 * u, r.y + 5.5 * u, 13.0 * u, 8.0 * u), 1.2 * u, front);
}

pub fn paint(app: &mut App) {
    let content = app.layout.content;
    let t = app.theme;
    let grid = app.grid();
    let n = app.list.entries.len();
    let scroll = app.list.scroll_y;
    app.gfx.push_clip(content);
    for (label, y) in grid.headers(n) {
        paint_header(app, &label, Rect::new(content.x, content.y + y - scroll, content.w, HEADER_H));
    }
    for i in grid.visible(scroll, content.h, n) {
        let c = grid.cell(i);
        let cell = Rect::new(content.x + c.x, content.y + c.y - scroll, c.w, c.h);
        let icon = Rect::new(cell.x + (cell.w - grid.icon) / 2.0, cell.y + 8.0, grid.icon, grid.icon);
        let halo = Rect::new(icon.x - 5.0, icon.y - 5.0, icon.w + 10.0, icon.h + 10.0);
        let selected = app.list.sel.is_selected(i);
        if selected {
            app.gfx.fill_round(halo, 8.0, t.text.with_alpha(0.10));
        } else if app.list.hover_row == Some(i) {
            app.gfx.fill_round(halo, 8.0, t.hover.with_alpha(t.hover.a * app.hover_alpha()));
        }
        let e = &app.list.entries[i];
        if e.is_dir() {
            paint_folder(app, icon);
        } else {
            let path = app.entry_path(&app.list.entries[i]);
            if let Some(t) = app.thumb_or_request(path, grid.icon) {
                app.gfx.bitmap_fit(&t, icon);
            } else {
                let idx = shell::icon_index(&app.list.entries[i], &mut app.icon_idx);
                if let Some(b) = app.gfx.icon(idx, IconSize::Jumbo) {
                    app.gfx.bitmap(&b, icon);
                }
            }
        }
        let name = shell::display_name(&app.list.entries[i]);
        let text = Rect::new(cell.x + 4.0, icon.bottom() + 8.0, cell.w - 8.0, NAME_H);
        let tags = app.tags.of(&app.entry_path(&app.list.entries[i]));
        if !tags.is_empty() {
            let (w, _) = app.gfx.measure_box(&name, &app.gfx.fonts.cell, text.w, text.h);
            let x = text.x + (text.w - w) / 2.0 - 8.0 - 6.0 * tags.len() as f32;
            paint_tag_dots(app, &tags, x.max(cell.x), text.y + 8.0);
        }
        let fg = if selected {
            let (w, h) = app.gfx.measure_box(&name, &app.gfx.fonts.cell, text.w, text.h);
            let pill = Rect::new(text.x + (text.w - w) / 2.0 - 4.0, text.y - 1.0, w + 8.0, h + 2.0);
            app.gfx.fill_round(pill, 4.0, t.accent);
            t.selection_text
        } else {
            t.text
        };
        app.gfx.text(&name, text, &app.gfx.fonts.cell, fg);
    }
    if let Some(b) = app.band_rect() {
        let r = Rect::new(content.x + b.x, content.y + b.y - scroll, b.w, b.h);
        app.gfx.fill_rect(r, t.accent.with_alpha(0.15));
        for (x1, y1, x2, y2) in [(r.x, r.y, r.right(), r.y), (r.x, r.bottom(), r.right(), r.bottom()), (r.x, r.y, r.x, r.bottom()), (r.right(), r.y, r.right(), r.bottom())] {
            app.gfx.line(x1, y1, x2, y2, t.accent.with_alpha(0.7));
        }
    }
    app.gfx.pop_clip();
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_fill_width_evenly() {
        let g = Grid::new(1000.0, 64.0);
        assert_eq!(g.cols, 9);
        assert!((g.cols as f32 * g.cell_w + 2.0 * PAD - 1000.0).abs() < 0.01, "cells spread over the full width");
        assert_eq!(Grid::new(50.0, 64.0).cols, 1, "never zero columns");
    }

    #[test]
    fn cell_hit_and_miss() {
        let g = Grid::new(1000.0, 64.0);
        assert_eq!(g.cell_at(PAD + 1.0, 1.0, 0.0, 20), Some(0));
        assert_eq!(g.cell_at(PAD + g.cell_w * 2.5, g.cell_h * 1.5, 0.0, 20), Some(11));
        assert_eq!(g.cell_at(PAD + g.cell_w * 5.0, g.cell_h * 2.5, 0.0, 20), None, "past the last item");
        assert_eq!(g.cell_at(1.0, 1.0, 0.0, 20), None, "left padding");
        assert_eq!(g.cell_at(PAD + 1.0, 1.0, g.cell_h, 20), Some(9), "scroll shifts rows");
    }

    #[test]
    fn visible_range_covers_partial_rows() {
        let g = Grid::new(1000.0, 64.0);
        assert_eq!(g.visible(0.0, g.cell_h * 2.0 + 1.0, 100), (0..27).collect::<Vec<_>>());
        assert_eq!(g.visible(g.cell_h * 10.0, g.cell_h, 95), (90..95).collect::<Vec<_>>());
        assert!(g.visible(0.0, 500.0, 0).is_empty());
        assert_eq!(g.content_h(19), 3.0 * g.cell_h);
    }

    #[test]
    fn arrows_move_in_two_dimensions() {
        let g = Grid::new(1000.0, 64.0); // 9 columns
        assert_eq!(g.step(Some(0), Dir::Right, 20), 1);
        assert_eq!(g.step(Some(8), Dir::Right, 20), 9, "right wraps to the next row");
        assert_eq!(g.step(Some(3), Dir::Down, 20), 12);
        assert_eq!(g.step(Some(12), Dir::Down, 20), 19, "down onto a short last row lands on the last item");
        assert_eq!(g.step(Some(3), Dir::Up, 20), 3, "top row stays");
        assert_eq!(g.step(Some(0), Dir::Left, 20), 0);
        assert_eq!(g.step(None, Dir::Down, 20), 0, "no focus starts at the first item");
    }

    #[test]
    fn scroll_keeps_focus_visible() {
        let g = Grid::new(1000.0, 64.0);
        assert_eq!(g.scroll_to_show(45, 0.0, g.cell_h * 2.0), g.cell_h * 4.0);
        assert_eq!(g.scroll_to_show(0, g.cell_h * 4.0, g.cell_h * 2.0), 0.0);
        assert_eq!(g.scroll_to_show(10, g.cell_h, g.cell_h * 2.0), g.cell_h);
    }

    #[test]
    fn drag_box_selects_touched_cells() {
        let g = Grid::new(1000.0, 64.0); // 9 columns
        let r = Rect::new(PAD + g.cell_w * 0.5, g.cell_h * 0.5, g.cell_w * 1.2, g.cell_h);
        assert_eq!(g.cells_in(r, 20), [0, 1, 9, 10]);
        let past_end = Rect::new(PAD, g.cell_h * 2.2, g.cell_w * 9.0, 10.0);
        assert_eq!(g.cells_in(past_end, 20), [18, 19], "only existing items");
        assert!(g.cells_in(Rect::new(0.0, 0.0, 2.0, 2.0), 20).is_empty(), "padding touches nothing");
    }

    fn grouped() -> Grid {
        // 9 columns; groups of 3 and 11 items, each with a header.
        Grid::new(1000.0, 64.0).grouped(vec![Group { label: "A".into(), range: 0..3 }, Group { label: "B".into(), range: 3..14 }])
    }

    #[test]
    fn groups_start_on_new_rows_under_headers() {
        let g = grouped();
        assert_eq!(g.cell(0).y, HEADER_H);
        assert_eq!(g.cell(3).y, HEADER_H + g.cell_h + HEADER_H, "second group starts below the first group's row and its own header");
        assert_eq!(g.cell(3).x, PAD, "and at the left edge");
        assert_eq!(g.content_h(14), 2.0 * HEADER_H + 3.0 * g.cell_h);
        let h = g.headers(14);
        assert_eq!(h, [("A".to_string(), 0.0), ("B".to_string(), HEADER_H + g.cell_h)]);
    }

    #[test]
    fn grouped_hits_skip_headers() {
        let g = grouped();
        assert_eq!(g.cell_at(PAD + 1.0, 2.0, 0.0, 14), None, "header row");
        assert_eq!(g.cell_at(PAD + 1.0, HEADER_H + 2.0, 0.0, 14), Some(0));
        assert_eq!(g.cell_at(PAD + g.cell_w * 4.0, HEADER_H + 2.0, 0.0, 14), None, "past the end of group A's row");
        let b = g.cell(12);
        assert_eq!(g.cell_at(b.x + 2.0, b.y + 2.0, 0.0, 14), Some(12));
    }

    #[test]
    fn arrows_cross_groups_by_row() {
        let g = grouped();
        assert_eq!(g.step(Some(1), Dir::Down, 14), 4, "row of A -> first row of B, same column");
        assert_eq!(g.step(Some(8), Dir::Up, 14), 2, "column clamps to the short row above");
        assert_eq!(g.step(Some(13), Dir::Up, 14), 4);
    }

    #[test]
    fn list_geometry_is_one_column() {
        let g = Grid::list(500.0);
        assert_eq!(g.cols, 1);
        assert_eq!(g.cell(3).y, 3.0 * crate::theme::ROW_H);
        assert_eq!(g.cell_at(10.0, crate::theme::ROW_H * 2.5, 0.0, 5), Some(2));
    }

    #[test]
    fn icon_size_is_clamped() {
        assert_eq!(clamp_icon(16.0), ICON_MIN);
        assert_eq!(clamp_icon(500.0), ICON_MAX);
        assert_eq!(clamp_icon(80.0), 80.0);
    }
}
