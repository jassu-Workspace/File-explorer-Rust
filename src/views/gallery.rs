use crate::app::App;
use crate::draw::IconSize;
use crate::layout::Rect;
use crate::shell;
use crate::views::columns::{INFO_W, paint_info_at};
use crate::views::grid::paint_folder;
use std::ops::Range;

/// Thumbnail size in the strip.
pub const THUMB: f32 = 64.0;
pub const GAP: f32 = 8.0;
pub const STRIP_PAD: f32 = 12.0;
pub const STRIP_H: f32 = THUMB + 24.0;

/// Strip item rect, x relative to the strip's left edge, y relative to the first thumbnail row.
pub fn strip_item(i: usize, scroll: f32) -> Rect {
    Rect::new(STRIP_PAD + i as f32 * (THUMB + GAP) - scroll, 0.0, THUMB, THUMB)
}

pub fn strip_at(x: f32, scroll: f32, n: usize) -> Option<usize> {
    let rel = x - STRIP_PAD + scroll;
    if rel < 0.0 {
        return None;
    }
    let i = (rel / (THUMB + GAP)) as usize;
    (rel - i as f32 * (THUMB + GAP) < THUMB && i < n).then_some(i)
}

pub fn strip_visible(scroll: f32, width: f32, n: usize) -> Range<usize> {
    let first = ((scroll - STRIP_PAD).max(0.0) / (THUMB + GAP)) as usize;
    let last = ((scroll + width - STRIP_PAD) / (THUMB + GAP)).ceil().max(0.0) as usize;
    first.min(n)..last.min(n)
}

/// Strip scroll that keeps item `i` fully visible.
pub fn strip_scroll_to_show(i: usize, scroll: f32, width: f32) -> f32 {
    let left = STRIP_PAD + i as f32 * (THUMB + GAP);
    if left - STRIP_PAD < scroll {
        left - STRIP_PAD
    } else if left + THUMB + STRIP_PAD > scroll + width {
        left + THUMB + STRIP_PAD - width
    } else {
        scroll
    }
}

/// (preview, strip, info) areas inside the content rect.
pub fn areas(content: Rect) -> (Rect, Rect, Rect) {
    let main_w = (content.w - INFO_W).max(0.0);
    let strip = Rect::new(content.x, content.bottom() - STRIP_H, main_w, STRIP_H);
    let preview = Rect::new(content.x + 24.0, content.y + 24.0, main_w - 48.0, content.h - STRIP_H - 36.0);
    let info = Rect::new(content.x + main_w, content.y, INFO_W, content.h);
    (preview, strip, info)
}

/// Draws a file's thumbnail (or its icon while none is cached) fitted into `r`.
fn paint_item(app: &mut App, i: usize, r: Rect) {
    let e = app.list.entries[i].clone();
    if e.is_dir() {
        let s = r.w.min(r.h);
        paint_folder(app, Rect::new(r.x + (r.w - s) / 2.0, r.y + (r.h - s) / 2.0, s, s));
        return;
    }
    let path = app.entry_path(&e);
    if let Some(t) = app.thumb_or_request(path, r.w.max(r.h)) {
        app.gfx.bitmap_fit(&t, r);
    } else {
        let idx = shell::icon_index(&e, &mut app.icon_idx);
        if let Some(b) = app.gfx.icon(idx, IconSize::Jumbo) {
            // Icons look poor stretched past 256 px.
            let s = r.w.min(r.h).min(256.0);
            app.gfx.bitmap(&b, Rect::new(r.x + (r.w - s) / 2.0, r.y + (r.h - s) / 2.0, s, s));
        }
    }
}

pub fn paint(app: &mut App) {
    let t = app.theme;
    let (preview, strip, info) = areas(app.layout.content);
    let n = app.list.entries.len();
    if let Some(f) = app.list.sel.focus().filter(|&f| f < n) {
        paint_item(app, f, Rect::new(preview.x, preview.y, preview.w, preview.h));
        let e = app.list.entries[f].clone();
        paint_info_at(app, &e, info.x, info.y);
    }
    app.gfx.line(strip.x, strip.y + 0.5, strip.right(), strip.y + 0.5, t.separator);
    app.gfx.line(info.x + 0.5, info.y, info.x + 0.5, info.bottom(), t.separator);
    app.gfx.push_clip(strip);
    for i in strip_visible(app.gallery_scroll, strip.w, n) {
        let r = strip_item(i, app.gallery_scroll);
        let r = Rect::new(strip.x + r.x, strip.y + 12.0, r.w, r.h);
        if app.list.sel.is_selected(i) {
            app.gfx.fill_round(Rect::new(r.x - 4.0, r.y - 4.0, r.w + 8.0, r.h + 8.0), 8.0, t.accent.with_alpha(0.35));
        }
        paint_item(app, i, r);
    }
    app.gfx.pop_clip();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_items_and_hits() {
        assert_eq!(strip_item(0, 0.0), Rect::new(STRIP_PAD, 0.0, THUMB, THUMB));
        assert_eq!(strip_item(2, 0.0).x, STRIP_PAD + 2.0 * (THUMB + GAP));
        assert_eq!(strip_at(STRIP_PAD + 1.0, 0.0, 10), Some(0));
        assert_eq!(strip_at(STRIP_PAD + THUMB + 1.0, 0.0, 10), None, "gap between thumbnails");
        assert_eq!(strip_at(STRIP_PAD + (THUMB + GAP) * 3.0 + 1.0, THUMB + GAP, 10), Some(4), "scroll shifts items");
        assert_eq!(strip_at(STRIP_PAD + (THUMB + GAP) * 20.0, 0.0, 10), None, "past the end");
    }

    #[test]
    fn strip_visible_and_reveal() {
        let w = (THUMB + GAP) * 5.0;
        assert_eq!(strip_visible(0.0, w, 100), 0..5, "item 5 starts past the strip");
        assert_eq!(strip_visible(0.0, w, 3), 0..3);
        let s = strip_scroll_to_show(20, 0.0, w);
        assert!(strip_item(20, s).right() <= w + 0.01 && strip_item(20, s).x >= 0.0);
        assert_eq!(strip_scroll_to_show(0, 500.0, w), 0.0);
    }
}
