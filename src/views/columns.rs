use crate::app::App;
use crate::draw::{Glyph, IconSize};
use crate::format::{format_modified, format_size};
use crate::layout::{Rect, row_at, visible_rows};
use crate::listing::Entry;
use crate::shell;
use crate::theme::{COLUMN_W, ROW_H};

pub const INFO_W: f32 = 260.0;

/// Horizontal scroll that shows the right end of the columns when they overflow.
pub fn fit_scroll(total_w: f32, viewport_w: f32, _scroll_x: f32) -> f32 {
    (total_w - viewport_w).max(0.0)
}

/// The file selected in the deepest column, if any (it gets the info panel).
pub fn selected_file(app: &App) -> Option<&Entry> {
    let c = app.columns.cols.last()?;
    let e = c.entries.as_ref()?.get(c.selected?)?;
    (!e.is_dir()).then_some(e)
}

pub fn total_width(app: &App) -> f32 {
    app.columns.cols.len() as f32 * COLUMN_W + if selected_file(app).is_some() { INFO_W } else { 0.0 }
}

fn col_rect(app: &App, i: usize) -> Rect {
    let c = app.layout.content;
    Rect::new(c.x + i as f32 * COLUMN_W - app.columns.scroll_x, c.y, COLUMN_W, c.h)
}

/// (column, row) under the point; row is `None` for empty space in the column.
pub fn hit(app: &App, x: f32, y: f32) -> Option<(usize, Option<usize>)> {
    if !app.layout.content.contains(x, y) {
        return None;
    }
    (0..app.columns.cols.len()).find(|&i| col_rect(app, i).contains(x, y)).map(|i| {
        let c = &app.columns.cols[i];
        let n = c.entries.as_ref().map_or(0, Vec::len);
        (i, row_at(y - app.layout.content.y, c.scroll_y, ROW_H, n))
    })
}

pub fn paint(app: &mut App) {
    let content = app.layout.content;
    let t = app.theme;
    let large = app.scale >= 1.5;
    app.gfx.push_clip(content);
    for i in 0..app.columns.cols.len() {
        let r = col_rect(app, i);
        if r.right() < content.x || r.x > content.right() {
            continue;
        }
        let focused = i == app.columns.focus_col;
        let (scroll_y, selected) = (app.columns.cols[i].scroll_y, app.columns.cols[i].selected);
        let Some(entries) = app.columns.cols[i].entries.take() else {
            app.gfx.line(r.right() - 0.5, r.y, r.right() - 0.5, r.bottom(), t.separator);
            continue;
        };
        for row in visible_rows(scroll_y, r.h, ROW_H, entries.len()) {
            let e = &entries[row];
            let y = r.y + row as f32 * ROW_H - scroll_y;
            let bg = Rect::new(r.x + 4.0, y + 1.0, r.w - 8.0, ROW_H - 2.0);
            let sel = selected == Some(row);
            if sel {
                // Finder dims the selection in columns that do not have keyboard focus.
                app.gfx.fill_round(bg, 5.0, if focused { t.accent } else { t.text.with_alpha(0.12) });
            }
            let fg = if sel && focused { t.selection_text } else { t.text };
            let icon = Rect::new(r.x + 10.0, y + (ROW_H - 16.0) / 2.0, 16.0, 16.0);
            if e.is_dir() {
                app.gfx.glyph(Glyph::Folder, icon, if sel && focused { fg } else { t.accent });
                app.gfx.glyph(Glyph::Chevron, Rect::new(r.right() - 22.0, y + 7.0, 10.0, 10.0), if sel && focused { fg } else { t.text_dim });
            } else {
                let idx = shell::icon_index(e, &mut app.icon_idx);
                if let Some(b) = app.gfx.icon(idx, if large { IconSize::Large } else { IconSize::Small }) {
                    app.gfx.bitmap(&b, icon);
                }
            }
            let name = shell::display_name(e);
            app.gfx.text(&name, Rect::new(r.x + 34.0, y, r.w - 60.0, ROW_H), &app.gfx.fonts.body, fg);
        }
        app.columns.cols[i].entries = Some(entries);
        app.gfx.line(r.right() - 0.5, r.y, r.right() - 0.5, r.bottom(), t.separator);
    }
    paint_info(app);
    app.gfx.pop_clip();
}

fn paint_info(app: &mut App) {
    let Some(e) = selected_file(app).cloned() else { return };
    let x = col_rect(app, app.columns.cols.len()).x;
    let top = app.layout.content.y;
    paint_info_at(app, &e, x, top);
}

/// Info panel (icon, name, kind, size, modified) for one item, INFO_W wide, starting at (x, top).
pub fn paint_info_at(app: &mut App, e: &Entry, x: f32, top: f32) {
    let t = app.theme;
    let icon = Rect::new(x + (INFO_W - 64.0) / 2.0, top + 32.0, 64.0, 64.0);
    if e.is_dir() {
        crate::views::grid::paint_folder(app, icon);
    } else {
        let idx = shell::icon_index(e, &mut app.icon_idx);
        if let Some(b) = app.gfx.icon(idx, IconSize::Jumbo) {
            app.gfx.bitmap(&b, icon);
        }
    }
    let name = shell::display_name(e);
    app.gfx.text(&name, Rect::new(x + 16.0, top + 108.0, INFO_W - 32.0, 24.0), &app.gfx.fonts.big, t.text);
    let kind = shell::type_name(e, &mut app.type_names);
    let modified = format_modified(shell::to_local(e.modified), shell::now_local());
    let size = if e.is_dir() { "--".to_string() } else { format_size(e.size) };
    let rows = [("Kind", kind), ("Size", size), ("Modified", modified)];
    for (k, (label, value)) in rows.iter().enumerate() {
        let y = top + 148.0 + k as f32 * 22.0;
        app.gfx.text(label, Rect::new(x + 16.0, y, 80.0, 22.0), &app.gfx.fonts.small, t.text_dim);
        app.gfx.text(value, Rect::new(x + 96.0, y, INFO_W - 112.0, 22.0), &app.gfx.fonts.body, t.text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_scroll_keeps_newest_column_visible() {
        assert_eq!(fit_scroll(660.0, 800.0, 0.0), 0.0);
        assert_eq!(fit_scroll(1100.0, 800.0, 0.0), 300.0);
        assert_eq!(fit_scroll(440.0, 800.0, 300.0), 0.0, "shrinks back when columns close");
    }
}
