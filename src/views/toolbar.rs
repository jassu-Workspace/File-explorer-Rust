use crate::app::{App, Hot};
use crate::draw::Glyph;
use crate::layout::{Layout, Rect};

pub struct ToolbarRects {
    pub back: Rect,
    pub forward: Rect,
    pub title: Rect,
    pub view_list: Rect,
    pub view_cols: Rect,
    pub search: Rect,
}

impl ToolbarRects {
    pub fn interactive(&self) -> [Rect; 6] {
        [self.back, self.forward, self.title, self.view_list, self.view_cols, self.search]
    }
}

pub fn toolbar_rects(l: &Layout) -> ToolbarRects {
    let x0 = l.sidebar.w + 12.0;
    let search = Rect::new((l.min_btn.x - 12.0 - 220.0).max(x0 + 140.0), 11.0, 220.0, 30.0);
    let view_list = Rect::new(search.x - 12.0 - 64.0, 12.0, 32.0, 28.0);
    let view_cols = Rect::new(view_list.right(), 12.0, 32.0, 28.0);
    let back = Rect::new(x0, 12.0, 28.0, 28.0);
    let forward = Rect::new(back.right() + 2.0, 12.0, 28.0, 28.0);
    let title = Rect::new(forward.right() + 10.0, 10.0, view_list.x - 16.0 - forward.right() - 10.0, 32.0);
    ToolbarRects { back, forward, title, view_list, view_cols, search }
}

pub fn paint(app: &App) {
    let (g, t, l) = (&app.gfx, &app.theme, &app.layout);
    let r = toolbar_rects(l);
    // Toolbar right of the sidebar column.
    g.fill_rect(Rect::new(l.sidebar.w, 0.0, l.toolbar.w - l.sidebar.w, l.toolbar.h), t.window_bg);
    g.line(l.sidebar.w, l.toolbar.h - 0.5, l.toolbar.w, l.toolbar.h - 0.5, t.separator);

    let nav_btn = |rect: Rect, glyph: Glyph, enabled: bool, hot: bool| {
        if hot && enabled {
            g.fill_round(rect, 6.0, t.hover);
        }
        let c = if enabled { t.text } else { t.text_dim.with_alpha(0.45) };
        g.glyph(glyph, Rect::new(rect.x + 6.0, rect.y + 6.0, 16.0, 16.0), c);
    };
    nav_btn(r.back, Glyph::Back, app.can_back(), app.hot == Hot::Back);
    nav_btn(r.forward, Glyph::Forward, app.can_forward(), app.hot == Hot::Forward);

    if app.address_visible() {
        g.fill_round(r.title, 8.0, t.field_bg);
    } else {
        g.text(&app.title(), r.title, &g.fonts.title, t.text);
    }

    // View switcher: a segmented control.
    let seg = Rect::new(r.view_list.x, r.view_list.y, r.view_list.w + r.view_cols.w, r.view_list.h);
    g.fill_round(seg, 6.0, t.field_bg);
    let active = if app.columns_view() { r.view_cols } else { r.view_list };
    g.fill_round(Rect::new(active.x + 2.0, active.y + 2.0, active.w - 4.0, active.h - 4.0), 5.0, t.content_bg);
    for (rect, glyph) in [(r.view_list, Glyph::List), (r.view_cols, Glyph::Columns)] {
        g.glyph(glyph, Rect::new(rect.x + 8.0, rect.y + 6.0, 16.0, 16.0), t.text);
    }

    // Search field background; the EDIT child sits on top of it.
    g.fill_round(r.search, 8.0, t.field_bg);
    g.glyph(Glyph::Search, Rect::new(r.search.x + 8.0, r.search.y + 8.0, 14.0, 14.0), t.text_dim);
    if app.search_placeholder_visible() {
        let tr = Rect::new(r.search.x + 28.0, r.search.y, r.search.w - 36.0, r.search.h);
        g.text(&app.search_placeholder(), tr, &g.fonts.body, t.text_dim);
    }

    // Caption buttons.
    let maxed = app.maximized();
    for (rect, glyph, hot) in [
        (l.min_btn, Glyph::Min, Hot::Min),
        (l.max_btn, if maxed { Glyph::Restore } else { Glyph::Max }, Hot::Max),
        (l.close_btn, Glyph::Close, Hot::Close),
    ] {
        let is_hot = app.hot == hot;
        let mut fg = t.text;
        if is_hot && hot == Hot::Close {
            g.fill_rect(rect, t.close_hover);
            fg = t.selection_text;
        } else if is_hot {
            g.fill_rect(rect, t.hover);
        }
        g.glyph(glyph, Rect::new(rect.x + (rect.w - 16.0) / 2.0, rect.y + (rect.h - 16.0) / 2.0, 16.0, 16.0), fg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::compute_layout;

    #[test]
    fn toolbar_controls_do_not_overlap_caption_buttons() {
        let l = compute_layout(1100.0, 700.0, 200.0, 1.0);
        let r = toolbar_rects(&l);
        assert!(r.search.right() <= l.min_btn.x);
        assert!(r.view_cols.right() <= r.search.x);
        assert!(r.title.right() <= r.view_list.x);
        assert!(r.back.x >= l.sidebar.w);
    }
}
