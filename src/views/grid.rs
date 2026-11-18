use crate::app::App;
use crate::draw::IconSize;
use crate::layout::Rect;
use crate::shell;
use crate::theme::Color;
use std::ops::Range;

/// Left/right padding of the grid inside the content area.
pub const PAD: f32 = 12.0;
pub const ICON_MIN: f32 = 48.0;
pub const ICON_MAX: f32 = 128.0;
pub const ICON_DEFAULT: f32 = 64.0;
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

/// Icon-view geometry for a content width and icon size. Cells are stretched to fill the row evenly.
#[derive(Clone, Copy, Debug)]
pub struct Grid {
    pub cols: usize,
    pub cell_w: f32,
    pub cell_h: f32,
    pub icon: f32,
}

impl Grid {
    pub fn new(width: f32, icon: f32) -> Grid {
        let min_w = icon + 40.0;
        let usable = (width - 2.0 * PAD).max(min_w);
        let cols = ((usable / min_w).floor() as usize).max(1);
        Grid { cols, cell_w: usable / cols as f32, cell_h: icon + 54.0, icon }
    }

    fn rows(&self, n: usize) -> usize {
        n.div_ceil(self.cols)
    }

    pub fn content_h(&self, n: usize) -> f32 {
        self.rows(n) as f32 * self.cell_h
    }

    /// Cell rect relative to the grid origin (content top-left), before scrolling.
    pub fn cell(&self, i: usize) -> Rect {
        Rect::new(PAD + (i % self.cols) as f32 * self.cell_w, (i / self.cols) as f32 * self.cell_h, self.cell_w, self.cell_h)
    }

    /// Item under a point given relative to the content top-left.
    pub fn cell_at(&self, x: f32, y: f32, scroll: f32, n: usize) -> Option<usize> {
        if x < PAD || y < 0.0 {
            return None;
        }
        let col = ((x - PAD) / self.cell_w) as usize;
        let row = ((y + scroll) / self.cell_h) as usize;
        let i = row * self.cols + col;
        (col < self.cols && i < n).then_some(i)
    }

    pub fn visible(&self, scroll: f32, viewport_h: f32, n: usize) -> Range<usize> {
        let first = (scroll.max(0.0) / self.cell_h) as usize * self.cols;
        let last_row = ((scroll.max(0.0) + viewport_h) / self.cell_h).ceil() as usize;
        first.min(n)..(last_row * self.cols).min(n)
    }

    /// Items whose cell intersects `r` (grid coordinates, scroll included).
    pub fn cells_in(&self, r: Rect, n: usize) -> Vec<usize> {
        let col = |x: f32| ((x - PAD) / self.cell_w).floor().clamp(0.0, self.cols as f32 - 1.0) as usize;
        if r.right() <= PAD || r.x >= PAD + self.cols as f32 * self.cell_w || r.bottom() <= 0.0 {
            return Vec::new();
        }
        let (c0, c1) = (col(r.x), col(r.right() - 0.01));
        let (r0, r1) = ((r.y.max(0.0) / self.cell_h) as usize, ((r.bottom() - 0.01) / self.cell_h) as usize);
        (r0..=r1).flat_map(|row| (c0..=c1).map(move |c| row * self.cols + c)).filter(|&i| i < n).collect()
    }

    /// Next focus for an arrow key. Moving down onto a short last row lands on the last item.
    pub fn step(&self, cur: Option<usize>, d: Dir, n: usize) -> usize {
        let Some(c) = cur else { return 0 };
        if n == 0 {
            return 0;
        }
        match d {
            Dir::Left => c.saturating_sub(1),
            Dir::Right => (c + 1).min(n - 1),
            Dir::Up => c.checked_sub(self.cols).unwrap_or(c),
            Dir::Down if c + self.cols < n => c + self.cols,
            Dir::Down if c / self.cols < self.rows(n) - 1 => n - 1,
            Dir::Down => c,
        }
    }

    /// Scroll offset that keeps item `i` fully visible.
    pub fn scroll_to_show(&self, i: usize, scroll: f32, viewport_h: f32) -> f32 {
        let top = (i / self.cols) as f32 * self.cell_h;
        if top < scroll {
            top
        } else if top + self.cell_h > scroll + viewport_h {
            top + self.cell_h - viewport_h
        } else {
            scroll
        }
    }
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
        assert_eq!(g.visible(0.0, g.cell_h * 2.0 + 1.0, 100), 0..27);
        assert_eq!(g.visible(g.cell_h * 10.0, g.cell_h, 95), 90..95);
        assert_eq!(g.visible(0.0, 500.0, 0), 0..0);
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

    #[test]
    fn icon_size_is_clamped() {
        assert_eq!(clamp_icon(16.0), ICON_MIN);
        assert_eq!(clamp_icon(500.0), ICON_MAX);
        assert_eq!(clamp_icon(80.0), 80.0);
    }
}
