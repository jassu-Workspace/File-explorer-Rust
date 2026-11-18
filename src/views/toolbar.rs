use crate::app::{App, Hot};
use crate::draw::Glyph;
use crate::layout::{Layout, Rect};

/// Caption strip under each toolbar control ("View", "Share", ...), like Finder's "icon and text" toolbar.
pub const CAPTION_H: f32 = 16.0;
/// Below this title width, Group/Share/Add Tags hide (their actions stay in the Action menu).
pub const MIN_TITLE_W: f32 = 120.0;
const Y: f32 = 10.0;
const H: f32 = 28.0;
const BTN_W: f32 = 44.0;
const SEG_W: f32 = 30.0;
const SEARCH_OPEN_W: f32 = 200.0;
const SEARCH_CLOSED_W: f32 = 32.0;
const SEARCH_MIN_W: f32 = 80.0;
/// Narrowest the title may get when the search field is open.
const TITLE_FLOOR_W: f32 = 40.0;
/// Minimum window width (DIPs) at which the toolbar fits; enforced in WM_GETMINMAXINFO.
pub const MIN_WINDOW_W: f32 = 800.0;

pub const VIEW_GLYPHS: [Glyph; 4] = [Glyph::Grid, Glyph::List, Glyph::Columns, Glyph::Gallery];

pub struct ToolbarRects {
    pub back: Rect,
    pub forward: Rect,
    pub title: Rect,
    /// Icon, List, Columns, Gallery.
    pub views: [Rect; 4],
    pub group: Option<Rect>,
    pub share: Option<Rect>,
    pub tags: Option<Rect>,
    pub action: Rect,
    pub search: Rect,
}

impl ToolbarRects {
    pub fn interactive(&self) -> Vec<Rect> {
        let mut v = vec![self.back, self.forward, self.title, self.action, self.search];
        v.extend(self.views);
        v.extend([self.group, self.share, self.tags].into_iter().flatten());
        v
    }

    /// The toolbar control under a point.
    pub fn hit(&self, x: f32, y: f32) -> Hot {
        if let Some(i) = self.views.iter().position(|r| r.contains(x, y)) {
            return Hot::View(i as u8);
        }
        [
            (Some(self.back), Hot::Back),
            (Some(self.forward), Hot::Forward),
            (self.group, Hot::Group),
            (self.share, Hot::Share),
            (self.tags, Hot::Tags),
            (Some(self.action), Hot::Action),
            (Some(self.search), Hot::Search),
        ]
        .into_iter()
        .find(|(r, _)| r.is_some_and(|r| r.contains(x, y)))
        .map_or(Hot::None, |(_, h)| h)
    }
}

/// Lays out right to left from the caption buttons: Search, Action, Add Tags, Share, Group, View.
pub fn toolbar_rects(l: &Layout, search_open: bool) -> ToolbarRects {
    let x0 = l.sidebar.w + 12.0;
    let back = Rect::new(x0, Y, 28.0, H);
    let forward = Rect::new(back.right(), Y, 28.0, H);
    let title_x = forward.right() + 12.0;
    let right = l.min_btn.x - 10.0;
    // The open field shrinks before it squeezes the title: Action + View control + gaps need 196 DIPs.
    let room = right - title_x - TITLE_FLOOR_W - (8.0 + BTN_W + 12.0 + 4.0 * SEG_W + 12.0);
    let sw = if search_open { SEARCH_OPEN_W.min(room).max(SEARCH_MIN_W) } else { SEARCH_CLOSED_W };
    let search = Rect::new((right - sw).max(title_x), Y, sw.min(right - title_x).max(0.0), H);
    let action = Rect::new(search.x - 8.0 - BTN_W, Y, BTN_W, H);
    let build = |with_extras: bool| {
        let mut x = action.x;
        let mut next = || {
            x -= 4.0 + BTN_W;
            Rect::new(x, Y, BTN_W, H)
        };
        let (tags, share, group) = if with_extras { (Some(next()), Some(next()), Some(next())) } else { (None, None, None) };
        let left = group.map_or(action.x, |g| g.x) - 12.0 - 4.0 * SEG_W;
        let views = [0, 1, 2, 3].map(|i| Rect::new(left + i as f32 * SEG_W, Y, SEG_W, H));
        (views, group, share, tags)
    };
    let (mut views, mut group, mut share, mut tags) = build(true);
    if views[0].x - 12.0 - title_x < MIN_TITLE_W {
        (views, group, share, tags) = build(false);
    }
    let title = Rect::new(title_x, Y, (views[0].x - 12.0 - title_x).max(0.0), H);
    ToolbarRects { back, forward, title, views, group, share, tags, action, search }
}

fn caption_rect(r: Rect) -> Rect {
    Rect::new(r.x - 12.0, r.bottom() + 2.0, r.w + 24.0, CAPTION_H)
}

pub fn paint(app: &App) {
    let (g, t, l) = (&app.gfx, &app.theme, &app.layout);
    let r = app.toolbar();
    // Toolbar right of the sidebar column.
    g.fill_rect(Rect::new(l.sidebar.w, 0.0, l.toolbar.w - l.sidebar.w, l.toolbar.h), t.window_bg);
    g.line(l.sidebar.w, l.toolbar.h - 0.5, l.toolbar.w, l.toolbar.h - 0.5, t.separator);
    let caption = |under: Rect, text: &str| g.text(text, caption_rect(under), &g.fonts.caption, t.text_dim);

    let button = |rect: Rect, glyph: Glyph, enabled: bool, hot: Hot| {
        if app.hot == hot && enabled {
            g.fill_round(rect, 6.0, t.hover);
        }
        let c = if enabled { t.text } else { t.text_dim.with_alpha(0.45) };
        g.glyph(glyph, Rect::new(rect.x + (rect.w - 16.0) / 2.0, rect.y + (rect.h - 16.0) / 2.0, 16.0, 16.0), c);
    };
    button(r.back, Glyph::Back, app.can_back(), Hot::Back);
    button(r.forward, Glyph::Forward, app.can_forward(), Hot::Forward);
    caption(Rect::new(r.back.x, r.back.y, r.forward.right() - r.back.x, r.back.h), "Back/Forward");

    if app.address_visible() {
        g.fill_round(r.title, 8.0, t.field_bg);
    } else {
        g.text(&app.title(), r.title, &g.fonts.title, t.text);
    }

    // View switcher: a segmented control.
    let seg = Rect::new(r.views[0].x, r.views[0].y, r.views[3].right() - r.views[0].x, H);
    g.fill_round(seg, 6.0, t.field_bg);
    let active = r.views[app.view_index()];
    g.fill_round(Rect::new(active.x + 2.0, active.y + 2.0, active.w - 4.0, active.h - 4.0), 5.0, t.content_bg);
    for (rect, glyph) in r.views.iter().zip(VIEW_GLYPHS) {
        g.glyph(glyph, Rect::new(rect.x + (rect.w - 16.0) / 2.0, rect.y + 6.0, 16.0, 16.0), t.text);
    }
    caption(seg, "View");

    for (rect, glyph, hot, text) in [(r.group, Glyph::Group, Hot::Group, "Group"), (r.share, Glyph::Share, Hot::Share, "Share"), (r.tags, Glyph::Tag, Hot::Tags, "Add Tags")] {
        if let Some(rect) = rect {
            button(rect, glyph, true, hot);
            caption(rect, text);
        }
    }
    button(r.action, Glyph::More, true, Hot::Action);
    caption(r.action, "Action");

    // Search: an icon that opens into a field; the EDIT child sits on top of the field.
    if app.search_open() {
        g.fill_round(r.search, 8.0, t.field_bg);
        g.glyph(Glyph::Search, Rect::new(r.search.x + 8.0, r.search.y + 7.0, 14.0, 14.0), t.text_dim);
    } else {
        button(r.search, Glyph::Search, true, Hot::Search);
    }
    caption(r.search, "Search");

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

    fn all(r: &ToolbarRects) -> Vec<Rect> {
        let mut v = vec![r.back, r.forward, r.title];
        v.extend(r.views);
        v.extend([r.group, r.share, r.tags].into_iter().flatten());
        v.extend([r.action, r.search]);
        v
    }

    #[test]
    fn toolbar_fits_without_overlap() {
        for (w, side) in [(MIN_WINDOW_W, 200.0), (MIN_WINDOW_W, 320.0), (900.0, 320.0), (1100.0, 200.0), (2560.0, 320.0)] {
            for open in [false, true] {
                let l = compute_layout(w, 700.0, side, 1.0);
                let r = toolbar_rects(&l, open);
                let v = all(&r);
                for (i, a) in v.iter().enumerate() {
                    assert!(a.x >= l.sidebar.w && a.right() <= l.min_btn.x, "w={w} open={open} #{i} {a:?}");
                    assert!(a.bottom() + CAPTION_H <= l.toolbar.h, "caption must fit: {a:?}");
                    for b in &v[i + 1..] {
                        let overlap = a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom();
                        assert!(!overlap, "w={w} open={open}: {a:?} overlaps {b:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn narrow_window_hides_secondary_buttons() {
        let wide = toolbar_rects(&compute_layout(1400.0, 700.0, 200.0, 1.0), false);
        assert!(wide.group.is_some() && wide.share.is_some() && wide.tags.is_some());
        let narrow = toolbar_rects(&compute_layout(MIN_WINDOW_W, 700.0, 200.0, 1.0), true);
        assert!(narrow.group.is_none() && narrow.share.is_none() && narrow.tags.is_none());
        assert!(narrow.title.w > 0.0, "title keeps some room");
    }

    #[test]
    fn search_expands_when_open() {
        let l = compute_layout(1400.0, 700.0, 200.0, 1.0);
        assert!(toolbar_rects(&l, true).search.w > toolbar_rects(&l, false).search.w * 3.0);
    }
}
