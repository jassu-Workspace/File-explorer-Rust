use crate::draw::Gfx;
use crate::layout::{self, Layout, compute_layout, nc_hit};
use crate::theme::Theme;
use crate::views::{self, toolbar::toolbar_rects};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMSBT_MAINWINDOW, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT, ScreenToClient};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{MARGINS, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, PCWSTR, Result, w};

const WINDOW_W: f32 = 1100.0;
const WINDOW_H: f32 = 700.0;
const RESIZE_BORDER: f32 = 6.0;

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

impl App {
    pub fn can_back(&self) -> bool {
        false
    }
    pub fn can_forward(&self) -> bool {
        false
    }
    pub fn title(&self) -> String {
        "Home".into()
    }
    pub fn columns_view(&self) -> bool {
        false
    }
    pub fn search_placeholder_visible(&self) -> bool {
        true
    }
    pub fn search_placeholder(&self) -> String {
        "Search".into()
    }
    pub fn maximized(&self) -> bool {
        unsafe { IsZoomed(self.hwnd).as_bool() }
    }

    pub fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn relayout(&mut self) {
        self.layout = compute_layout(self.w, self.h, self.sidebar_w, 1.0);
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

    fn handle(&mut self, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        let hwnd = self.hwnd;
        match msg {
            WM_PAINT => {
                self.paint();
                Some(LRESULT(0))
            }
            WM_ERASEBKGND => Some(LRESULT(1)),
            WM_SIZE => {
                let (pw, ph) = (lo(lp.0) as u32, hi(lp.0) as u32);
                self.w = pw as f32 / self.scale;
                self.h = ph as f32 / self.scale;
                self.gfx.resize(pw, ph);
                self.relayout();
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
                let (x, _y) = self.client_dip(lp);
                if self.dragging_splitter {
                    self.sidebar_w = x;
                    self.relayout();
                    self.invalidate();
                    return Some(LRESULT(0));
                }
                self.track_leave(false);
                let (x, y) = self.client_dip(lp);
                self.set_hot(self.hot_at(x, y));
                Some(LRESULT(0))
            }
            WM_LBUTTONDOWN => {
                let (x, y) = self.client_dip(lp);
                if self.layout.splitter.contains(x, y) {
                    self.dragging_splitter = true;
                    unsafe { SetCapture(hwnd) };
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
                    self.invalidate();
                }
                None
            }
            WM_DESTROY => {
                unsafe { PostQuitMessage(0) };
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

        let app = Box::new(App {
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
            pressed_nc: 0,
            dragging_splitter: false,
            tracking: false,
        });
        app.apply_theme_to_frame();
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
