use crate::app::App;
use crate::draw::Glyph;
use crate::format::{format_modified, format_size};
use crate::layout::{Rect, row_at, visible_rows};
use crate::listing::{Entry, SortKey, sort_entries};
use crate::selection::{Selection, TypeAhead, find_prefix};
use crate::shell;
use crate::theme::ROW_H;
use std::time::Instant;
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_DOWN, VK_END, VK_HOME, VK_NEXT, VK_PRIOR, VK_RETURN, VK_UP};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Col {
    Name,
    Modified,
    Size,
    Kind,
    Where,
}

impl Col {
    fn title(self) -> &'static str {
        match self {
            Col::Name => "Name",
            Col::Modified => "Date Modified",
            Col::Size => "Size",
            Col::Kind => "Kind",
            Col::Where => "Where",
        }
    }
    fn sort_key(self) -> Option<SortKey> {
        match self {
            Col::Name => Some(SortKey::Name),
            Col::Modified => Some(SortKey::Modified),
            Col::Size => Some(SortKey::Size),
            Col::Kind => Some(SortKey::Kind),
            Col::Where => None,
        }
    }
}

const NAME_MIN: f32 = 200.0;
const PAD: f32 = 10.0;

/// (column, x offset from the list's left edge, width). Name takes the remaining space.
pub fn columns(width: f32, show_where: bool) -> Vec<(Col, f32, f32)> {
    let mut fixed = vec![(Col::Modified, 170.0), (Col::Size, 90.0), (Col::Kind, 140.0)];
    if show_where {
        fixed.insert(0, (Col::Where, 200.0));
    }
    let used: f32 = fixed.iter().map(|c| c.1).sum();
    let mut out = vec![(Col::Name, 0.0, (width - used).max(NAME_MIN))];
    let mut x = out[0].2;
    for (c, w) in fixed {
        out.push((c, x, w));
        x += w;
    }
    out
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ListAction {
    None,
    Open(usize),
    Redraw,
}

#[derive(Debug)]
pub struct ListView {
    pub entries: Vec<Entry>,
    pub sort: (SortKey, bool),
    pub scroll_y: f32,
    pub sel: Selection,
    pub type_ahead: TypeAhead,
    pub show_where: bool,
    pub hover_row: Option<usize>,
}

impl Default for ListView {
    fn default() -> ListView {
        ListView {
            entries: Vec::new(),
            sort: (SortKey::Name, false),
            scroll_y: 0.0,
            sel: Selection::new(),
            type_ahead: TypeAhead::default(),
            show_where: false,
            hover_row: None,
        }
    }
}

impl ListView {
    pub fn set_entries(&mut self, mut v: Vec<Entry>) {
        sort_entries(&mut v, self.sort.0, self.sort.1);
        self.entries = v;
        self.sel.clear();
        self.scroll_y = 0.0;
        self.hover_row = None;
    }

    pub fn resort(&mut self) {
        sort_entries(&mut self.entries, self.sort.0, self.sort.1);
    }

    pub fn click_header(&mut self, col: Col) {
        let Some(key) = col.sort_key() else { return };
        self.sort = if self.sort.0 == key { (key, !self.sort.1) } else { (key, false) };
        self.resort();
        self.sel.clear();
    }

    pub fn click_row(&mut self, row: usize, m: Mods) {
        if m.shift {
            self.sel.shift_click(row);
        } else if m.ctrl {
            self.sel.ctrl_click(row);
        } else {
            self.sel.click(row);
        }
    }

    pub fn max_scroll(&self, viewport_h: f32) -> f32 {
        (self.entries.len() as f32 * ROW_H - viewport_h).max(0.0)
    }

    pub fn on_wheel(&mut self, notches: f32, viewport_h: f32) {
        self.scroll_y = (self.scroll_y - notches * 3.0 * ROW_H).clamp(0.0, self.max_scroll(viewport_h));
    }

    pub fn ensure_visible(&mut self, i: usize, viewport_h: f32) {
        let top = i as f32 * ROW_H;
        if top < self.scroll_y {
            self.scroll_y = top;
        } else if top + ROW_H > self.scroll_y + viewport_h {
            self.scroll_y = (top + ROW_H - viewport_h).max(0.0);
        }
    }

    pub fn on_key(&mut self, vk: u16, m: Mods, viewport_h: f32) -> ListAction {
        let n = self.entries.len();
        let page = ((viewport_h / ROW_H).floor() as isize - 1).max(1);
        let delta = match vk {
            v if v == VK_DOWN.0 => {
                if self.sel.focus().is_none() {
                    0
                } else {
                    1
                }
            }
            v if v == VK_UP.0 => -1,
            v if v == VK_NEXT.0 => page,
            v if v == VK_PRIOR.0 => -page,
            v if v == VK_HOME.0 => -(n as isize),
            v if v == VK_END.0 => n as isize,
            v if v == VK_RETURN.0 => return self.sel.focus().filter(|&f| f < n).map_or(ListAction::None, ListAction::Open),
            0x41 if m.ctrl => {
                self.sel.select_all(n);
                return ListAction::Redraw;
            }
            _ => return ListAction::None,
        };
        if n == 0 {
            return ListAction::None;
        }
        self.sel.move_focus(delta, m.shift, n);
        if let Some(f) = self.sel.focus() {
            self.ensure_visible(f, viewport_h);
        }
        ListAction::Redraw
    }

    pub fn on_char(&mut self, ch: char, now: Instant) -> ListAction {
        let prefix = self.type_ahead.push(ch, now).to_string();
        match find_prefix(self.entries.iter().map(|e| e.name.as_os_str()), &prefix) {
            Some(i) => {
                self.sel.click(i);
                ListAction::Redraw
            }
            None => ListAction::None,
        }
    }

    pub fn row_at(&self, y_in_rows: f32) -> Option<usize> {
        row_at(y_in_rows, self.scroll_y, ROW_H, self.entries.len())
    }

    pub fn col_at(&self, x_in_list: f32, width: f32) -> Option<Col> {
        columns(width, self.show_where).into_iter().find(|c| x_in_list >= c.1 && x_in_list < c.1 + c.2).map(|c| c.0)
    }
}

pub fn paint(app: &mut App) {
    let (header, rows) = (app.layout.header, app.layout.rows);
    let t = app.theme;
    let large = app.scale >= 1.5;
    let today = shell::now_local();
    let cols = columns(rows.w, app.list.show_where);

    // Header.
    app.gfx.fill_rect(header, t.content_bg);
    for &(c, x, w) in &cols {
        let r = Rect::new(header.x + x + PAD, header.y, w - 2.0 * PAD, header.h);
        // Size values are right-aligned, so its title is too; the sort arrow goes on the free side.
        let right = c == Col::Size;
        let font = if right { &app.gfx.fonts.small_right } else { &app.gfx.fonts.small };
        app.gfx.text(c.title(), r, font, t.text_dim);
        if c.sort_key() == Some(app.list.sort.0) {
            let tw = app.gfx.measure(c.title(), &app.gfx.fonts.small);
            let g = if app.list.sort.1 { Glyph::SortDown } else { Glyph::SortUp };
            let ax = if right { r.right() - tw - 14.0 } else { r.x + tw + 4.0 };
            app.gfx.glyph(g, Rect::new(ax, header.y + (header.h - 10.0) / 2.0, 10.0, 10.0), t.text_dim);
        }
    }
    app.gfx.line(header.x, header.bottom() - 0.5, header.right(), header.bottom() - 0.5, t.separator);

    app.gfx.push_clip(rows);
    let range = visible_rows(app.list.scroll_y, rows.h, ROW_H, app.list.entries.len());
    for i in range {
        let y = rows.y + i as f32 * ROW_H - app.list.scroll_y;
        let selected = app.list.sel.is_selected(i);
        let bg = Rect::new(rows.x + 6.0, y + 1.0, rows.w - 12.0, ROW_H - 2.0);
        if selected {
            app.gfx.fill_round(bg, 5.0, t.accent);
        } else if app.list.hover_row == Some(i) {
            app.gfx.fill_round(bg, 5.0, t.hover);
        }
        let (fg, dim) = if selected { (t.selection_text, t.selection_text.with_alpha(0.85)) } else { (t.text, t.text_dim) };
        let e = &app.list.entries[i];
        for &(c, x, w) in &cols {
            let cell = Rect::new(rows.x + x + PAD, y, w - 2.0 * PAD, ROW_H);
            match c {
                Col::Name => {
                    let icon = Rect::new(cell.x, y + (ROW_H - 16.0) / 2.0, 16.0, 16.0);
                    if e.is_dir() {
                        app.gfx.glyph(Glyph::Folder, icon, if selected { fg } else { t.accent });
                    } else {
                        let idx = shell::icon_index(e, &mut app.icon_idx);
                        if let Some(b) = app.gfx.icon(idx, large) {
                            app.gfx.bitmap(&b, icon);
                        }
                    }
                    let name = e.name.to_string_lossy();
                    app.gfx.text(&name, Rect::new(cell.x + 24.0, y, cell.w - 24.0, ROW_H), &app.gfx.fonts.body, fg);
                }
                Col::Modified => {
                    let s = format_modified(shell::to_local(e.modified), today);
                    app.gfx.text(&s, cell, &app.gfx.fonts.body, dim);
                }
                Col::Size => {
                    let s = if e.is_dir() { "--".to_string() } else { format_size(e.size) };
                    app.gfx.text(&s, cell, &app.gfx.fonts.body_right, dim);
                }
                Col::Kind => {
                    let s = shell::type_name(e, &mut app.type_names);
                    app.gfx.text(&s, cell, &app.gfx.fonts.body, dim);
                }
                Col::Where => {
                    let s = e.rel_dir.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
                    let s = if s.is_empty() { ".".to_string() } else { s };
                    app.gfx.text(&s, cell, &app.gfx.fonts.body, dim);
                }
            }
        }
    }
    app.gfx.pop_clip();

    if let Some(msg) = &app.error {
        let banner = Rect::new(rows.x + 16.0, rows.y + 12.0, rows.w - 32.0, 36.0);
        // Opaque, so rows underneath do not show through the message.
        app.gfx.fill_round(banner, 8.0, t.close_hover.with_alpha(0.18).over(t.content_bg));
        app.gfx.text(msg, Rect::new(banner.x + 14.0, banner.y, banner.w - 28.0, banner.h), &app.gfx.fonts.body, t.text);
    } else if app.list.entries.is_empty() && !app.loading {
        let r = Rect::new(rows.x, rows.y + rows.h / 2.0 - 40.0, rows.w, 30.0);
        app.gfx.text(app.empty_text(), r, &app.gfx.fonts.big, t.text_dim);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::listing::Entry;
    use std::time::{Duration, Instant};

    fn e(name: &str, dir: bool, size: u64) -> Entry {
        Entry { name: name.into(), size, modified: 0, attrs: if dir { 0x10 } else { 0x20 }, rel_dir: None }
    }

    fn lv() -> ListView {
        let mut l = ListView::default();
        l.set_entries((0..100).map(|i| e(&format!("f{i:03}"), false, i)).collect());
        l
    }

    const NONE: Mods = Mods { ctrl: false, shift: false };

    #[test]
    fn columns_fill_width() {
        let c = columns(1000.0, false);
        assert_eq!(c.iter().map(|c| c.0).collect::<Vec<_>>(), [Col::Name, Col::Modified, Col::Size, Col::Kind]);
        let last = c.last().unwrap();
        assert!((last.1 + last.2 - 1000.0).abs() < 0.01);
        assert!(columns(300.0, false)[0].2 >= 200.0);
        assert!(columns(1000.0, true).iter().any(|c| c.0 == Col::Where));
    }

    #[test]
    fn header_click_toggles_sort() {
        let mut l = lv();
        l.click_header(Col::Size);
        assert_eq!(l.sort, (SortKey::Size, false));
        l.click_header(Col::Size);
        assert_eq!(l.sort, (SortKey::Size, true));
        assert_eq!(l.entries[0].size, 99);
        l.click_header(Col::Name);
        assert_eq!(l.sort, (SortKey::Name, false));
        l.click_header(Col::Where);
        assert_eq!(l.sort, (SortKey::Name, false));
    }

    #[test]
    fn keys_move_and_open() {
        let mut l = lv();
        assert!(matches!(l.on_key(VK_DOWN.0, NONE, 240.0), ListAction::Redraw));
        assert_eq!(l.sel.focus(), Some(0));
        l.on_key(VK_DOWN.0, Mods { ctrl: false, shift: true }, 240.0);
        assert_eq!(l.sel.selected(), [0, 1]);
        l.on_key(VK_END.0, NONE, 240.0);
        assert_eq!(l.sel.selected(), [99]);
        assert!(l.scroll_y > 0.0);
        assert!(matches!(l.on_key(VK_RETURN.0, NONE, 240.0), ListAction::Open(99)));
        l.on_key(0x41, Mods { ctrl: true, shift: false }, 240.0);
        assert_eq!(l.sel.selected().len(), 100);
    }

    #[test]
    fn wheel_clamps() {
        let mut l = lv();
        l.on_wheel(-1000.0, 240.0);
        assert_eq!(l.scroll_y, l.max_scroll(240.0));
        l.on_wheel(1000.0, 240.0);
        assert_eq!(l.scroll_y, 0.0);
        let mut empty = ListView::default();
        empty.on_wheel(-3.0, 240.0);
        assert_eq!(empty.scroll_y, 0.0);
    }

    #[test]
    fn type_to_select_jumps() {
        let mut l = lv();
        let t = Instant::now();
        l.on_char('f', t);
        l.on_char('0', t + Duration::from_millis(100));
        l.on_char('5', t + Duration::from_millis(200));
        assert_eq!(l.sel.focus(), Some(50));
    }

    #[test]
    fn set_entries_sorts_and_resets() {
        let mut l = lv();
        l.click_row(5, NONE);
        l.scroll_y = 100.0;
        l.set_entries(vec![e("b", false, 0), e("a", true, 0)]);
        assert_eq!(l.entries[0].name, "a");
        assert!(l.sel.selected().is_empty());
        assert_eq!(l.scroll_y, 0.0);
    }
}
