//! Native single-line EDIT controls (address bar, search field): IME, selection and clipboard come free.
use crate::layout::Rect;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, DEFAULT_CHARSET, DEFAULT_PITCH, DeleteObject, FF_DONTCARE, FW_NORMAL, HFONT,
    OUT_DEFAULT_PRECIS,
};
use windows::Win32::UI::Controls::{EM_SETCUEBANNER, EM_SETMARGINS, EM_SETSEL};
use windows::Win32::UI::Input::KeyboardAndMouse::{SetFocus, VK_DOWN, VK_ESCAPE, VK_RETURN, VK_UP};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, Result, w};

pub const ID_ADDRESS: usize = 101;
pub const ID_SEARCH: usize = 102;
pub const ID_RENAME: usize = 103;
/// Posted to the parent when Enter, Esc, Up or Down is pressed in an edit: wparam = control id, lparam = virtual key.
pub const WM_EDIT_KEY: u32 = WM_APP + 10;

const FONT_PX: f32 = 13.0;

unsafe extern "system" fn subclass(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, id: usize, _: usize) -> LRESULT {
    unsafe {
        let vk = wp.0 as u16;
        match msg {
            WM_KEYDOWN if [VK_RETURN.0, VK_ESCAPE.0, VK_UP.0, VK_DOWN.0].contains(&vk) => {
                if let Ok(parent) = GetParent(hwnd) {
                    let _ = PostMessageW(Some(parent), WM_EDIT_KEY, WPARAM(id), LPARAM(vk as isize));
                }
                LRESULT(0)
            }
            // Swallow the matching WM_CHAR so the control does not beep.
            WM_CHAR if wp.0 == 0x0D || wp.0 == 0x1B => LRESULT(0),
            _ => DefSubclassProc(hwnd, msg, wp, lp),
        }
    }
}

pub fn create(parent: HWND, id: usize) -> Result<HWND> {
    unsafe {
        let h = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("EDIT"),
            w!(""),
            WS_CHILD | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            0,
            0,
            10,
            10,
            Some(parent),
            Some(HMENU(id as *mut _)),
            None,
            None,
        )?;
        let _ = SetWindowSubclass(h, Some(subclass), id, 0);
        Ok(h)
    }
}

pub fn make_font(scale: f32) -> HFONT {
    unsafe {
        CreateFontW(
            -(FONT_PX * scale).round() as i32,
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
            w!("Segoe UI Variable Text"),
        )
    }
}

pub fn set_font(h: HWND, f: HFONT) {
    unsafe {
        SendMessageW(h, WM_SETFONT, Some(WPARAM(f.0 as usize)), Some(LPARAM(1)));
        SendMessageW(h, EM_SETMARGINS, Some(WPARAM(EC_LEFTMARGIN as usize | EC_RIGHTMARGIN as usize)), Some(LPARAM(0)));
    }
}

pub fn delete_font(f: HFONT) {
    if !f.is_invalid() {
        unsafe {
            let _ = DeleteObject(f.into());
        }
    }
}

pub fn text(h: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(h).max(0) as usize;
        let mut buf = vec![0u16; n + 1];
        let got = GetWindowTextW(h, &mut buf).max(0) as usize;
        String::from_utf16_lossy(&buf[..got])
    }
}

pub fn set_text(h: HWND, s: &str) {
    unsafe {
        let _ = SetWindowTextW(h, &HSTRING::from(s));
    }
}

/// Positions the control inside `r` (DIPs), vertically centered, and shows it.
pub fn show_in(h: HWND, r: Rect, scale: f32) {
    let line = (FONT_PX * 1.45 * scale).round() as i32;
    let x = (r.x * scale).round() as i32;
    let y = (r.y * scale + (r.h * scale - line as f32) / 2.0).round() as i32;
    unsafe {
        let _ = MoveWindow(h, x, y, (r.w * scale).round() as i32, line, true);
        let _ = ShowWindow(h, SW_SHOWNA);
    }
}

pub fn focus_select_all(h: HWND) {
    unsafe {
        let _ = SetFocus(Some(h));
        SendMessageW(h, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
    }
}

/// Focuses the control and selects characters `start..end` (UTF-16 units).
pub fn focus_select(h: HWND, start: usize, end: usize) {
    unsafe {
        let _ = SetFocus(Some(h));
        SendMessageW(h, EM_SETSEL, Some(WPARAM(start)), Some(LPARAM(end as isize)));
    }
}

/// Grey hint text shown while the field is empty, even when focused.
pub fn set_cue(h: HWND, text: &str) {
    let w = HSTRING::from(text);
    unsafe {
        SendMessageW(h, EM_SETCUEBANNER, Some(WPARAM(1)), Some(LPARAM(w.as_ptr() as isize)));
    }
}

pub fn hide(h: HWND) {
    unsafe {
        let _ = ShowWindow(h, SW_HIDE);
    }
}

pub fn visible(h: HWND) -> bool {
    unsafe { IsWindowVisible(h).as_bool() }
}
