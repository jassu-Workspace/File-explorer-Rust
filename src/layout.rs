use std::ops::Range;

pub const HTCLIENT: u32 = 1;
pub const HTCAPTION: u32 = 2;
pub const HTMINBUTTON: u32 = 8;
pub const HTMAXBUTTON: u32 = 9;
pub const HTLEFT: u32 = 10;
pub const HTRIGHT: u32 = 11;
pub const HTTOP: u32 = 12;
pub const HTTOPLEFT: u32 = 13;
pub const HTTOPRIGHT: u32 = 14;
pub const HTBOTTOM: u32 = 15;
pub const HTBOTTOMLEFT: u32 = 16;
pub const HTBOTTOMRIGHT: u32 = 17;
pub const HTCLOSE: u32 = 20;

pub const TOOLBAR_H: f32 = 52.0;
pub const PATHBAR_H: f32 = 28.0;
pub const HEADER_H: f32 = 28.0;
pub const CAPTION_BTN_W: f32 = 46.0;
pub const CAPTION_BTN_H: f32 = 32.0;
pub const SIDEBAR_MIN: f32 = 160.0;
pub const SIDEBAR_MAX: f32 = 320.0;

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w: w.max(0.0), h: h.max(0.0) }
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Layout {
    pub toolbar: Rect,
    pub sidebar: Rect,
    pub splitter: Rect,
    pub content: Rect,
    pub header: Rect,
    pub rows: Rect,
    pub pathbar: Rect,
    pub min_btn: Rect,
    pub max_btn: Rect,
    pub close_btn: Rect,
}

pub fn compute_layout(w: f32, h: f32, sidebar_w: f32, scale: f32) -> Layout {
    let (tb, pb, hd) = (TOOLBAR_H * scale, PATHBAR_H * scale, HEADER_H * scale);
    let (bw, bh) = (CAPTION_BTN_W * scale, CAPTION_BTN_H * scale);
    let sw = sidebar_w.clamp(SIDEBAR_MIN * scale, SIDEBAR_MAX * scale).min(w);
    let mid_h = (h - tb - pb).max(0.0);
    let content = Rect::new(sw, tb, w - sw, mid_h);
    Layout {
        toolbar: Rect::new(0.0, 0.0, w, tb),
        sidebar: Rect::new(0.0, tb, sw, mid_h),
        splitter: Rect::new(sw - 2.0 * scale, tb, 4.0 * scale, mid_h),
        content,
        header: Rect::new(content.x, content.y, content.w, hd.min(mid_h)),
        rows: Rect::new(content.x, content.y + hd, content.w, mid_h - hd),
        pathbar: Rect::new(0.0, (h - pb).max(0.0), w, pb),
        min_btn: Rect::new(w - 3.0 * bw, 0.0, bw, bh),
        max_btn: Rect::new(w - 2.0 * bw, 0.0, bw, bh),
        close_btn: Rect::new(w - bw, 0.0, bw, bh),
    }
}

pub fn visible_rows(scroll_y: f32, viewport_h: f32, row_h: f32, count: usize) -> Range<usize> {
    if count == 0 || row_h <= 0.0 {
        return 0..0;
    }
    let s = scroll_y.max(0.0);
    let first = ((s / row_h).floor() as usize).min(count);
    let end = (((s + viewport_h.max(0.0)) / row_h).ceil() as usize).min(count);
    first..end.max(first)
}

pub fn row_at(y_in_rows: f32, scroll_y: f32, row_h: f32, count: usize) -> Option<usize> {
    if y_in_rows < 0.0 || row_h <= 0.0 {
        return None;
    }
    let i = ((y_in_rows + scroll_y) / row_h).floor() as usize;
    (i < count).then_some(i)
}

#[allow(clippy::too_many_arguments)]
pub fn nc_hit(l: &Layout, interactive: &[Rect], x: f32, y: f32, w: f32, h: f32, border: f32, maximized: bool) -> u32 {
    if !maximized {
        let (left, right, top, bottom) = (x < border, x >= w - border, y < border, y >= h - border);
        let edge = match (left, right, top, bottom) {
            (true, _, true, _) => HTTOPLEFT,
            (_, true, true, _) => HTTOPRIGHT,
            (true, _, _, true) => HTBOTTOMLEFT,
            (_, true, _, true) => HTBOTTOMRIGHT,
            (true, _, _, _) => HTLEFT,
            (_, true, _, _) => HTRIGHT,
            (_, _, true, _) => HTTOP,
            (_, _, _, true) => HTBOTTOM,
            _ => 0,
        };
        if edge != 0 {
            return edge;
        }
    }
    if l.close_btn.contains(x, y) {
        HTCLOSE
    } else if l.max_btn.contains(x, y) {
        HTMAXBUTTON
    } else if l.min_btn.contains(x, y) {
        HTMINBUTTON
    } else if interactive.iter().any(|r| r.contains(x, y)) {
        HTCLIENT
    } else if l.toolbar.contains(x, y) {
        HTCAPTION
    } else {
        HTCLIENT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_regions_tile() {
        let l = compute_layout(1200.0, 800.0, 200.0, 1.0);
        assert_eq!(l.toolbar, Rect { x: 0.0, y: 0.0, w: 1200.0, h: 52.0 });
        assert_eq!(l.pathbar.y, 772.0);
        assert_eq!(l.sidebar, Rect { x: 0.0, y: 52.0, w: 200.0, h: 720.0 });
        assert_eq!(l.content.x, 200.0);
        assert_eq!(l.content.w, 1000.0);
        assert_eq!(l.header.h, 28.0);
        assert_eq!(l.rows.y, 80.0);
        assert_eq!(l.close_btn.x, 1154.0);
        assert_eq!(l.max_btn.x, 1108.0);
        assert_eq!(l.min_btn.x, 1062.0);
    }

    #[test]
    fn sidebar_clamped() {
        assert_eq!(compute_layout(1200.0, 800.0, 50.0, 1.0).sidebar.w, 160.0);
        assert_eq!(compute_layout(1200.0, 800.0, 900.0, 1.0).sidebar.w, 320.0);
        assert_eq!(compute_layout(2400.0, 1600.0, 900.0, 2.0).sidebar.w, 640.0);
        assert_eq!(compute_layout(2400.0, 1600.0, 100.0, 2.0).sidebar.w, 320.0);
    }

    #[test]
    fn tiny_window_does_not_go_negative() {
        let l = compute_layout(100.0, 50.0, 200.0, 1.0);
        assert!(l.content.w >= 0.0 && l.rows.h >= 0.0);
    }

    #[test]
    fn visible_rows_bounds() {
        assert_eq!(visible_rows(0.0, 100.0, 24.0, 100_000), 0..5);
        assert_eq!(visible_rows(2_399_976.0, 100.0, 24.0, 100_000), 99_999..100_000);
        assert_eq!(visible_rows(0.0, 100.0, 24.0, 0), 0..0);
    }

    #[test]
    fn row_at_hits_and_misses() {
        assert_eq!(row_at(30.0, 0.0, 24.0, 10), Some(1));
        assert_eq!(row_at(10.0, 48.0, 24.0, 10), Some(2));
        assert_eq!(row_at(240.0, 0.0, 24.0, 10), None);
        assert_eq!(row_at(-1.0, 0.0, 24.0, 10), None);
    }

    #[test]
    fn nc_hit_regions() {
        let l = compute_layout(1200.0, 800.0, 200.0, 1.0);
        let search = Rect { x: 900.0, y: 10.0, w: 140.0, h: 30.0 };
        let hit = |x, y, max| nc_hit(&l, &[search], x, y, 1200.0, 800.0, 6.0, max);
        assert_eq!(hit(2.0, 2.0, false), HTTOPLEFT);
        assert_eq!(hit(1198.0, 798.0, false), HTBOTTOMRIGHT);
        assert_eq!(hit(600.0, 798.0, false), HTBOTTOM);
        assert_ne!(hit(2.0, 2.0, true), HTTOPLEFT);
        assert_eq!(hit(1180.0, 10.0, false), HTCLOSE);
        assert_eq!(hit(1120.0, 10.0, false), HTMAXBUTTON);
        assert_eq!(hit(1070.0, 10.0, false), HTMINBUTTON);
        assert_eq!(hit(500.0, 20.0, false), HTCAPTION);
        assert_eq!(hit(950.0, 20.0, false), HTCLIENT);
        assert_eq!(hit(500.0, 400.0, false), HTCLIENT);
    }
}
