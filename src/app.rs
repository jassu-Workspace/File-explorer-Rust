use crate::draw::Gfx;
use crate::edit::{self, ID_ADDRESS, WM_EDIT_KEY};
use crate::layout::{self, Layout, compute_layout, nc_hit};
use crate::listing::{Entry, read_folder};
use crate::nav::{Columns, Generation, History, NavKind, Outcome, apply_listing};
use crate::shell;
use crate::theme::Theme;
use crate::views::list::{ListAction, ListView, Mods};
use crate::views::sidebar::Sidebar;
use crate::views::{self, pathbar, toolbar::toolbar_rects};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMSBT_MAINWINDOW, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
};
use windows::Win32::Foundation::COLORREF;
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, HBRUSH, HDC, HFONT, InvalidateRect, PAINTSTRUCT, ScreenToClient, SetBkColor, SetTextColor,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{MARGINS, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent, VK_BACK, VK_CONTROL, VK_ESCAPE,
    VK_DOWN, VK_F5, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, PCWSTR, Result, w};

const WINDOW_W: f32 = 1100.0;
const WINDOW_H: f32 = 700.0;
const RESIZE_BORDER: f32 = 6.0;

pub const WM_LISTING_DONE: u32 = WM_APP + 1;
pub const WM_SEARCH_BATCH: u32 = WM_APP + 2;
pub const WM_SEARCH_DONE: u32 = WM_APP + 3;
pub const WM_COLUMN_DONE: u32 = WM_APP + 4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    List,
    Columns,
}

/// The element under the mouse, for hover drawing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hot {
    None,
    Min,
    Max,
    Close,
    Back,
    Forward,
    ViewList,
    ViewCols,
    Splitter,
}

/// Result of a background folder read, posted to the UI thread.
struct ListingMsg {
    generation: u64,
    path: PathBuf,
    kind: NavKind,
    result: io::Result<Vec<Entry>>,
    free: Option<u64>,
}

/// `HWND` is a raw pointer and not `Send`; worker threads carry it as an integer.
#[derive(Clone, Copy)]
pub struct SendHwnd(isize);

impl SendHwnd {
    pub fn post<T>(self, msg: u32, payload: Box<T>) {
        let raw = Box::into_raw(payload);
        let hwnd = HWND(self.0 as *mut _);
        if unsafe { PostMessageW(Some(hwnd), msg, WPARAM(0), LPARAM(raw as isize)) }.is_err() {
            // Window is gone: reclaim the payload instead of leaking it.
            drop(unsafe { Box::from_raw(raw) });
        }
    }
}

pub struct App {
    pub hwnd: HWND,
    pub gfx: Gfx,
    pub theme: Theme,
    pub mica: bool,
    /// Device pixels per DIP.
    pub scale: f32,
    /// Client size in DIPs.
    pub w: f32,
    pub h: f32,
    pub sidebar_w: f32,
    pub layout: Layout,
    pub hot: Hot,
    pub history: History,
    pub list: ListView,
    pub view: View,
    pub columns: Columns,
    pub error: Option<String>,
    pub loading: bool,
    pub show_hidden: bool,
    pub type_names: HashMap<String, String>,
    pub icon_idx: HashMap<String, i32>,
    pub sidebar: Sidebar,
    /// Free bytes on the current folder's drive.
    pub free: Option<u64>,
    pub address: HWND,
    edit_font: HFONT,
    edit_brush: HBRUSH,
    listing_gen: Generation,
    pressed_nc: u32,
    dragging_splitter: bool,
    tracking: bool,
}

fn lo(v: isize) -> i32 {
    (v & 0xFFFF) as i16 as i32
}

fn hi(v: isize) -> i32 {
    ((v >> 16) & 0xFFFF) as i16 as i32
}

fn key_down(vk: u16) -> bool {
    unsafe { GetKeyState(vk as i32) < 0 }
}

fn mods() -> Mods {
    Mods { ctrl: key_down(VK_CONTROL.0), shift: key_down(VK_SHIFT.0) }
}

/// Display name for a folder: drive label for roots, else the last path component.
pub fn folder_title(p: &Path) -> String {
    match p.file_name() {
        Some(n) => n.to_string_lossy().into_owned(),
        None => shell::drives().into_iter().find(|d| d.path == p).map_or_else(|| p.display().to_string(), |d| d.name),
    }
}

impl App {
    pub fn can_back(&self) -> bool {
        self.history.can_back()
    }
    pub fn can_forward(&self) -> bool {
        self.history.can_forward()
    }
    pub fn title(&self) -> String {
        folder_title(self.current_dir())
    }
    pub fn columns_view(&self) -> bool {
        self.view == View::Columns
    }

    /// Items shown in the path bar count: the deepest loaded column in column view.
    pub fn item_count(&self) -> usize {
        if self.columns_view() {
            self.columns.cols.iter().rev().find_map(|c| c.entries.as_ref()).map_or(0, Vec::len)
        } else {
            self.list.entries.len()
        }
    }

    pub fn set_view(&mut self, v: View) {
        if self.view == v {
            return;
        }
        self.view = v;
        if v == View::Columns {
            let root = self.history.current().to_path_buf();
            self.columns.open_root(root.clone());
            self.columns.loaded(&root, self.list.entries.clone());
        } else {
            self.columns = Columns::default();
        }
        self.invalidate();
    }

    fn load_column(&self, path: PathBuf) {
        let (hwnd, show_hidden) = (self.send_hwnd(), self.show_hidden);
        std::thread::spawn(move || {
            let result = read_folder(&path, show_hidden);
            hwnd.post(WM_COLUMN_DONE, Box::new((path, result)));
        });
    }

    fn on_column(&mut self, path: PathBuf, result: io::Result<Vec<Entry>>) {
        match result {
            Ok(mut v) => {
                crate::listing::sort_entries(&mut v, self.list.sort.0, self.list.sort.1);
                self.columns.loaded(&path, v);
            }
            Err(e) => {
                self.columns.loaded(&path, Vec::new());
                self.error = Some(crate::nav::error_message(&path, &e));
            }
        }
        self.invalidate();
    }

    fn column_select(&mut self, col: usize, row: usize) {
        self.error = None;
        if let Some(p) = self.columns.select(col, row) {
            self.load_column(p);
        }
        let viewport = self.layout.content.w;
        self.columns.scroll_x = views::columns::fit_scroll(views::columns::total_width(self), viewport, self.columns.scroll_x);
        self.columns.drop_offscreen(viewport, crate::theme::COLUMN_W);
        let (rows_h, c) = (self.layout.content.h, &mut self.columns.cols[col]);
        let top = row as f32 * crate::theme::ROW_H;
        if top < c.scroll_y {
            c.scroll_y = top;
        } else if top + crate::theme::ROW_H > c.scroll_y + rows_h {
            c.scroll_y = top + crate::theme::ROW_H - rows_h;
        }
        self.invalidate();
    }

    /// Wheel scrolls the column under the cursor; Shift+wheel scrolls across columns.
    fn column_wheel(&mut self, notches: f32, lp: LPARAM) {
        let step = notches * 3.0 * crate::theme::ROW_H;
        let content = self.layout.content;
        if mods().shift {
            let max = (views::columns::total_width(self) - content.w).max(0.0);
            self.columns.scroll_x = (self.columns.scroll_x - step).clamp(0.0, max);
            self.columns.drop_offscreen(content.w, crate::theme::COLUMN_W);
            // Columns scrolled back into view reload their freed entries.
            let first = (self.columns.scroll_x / crate::theme::COLUMN_W) as usize;
            let reload: Vec<PathBuf> =
                self.columns.cols.iter().skip(first).filter(|c| c.entries.is_none()).map(|c| c.path.clone()).collect();
            for p in reload {
                self.load_column(p);
            }
            return;
        }
        let mut pt = POINT { x: lo(lp.0), y: hi(lp.0) };
        unsafe {
            let _ = ScreenToClient(self.hwnd, &mut pt);
        }
        let (x, y) = (pt.x as f32 / self.scale, pt.y as f32 / self.scale);
        if let Some((col, _)) = views::columns::hit(self, x, y) {
            let c = &mut self.columns.cols[col];
            let max = (c.entries.as_ref().map_or(0, Vec::len) as f32 * crate::theme::ROW_H - content.h).max(0.0);
            c.scroll_y = (c.scroll_y - step).clamp(0.0, max);
        }
    }

    /// Arrow keys and Enter in column view.
    fn column_key(&mut self, vk: u16) -> bool {
        let fc = self.columns.focus_col;
        let Some(c) = self.columns.cols.get(fc) else { return false };
        let n = c.entries.as_ref().map_or(0, Vec::len);
        let cur = c.selected;
        match vk {
            v if v == VK_DOWN.0 && n > 0 => self.column_select(fc, cur.map_or(0, |s| (s + 1).min(n - 1))),
            v if v == VK_UP.0 && n > 0 => self.column_select(fc, cur.map_or(0, |s| s.saturating_sub(1))),
            v if v == VK_LEFT.0 && fc > 0 => {
                self.columns.focus_col -= 1;
                self.columns.cols.truncate(fc + 1);
                self.invalidate();
            }
            v if v == VK_RIGHT.0 => {
                if self.columns.cols.get(fc + 1).and_then(|c| c.entries.as_ref()).is_some_and(|e| !e.is_empty()) {
                    self.column_select(fc + 1, 0);
                }
            }
            v if v == VK_RETURN.0 => {
                if let Some(e) = cur.and_then(|s| c.entries.as_ref()?.get(s)) {
                    let path = c.path.join(&e.name);
                    if !e.is_dir() {
                        self.open_path(&path);
                    }
                }
            }
            _ => return false,
        }
        true
    }
    pub fn search_placeholder_visible(&self) -> bool {
        true
    }
    pub fn search_placeholder(&self) -> String {
        format!("Search in {}", self.title())
    }
    pub fn empty_text(&self) -> &'static str {
        "This folder is empty"
    }
    /// Path-bar status that replaces the item count (search progress).
    pub fn status_override(&self) -> Option<String> {
        None
    }
    pub fn address_visible(&self) -> bool {
        edit::visible(self.address)
    }

    fn show_address(&mut self) {
        let r = toolbar_rects(&self.layout).title;
        edit::set_text(self.address, &self.current_dir().display().to_string());
        edit::show_in(self.address, crate::layout::Rect::new(r.x + 8.0, r.y, r.w - 16.0, r.h), self.scale);
        edit::focus_select_all(self.address);
        self.invalidate();
    }

    fn hide_address(&mut self) {
        if self.address_visible() {
            edit::hide(self.address);
            unsafe {
                let _ = SetFocus(Some(self.hwnd));
            }
            self.invalidate();
        }
    }

    fn reposition_edits(&self) {
        if self.address_visible() {
            let r = toolbar_rects(&self.layout).title;
            edit::show_in(self.address, crate::layout::Rect::new(r.x + 8.0, r.y, r.w - 16.0, r.h), self.scale);
        }
    }

    /// GDI font and brush for the EDIT children; rebuilt on DPI or theme change.
    fn rebuild_edit_style(&mut self) {
        edit::delete_font(self.edit_font);
        self.edit_font = edit::make_font(self.scale);
        edit::set_font(self.address, self.edit_font);
        if !self.edit_brush.is_invalid() {
            unsafe {
                let _ = DeleteObject(self.edit_brush.into());
            }
        }
        let bg = self.theme.field_bg.over(self.theme.window_bg);
        self.edit_brush = unsafe { CreateSolidBrush(COLORREF(bg.to_colorref())) };
    }

    fn on_edit_key(&mut self, id: usize, vk: u16) {
        if id == ID_ADDRESS {
            if vk == VK_RETURN.0 {
                let text = edit::text(self.address);
                let t = text.trim().trim_matches('"');
                if !t.is_empty() {
                    self.navigate(PathBuf::from(t), NavKind::Visit);
                }
                self.hide_address();
            } else if vk == VK_ESCAPE.0 {
                self.hide_address();
            }
        }
    }
    /// The folder the user is "in": the deepest column in column view, else the listed folder.
    pub fn current_dir(&self) -> &Path {
        if self.columns_view() && !self.columns.cols.is_empty() { self.columns.current_dir() } else { self.history.current() }
    }
    pub fn maximized(&self) -> bool {
        unsafe { IsZoomed(self.hwnd).as_bool() }
    }

    pub fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn send_hwnd(&self) -> SendHwnd {
        SendHwnd(self.hwnd.0 as isize)
    }

    /// Reads `path` on a worker thread. History changes only when the read succeeds.
    pub fn navigate(&mut self, path: PathBuf, kind: NavKind) {
        let generation = self.listing_gen.next();
        let (hwnd, show_hidden) = (self.send_hwnd(), self.show_hidden);
        self.loading = true;
        std::thread::spawn(move || {
            let result = read_folder(&path, show_hidden);
            // Off the UI thread: GetDiskFreeSpaceEx can stall on network shares.
            let free = shell::free_space(&path);
            hwnd.post(WM_LISTING_DONE, Box::new(ListingMsg { generation, path, kind, result, free }));
        });
    }

    fn on_listing(&mut self, m: ListingMsg) {
        let ok = self.listing_gen.is_current(m.generation);
        match apply_listing(&mut self.history, ok, m.result.as_ref().err(), m.path, m.kind) {
            Outcome::Stale => return,
            Outcome::Failed(msg) => self.error = Some(msg),
            Outcome::Loaded => {
                self.error = None;
                self.free = m.free;
                self.list.set_entries(m.result.unwrap_or_default());
                if self.columns_view() {
                    let root = self.history.current().to_path_buf();
                    self.columns.open_root(root.clone());
                    self.columns.loaded(&root, self.list.entries.clone());
                }
            }
        }
        self.loading = false;
        self.invalidate();
    }

    fn go_back(&mut self) {
        if let Some(p) = self.history.back_target() {
            self.navigate(p.to_path_buf(), NavKind::Back);
        }
    }

    fn go_forward(&mut self) {
        if let Some(p) = self.history.forward_target() {
            self.navigate(p.to_path_buf(), NavKind::Forward);
        }
    }

    fn go_up(&mut self) {
        if let Some(p) = self.history.parent() {
            self.navigate(p, NavKind::Visit);
        }
    }

    fn refresh(&mut self) {
        self.navigate(self.history.current().to_path_buf(), NavKind::Refresh);
    }

    /// Opens a file with its default app. Folders are handled by navigation.
    fn open_path(&mut self, _path: &Path) {}

    /// Full path of a list entry (search results carry their folder relative to the listed one).
    pub fn entry_path(&self, e: &Entry) -> PathBuf {
        let mut p = self.history.current().to_path_buf();
        if let Some(rel) = &e.rel_dir {
            p.push(rel);
        }
        p.push(&e.name);
        p
    }

    fn open_index(&mut self, i: usize) {
        let Some(e) = self.list.entries.get(i) else { return };
        let path = self.entry_path(e);
        if e.is_dir() {
            self.navigate(path, NavKind::Visit);
        } else {
            self.open_path(&path);
        }
    }

    fn handle_list_action(&mut self, a: ListAction) {
        match a {
            ListAction::Open(i) => self.open_index(i),
            ListAction::Redraw => self.invalidate(),
            ListAction::None => {}
        }
    }

    fn relayout(&mut self) {
        self.layout = compute_layout(self.w, self.h, self.sidebar_w, 1.0);
        let max = self.list.max_scroll(self.layout.rows.h);
        self.list.scroll_y = self.list.scroll_y.min(max);
    }

    fn apply_theme_to_frame(&self) {
        let dark = BOOL::from(self.theme.dark);
        unsafe {
            let _ = DwmSetWindowAttribute(self.hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as _, 4);
        }
    }

    fn hot_at(&self, x: f32, y: f32) -> Hot {
        let r = toolbar_rects(&self.layout);
        if self.layout.splitter.contains(x, y) {
            Hot::Splitter
        } else if r.back.contains(x, y) {
            Hot::Back
        } else if r.forward.contains(x, y) {
            Hot::Forward
        } else if r.view_list.contains(x, y) {
            Hot::ViewList
        } else if r.view_cols.contains(x, y) {
            Hot::ViewCols
        } else {
            Hot::None
        }
    }

    fn set_hot(&mut self, h: Hot) {
        if self.hot != h {
            self.hot = h;
            self.invalidate();
        }
    }

    fn track_leave(&mut self, nonclient: bool) {
        if self.tracking {
            return;
        }
        let mut t = TRACKMOUSEEVENT {
            cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: if nonclient { TME_LEAVE | TME_NONCLIENT } else { TME_LEAVE },
            hwndTrack: self.hwnd,
            dwHoverTime: 0,
        };
        self.tracking = unsafe { TrackMouseEvent(&mut t).is_ok() };
    }

    fn client_dip(&self, lp: LPARAM) -> (f32, f32) {
        (lo(lp.0) as f32 / self.scale, hi(lp.0) as f32 / self.scale)
    }

    fn paint(&mut self) {
        let mut ps = PAINTSTRUCT::default();
        unsafe { BeginPaint(self.hwnd, &mut ps) };
        let (pw, ph) = ((self.w * self.scale).round() as u32, (self.h * self.scale).round() as u32);
        if self.gfx.ensure_target(self.hwnd, pw, ph).is_ok() {
            self.gfx.set_dpi(96.0 * self.scale);
            self.gfx.begin(crate::theme::Color::TRANSPARENT);
            views::paint(self);
            if self.gfx.end().is_ok() && !self.gfx.has_target() {
                self.invalidate(); // device lost: repaint with a fresh target
            }
        }
        unsafe {
            let _ = EndPaint(self.hwnd, &ps);
        }
    }

    fn on_left_down(&mut self, x: f32, y: f32) {
        let l = self.layout;
        let r = toolbar_rects(&l);
        if l.splitter.contains(x, y) {
            self.dragging_splitter = true;
            unsafe { SetCapture(self.hwnd) };
        } else if r.back.contains(x, y) {
            self.go_back();
        } else if r.forward.contains(x, y) {
            self.go_forward();
        } else if let Some(i) = self.sidebar.hit(x, y, l.sidebar) {
            if let Some(p) = self.sidebar.place(i) {
                self.navigate(p.path.clone(), NavKind::Visit);
            }
        } else if let Some(p) = pathbar::hit(self, x, y) {
            self.navigate(p, NavKind::Visit);
        } else if r.view_list.contains(x, y) {
            self.set_view(View::List);
        } else if r.view_cols.contains(x, y) {
            self.set_view(View::Columns);
        } else if self.columns_view() {
            if let Some((col, row)) = views::columns::hit(self, x, y) {
                match row {
                    Some(row) => self.column_select(col, row),
                    None => {
                        self.columns.focus_col = col;
                        self.invalidate();
                    }
                }
            }
        } else if l.header.contains(x, y) {
            if let Some(c) = self.list.col_at(x - l.header.x, l.header.w) {
                self.list.click_header(c);
                self.invalidate();
            }
        } else if l.rows.contains(x, y) {
            match self.list.row_at(y - l.rows.y) {
                Some(i) => self.list.click_row(i, mods()),
                None => self.list.sel.clear(),
            }
            self.invalidate();
        }
    }

    fn on_key(&mut self, vk: u16) -> bool {
        match vk {
            v if v == VK_F5.0 => self.refresh(),
            v if v == VK_BACK.0 => self.go_back(),
            0x4C if mods().ctrl => self.show_address(), // Ctrl+L
            0x31 if mods().ctrl => self.set_view(View::List), // Ctrl+1
            0x32 if mods().ctrl => self.set_view(View::Columns), // Ctrl+2
            _ if self.columns_view() => return self.column_key(vk),
            _ => {
                let a = self.list.on_key(vk, mods(), self.layout.rows.h);
                if a == ListAction::None {
                    return false;
                }
                self.handle_list_action(a);
            }
        }
        true
    }

    fn on_sys_key(&mut self, vk: u16) -> bool {
        match vk {
            v if v == VK_LEFT.0 => self.go_back(),
            v if v == VK_RIGHT.0 => self.go_forward(),
            v if v == VK_UP.0 => self.go_up(),
            _ => return false,
        }
        true
    }

    fn handle(&mut self, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        let hwnd = self.hwnd;
        match msg {
            WM_PAINT => {
                self.paint();
                Some(LRESULT(0))
            }
            WM_ERASEBKGND => Some(LRESULT(1)),
            WM_LISTING_DONE => {
                let m = unsafe { Box::from_raw(lp.0 as *mut ListingMsg) };
                self.on_listing(*m);
                Some(LRESULT(0))
            }
            WM_COLUMN_DONE => {
                let m = unsafe { Box::from_raw(lp.0 as *mut (PathBuf, io::Result<Vec<Entry>>)) };
                let (path, result) = *m;
                self.on_column(path, result);
                Some(LRESULT(0))
            }
            WM_EDIT_KEY => {
                self.on_edit_key(wp.0, lp.0 as u16);
                Some(LRESULT(0))
            }
            WM_COMMAND => {
                let (id, code) = (wp.0 & 0xFFFF, ((wp.0 >> 16) & 0xFFFF) as u32);
                if id == ID_ADDRESS && code == EN_KILLFOCUS {
                    self.hide_address();
                }
                Some(LRESULT(0))
            }
            WM_CTLCOLOREDIT => {
                let hdc = HDC(wp.0 as *mut _);
                let bg = self.theme.field_bg.over(self.theme.window_bg);
                unsafe {
                    SetTextColor(hdc, COLORREF(self.theme.text.to_colorref()));
                    SetBkColor(hdc, COLORREF(bg.to_colorref()));
                }
                Some(LRESULT(self.edit_brush.0 as isize))
            }
            WM_DEVICECHANGE => {
                // DBT_DEVICEARRIVAL / DBT_DEVICEREMOVECOMPLETE: a drive may have come or gone.
                if matches!(wp.0, 0x8000 | 0x8004) {
                    self.sidebar.drives = shell::drives();
                    self.invalidate();
                }
                None
            }
            WM_SIZE => {
                let (pw, ph) = (lo(lp.0) as u32, hi(lp.0) as u32);
                self.w = pw as f32 / self.scale;
                self.h = ph as f32 / self.scale;
                self.gfx.resize(pw, ph);
                self.relayout();
                self.reposition_edits();
                self.invalidate();
                Some(LRESULT(0))
            }
            WM_NCACTIVATE => Some(unsafe { DefWindowProcW(hwnd, msg, wp, LPARAM(-1)) }),
            WM_NCHITTEST => {
                let mut pt = POINT { x: lo(lp.0), y: hi(lp.0) };
                unsafe {
                    let _ = ScreenToClient(hwnd, &mut pt);
                }
                let (x, y) = (pt.x as f32 / self.scale, pt.y as f32 / self.scale);
                let inter = toolbar_rects(&self.layout).interactive();
                let hit = nc_hit(&self.layout, &inter, x, y, self.w, self.h, RESIZE_BORDER, self.maximized());
                Some(LRESULT(hit as isize))
            }
            WM_NCMOUSEMOVE => {
                let hot = match wp.0 as u32 {
                    layout::HTMINBUTTON => Hot::Min,
                    layout::HTMAXBUTTON => Hot::Max,
                    layout::HTCLOSE => Hot::Close,
                    _ => Hot::None,
                };
                self.track_leave(true);
                self.set_hot(hot);
                None
            }
            WM_NCMOUSELEAVE | WM_MOUSELEAVE => {
                self.tracking = false;
                self.set_hot(Hot::None);
                if self.list.hover_row.take().is_some() {
                    self.invalidate();
                }
                None
            }
            WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK if matches!(wp.0 as u32, layout::HTMINBUTTON | layout::HTMAXBUTTON | layout::HTCLOSE) => {
                // Swallow so DefWindowProc does not draw the classic caption buttons.
                self.pressed_nc = wp.0 as u32;
                Some(LRESULT(0))
            }
            WM_NCLBUTTONUP => {
                let code = wp.0 as u32;
                let pressed = std::mem::replace(&mut self.pressed_nc, 0);
                if code != pressed {
                    return None;
                }
                unsafe {
                    match code {
                        layout::HTMINBUTTON => {
                            let _ = ShowWindow(hwnd, SW_MINIMIZE);
                        }
                        layout::HTMAXBUTTON => {
                            let _ = ShowWindow(hwnd, if self.maximized() { SW_RESTORE } else { SW_MAXIMIZE });
                        }
                        layout::HTCLOSE => {
                            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                        }
                        _ => return None,
                    }
                }
                Some(LRESULT(0))
            }
            WM_MOUSEMOVE => {
                let (x, y) = self.client_dip(lp);
                if self.dragging_splitter {
                    self.sidebar_w = x;
                    self.relayout();
                    self.invalidate();
                    return Some(LRESULT(0));
                }
                self.track_leave(false);
                self.set_hot(self.hot_at(x, y));
                let rows = self.layout.rows;
                let hover = if rows.contains(x, y) { self.list.row_at(y - rows.y) } else { None };
                let side = self.sidebar.hit(x, y, self.layout.sidebar);
                if hover != self.list.hover_row || side != self.sidebar.hover {
                    self.list.hover_row = hover;
                    self.sidebar.hover = side;
                    self.invalidate();
                }
                Some(LRESULT(0))
            }
            WM_LBUTTONDOWN => {
                unsafe {
                    let _ = SetFocus(Some(hwnd));
                }
                let (x, y) = self.client_dip(lp);
                self.on_left_down(x, y);
                Some(LRESULT(0))
            }
            WM_LBUTTONDBLCLK => {
                let (x, y) = self.client_dip(lp);
                let rows = self.layout.rows;
                if self.columns_view() && self.layout.content.contains(x, y) {
                    // Folders already opened on the first click; a double-click opens files.
                    if let Some((col, Some(row))) = views::columns::hit(self, x, y) {
                        let c = &self.columns.cols[col];
                        if let Some(e) = c.entries.as_ref().and_then(|v| v.get(row)).filter(|e| !e.is_dir()) {
                            let path = c.path.join(&e.name);
                            self.open_path(&path);
                        }
                    }
                } else if !self.columns_view()
                    && rows.contains(x, y)
                    && let Some(i) = self.list.row_at(y - rows.y)
                {
                    self.open_index(i);
                } else if toolbar_rects(&self.layout).title.contains(x, y) {
                    self.show_address();
                } else {
                    self.on_left_down(x, y);
                }
                Some(LRESULT(0))
            }
            WM_LBUTTONUP => {
                if self.dragging_splitter {
                    self.dragging_splitter = false;
                    unsafe {
                        let _ = ReleaseCapture();
                    }
                    // Store the clamped value so a later drag starts from what is on screen.
                    self.sidebar_w = self.layout.sidebar.w;
                }
                Some(LRESULT(0))
            }
            WM_XBUTTONUP => {
                // Mouse back/forward buttons.
                match hi(wp.0 as isize) {
                    1 => self.go_back(),
                    2 => self.go_forward(),
                    _ => {}
                }
                Some(LRESULT(1))
            }
            WM_MOUSEWHEEL => {
                let notches = hi(wp.0 as isize) as f32 / WHEEL_DELTA as f32;
                if self.columns_view() {
                    self.column_wheel(notches, lp);
                } else {
                    self.list.on_wheel(notches, self.layout.rows.h);
                }
                self.invalidate();
                Some(LRESULT(0))
            }
            WM_KEYDOWN => self.on_key(wp.0 as u16).then_some(LRESULT(0)),
            WM_SYSKEYDOWN => self.on_sys_key(wp.0 as u16).then_some(LRESULT(0)),
            WM_CHAR => {
                let ch = char::from_u32(wp.0 as u32).filter(|c| !c.is_control())?;
                let a = self.list.on_char(ch, Instant::now());
                self.handle_list_action(a);
                Some(LRESULT(0))
            }
            WM_SETCURSOR => {
                if lo(lp.0) as u32 == layout::HTCLIENT && (self.hot == Hot::Splitter || self.dragging_splitter) {
                    unsafe {
                        if let Ok(c) = LoadCursorW(None, IDC_SIZEWE) {
                            SetCursor(Some(c));
                        }
                    }
                    return Some(LRESULT(1));
                }
                None
            }
            WM_GETMINMAXINFO => {
                let mmi = unsafe { &mut *(lp.0 as *mut MINMAXINFO) };
                mmi.ptMinTrackSize = POINT { x: (640.0 * self.scale) as i32, y: (400.0 * self.scale) as i32 };
                Some(LRESULT(0))
            }
            WM_DPICHANGED => {
                self.scale = (wp.0 & 0xFFFF) as f32 / 96.0;
                self.gfx.set_dpi(96.0 * self.scale);
                self.rebuild_edit_style();
                let r = unsafe { &*(lp.0 as *const RECT) };
                unsafe {
                    let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                }
                Some(LRESULT(0))
            }
            WM_SETTINGCHANGE => {
                let is_color = lp.0 != 0 && unsafe { PCWSTR(lp.0 as *const u16).to_string() }.is_ok_and(|s| s == "ImmersiveColorSet");
                if is_color {
                    self.theme = Theme::system();
                    self.apply_theme_to_frame();
                    self.rebuild_edit_style();
                    self.invalidate();
                }
                None
            }
            WM_DESTROY => {
                edit::delete_font(self.edit_font);
                unsafe {
                    let _ = DeleteObject(self.edit_brush.into());
                    PostQuitMessage(0);
                }
                Some(LRESULT(0))
            }
            _ => None,
        }
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCALCSIZE && wp.0 != 0 {
            // Client area covers the whole window: we draw our own title bar.
            if IsZoomed(hwnd).as_bool() {
                // A maximized window hangs its frame off-screen; inset so content stays visible.
                let dpi = GetDpiForWindow(hwnd);
                let pad = GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
                let fx = GetSystemMetricsForDpi(SM_CXFRAME, dpi) + pad;
                let fy = GetSystemMetricsForDpi(SM_CYFRAME, dpi) + pad;
                let p = &mut *(lp.0 as *mut NCCALCSIZE_PARAMS);
                p.rgrc[0].left += fx;
                p.rgrc[0].right -= fx;
                p.rgrc[0].top += fy;
                p.rgrc[0].bottom -= fy;
            }
            return LRESULT(0);
        }
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
        if msg == WM_NCDESTROY && !ptr.is_null() {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Box::from_raw(ptr));
        } else if !ptr.is_null()
            && let Some(r) = (*ptr).handle(msg, wp, lp)
        {
            return r;
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}

fn start_folder() -> PathBuf {
    shell::favorites().into_iter().next().map(|p| p.path).unwrap_or_else(|| PathBuf::from("C:\\"))
}

pub fn run() -> Result<()> {
    unsafe {
        let hinst = GetModuleHandleW(None)?;
        let class = w!("FExplorerMain");
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: class,
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            return Err(windows::core::Error::from_thread());
        }
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!("Files"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            WINDOW_W as i32,
            WINDOW_H as i32,
            None,
            None,
            Some(hinst.into()),
            None,
        )?;
        let scale = GetDpiForWindow(hwnd) as f32 / 96.0;
        let backdrop = DWMSBT_MAINWINDOW;
        let mica = DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &backdrop as *const _ as _, 4).is_ok();
        let margins = if mica { MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 } } else { MARGINS { cyTopHeight: 1, ..Default::default() } };
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);

        let start = start_folder();
        let mut app = Box::new(App {
            hwnd,
            gfx: Gfx::new()?,
            theme: Theme::system(),
            mica,
            scale,
            w: WINDOW_W,
            h: WINDOW_H,
            sidebar_w: 200.0,
            layout: Layout::default(),
            hot: Hot::None,
            history: History::new(start.clone()),
            list: ListView::default(),
            view: View::List,
            columns: Columns::default(),
            error: None,
            loading: false,
            show_hidden: false,
            type_names: HashMap::new(),
            icon_idx: HashMap::new(),
            sidebar: Sidebar::load(),
            free: None,
            address: edit::create(hwnd, ID_ADDRESS)?,
            edit_font: HFONT::default(),
            edit_brush: HBRUSH::default(),
            listing_gen: Generation::default(),
            pressed_nc: 0,
            dragging_splitter: false,
            tracking: false,
        });
        app.apply_theme_to_frame();
        app.rebuild_edit_style();
        app.navigate(start, NavKind::Refresh);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(app) as isize);
        // Re-run WM_NCCALCSIZE, then size the window for this monitor's DPI.
        SetWindowPos(hwnd, None, 0, 0, (WINDOW_W * scale) as i32, (WINDOW_H * scale) as i32, SWP_NOMOVE | SWP_NOZORDER | SWP_FRAMECHANGED)?;
        let _ = ShowWindow(hwnd, SW_SHOWDEFAULT);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        Ok(())
    }
}
