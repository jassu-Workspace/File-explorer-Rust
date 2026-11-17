use crate::draw::Gfx;
use crate::edit::{self, ID_ADDRESS, ID_RENAME, ID_SEARCH, WM_EDIT_KEY};
use crate::search;
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
use std::time::{Duration, Instant};
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
    VK_DELETE, VK_DOWN, VK_F2, VK_F5, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_UP,
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
const ID_SEARCH_TIMER: usize = 1;
const SEARCH_DEBOUNCE_MS: u32 = 150;
const ID_ANIM_TIMER: usize = 2;
const SCROLL_ANIM: Duration = Duration::from_millis(120);
const HOVER_ANIM: Duration = Duration::from_millis(100);
const COLUMN_ANIM: Duration = Duration::from_millis(150);

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
    pub search_edit: HWND,
    rename_edit: HWND,
    /// Row being renamed (list view).
    renaming: Option<usize>,
    /// After a reload, select this name and start renaming it (new folder).
    pending_rename: Option<std::ffi::OsString>,
    /// After a reload, select this name (Show in Folder).
    pending_select: Option<std::ffi::OsString>,
    search: Option<search::SearchHandle>,
    search_gen: Generation,
    /// True from the first search until the field is cleared or the user navigates away.
    pub searching: bool,
    search_running: bool,
    search_skipped: usize,
    search_root: PathBuf,
    pre_search_view: View,
    /// Windows "Show animations in Windows" setting.
    animations: bool,
    scroll_anim: Option<crate::layout::Anim>,
    cols_anim: Option<crate::layout::Anim>,
    hover_anim: Option<crate::layout::Anim>,
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

/// Display name for a folder: drive label for roots (from the cached drive list; querying volumes
/// on every paint can stall on a sleeping drive), else the last path component.
pub fn folder_title(p: &Path, drives: &[shell::Place]) -> String {
    match p.file_name() {
        Some(n) => n.to_string_lossy().into_owned(),
        None => drives.iter().find(|d| d.path == p).map_or_else(|| p.display().to_string(), |d| d.name.clone()),
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
        folder_title(self.current_dir(), &self.sidebar.drives)
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
        if self.searching {
            // The list holds search results, not the folder: leave search and reload in the new view.
            self.clear_search_field();
            self.end_search(false);
            self.view = v;
            self.refresh();
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
        let target = views::columns::fit_scroll(views::columns::total_width(self), viewport, self.columns.scroll_x);
        if self.animations && (target - self.columns.scroll_x).abs() > 0.5 {
            // Slide; off-screen columns are freed when the slide ends.
            self.cols_anim = Some(crate::layout::Anim::new(self.columns.scroll_x, target, COLUMN_ANIM, Instant::now()));
            self.start_anim_timer();
        } else {
            self.columns.scroll_x = target;
            self.columns.drop_offscreen(viewport, crate::theme::COLUMN_W);
        }
        let (rows_h, c) = (self.layout.content.h, &mut self.columns.cols[col]);
        let top = row as f32 * crate::theme::ROW_H;
        if top < c.scroll_y {
            c.scroll_y = top;
        } else if top + crate::theme::ROW_H > c.scroll_y + rows_h {
            c.scroll_y = top + crate::theme::ROW_H - rows_h;
        }
        self.invalidate();
    }

    fn start_anim_timer(&self) {
        unsafe { SetTimer(Some(self.hwnd), ID_ANIM_TIMER, 16, None) };
    }

    /// Advances running animations; stops the timer when none are left, so idle CPU stays at zero.
    fn tick(&mut self) {
        let now = Instant::now();
        if let Some(a) = self.scroll_anim {
            self.list.scroll_y = a.value(now);
            if a.done(now) {
                self.scroll_anim = None;
            }
        }
        if let Some(a) = self.cols_anim {
            self.columns.scroll_x = a.value(now);
            if a.done(now) {
                self.cols_anim = None;
                self.columns.drop_offscreen(self.layout.content.w, crate::theme::COLUMN_W);
            }
        }
        if self.hover_anim.is_some_and(|a| a.done(now)) {
            self.hover_anim = None;
        }
        if self.scroll_anim.is_none() && self.cols_anim.is_none() && self.hover_anim.is_none() {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), ID_ANIM_TIMER);
            }
        }
        self.invalidate();
    }

    /// Opacity of the row hover highlight while it fades in.
    pub fn hover_alpha(&self) -> f32 {
        self.hover_anim.map_or(1.0, |a| a.value(Instant::now()))
    }

    fn list_wheel(&mut self, notches: f32) {
        let (cur, h) = (self.list.scroll_y, self.layout.rows.h);
        // Repeated notches extend the running animation's target instead of restarting from the current frame.
        self.list.scroll_y = self.scroll_anim.map_or(cur, |a| a.to);
        self.list.on_wheel(notches, h);
        let target = self.list.scroll_y;
        if self.animations {
            self.list.scroll_y = cur;
            self.scroll_anim = Some(crate::layout::Anim::new(cur, target, SCROLL_ANIM, Instant::now()));
            self.start_anim_timer();
        }
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
        !edit::visible(self.search_edit)
    }

    /// "Where" column text: the result's folder, starting with the searched folder's name.
    pub fn where_label(&self, e: &Entry) -> String {
        let root = folder_title(&self.search_root, &self.sidebar.drives);
        match e.rel_dir.as_deref().filter(|p| !p.as_os_str().is_empty()) {
            Some(rel) => format!("{root}\\{}", rel.display()),
            None => root,
        }
    }

    fn search_field_rect(&self) -> crate::layout::Rect {
        let r = toolbar_rects(&self.layout).search;
        crate::layout::Rect::new(r.x + 28.0, r.y, r.w - 36.0, r.h)
    }

    fn focus_search(&mut self) {
        edit::show_in(self.search_edit, self.search_field_rect(), self.scale);
        edit::focus_select_all(self.search_edit);
        self.invalidate();
    }

    /// Starts (or restarts) a search for the search field's text in the current folder tree.
    fn run_search(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), ID_SEARCH_TIMER);
        }
        let query = edit::text(self.search_edit).trim().to_string();
        if query.is_empty() {
            self.end_search(true);
            return;
        }
        if self.search.is_none() && !self.searching {
            // First keystroke of a new search: remember where we came from.
            self.search_root = self.current_dir().to_path_buf();
            self.pre_search_view = self.view;
        }
        self.search = None; // dropping the handle cancels the previous walk
        self.searching = true;
        self.search_running = true;
        self.search_skipped = 0;
        self.view = View::List;
        self.columns = Columns::default();
        self.list.show_where = true;
        self.list.set_entries(Vec::new());
        self.error = None;
        let generation = self.search_gen.next();
        let (h1, h2) = (self.send_hwnd(), self.send_hwnd());
        self.search = Some(search::start_search(
            self.search_root.clone(),
            &query,
            self.show_hidden,
            move |batch| h1.post(WM_SEARCH_BATCH, Box::new((generation, batch))),
            move |stats| h2.post(WM_SEARCH_DONE, Box::new((generation, stats))),
        ));
        self.invalidate();
    }

    fn on_search_batch(&mut self, generation: u64, mut batch: Vec<Entry>) {
        if !self.searching || !self.search_gen.is_current(generation) {
            return;
        }
        self.list.entries.append(&mut batch);
        // Keep results sorted while the list is small; past that, sort once when the walk ends.
        if self.list.entries.len() < 50_000 {
            self.list.resort();
        }
        self.invalidate();
    }

    fn on_search_done(&mut self, generation: u64, stats: search::SearchStats) {
        if stats.cancelled || !self.searching || !self.search_gen.is_current(generation) {
            return;
        }
        self.search_running = false;
        self.search_skipped = stats.skipped_denied;
        if self.list.entries.len() >= 50_000 {
            self.list.resort();
        }
        self.invalidate();
    }

    /// Leaves search mode. `reload` restores the folder listing and the view used before searching.
    fn end_search(&mut self, reload: bool) {
        let was = self.searching;
        self.search = None;
        self.searching = false;
        self.search_running = false;
        self.list.show_where = false;
        if !was {
            return;
        }
        if reload {
            self.view = self.pre_search_view;
            self.refresh();
        }
    }

    fn clear_search_field(&mut self) {
        edit::set_text(self.search_edit, "");
        edit::hide(self.search_edit);
        unsafe {
            let _ = KillTimer(Some(self.hwnd), ID_SEARCH_TIMER);
            let _ = SetFocus(Some(self.hwnd));
        }
    }
    pub fn search_placeholder(&self) -> String {
        format!("Search in {}", self.title())
    }
    pub fn empty_text(&self) -> &'static str {
        "This folder is empty"
    }
    /// Path-bar status that replaces the item count (search progress).
    pub fn status_override(&self) -> Option<String> {
        self.searching.then(|| search::status_text(self.search_running, self.list.entries.len(), self.search_skipped))
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
        if edit::visible(self.search_edit) {
            edit::show_in(self.search_edit, self.search_field_rect(), self.scale);
        }
    }

    /// GDI font and brush for the EDIT children; rebuilt on DPI or theme change.
    fn rebuild_edit_style(&mut self) {
        edit::delete_font(self.edit_font);
        self.edit_font = edit::make_font(self.scale);
        edit::set_font(self.address, self.edit_font);
        edit::set_font(self.search_edit, self.edit_font);
        edit::set_font(self.rename_edit, self.edit_font);
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
        } else if id == ID_RENAME {
            if vk == VK_RETURN.0 {
                self.commit_rename();
            } else if vk == VK_ESCAPE.0 {
                self.cancel_rename();
            }
        } else if id == ID_SEARCH {
            if vk == VK_RETURN.0 {
                self.run_search();
            } else if vk == VK_ESCAPE.0 {
                self.clear_search_field();
                self.end_search(true);
            } else if vk == VK_DOWN.0 {
                // Down arrow moves from the field into the results.
                unsafe {
                    let _ = SetFocus(Some(self.hwnd));
                }
                let a = self.list.on_key(VK_DOWN.0, Mods::default(), self.layout.rows.h);
                self.handle_list_action(a);
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
        if self.searching {
            // Navigating away ends the search; the new folder replaces the results.
            self.clear_search_field();
            self.end_search(false);
            self.view = self.pre_search_view;
        }
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
                } else {
                    self.apply_pending_selection();
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
    fn open_path(&mut self, path: &Path) {
        if shell::open(Some(self.hwnd), path).is_err() {
            self.error = Some(format!("Can't open {}.", path.display()));
            self.invalidate();
        }
    }

    /// Full paths of the selected items (list rows, or the focused column's selection).
    fn selected_paths(&self) -> Vec<PathBuf> {
        if self.columns_view() {
            let Some(c) = self.columns.cols.get(self.columns.focus_col) else { return Vec::new() };
            return c.selected.and_then(|s| c.entries.as_ref()?.get(s)).map(|e| vec![c.path.join(&e.name)]).unwrap_or_default();
        }
        self.list.sel.selected().into_iter().filter_map(|i| self.list.entries.get(i)).map(|e| self.entry_path(e)).collect()
    }

    /// Reloads what is on screen after a file action.
    fn reload_current(&mut self) {
        if self.columns_view() {
            let fc = self.columns.focus_col;
            self.columns.cols.truncate(fc + 1);
            if let Some(c) = self.columns.cols.get_mut(fc) {
                c.selected = None;
                let p = c.path.clone();
                self.load_column(p);
            }
        } else if !self.searching {
            self.refresh();
        }
    }

    fn report(&mut self, what: &str, r: windows::core::Result<()>) {
        if let Err(e) = r {
            self.error = Some(format!("{what} failed: {}", e.message()));
            self.invalidate();
        }
    }

    fn copy_selection(&mut self, cut: bool) {
        let paths = self.selected_paths();
        if !paths.is_empty() {
            let r = shell::clipboard_put(Some(self.hwnd), &paths, cut);
            self.report(if cut { "Cut" } else { "Copy" }, r);
        }
    }

    fn paste_here(&mut self) {
        let Some((items, cut)) = shell::clipboard_get(Some(self.hwnd)) else { return };
        let dest = self.current_dir().to_path_buf();
        let r = shell::paste(Some(self.hwnd), &items, cut, &dest);
        if r.is_ok() && cut {
            // A moved file is gone from its source; a second paste must not try again.
            shell::clipboard_clear(Some(self.hwnd));
        }
        self.report("Paste", r);
        self.reload_current();
    }

    fn delete_selection(&mut self, permanent: bool) {
        let paths = self.selected_paths();
        if paths.is_empty() {
            return;
        }
        let r = shell::delete(Some(self.hwnd), &paths, permanent, true);
        if self.searching {
            // Keep the results; drop rows whose file is gone.
            let gone: Vec<bool> = self.list.entries.iter().map(|e| !self.entry_path(e).exists()).collect();
            let mut it = gone.into_iter();
            self.list.entries.retain(|_| !it.next().unwrap_or(false));
            self.list.sel.clear();
            self.invalidate();
        }
        self.report("Delete", r);
        self.reload_current();
    }

    fn create_folder(&mut self) {
        if self.searching {
            return;
        }
        let dir = self.current_dir().to_path_buf();
        match shell::new_folder(Some(self.hwnd), &dir) {
            Ok(p) => {
                if !self.columns_view() {
                    self.pending_rename = p.file_name().map(|n| n.to_os_string());
                }
                self.reload_current();
            }
            Err(e) => self.report("New folder", Err(e)),
        }
    }

    /// Inline rename over the focused row's name (list view).
    fn start_rename(&mut self) {
        if self.columns_view() {
            return;
        }
        let Some(i) = self.list.sel.focus().filter(|&i| i < self.list.entries.len()) else { return };
        self.list.ensure_visible(i, self.layout.rows.h);
        let rows = self.layout.rows;
        let name_w = views::list::columns(rows.w, self.list.show_where)[0].2;
        let y = rows.y + i as f32 * crate::theme::ROW_H - self.list.scroll_y;
        let r = crate::layout::Rect::new(rows.x + 10.0 + 24.0 - 2.0, y, name_w - 40.0, crate::theme::ROW_H);
        let e = &self.list.entries[i];
        let name = e.name.to_string_lossy().into_owned();
        // Select the stem so typing keeps the extension, like Explorer.
        let stem_len = if e.is_dir() { name.encode_utf16().count() } else {
            Path::new(&name).file_stem().map_or(0, |s| s.to_string_lossy().encode_utf16().count())
        };
        self.renaming = Some(i);
        edit::set_text(self.rename_edit, &name);
        edit::show_in(self.rename_edit, r, self.scale);
        edit::focus_select(self.rename_edit, 0, stem_len);
        self.invalidate();
    }

    fn cancel_rename(&mut self) {
        if self.renaming.take().is_some() {
            edit::hide(self.rename_edit);
            unsafe {
                let _ = SetFocus(Some(self.hwnd));
            }
            self.invalidate();
        }
    }

    fn commit_rename(&mut self) {
        let Some(i) = self.renaming.take() else { return };
        let new_name = edit::text(self.rename_edit).trim().to_string();
        edit::hide(self.rename_edit);
        unsafe {
            let _ = SetFocus(Some(self.hwnd));
        }
        let Some(e) = self.list.entries.get(i) else { return };
        if new_name == e.name.to_string_lossy() {
            self.invalidate();
            return;
        }
        if let Err(m) = shell::validate_name(&new_name) {
            self.error = Some(m.to_string());
            self.invalidate();
            return;
        }
        let path = self.entry_path(e);
        let r = shell::rename(Some(self.hwnd), &path, &new_name);
        if r.is_ok() {
            self.list.entries[i].name = new_name.clone().into();
            self.pending_select = Some(new_name.into());
        }
        self.report("Rename", r);
        self.reload_current();
        self.invalidate();
    }

    /// After a reload: select a renamed or "Show in Folder" item, or start renaming a new folder.
    fn apply_pending_selection(&mut self) {
        let rename = self.pending_rename.take();
        let Some(name) = rename.clone().or_else(|| self.pending_select.take()) else { return };
        if let Some(i) = self.list.entries.iter().position(|e| e.name == name) {
            self.list.click_row(i, Mods::default());
            self.list.ensure_visible(i, self.layout.rows.h);
            if rename.is_some() {
                self.start_rename();
            }
        }
    }

    fn on_context_menu(&mut self, lp: LPARAM) {
        let keyboard = lp.0 == -1;
        let mut screen = POINT { x: lo(lp.0), y: hi(lp.0) };
        let mut on_item = false;
        if keyboard {
            // Shift+F10 / Menu key: anchor at the focused row.
            let rows = self.layout.rows;
            let y = self.list.sel.focus().map_or(rows.y, |i| rows.y + (i as f32 + 0.5) * crate::theme::ROW_H - self.list.scroll_y);
            screen = POINT { x: ((rows.x + 40.0) * self.scale) as i32, y: (y * self.scale) as i32 };
            unsafe {
                let _ = windows::Win32::Graphics::Gdi::ClientToScreen(self.hwnd, &mut screen);
            }
            on_item = !self.selected_paths().is_empty();
        } else {
            let mut pt = screen;
            unsafe {
                let _ = ScreenToClient(self.hwnd, &mut pt);
            }
            let (x, y) = (pt.x as f32 / self.scale, pt.y as f32 / self.scale);
            if self.columns_view() {
                if let Some((col, Some(row))) = views::columns::hit(self, x, y) {
                    if self.columns.cols[col].selected != Some(row) {
                        self.column_select(col, row);
                    }
                    self.columns.focus_col = col;
                    on_item = true;
                }
            } else if self.layout.rows.contains(x, y) {
                if let Some(i) = self.list.row_at(y - self.layout.rows.y) {
                    if !self.list.sel.is_selected(i) {
                        self.list.click_row(i, Mods::default());
                    }
                    on_item = true;
                }
            } else if !self.layout.content.contains(x, y) {
                return;
            }
        }
        self.invalidate();
        self.context_menu(screen, on_item);
    }

    /// Right-click menu. `on_item` is false for empty space.
    fn context_menu(&mut self, screen: POINT, on_item: bool) {
        const OPEN: usize = 1;
        const SHOW: usize = 2;
        const RENAME: usize = 3;
        const COPY: usize = 4;
        const CUT: usize = 5;
        const DELETE: usize = 6;
        const PASTE: usize = 7;
        const NEW_FOLDER: usize = 8;
        const HIDDEN: usize = 9;
        let can_paste = shell::clipboard_get(Some(self.hwnd)).is_some();
        let cmd = unsafe {
            let Ok(m) = CreatePopupMenu() else { return };
            let add = |id: usize, text: PCWSTR, flags: MENU_ITEM_FLAGS| {
                let _ = AppendMenuW(m, MF_STRING | flags, id, text);
            };
            if on_item {
                add(OPEN, w!("Open"), MF_ENABLED);
                if self.searching {
                    add(SHOW, w!("Show in Folder"), MF_ENABLED);
                }
                let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
                add(RENAME, w!("Rename\tF2"), if self.columns_view() { MF_GRAYED } else { MF_ENABLED });
                add(COPY, w!("Copy\tCtrl+C"), MF_ENABLED);
                add(CUT, w!("Cut\tCtrl+X"), MF_ENABLED);
                add(DELETE, w!("Delete\tDel"), MF_ENABLED);
            } else {
                add(PASTE, w!("Paste\tCtrl+V"), if can_paste && !self.searching { MF_ENABLED } else { MF_GRAYED });
                add(NEW_FOLDER, w!("New Folder\tCtrl+Shift+N"), if self.searching { MF_GRAYED } else { MF_ENABLED });
                let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
                add(HIDDEN, w!("Show Hidden Files"), if self.show_hidden { MF_CHECKED } else { MF_UNCHECKED });
            }
            let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, screen.x, screen.y, None, self.hwnd, None);
            let _ = DestroyMenu(m);
            cmd.0 as usize
        };
        match cmd {
            OPEN => {
                if self.columns_view() {
                    if let Some(p) = self.selected_paths().into_iter().next() {
                        self.open_path(&p);
                    }
                } else if let Some(i) = self.list.sel.focus() {
                    self.open_index(i);
                }
            }
            SHOW => {
                let target = self.list.sel.focus().and_then(|i| self.list.entries.get(i)).map(|e| (self.entry_path(e), e.name.clone()));
                if let Some((path, name)) = target
                    && let Some(parent) = path.parent()
                {
                    self.pending_select = Some(name);
                    self.navigate(parent.to_path_buf(), NavKind::Visit);
                }
            }
            RENAME => self.start_rename(),
            COPY => self.copy_selection(false),
            CUT => self.copy_selection(true),
            DELETE => self.delete_selection(false),
            PASTE => self.paste_here(),
            NEW_FOLDER => self.create_folder(),
            HIDDEN => {
                self.show_hidden = !self.show_hidden;
                self.reload_current();
            }
            _ => {}
        }
    }

    /// Full path of a list entry (search results carry their folder relative to the search root).
    pub fn entry_path(&self, e: &Entry) -> PathBuf {
        let mut p = if self.searching { self.search_root.clone() } else { self.history.current().to_path_buf() };
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
        } else if r.search.contains(x, y) {
            self.focus_search();
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
            0x46 if mods().ctrl => self.focus_search(), // Ctrl+F
            0x43 if mods().ctrl => self.copy_selection(false), // Ctrl+C
            0x58 if mods().ctrl => self.copy_selection(true),  // Ctrl+X
            0x56 if mods().ctrl => self.paste_here(),           // Ctrl+V
            0x4E if mods().ctrl && mods().shift => self.create_folder(), // Ctrl+Shift+N
            v if v == VK_F2.0 => self.start_rename(),
            v if v == VK_DELETE.0 => self.delete_selection(mods().shift),
            v if v == VK_ESCAPE.0 && self.searching => {
                self.clear_search_field();
                self.end_search(true);
            }
            0x31 if mods().ctrl => self.set_view(View::List), // Ctrl+1
            0x32 if mods().ctrl => self.set_view(View::Columns), // Ctrl+2
            _ if self.columns_view() => return self.column_key(vk),
            _ => {
                // Keyboard moves set the scroll directly; a running wheel animation would fight them.
                if let Some(a) = self.scroll_anim.take() {
                    self.list.scroll_y = a.to;
                }
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
            WM_CONTEXTMENU => {
                self.on_context_menu(lp);
                Some(LRESULT(0))
            }
            WM_TIMER if wp.0 == ID_ANIM_TIMER => {
                self.tick();
                Some(LRESULT(0))
            }
            WM_TIMER if wp.0 == ID_SEARCH_TIMER => {
                self.run_search();
                Some(LRESULT(0))
            }
            WM_SEARCH_BATCH => {
                let m = unsafe { Box::from_raw(lp.0 as *mut (u64, Vec<Entry>)) };
                let (g, batch) = *m;
                self.on_search_batch(g, batch);
                Some(LRESULT(0))
            }
            WM_SEARCH_DONE => {
                let m = unsafe { Box::from_raw(lp.0 as *mut (u64, search::SearchStats)) };
                self.on_search_done(m.0, m.1);
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
                } else if id == ID_RENAME && code == EN_KILLFOCUS {
                    self.commit_rename();
                } else if id == ID_SEARCH && code == EN_CHANGE {
                    unsafe { SetTimer(Some(hwnd), ID_SEARCH_TIMER, SEARCH_DEBOUNCE_MS, None) };
                } else if id == ID_SEARCH && code == EN_KILLFOCUS && edit::text(self.search_edit).trim().is_empty() {
                    edit::hide(self.search_edit);
                    self.invalidate();
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
                if hover != self.list.hover_row && hover.is_some() && self.animations {
                    self.hover_anim = Some(crate::layout::Anim::new(0.0, 1.0, HOVER_ANIM, Instant::now()));
                    self.start_anim_timer();
                }
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
                // The rename box is placed over a row; scrolling would leave it behind.
                self.commit_rename();
                if self.columns_view() {
                    self.column_wheel(notches, lp);
                } else {
                    self.list_wheel(notches);
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

fn animations_enabled() -> bool {
    let mut on = BOOL::from(true);
    let ok = unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, Some(&mut on as *mut BOOL as _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)) };
    ok.is_err() || on.as_bool()
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
            search_edit: edit::create(hwnd, ID_SEARCH)?,
            rename_edit: edit::create(hwnd, ID_RENAME)?,
            renaming: None,
            pending_rename: None,
            pending_select: None,
            search: None,
            search_gen: Generation::default(),
            searching: false,
            search_running: false,
            search_skipped: 0,
            search_root: start.clone(),
            pre_search_view: View::List,
            animations: animations_enabled(),
            scroll_anim: None,
            cols_anim: None,
            hover_anim: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw::Glyph;
    use crate::shell::Place;

    #[test]
    fn folder_title_uses_cached_drive_names() {
        let drives = [Place { name: "Data (D:)".into(), path: PathBuf::from("D:\\"), glyph: Glyph::Drive }];
        assert_eq!(folder_title(Path::new("D:\\"), &drives), "Data (D:)");
        assert_eq!(folder_title(Path::new(r"D:\photos"), &drives), "photos");
        assert_eq!(folder_title(Path::new("E:\\"), &drives), "E:\\", "unknown roots fall back to the path");
    }
}
