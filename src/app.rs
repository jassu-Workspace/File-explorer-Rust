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
use crate::transfer::Op;
use crate::views::transfer::Prompt;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use crate::views::toolbar::{ToolbarRects, toolbar_rects};
use crate::views::{self, pathbar};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMSBT_MAINWINDOW, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
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
    VK_DELETE, VK_DOWN, VK_F2, VK_F5, VK_LEFT, VK_MENU, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_UP,
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
pub const WM_THUMB_DONE: u32 = WM_APP + 5;
pub const WM_TRANSFER: u32 = WM_APP + 6;
const ID_SEARCH_TIMER: usize = 1;
const SEARCH_DEBOUNCE_MS: u32 = 150;
const ID_ANIM_TIMER: usize = 2;
const ID_HOVER_SELECT: usize = 3;
/// Menu ids 100..107 are the seven color tags.
const TAG_CMD: usize = 100;
/// View menu ids: 200..203 icon presets, 204 details, 205 columns, 206 gallery.
const VIEW_CMD: usize = 200;
/// Sort menu ids: 220 + SortKey index, 230 ascending, 231 descending.
const SORT_CMD: usize = 220;
/// Group menu ids: 240 + GroupBy index.
const GROUP_CMD: usize = 240;
const SORT_KEYS: [(crate::listing::SortKey, &str); 5] = [
    (crate::listing::SortKey::Name, "Name"),
    (crate::listing::SortKey::Modified, "Date modified"),
    (crate::listing::SortKey::Created, "Date created"),
    (crate::listing::SortKey::Kind, "Type"),
    (crate::listing::SortKey::Size, "Size"),
];
const GROUPS: [(GroupBy, &str); 5] = [(GroupBy::None, "(None)"), (GroupBy::Kind, "Type"), (GroupBy::Date, "Date modified"), (GroupBy::Size, "Size"), (GroupBy::Name, "Name")];
const SCROLL_ANIM: Duration = Duration::from_millis(120);
const HOVER_ANIM: Duration = Duration::from_millis(100);
const COLUMN_ANIM: Duration = Duration::from_millis(150);

/// Finder's "Group" button.
/// What a mouse press landed on, for single-click open.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PressTarget {
    Item(usize),
    Column(usize, usize),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GroupBy {
    None,
    Kind,
    Date,
    Size,
    Name,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Icon,
    List,
    Columns,
    Gallery,
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
    View(u8),
    Group,
    Share,
    Tags,
    Action,
    Search,
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
    /// Drag-box selection in icon view: start point in grid coordinates, the selection it adds to, current point.
    band: Option<(f32, f32, Vec<usize>, f32, f32)>,
    /// Icon-view icon size in DIPs (Ctrl+Plus / Ctrl+Minus).
    pub icon_size: f32,
    pub group_by: GroupBy,
    pub tags: crate::tags::Tags,
    prefs: crate::prefs::Prefs,
    pub show_pathbar: bool,
    /// The list sort was switched to newest-first for Recents and goes back to Name when leaving it.
    recents_sort: bool,
    share: Option<crate::share::Share>,
    /// Set while the list shows the files of one sidebar tag.
    pub tag_view: Option<crate::tags::Tag>,
    /// Groups over `list.entries` for the current `group_by` (empty when not grouping).
    pub groups: Vec<crate::listing::Group>,
    /// Gallery strip horizontal scroll.
    pub gallery_scroll: f32,
    thumbnailer: Option<crate::thumbs::Thumbnailer>,
    thumb_pending: std::collections::HashSet<crate::thumbs::ThumbKey>,
    /// Files Windows has no thumbnail for; not asked again.
    thumb_none: std::collections::HashSet<crate::thumbs::ThumbKey>,
    thumb_exts: crate::thumbs::ExtLearner,
    pub columns: Columns,
    pub error: Option<String>,
    pub loading: bool,
    pub settings: crate::settings::Settings,
    win_defaults: crate::settings::WinDefaults,
    pub settings_open: bool,
    sheet_anim: Option<crate::layout::Anim>,
    /// Running transfers and their animations.
    pub xfer: views::transfer::Transfers,
    /// Press on an item, for single-click open on release: (x, y, target, Ctrl/Shift held).
    press: Option<(f32, f32, PressTarget, bool)>,
    /// Item waiting for the hover-select timer (single-click mode).
    hover_pending: Option<usize>,
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
        match self.tag_view {
            Some(t) => t.name().to_string(),
            None => folder_title(self.current_dir(), &self.sidebar.drives),
        }
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
        let key = self.history.current().to_string_lossy().into_owned();
        self.prefs.set(&key, view_name(v));
        let _ = self.prefs.save();
        if v == View::Columns {
            let root = self.history.current().to_path_buf();
            self.columns.open_root(root.clone());
            self.columns.loaded(&root, self.list.entries.clone());
        } else {
            self.columns = Columns::default();
        }
        if v == View::Gallery && self.list.sel.focus().is_none() && !self.list.entries.is_empty() {
            // The gallery always previews something.
            self.list.sel.click(0);
        }
        if let Some(f) = self.list.sel.focus() {
            self.reveal(f);
        }
        self.list.scroll_y = self.list.scroll_y.min(self.max_scroll());
        self.invalidate();
    }

    fn load_column(&self, path: PathBuf) {
        let (hwnd, settings) = (self.send_hwnd(), self.settings);
        std::thread::spawn(move || {
            let result = read_folder(&path, |a| settings.visible(a));
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
        if self.sheet_anim.is_some_and(|a| a.done(now)) {
            self.sheet_anim = None;
        }
        if self.hover_anim.is_some_and(|a| a.done(now)) {
            self.hover_anim = None;
        }
        self.xfer.cleanup(now);
        let recycling = self.xfer.rows.iter().any(|r| r.op == crate::transfer::Op::Recycle && r.done.is_none());
        if let Some(b) = self.xfer.bin.as_mut().filter(|b| b.landed.is_none() && !recycling && self.xfer.flights.is_empty()) {
            b.landed = Some(now);
        }
        if self.scroll_anim.is_none() && self.cols_anim.is_none() && self.hover_anim.is_none() && self.sheet_anim.is_none() && !self.xfer.active() {
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

    /// Drag box in grid coordinates (scroll included), normalized to a positive size.
    pub fn band_rect(&self) -> Option<crate::layout::Rect> {
        let (x0, y0, _, x1, y1) = self.band.as_ref()?;
        Some(crate::layout::Rect::new(x0.min(*x1), y0.min(*y1), (x1 - x0).abs(), (y1 - y0).abs()))
    }

    fn update_band(&mut self, x: f32, y: f32) {
        let a = self.item_area();
        let (gx, gy) = (x - a.x, (y - a.y).clamp(0.0, a.h) + self.list.scroll_y);
        let Some(b) = self.band.as_mut() else { return };
        (b.3, b.4) = (gx, gy);
        let mut sel = b.2.clone();
        if let Some(r) = self.band_rect() {
            sel.extend(self.grid().cells_in(r, self.list.entries.len()));
        }
        self.list.sel.set(sel);
        self.invalidate();
    }

    /// The cached thumbnail for a file, or `None` after queueing a request for it.
    pub fn thumb_or_request(&mut self, path: PathBuf, drawn_px: f32) -> Option<windows::Win32::Graphics::Direct2D::ID2D1Bitmap> {
        let key = (path, crate::thumbs::bucket(drawn_px * self.scale));
        if let Some(b) = self.gfx.thumb(&key) {
            return Some(b);
        }
        let ext = key.0.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if self.thumb_exts.should_request(&ext) && !self.thumb_none.contains(&key) && self.thumb_pending.insert(key.clone()) {
            if self.thumbnailer.is_none() {
                self.thumbnailer = Some(crate::thumbs::Thumbnailer::start(self.send_hwnd()));
            }
            if let Some(t) = &self.thumbnailer {
                t.request(key);
            }
        }
        None
    }

    fn on_thumb(&mut self, d: crate::thumbs::ThumbDone) {
        let current = self.thumbnailer.as_ref().is_some_and(|t| t.is_current(d.generation));
        self.thumb_pending.remove(&d.key);
        let ext = d.key.0.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        self.thumb_exts.record(&ext, d.bitmap.is_some());
        match d.bitmap {
            Some((h, alpha)) if current => {
                self.gfx.add_thumb(d.key, h, alpha);
                self.invalidate();
            }
            Some((h, _)) => crate::thumbs::free(h),
            None => {
                if self.thumb_none.len() > 50_000 {
                    self.thumb_none.clear(); // keep the negative cache bounded
                }
                self.thumb_none.insert(d.key);
            }
        }
    }

    /// Lists every file with `tag` (search-style results with a Where column).
    fn show_tag(&mut self, tag: crate::tags::Tag) {
        use std::os::windows::fs::MetadataExt;
        if !self.searching {
            self.pre_search_view = self.view;
        }
        self.clear_search_field();
        self.search = None;
        self.searching = true;
        self.search_running = false;
        self.search_skipped = 0;
        self.search_root = PathBuf::new(); // results carry absolute folders in rel_dir
        self.tag_view = Some(tag);
        self.view = View::List;
        self.columns = Columns::default();
        self.list.show_where = true;
        self.error = None;
        let entries = self
            .tags
            .files(tag)
            .into_iter()
            .filter_map(|p| {
                let m = std::fs::symlink_metadata(&p).ok()?;
                Some(Entry { name: p.file_name()?.to_os_string(), size: m.file_size(), modified: m.last_write_time(), created: m.creation_time(), attrs: m.file_attributes(), rel_dir: p.parent().map(Path::to_path_buf) })
            })
            .collect();
        self.list.set_entries(entries);
        self.regroup();
        self.invalidate();
    }

    /// Menu of the seven color tags, checked when every selected item has the tag. Item ids are 100 + tag index.
    fn tag_menu(&self) -> Option<HMENU> {
        let paths = self.selected_paths();
        let m = unsafe { CreatePopupMenu().ok()? };
        for (k, tag) in crate::tags::Tag::ALL.into_iter().enumerate() {
            let on = !paths.is_empty() && paths.iter().all(|p| self.tags.of(p).contains(&tag));
            let flags = MF_STRING | if on { MF_CHECKED } else { MF_UNCHECKED } | if paths.is_empty() { MF_GRAYED } else { MF_ENABLED };
            unsafe {
                let _ = AppendMenuW(m, flags, TAG_CMD + k, &windows::core::HSTRING::from(tag.name()));
            }
        }
        Some(m)
    }

    /// Applies a menu command from `tag_menu`.
    fn apply_tag_cmd(&mut self, cmd: usize) -> bool {
        let Some(tag) = cmd.checked_sub(TAG_CMD).and_then(|k| crate::tags::Tag::ALL.get(k).copied()) else { return false };
        let paths = self.selected_paths();
        self.tags.toggle(&paths, tag);
        let r = self.tags.save().map_err(windows::core::Error::from);
        self.report("Saving tags", r);
        self.invalidate();
        true
    }

    /// Opens the Windows share sheet for the selection.
    fn share_selection(&mut self) {
        let paths = self.selected_paths();
        if paths.is_empty() {
            self.error = Some("Select files to share.".into());
            self.invalidate();
            return;
        }
        if self.share.is_none() {
            match crate::share::Share::register(self.hwnd) {
                Ok(s) => self.share = Some(s),
                Err(e) => return self.report("Share", Err(e)),
            }
        }
        if let Some(s) = &self.share {
            let r = s.show(self.hwnd, paths);
            self.report("Share", r);
        }
    }

    fn tags_button(&mut self, r: &ToolbarRects) {
        let Some(at) = r.tags else { return };
        let mut pt = POINT { x: (at.x * self.scale) as i32, y: (at.bottom() * self.scale) as i32 };
        let Some(m) = self.tag_menu() else { return };
        let cmd = unsafe {
            let _ = windows::Win32::Graphics::Gdi::ClientToScreen(self.hwnd, &mut pt);
            let c = TrackPopupMenu(m, TPM_RETURNCMD, pt.x, pt.y, None, self.hwnd, None);
            let _ = DestroyMenu(m);
            c.0 as usize
        };
        self.apply_tag_cmd(cmd);
    }

    fn saved_view(&self, folder: &Path) -> Option<View> {
        match self.prefs.get(&folder.to_string_lossy())? {
            "icon" => Some(View::Icon),
            "list" => Some(View::List),
            "columns" => Some(View::Columns),
            "gallery" => Some(View::Gallery),
            _ => None,
        }
    }

    /// Changes (or with `None`, clears) a Files-only setting, saves it, and applies it.
    pub fn change_setting(&mut self, key: crate::settings::Key, value: Option<&str>) {
        crate::settings::set_override(&mut self.prefs, key, value);
        let r = self.prefs.save().map_err(windows::core::Error::from);
        self.report("Saving settings", r);
        self.apply_settings();
    }

    /// Re-resolves settings (after a change here or in Windows) and reloads when visibility changed.
    fn apply_settings(&mut self) {
        let old = self.settings;
        self.settings = crate::settings::Settings::resolve(self.win_defaults, &self.prefs);
        if (old.show_hidden, old.show_system) != (self.settings.show_hidden, self.settings.show_system) {
            self.reload_current();
        }
        self.invalidate();
    }

    pub fn windows_defaults(&self) -> crate::settings::WinDefaults {
        self.win_defaults
    }

    pub fn sheet_progress(&self) -> f32 {
        self.sheet_anim.map_or(1.0, |a| a.value(Instant::now()))
    }

    fn open_settings(&mut self) {
        self.settings_open = true;
        if self.animations {
            self.sheet_anim = Some(crate::layout::Anim::new(0.0, 1.0, Duration::from_millis(220), Instant::now()));
            self.start_anim_timer();
        }
        self.invalidate();
    }

    fn on_sheet(&mut self, a: views::settings::Action) {
        use crate::settings::{ClickMode, Key, PanelStyle};
        use views::settings::Action;
        let flip = |on: bool| Some(if on { "0" } else { "1" });
        match a {
            Action::Click(m) => self.change_setting(Key::Click, Some(if m == ClickMode::Single { "single" } else { "double" })),
            Action::Ext => self.change_setting(Key::Ext, flip(self.settings.show_ext)),
            Action::Hidden => self.change_setting(Key::Hidden, flip(self.settings.show_hidden)),
            Action::System => self.change_setting(Key::System, flip(self.settings.show_system)),
            Action::PathBar => self.toggle_pathbar(),
            Action::Panel(p) => {
                let v = match p {
                    PanelStyle::Pill => "pill",
                    PanelStyle::Card => "card",
                    PanelStyle::Stack => "stack",
                };
                self.change_setting(Key::Panel, Some(v));
            }
            Action::UseWindows => {
                crate::settings::reset_to_windows(&mut self.prefs);
                let _ = self.prefs.save();
                self.apply_settings();
            }
            Action::Done => self.settings_open = false,
        }
        self.invalidate();
    }

    /// Opens the item a single click landed on (single-click mode), unless the press turned into a drag.
    fn on_click_release(&mut self, x: f32, y: f32) {
        let Some((px, py, target, mods_held)) = self.press.take() else { return };
        let threshold = unsafe { GetSystemMetrics(SM_CXDRAG) } as f32 / self.scale;
        if !crate::settings::click_opens(self.settings.click, ((x - px).powi(2) + (y - py).powi(2)).sqrt(), threshold, mods_held) {
            return;
        }
        match target {
            PressTarget::Item(i) if self.item_at(x, y) == Some(Some(i)) => self.open_index(i),
            PressTarget::Column(c, r) if views::columns::hit(self, x, y) == Some((c, Some(r))) => {
                let col = &self.columns.cols[c];
                if let Some(e) = col.entries.as_ref().and_then(|v| v.get(r)).filter(|e| !e.is_dir()) {
                    let path = col.path.join(&e.name);
                    self.open_path(&path);
                }
            }
            _ => {}
        }
    }

    /// Builds the View, Sort by and Group by submenus (Explorer's right-click on empty space).
    fn view_sort_group_menus(&self, into: HMENU) {
        let radio = |on: bool| MF_STRING | if on { MF_CHECKED } else { MF_UNCHECKED };
        unsafe {
            if let Ok(v) = CreatePopupMenu() {
                let preset = views::grid::IconPreset::nearest(self.icon_size);
                for (k, p) in views::grid::IconPreset::ALL.into_iter().enumerate() {
                    let _ = AppendMenuW(v, radio(self.view == View::Icon && p == preset), VIEW_CMD + k, &windows::core::HSTRING::from(p.label()));
                }
                let _ = AppendMenuW(v, MF_SEPARATOR, 0, None);
                for (k, (view, label)) in [(View::List, "Details"), (View::Columns, "Columns"), (View::Gallery, "Gallery")].into_iter().enumerate() {
                    let _ = AppendMenuW(v, radio(self.view == view), VIEW_CMD + 4 + k, &windows::core::HSTRING::from(label));
                }
                let _ = AppendMenuW(into, MF_POPUP, v.0 as usize, w!("View"));
            }
            if let Ok(s) = CreatePopupMenu() {
                for (k, (key, label)) in SORT_KEYS.into_iter().enumerate() {
                    let _ = AppendMenuW(s, radio(self.list.sort.0 == key), SORT_CMD + k, &windows::core::HSTRING::from(label));
                }
                let _ = AppendMenuW(s, MF_SEPARATOR, 0, None);
                let _ = AppendMenuW(s, radio(!self.list.sort.1), SORT_CMD + 10, w!("Ascending"));
                let _ = AppendMenuW(s, radio(self.list.sort.1), SORT_CMD + 11, w!("Descending"));
                let _ = AppendMenuW(into, MF_POPUP, s.0 as usize, w!("Sort by"));
            }
            if let Ok(g) = CreatePopupMenu() {
                for (k, (gb, label)) in GROUPS.into_iter().enumerate() {
                    let _ = AppendMenuW(g, radio(self.group_by == gb), GROUP_CMD + k, &windows::core::HSTRING::from(label));
                }
                let _ = AppendMenuW(into, MF_POPUP, g.0 as usize, w!("Group by"));
            }
        }
    }

    /// Applies a View/Sort/Group menu command; false when `cmd` is not one of them.
    fn apply_view_cmd(&mut self, cmd: usize) -> bool {
        match cmd {
            c if (VIEW_CMD..VIEW_CMD + 4).contains(&c) => {
                self.set_view(View::Icon);
                self.icon_size = views::grid::IconPreset::ALL[c - VIEW_CMD].size();
                if let Some(f) = self.list.sel.focus() {
                    self.reveal(f);
                }
                self.list.scroll_y = self.list.scroll_y.min(self.max_scroll());
            }
            c if c == VIEW_CMD + 4 => self.set_view(View::List),
            c if c == VIEW_CMD + 5 => self.set_view(View::Columns),
            c if c == VIEW_CMD + 6 => self.set_view(View::Gallery),
            c if (SORT_CMD..SORT_CMD + SORT_KEYS.len()).contains(&c) || c == SORT_CMD + 10 || c == SORT_CMD + 11 => {
                let (mut key, mut desc) = self.list.sort;
                match c - SORT_CMD {
                    10 => desc = false,
                    11 => desc = true,
                    k => key = SORT_KEYS[k].0,
                }
                let keep = self.list.selection_keys();
                self.list.sort = (key, desc);
                self.list.resort();
                self.regroup();
                self.list.restore_selection(&keep);
            }
            c if (GROUP_CMD..GROUP_CMD + GROUPS.len()).contains(&c) => {
                self.group_by = GROUPS[c - GROUP_CMD].0;
                self.list.sel.clear();
                self.regroup();
                self.list.scroll_y = 0.0;
            }
            _ => return false,
        }
        self.invalidate();
        true
    }

    fn toggle_pathbar(&mut self) {
        self.show_pathbar = !self.show_pathbar;
        self.prefs.set("@pathbar", if self.show_pathbar { "1" } else { "0" });
        let _ = self.prefs.save();
        self.relayout();
        self.invalidate();
    }

    pub fn grid(&self) -> views::grid::Grid {
        views::grid::Grid::new(self.layout.content.w, self.icon_size).grouped(self.groups.clone())
    }

    /// Row geometry of the list view (one column, with group headers when grouping).
    pub fn list_grid(&self) -> views::grid::Grid {
        views::grid::Grid::list(self.layout.rows.w).grouped(self.groups.clone())
    }

    /// Re-sorts entries into groups for `group_by`. Call after the entries or the sort change.
    fn regroup(&mut self) {
        if self.group_by == GroupBy::None {
            self.groups.clear();
            return;
        }
        let today = shell::now_local();
        let by = self.group_by;
        let names = &mut self.type_names;
        let mut v = std::mem::take(&mut self.list.entries);
        self.groups = crate::listing::group_entries(&mut v, |e| match by {
            // Folders first, then kinds alphabetically.
            GroupBy::Kind => (if e.is_dir() { -1 } else { 0 }, shell::type_name(e, names)),
            GroupBy::Date => crate::format::date_group(shell::to_local(e.modified), today),
            GroupBy::Size => {
                let (rank, label) = crate::format::size_group(e.is_dir(), e.size);
                (rank, label.to_string())
            }
            GroupBy::Name => {
                let c = e.name.to_string_lossy().chars().next().map_or('#', |c| c.to_ascii_uppercase());
                (0, if c.is_ascii_alphabetic() { c.to_string() } else { "#".to_string() })
            }
            GroupBy::None => (0, String::new()),
        });
        self.list.entries = v;
    }

    fn group_menu(&mut self, r: &ToolbarRects) {
        let Some(at) = r.group else { return };
        let mut pt = POINT { x: (at.x * self.scale) as i32, y: (at.bottom() * self.scale) as i32 };
        let choices = [(GroupBy::None, w!("None")), (GroupBy::Kind, w!("Kind")), (GroupBy::Date, w!("Date Modified")), (GroupBy::Size, w!("Size")), (GroupBy::Name, w!("Name"))];
        let cmd = unsafe {
            let _ = windows::Win32::Graphics::Gdi::ClientToScreen(self.hwnd, &mut pt);
            let Ok(m) = CreatePopupMenu() else { return };
            for (i, (g, text)) in choices.iter().enumerate() {
                let checked = if *g == self.group_by { MF_CHECKED } else { MF_UNCHECKED };
                let _ = AppendMenuW(m, MF_STRING | checked, i + 1, *text);
            }
            let cmd = TrackPopupMenu(m, TPM_RETURNCMD, pt.x, pt.y, None, self.hwnd, None);
            let _ = DestroyMenu(m);
            cmd.0 as usize
        };
        if let Some((g, _)) = cmd.checked_sub(1).and_then(|i| choices.get(i)) {
            self.group_by = *g;
            self.list.sel.clear();
            self.regroup();
            self.list.scroll_y = 0.0;
            self.invalidate();
        }
    }

    fn zoom_icons(&mut self, delta: f32) {
        self.icon_size = views::grid::clamp_icon(self.icon_size + delta);
        if let Some(f) = self.list.sel.focus() {
            self.reveal(f);
        }
        self.list.scroll_y = self.list.scroll_y.min(self.max_scroll());
        self.invalidate();
    }

    /// Area holding the items of the list or icon view.
    fn item_area(&self) -> crate::layout::Rect {
        match self.view {
            View::Icon => self.layout.content,
            View::Gallery => views::gallery::areas(self.layout.content).1,
            _ => self.layout.rows,
        }
    }

    /// `None` outside the item area; `Some(None)` on empty space inside it.
    fn item_at(&self, x: f32, y: f32) -> Option<Option<usize>> {
        let a = self.item_area();
        if self.columns_view() || !a.contains(x, y) {
            return None;
        }
        Some(match self.view {
            View::Icon => self.grid().cell_at(x - a.x, y - a.y, self.list.scroll_y, self.list.entries.len()),
            View::Gallery => views::gallery::strip_at(x - a.x, self.gallery_scroll, self.list.entries.len()).filter(|_| y - a.y >= 12.0),
            _ => self.list_grid().cell_at(x - a.x, y - a.y, self.list.scroll_y, self.list.entries.len()),
        })
    }

    /// Where an item's name is drawn (inline rename and keyboard context menus anchor here).
    fn name_rect(&self, i: usize) -> crate::layout::Rect {
        let a = self.item_area();
        if self.view == View::Gallery {
            let r = views::gallery::strip_item(i, self.gallery_scroll);
            return crate::layout::Rect::new(a.x + r.x, a.y + 12.0, r.w, r.h);
        }
        if self.view == View::Icon {
            let g = self.grid();
            let c = g.cell(i);
            return crate::layout::Rect::new(a.x + c.x + 4.0, a.y + c.y - self.list.scroll_y + 16.0 + g.icon, c.w - 8.0, 20.0);
        }
        let name_w = views::list::columns(a.w, self.list.show_where)[0].2;
        let y = a.y + self.list_grid().cell(i).y - self.list.scroll_y;
        crate::layout::Rect::new(a.x + 32.0, y, name_w - 40.0, crate::theme::ROW_H)
    }

    fn max_scroll(&self) -> f32 {
        let h = self.item_area().h;
        match self.view {
            View::Icon => (self.grid().content_h(self.list.entries.len()) - h).max(0.0),
            View::Gallery => 0.0,
            _ => (self.list_grid().content_h(self.list.entries.len()) - h).max(0.0),
        }
    }

    /// Scrolls so item `i` is fully visible.
    fn reveal(&mut self, i: usize) {
        let h = self.item_area().h;
        match self.view {
            View::Icon => self.list.scroll_y = self.grid().scroll_to_show(i, self.list.scroll_y, h),
            View::Gallery => self.gallery_scroll = views::gallery::strip_scroll_to_show(i, self.gallery_scroll, self.item_area().w),
            _ => self.list.scroll_y = self.list_grid().scroll_to_show(i, self.list.scroll_y, h),
        }
    }

    fn list_wheel(&mut self, notches: f32) {
        let cur = self.list.scroll_y;
        // Repeated notches extend the running animation's target instead of restarting from the current frame.
        let base = self.scroll_anim.map_or(cur, |a| a.to);
        let target = (base - notches * 3.0 * crate::theme::ROW_H).clamp(0.0, self.max_scroll());
        self.list.scroll_y = target;
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
    /// Search shows as a field while focused, typed in, or showing results; otherwise as an icon.
    pub fn search_open(&self) -> bool {
        self.searching || edit::visible(self.search_edit)
    }

    pub fn toolbar(&self) -> ToolbarRects {
        toolbar_rects(&self.layout, self.search_open())
    }

    /// Active segment of the view control: Icon, List, Columns, Gallery.
    pub fn view_index(&self) -> usize {
        match self.view {
            View::Icon => 0,
            View::List => 1,
            View::Columns => 2,
            View::Gallery => 3,
        }
    }

    /// "Where" column text: the result's folder, starting with the searched folder's name.
    pub fn where_label(&self, e: &Entry) -> String {
        if self.tag_view.is_some() {
            return e.rel_dir.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
        }
        let root = folder_title(&self.search_root, &self.sidebar.drives);
        match e.rel_dir.as_deref().filter(|p| !p.as_os_str().is_empty()) {
            Some(rel) => format!("{root}\\{}", rel.display()),
            None => root,
        }
    }

    fn search_field_rect(&self) -> crate::layout::Rect {
        let r = toolbar_rects(&self.layout, true).search;
        crate::layout::Rect::new(r.x + 28.0, r.y, r.w - 36.0, r.h)
    }

    fn focus_search(&mut self) {
        edit::set_cue(self.search_edit, &format!("Search in {}", self.title()));
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
            { let s = self.settings; move |a| s.visible(a) },
            move |batch| h1.post(WM_SEARCH_BATCH, Box::new((generation, batch))),
            move |stats| h2.post(WM_SEARCH_DONE, Box::new((generation, stats))),
        ));
        self.invalidate();
    }

    fn on_search_batch(&mut self, generation: u64, mut batch: Vec<Entry>) {
        if !self.searching || !self.search_gen.is_current(generation) {
            return;
        }
        // New rows reorder the list; keep the user's selection on the same files (a Delete must hit what they chose).
        let keep = self.list.selection_keys();
        self.list.entries.append(&mut batch);
        // Keep results sorted while the list is small; past that, sort once when the walk ends.
        if self.list.entries.len() < 50_000 {
            self.list.resort();
        }
        self.regroup();
        self.list.restore_selection(&keep);
        self.invalidate();
    }

    fn on_search_done(&mut self, generation: u64, stats: search::SearchStats) {
        if stats.cancelled || !self.searching || !self.search_gen.is_current(generation) {
            return;
        }
        self.search_running = false;
        self.search_skipped = stats.skipped_denied;
        let keep = self.list.selection_keys();
        if self.list.entries.len() >= 50_000 {
            self.list.resort();
        }
        self.regroup();
        self.list.restore_selection(&keep);
        self.invalidate();
    }

    /// Leaves search mode. `reload` restores the folder listing and the view used before searching.
    fn end_search(&mut self, reload: bool) {
        self.tag_view = None;
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
        let r = self.toolbar().title;
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
            let r = self.toolbar().title;
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
        let (hwnd, settings) = (self.send_hwnd(), self.settings);
        self.loading = true;
        if let Some(t) = &self.thumbnailer {
            t.new_generation();
        }
        self.thumb_pending.clear();
        self.thumb_exts.reset();
        std::thread::spawn(move || {
            let result = read_folder(&path, |a| settings.visible(a));
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
                let here = self.history.current().to_path_buf();
                if m.kind != NavKind::Refresh
                    && let Some(v) = self.saved_view(&here)
                {
                    self.view = v;
                }
                let mut entries = m.result.unwrap_or_default();
                let recents = self.sidebar.favorites.iter().any(|p| p.name == "Recents" && p.path == here);
                if recents {
                    // Windows keeps jump-list data folders in Recent; Explorer hides them, so do we.
                    entries.retain(|e| !(e.is_dir() && (e.name == "AutomaticDestinations" || e.name == "CustomDestinations")));
                    self.list.sort = (crate::listing::SortKey::Modified, true);
                    self.recents_sort = true;
                } else if std::mem::take(&mut self.recents_sort) {
                    self.list.sort = (crate::listing::SortKey::Name, false);
                }
                self.list.set_entries(entries);
                self.regroup();
                if self.view == View::Gallery && !self.list.entries.is_empty() {
                    self.list.sel.click(0);
                }
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
        if cut {
            // A moved file is gone from its source; a second paste must not try again.
            shell::clipboard_clear(Some(self.hwnd));
        }
        let dest = self.current_dir().to_path_buf();
        self.start_transfer(if cut { Op::Move } else { Op::Copy }, items, Some(dest));
    }

    fn delete_selection(&mut self, permanent: bool) {
        let paths = self.selected_paths();
        if !paths.is_empty() {
            self.start_transfer(if permanent { Op::Delete } else { Op::Recycle }, paths, None);
        }
    }

    /// Starts a copy, move, recycle or delete in the custom transfer UI. Conflicts and permanent deletes ask first.
    pub fn start_transfer(&mut self, op: Op, items: Vec<PathBuf>, dest: Option<PathBuf>) {
        let shown = Instant::now();
        if op == Op::Delete {
            self.xfer.prompt = Some(Prompt::ConfirmDelete { items, shown });
        } else if let Some(d) = dest.as_ref().filter(|_| matches!(op, Op::Copy | Op::Move)) {
            let conflicts = crate::transfer::find_conflicts(&items, d, |p| std::fs::symlink_metadata(p).is_ok());
            if conflicts.is_empty() {
                return self.run_transfer(op, items, dest, &HashMap::new());
            }
            self.xfer.prompt = Some(Prompt::Conflict { op, items, dest: d.clone(), conflicts, idx: 0, answers: HashMap::new(), apply_all: false, shown });
        } else {
            return self.run_transfer(op, items, dest, &HashMap::new());
        }
        self.start_anim_timer();
        self.invalidate();
    }

    fn run_transfer(&mut self, op: Op, items: Vec<PathBuf>, dest: Option<PathBuf>, answers: &HashMap<PathBuf, crate::transfer::Choice>) {
        let job = match crate::transfer::build_job(op, &items, dest.as_deref(), answers) {
            Ok(j) if !j.is_empty() => j,
            Ok(_) => return,
            Err(e) => {
                self.error = Some(e);
                return self.invalidate();
            }
        };
        let now = Instant::now();
        self.launch_flights(op, &items);
        if op == Op::Recycle {
            match self.xfer.bin.as_mut() {
                Some(b) => b.landed = None,
                None => self.xfer.bin = Some(views::transfer::Bin { shown: now, landed: None }),
            }
        }
        let letter = |p: &Path| p.to_string_lossy().get(..2).map(str::to_uppercase);
        let drives = dest.as_deref().and_then(|d| {
            let (a, b) = (letter(items.first()?)?, letter(d)?);
            (a != b).then_some((a, b))
        });
        self.xfer.next_id += 1;
        let id = self.xfer.next_id;
        let cancel = Arc::new(AtomicBool::new(false));
        self.xfer.rows.push(views::transfer::Row {
            id,
            op,
            dest_name: dest.as_deref().map(|d| folder_title(d, &self.sidebar.drives)).unwrap_or_default(),
            progress: crate::transfer::Progress::new(job.count(), 0),
            current: String::new(),
            started: now,
            failed: Vec::new(),
            done: None,
            cancel: cancel.clone(),
            drives,
            items,
            dest,
            shake_at: None,
            flipped_at: None,
        });
        let send = self.send_hwnd();
        std::thread::spawn(move || crate::transfer::run(job, cancel, move |u| send.post(WM_TRANSFER, Box::new((id, u)))));
        self.start_anim_timer();
        self.invalidate();
    }

    /// On-screen center of item `i`'s icon, when it is visible in the list or icon view.
    fn item_point(&self, i: usize) -> Option<(f32, f32)> {
        let a = self.item_area();
        let (x, y) = match self.view {
            View::Icon => {
                let g = self.grid();
                let c = g.cell(i);
                (a.x + c.x + c.w / 2.0, a.y + c.y - self.list.scroll_y + self.icon_size / 2.0 + 8.0)
            }
            View::List => {
                let c = self.list_grid().cell(i);
                (a.x + 28.0, a.y + c.y - self.list.scroll_y + c.h / 2.0)
            }
            _ => return None,
        };
        a.contains(x, y).then_some((x, y))
    }

    /// Selected items lift off and arc to the Recycle Bin (recycle) or drop into the folder (paste).
    fn launch_flights(&mut self, op: Op, items: &[PathBuf]) {
        let content = self.layout.content;
        let start = Instant::now();
        let mut from: Vec<(PathBuf, (f32, f32))> = Vec::new();
        if op == Op::Recycle && !self.columns_view() {
            for i in self.list.sel.selected() {
                if let (Some(e), Some(pt)) = (self.list.entries.get(i), self.item_point(i)) {
                    from.push((self.entry_path(e), pt));
                }
            }
        }
        let to = match op {
            Op::Recycle => views::transfer::bin_mouth(content),
            _ => (content.x + content.w / 2.0, content.y + content.h / 2.0),
        };
        if from.is_empty() && op != Op::Delete {
            // Nothing visible to lift: items come from the panel at the bottom.
            let origin = (content.x + content.w / 2.0, content.bottom() - 40.0);
            from = items.iter().take(6).map(|p| (p.clone(), if op == Op::Recycle { (content.x + content.w / 2.0, content.y + content.h / 2.0) } else { origin })).collect();
        }
        for (k, (path, pt)) in from.into_iter().take(12).enumerate() {
            let is_dir = path.is_dir();
            self.xfer.flights.push(views::transfer::Flight { path, is_dir, from: pt, to, start, delay: 0.12 + k as f32 * 0.06, dur: 0.55 });
        }
    }

    fn on_transfer(&mut self, id: u64, u: crate::transfer::Update) {
        use crate::transfer::Update;
        let now = Instant::now();
        let Some(row) = self.xfer.rows.iter_mut().find(|r| r.id == id) else { return };
        match u {
            Update::Started { items, bytes } => row.progress = crate::transfer::Progress::new(items, bytes),
            Update::Progress { fraction, current } => {
                row.progress.sample(now.duration_since(row.started).as_secs_f64(), fraction);
                if !current.is_empty() && current != row.current {
                    row.current = current;
                    row.flipped_at = Some(now);
                }
            }
            Update::Failed { path, message } => {
                row.failed.push((path, message));
                row.shake_at = Some(now);
            }
            Update::Done { cancelled } => {
                row.done = Some((now, cancelled));
                let (op, items, dest) = (row.op, row.items.clone(), row.dest.clone());
                self.finish_transfer(op, &items, dest.as_deref(), cancelled);
            }
        }
        self.start_anim_timer();
        self.invalidate();
    }

    /// Keeps tags in step with moved or deleted files and reloads what is on screen.
    fn finish_transfer(&mut self, op: Op, items: &[PathBuf], dest: Option<&Path>, cancelled: bool) {
        let gone: Vec<&PathBuf> = items.iter().filter(|p| std::fs::symlink_metadata(p).is_err()).collect();
        match (op, dest) {
            (Op::Move, Some(d)) => {
                for p in &gone {
                    if let Some(name) = p.file_name().filter(|n| d.join(n).exists()) {
                        self.tags.renamed(p, &d.join(name));
                    }
                }
            }
            (Op::Recycle | Op::Delete, _) => gone.iter().for_each(|p| self.tags.deleted(p)),
            _ => {}
        }
        let _ = self.tags.save();
        if op == Op::Recycle && !cancelled && !gone.is_empty() {
            let n = gone.len();
            let text = if n == 1 { format!("Moved “{}” to Recycle Bin", gone[0].file_name().unwrap_or_default().to_string_lossy()) } else { format!("Moved {n} items to Recycle Bin") };
            self.xfer.toast = Some(views::transfer::Toast { text, shown: Instant::now(), undo: Vec::new() });
        }
        if self.searching {
            // Keep the results; drop rows whose file is gone.
            let gone: Vec<bool> = self.list.entries.iter().map(|e| std::fs::symlink_metadata(self.entry_path(e)).is_err()).collect();
            let mut it = gone.into_iter();
            self.list.entries.retain(|_| !it.next().unwrap_or(false));
            self.regroup();
            self.list.sel.clear();
            self.invalidate();
        }
        self.reload_current();
    }

    fn on_transfer_hit(&mut self, h: views::transfer::Hit) {
        use views::transfer::{Hit, PromptButton as B};
        match h {
            Hit::Cancel(id) => {
                if let Some(r) = self.xfer.rows.iter().find(|r| r.id == id) {
                    r.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            }
            Hit::Retry(id) | Hit::Skip(id) => {
                let Some(r) = self.xfer.rows.iter_mut().find(|r| r.id == id) else { return };
                let failed: Vec<PathBuf> = std::mem::take(&mut r.failed).into_iter().map(|f| f.0).collect();
                let (op, dest) = (r.op, r.dest.clone());
                if r.done.is_some() {
                    r.done = Some((Instant::now() - Duration::from_secs(10), false)); // expire at once
                }
                if matches!(h, Hit::Retry(_)) {
                    let retry: Vec<PathBuf> = failed.into_iter().filter(|p| std::fs::symlink_metadata(p).is_ok()).collect();
                    if !retry.is_empty() {
                        self.start_transfer(op, retry, dest);
                    }
                }
            }
            Hit::Prompt(b) => match self.xfer.prompt.take() {
                Some(Prompt::ConfirmDelete { items, .. }) => {
                    if b == B::Delete {
                        self.run_transfer(Op::Delete, items, None, &HashMap::new());
                    }
                }
                Some(Prompt::Conflict { op, items, dest, conflicts, mut idx, mut answers, mut apply_all, shown }) => {
                    let choice = match b {
                        B::Skip => Some(crate::transfer::Choice::Skip),
                        B::KeepBoth => Some(crate::transfer::Choice::KeepBoth),
                        B::Replace => Some(crate::transfer::Choice::Replace),
                        _ => None,
                    };
                    match choice {
                        None => apply_all = !apply_all,
                        Some(c) => {
                            let end = if apply_all { conflicts.len() } else { idx + 1 };
                            for p in &conflicts[idx..end] {
                                answers.insert(p.clone(), c);
                            }
                            idx = end;
                        }
                    }
                    if idx >= conflicts.len() {
                        self.run_transfer(op, items, Some(dest), &answers);
                    } else {
                        self.xfer.prompt = Some(Prompt::Conflict { op, items, dest, conflicts, idx, answers, apply_all, shown });
                    }
                }
                None => {}
            },
            Hit::PromptCard | Hit::Row | Hit::Undo => {}
        }
        self.start_anim_timer();
        self.invalidate();
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
        if self.columns_view() || self.view == View::Gallery {
            return;
        }
        let Some(i) = self.list.sel.focus().filter(|&i| i < self.list.entries.len()) else { return };
        self.reveal(i);
        let r = self.name_rect(i);
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
            if let Some(parent) = path.parent() {
                self.tags.renamed(&path, &parent.join(&new_name));
                let _ = self.tags.save();
            }
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
            self.reveal(i);
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
            let a = self.item_area();
            let (x, y) = self.list.sel.focus().map_or((a.x + 40.0, a.y), |i| {
                let r = self.name_rect(i);
                (r.x + 8.0, r.y + r.h / 2.0)
            });
            screen = POINT { x: (x * self.scale) as i32, y: (y * self.scale) as i32 };
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
            } else if let Some(hit) = self.item_at(x, y) {
                if let Some(i) = hit {
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
        const PATHBAR: usize = 10;
        const SETTINGS: usize = 11;
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
                let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
                if let Some(sub) = self.tag_menu() {
                    let _ = AppendMenuW(m, MF_POPUP, sub.0 as usize, w!("Tags"));
                }
            } else {
                self.view_sort_group_menus(m);
                let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
                add(PASTE, w!("Paste\tCtrl+V"), if can_paste && !self.searching { MF_ENABLED } else { MF_GRAYED });
                add(NEW_FOLDER, w!("New Folder\tCtrl+Shift+N"), if self.searching { MF_GRAYED } else { MF_ENABLED });
                let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
                add(HIDDEN, w!("Show Hidden Files"), if self.settings.show_hidden { MF_CHECKED } else { MF_UNCHECKED });
                add(PATHBAR, w!("Show Path Bar\tCtrl+Alt+P"), if self.show_pathbar { MF_CHECKED } else { MF_UNCHECKED });
                let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
                add(SETTINGS, w!("Settings…\tCtrl+,"), MF_ENABLED);
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
                let v = if self.settings.show_hidden { "0" } else { "1" };
                self.change_setting(crate::settings::Key::Hidden, Some(v));
                self.reload_current();
            }
            PATHBAR => self.toggle_pathbar(),
            SETTINGS => self.open_settings(),
            c => {
                if !self.apply_view_cmd(c) {
                    self.apply_tag_cmd(c);
                }
            }
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
            ListAction::Redraw => {
                if let Some(f) = self.list.sel.focus() {
                    self.reveal(f);
                }
                self.invalidate();
            }
            ListAction::None => {}
        }
    }

    fn relayout(&mut self) {
        self.layout = compute_layout(self.w, self.h, self.sidebar_w, 1.0);
        if !self.show_pathbar {
            layout::hide_pathbar(&mut self.layout, self.h);
        }
        self.list.scroll_y = self.list.scroll_y.min(self.max_scroll());
    }

    fn apply_theme_to_frame(&self) {
        let dark = BOOL::from(self.theme.dark);
        unsafe {
            let _ = DwmSetWindowAttribute(self.hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as _, 4);
        }
    }

    fn hot_at(&self, x: f32, y: f32) -> Hot {
        if self.layout.splitter.contains(x, y) { Hot::Splitter } else { self.toolbar().hit(x, y) }
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
        if let Some(h) = views::transfer::hit(self, x, y) {
            return self.on_transfer_hit(h);
        }
        if self.settings_open {
            let window = crate::layout::Rect::new(0.0, 0.0, self.w, self.h);
            match views::settings::sheet_hit(window, x, y) {
                Some(a) => self.on_sheet(a),
                None if !views::settings::sheet_layout(window).0.contains(x, y) => {
                    self.settings_open = false;
                    self.invalidate();
                }
                None => {}
            }
            return;
        }
        let l = self.layout;
        let r = self.toolbar();
        let tool = r.hit(x, y);
        if l.splitter.contains(x, y) {
            self.dragging_splitter = true;
            unsafe { SetCapture(self.hwnd) };
        } else if tool != Hot::None {
            self.on_toolbar(tool, &r);
        } else if let Some(s) = self.sidebar.header_at(x, y, l.sidebar) {
            self.sidebar.collapsed[s] = !self.sidebar.collapsed[s];
            self.invalidate();
        } else if let Some(i) = self.sidebar.hit(x, y, l.sidebar) {
            if let Some(tag) = self.sidebar.tag_at(i) {
                self.show_tag(tag);
            } else if let Some(p) = self.sidebar.place(i) {
                self.navigate(p.path.clone(), NavKind::Visit);
            }
        } else if let Some(p) = pathbar::hit(self, x, y) {
            self.navigate(p, NavKind::Visit);
        } else if self.columns_view() {
            if let Some((col, row)) = views::columns::hit(self, x, y) {
                match row {
                    Some(row) => {
                        self.column_select(col, row);
                        self.press = Some((x, y, PressTarget::Column(col, row), false));
                    }
                    None => {
                        self.columns.focus_col = col;
                        self.invalidate();
                    }
                }
            }
        } else if self.view == View::List && l.header.contains(x, y) {
            if let Some(c) = self.list.col_at(x - l.header.x, l.header.w) {
                self.list.click_header(c);
                self.regroup();
                self.invalidate();
            }
        } else if self.view == View::Icon && self.item_at(x, y) == Some(None) {
            // Empty space in icon view starts a drag box.
            let m = mods();
            if !m.ctrl {
                self.list.sel.clear();
            }
            let a = self.item_area();
            let (gx, gy) = (x - a.x, y - a.y + self.list.scroll_y);
            self.band = Some((gx, gy, self.list.sel.selected(), gx, gy));
            unsafe { SetCapture(self.hwnd) };
            self.invalidate();
        } else if let Some(hit) = self.item_at(x, y) {
            match hit {
                Some(i) => {
                    let m = mods();
                    self.list.click_row(i, m);
                    self.press = Some((x, y, PressTarget::Item(i), m.ctrl || m.shift));
                }
                None => self.list.sel.clear(),
            }
            self.invalidate();
        }
    }

    fn on_toolbar(&mut self, tool: Hot, r: &ToolbarRects) {
        match tool {
            Hot::Back => self.go_back(),
            Hot::Forward => self.go_forward(),
            Hot::View(0) => self.set_view(View::Icon),
            Hot::View(1) => self.set_view(View::List),
            Hot::View(2) => self.set_view(View::Columns),
            Hot::View(3) => self.set_view(View::Gallery),
            Hot::Search => self.focus_search(),
            Hot::Group => self.group_menu(r),
            Hot::Tags => self.tags_button(r),
            Hot::Share => self.share_selection(),
            Hot::Action => {
                // Same menu as right-click, anchored under the button.
                let mut pt = POINT { x: (r.action.x * self.scale) as i32, y: (r.action.bottom() * self.scale) as i32 };
                unsafe {
                    let _ = windows::Win32::Graphics::Gdi::ClientToScreen(self.hwnd, &mut pt);
                }
                let on_item = !self.selected_paths().is_empty();
                self.context_menu(pt, on_item);
            }
            _ => {}
        }
    }

    fn on_key(&mut self, vk: u16) -> bool {
        match vk {
            v if v == VK_F5.0 => self.refresh(),
            v if v == VK_BACK.0 => self.go_back(),
            0x4C if mods().ctrl => self.show_address(), // Ctrl+L
            0x46 if mods().ctrl => self.focus_search(), // Ctrl+F
            0x50 if mods().ctrl && key_down(VK_MENU.0) => self.toggle_pathbar(), // Ctrl+Alt+P
            0x43 if mods().ctrl => self.copy_selection(false), // Ctrl+C
            0x58 if mods().ctrl => self.copy_selection(true),  // Ctrl+X
            0x56 if mods().ctrl => self.paste_here(),           // Ctrl+V
            0x4E if mods().ctrl && mods().shift => self.create_folder(), // Ctrl+Shift+N
            v if v == VK_F2.0 => self.start_rename(),
            v if v == VK_DELETE.0 => self.delete_selection(mods().shift),
            v if v == VK_ESCAPE.0 && self.xfer.prompt.is_some() => {
                self.xfer.prompt = None;
                self.invalidate();
            }
            v if v == VK_ESCAPE.0 && self.settings_open => {
                self.settings_open = false;
                self.invalidate();
            }
            0xBC if mods().ctrl => self.open_settings(), // Ctrl+,
            v if v == VK_ESCAPE.0 && self.searching => {
                self.clear_search_field();
                self.end_search(true);
            }
            0x31..=0x34 if mods().ctrl && mods().shift => {
                // Ctrl+Shift+1..4: extra large, large, medium, small icons (Explorer's shortcuts).
                self.apply_view_cmd(VIEW_CMD + (vk - 0x31) as usize);
            }
            0x36 if mods().ctrl && mods().shift => self.set_view(View::List), // Ctrl+Shift+6: details
            0x31 if mods().ctrl => self.set_view(View::Icon), // Ctrl+1
            0x32 if mods().ctrl => self.set_view(View::List), // Ctrl+2
            0x33 if mods().ctrl => self.set_view(View::Columns), // Ctrl+3
            0x34 if mods().ctrl => self.set_view(View::Gallery), // Ctrl+4
            0xBB | 0x6B if mods().ctrl && self.view == View::Icon => self.zoom_icons(16.0), // Ctrl+Plus
            0xBD | 0x6D if mods().ctrl && self.view == View::Icon => self.zoom_icons(-16.0), // Ctrl+Minus
            v if self.view == View::Gallery && [VK_LEFT.0, VK_RIGHT.0, VK_UP.0, VK_DOWN.0].contains(&v) => {
                let n = self.list.entries.len();
                if n > 0 {
                    let cur = self.list.sel.focus();
                    let next = if v == VK_LEFT.0 || v == VK_UP.0 { cur.map_or(0, |c| c.saturating_sub(1)) } else { cur.map_or(0, |c| (c + 1).min(n - 1)) };
                    self.list.sel.click(next);
                    self.reveal(next);
                    self.invalidate();
                }
            }
            v if self.view == View::Icon && [VK_LEFT.0, VK_RIGHT.0, VK_UP.0, VK_DOWN.0].contains(&v) => {
                use views::grid::Dir;
                let d = match v {
                    v if v == VK_LEFT.0 => Dir::Left,
                    v if v == VK_RIGHT.0 => Dir::Right,
                    v if v == VK_UP.0 => Dir::Up,
                    _ => Dir::Down,
                };
                let n = self.list.entries.len();
                if n > 0 {
                    let next = self.grid().step(self.list.sel.focus(), d, n);
                    if mods().shift { self.list.sel.shift_click(next) } else { self.list.sel.click(next) }
                    self.reveal(next);
                    self.invalidate();
                }
            }
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
            WM_TIMER if wp.0 == ID_HOVER_SELECT => {
                unsafe {
                    let _ = KillTimer(Some(hwnd), ID_HOVER_SELECT);
                }
                if let Some(i) = self.hover_pending.take().filter(|&i| self.list.hover_row == Some(i) && !self.settings_open) {
                    self.list.sel.click(i);
                    self.invalidate();
                }
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
            WM_TRANSFER => {
                let m = unsafe { Box::from_raw(lp.0 as *mut (u64, crate::transfer::Update)) };
                let (id, u) = *m;
                self.on_transfer(id, u);
                Some(LRESULT(0))
            }
            WM_THUMB_DONE => {
                let d = unsafe { Box::from_raw(lp.0 as *mut crate::thumbs::ThumbDone) };
                self.on_thumb(*d);
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
                let inter = self.toolbar().interactive();
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
                if self.band.is_some() {
                    self.update_band(x, y);
                    return Some(LRESULT(0));
                }
                if self.dragging_splitter {
                    self.sidebar_w = x;
                    self.relayout();
                    self.invalidate();
                    return Some(LRESULT(0));
                }
                self.track_leave(false);
                self.set_hot(self.hot_at(x, y));
                let hover = self.item_at(x, y).flatten();
                if self.settings.click == crate::settings::ClickMode::Single && hover != self.list.hover_row {
                    self.hover_pending = hover;
                    if hover.is_some() && self.press.is_none() {
                        let mut ms = 400u32;
                        unsafe {
                            let _ = SystemParametersInfoW(SPI_GETMOUSEHOVERTIME, 0, Some(&mut ms as *mut u32 as _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
                            SetTimer(Some(hwnd), ID_HOVER_SELECT, ms, None);
                        }
                    }
                }
                let side = self.sidebar.hit(x, y, self.layout.sidebar);
                let header = self.sidebar.header_at(x, y, self.layout.sidebar);
                if header != self.sidebar.hover_header {
                    self.sidebar.hover_header = header;
                    self.invalidate();
                }
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
                let on_item = self.item_at(x, y).flatten().is_some() || (self.columns_view() && self.layout.content.contains(x, y));
                if self.settings.click == crate::settings::ClickMode::Single && on_item && !self.settings_open {
                    return Some(LRESULT(0)); // the first click already opened it
                }
                if self.settings_open {
                    self.on_left_down(x, y);
                } else if self.columns_view() && self.layout.content.contains(x, y) {
                    // Folders already opened on the first click; a double-click opens files.
                    if let Some((col, Some(row))) = views::columns::hit(self, x, y) {
                        let c = &self.columns.cols[col];
                        if let Some(e) = c.entries.as_ref().and_then(|v| v.get(row)).filter(|e| !e.is_dir()) {
                            let path = c.path.join(&e.name);
                            self.open_path(&path);
                        }
                    }
                } else if let Some(Some(i)) = self.item_at(x, y) {
                    self.open_index(i);
                } else if self.toolbar().title.contains(x, y) {
                    self.show_address();
                } else {
                    self.on_left_down(x, y);
                }
                Some(LRESULT(0))
            }
            WM_LBUTTONUP => {
                let (ux, uy) = self.client_dip(lp);
                if self.band.is_none() && !self.dragging_splitter {
                    self.on_click_release(ux, uy);
                }
                self.press = None;
                if self.band.take().is_some() {
                    unsafe {
                        let _ = ReleaseCapture();
                    }
                    self.invalidate();
                }
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
                if self.view == View::Gallery {
                    let n = self.list.entries.len() as f32;
                    let max = (views::gallery::STRIP_PAD * 2.0 + n * (views::gallery::THUMB + views::gallery::GAP) - self.item_area().w).max(0.0);
                    self.gallery_scroll = (self.gallery_scroll - notches * 3.0 * views::gallery::THUMB).clamp(0.0, max);
                } else if self.columns_view() {
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
                mmi.ptMinTrackSize = POINT { x: (crate::views::toolbar::MIN_WINDOW_W * self.scale) as i32, y: (400.0 * self.scale) as i32 };
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
                let fresh = crate::settings::read_windows();
                if fresh != self.win_defaults {
                    self.win_defaults = fresh;
                    self.apply_settings();
                }
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

fn view_name(v: View) -> &'static str {
    match v {
        View::Icon => "icon",
        View::List => "list",
        View::Columns => "columns",
        View::Gallery => "gallery",
    }
}

fn animations_enabled() -> bool {
    let mut on = BOOL::from(true);
    let ok = unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, Some(&mut on as *mut BOOL as _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)) };
    ok.is_err() || on.as_bool()
}

fn start_folder() -> PathBuf {
    shell::home().unwrap_or_else(|| PathBuf::from("C:\\"))
}

/// Folder to open from the command line (`fexplorer.exe <folder-or-file>`); a file opens its folder with the file selected.
fn start_target(arg: Option<std::ffi::OsString>) -> Option<(PathBuf, Option<std::ffi::OsString>)> {
    let raw = arg?.to_string_lossy().trim().trim_matches('"').to_string();
    let p = PathBuf::from(raw);
    if p.is_dir() {
        Some((p, None))
    } else if p.is_file() {
        Some((p.parent()?.to_path_buf(), p.file_name().map(|n| n.to_os_string())))
    } else {
        None
    }
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
            // Icon resource 1 from app.rc (taskbar and Alt+Tab); absent when built without the Windows SDK.
            hIcon: LoadIconW(Some(hinst.into()), PCWSTR(std::ptr::without_provenance(1))).unwrap_or_default(),
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
        let corners = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corners as *const _ as _, 4); // Win 11 only; ignored on Win 10
        let margins = if mica { MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 } } else { MARGINS { cyTopHeight: 1, ..Default::default() } };
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);

        let (start, select) = start_target(std::env::args_os().nth(1)).unwrap_or_else(|| (start_folder(), None));
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
            view: View::Icon,
            icon_size: views::grid::ICON_DEFAULT,
            band: None,
            group_by: GroupBy::None,
            tags: crate::tags::Tags::load(),
            prefs: crate::prefs::Prefs::load(),
            show_pathbar: false,
            recents_sort: false,
            share: None,
            tag_view: None,
            groups: Vec::new(),
            gallery_scroll: 0.0,
            thumbnailer: None,
            thumb_pending: Default::default(),
            thumb_none: Default::default(),
            thumb_exts: Default::default(),
            columns: Columns::default(),
            error: None,
            loading: false,
            settings: crate::settings::Settings::resolve(crate::settings::read_windows(), &crate::prefs::Prefs::new(1)),
            win_defaults: crate::settings::read_windows(),
            settings_open: false,
            sheet_anim: None,
            xfer: Default::default(),
            press: None,
            hover_pending: None,
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
            pre_search_view: View::Icon,
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
        app.show_pathbar = app.prefs.get("@pathbar") == Some("1");
        app.settings = crate::settings::Settings::resolve(app.win_defaults, &app.prefs);
        app.rebuild_edit_style();
        app.pending_select = select;
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
    fn start_target_from_command_line() {
        let d = std::env::temp_dir().join(format!("fx-arg-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("f.txt"), "").unwrap();
        assert_eq!(start_target(Some(d.clone().into_os_string())), Some((d.clone(), None)));
        assert_eq!(start_target(Some(d.join("f.txt").into_os_string())), Some((d.clone(), Some("f.txt".into()))), "a file opens its folder with the file selected");
        assert_eq!(start_target(Some(format!("\"{}\"", d.display()).into())), Some((d.clone(), None)), "stray quotes are stripped");
        assert_eq!(start_target(Some(d.join("missing").into_os_string())), None);
        assert_eq!(start_target(None), None);
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn folder_title_uses_cached_drive_names() {
        let drives = [Place { name: "Data (D:)".into(), path: PathBuf::from("D:\\"), glyph: Glyph::Drive }];
        assert_eq!(folder_title(Path::new("D:\\"), &drives), "Data (D:)");
        assert_eq!(folder_title(Path::new(r"D:\photos"), &drives), "photos");
        assert_eq!(folder_title(Path::new("E:\\"), &drives), "E:\\", "unknown roots fall back to the path");
    }
}
