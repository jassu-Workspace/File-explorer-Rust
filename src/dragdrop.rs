//! Drag and drop: our items can be dragged out (shell data object), and files dropped on the window go through the
//! transfer UI with Explorer's rules (same drive moves, other drive copies, Ctrl copies, Shift moves).
use crate::transfer::Op;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, POINTL, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{CoTaskMemFree, DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL};
use windows::Win32::System::Ole::{CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_MOVE, DROPEFFECT_NONE, IDropTarget, IDropTarget_Impl, ReleaseStgMedium};
use windows::Win32::System::SystemServices::{MK_CONTROL, MK_SHIFT, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{BHID_DataObject, HDROP, SHCreateShellItemArrayFromIDLists, SHDoDragDrop, SHParseDisplayName};
use windows::Win32::UI::WindowsAndMessaging::SendMessageW;
use windows::core::{HSTRING, Ref, implement};

/// Sent synchronously to the window while a drag is over it; `lparam` is `*mut Query`.
pub const WM_DRAG_QUERY: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 7;

/// One drag-over or drop step, answered by the app.
pub struct Query {
    /// Client pixels; `None` when the drag left the window.
    pub at: Option<(i32, i32)>,
    pub items: Vec<PathBuf>,
    pub ctrl: bool,
    pub shift: bool,
    pub drop: bool,
    /// Set by the app: what a drop here would do.
    pub op: Option<Op>,
}

fn drive(p: &Path) -> String {
    p.components().next().map(|c| c.as_os_str().to_string_lossy().to_uppercase()).unwrap_or_default()
}

/// What dropping `items` on folder `dest` does, or `None` when it cannot go there.
pub fn drop_op(items: &[PathBuf], dest: &Path, ctrl: bool, shift: bool) -> Option<Op> {
    if items.is_empty() || items.iter().any(|i| dest.starts_with(i)) {
        return None; // a folder cannot go into itself
    }
    let same_folder = items.iter().all(|i| i.parent() == Some(dest));
    let op = if ctrl {
        Op::Copy
    } else if shift || items.iter().all(|i| drive(i) == drive(dest)) {
        Op::Move
    } else {
        Op::Copy
    };
    // Moving onto the folder it is already in does nothing; Ctrl+drop there makes copies, like Explorer.
    (!(same_folder && op == Op::Move)).then_some(op)
}

fn effect(op: Option<Op>) -> DROPEFFECT {
    match op {
        Some(Op::Copy) => DROPEFFECT_COPY,
        Some(Op::Move) => DROPEFFECT_MOVE,
        _ => DROPEFFECT_NONE,
    }
}

fn paths(data: &IDataObject) -> Vec<PathBuf> {
    let fmt = FORMATETC { cfFormat: CF_HDROP.0, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
    unsafe {
        let Ok(mut m) = data.GetData(&fmt) else { return Vec::new() };
        let v = crate::shell::hdrop_paths(HDROP(m.u.hGlobal.0));
        ReleaseStgMedium(&mut m);
        v
    }
}

#[implement(IDropTarget)]
pub struct Target {
    hwnd: isize,
    items: RefCell<Vec<PathBuf>>,
}

impl Target {
    pub fn create(hwnd: HWND) -> IDropTarget {
        Target { hwnd: hwnd.0 as isize, items: RefCell::new(Vec::new()) }.into()
    }

    fn ask(&self, keys: MODIFIERKEYS_FLAGS, pt: Option<&POINTL>, drop: bool) -> DROPEFFECT {
        let hwnd = HWND(self.hwnd as *mut _);
        let at = pt.map(|pt| {
            let mut p = POINT { x: pt.x, y: pt.y };
            unsafe {
                let _ = ScreenToClient(hwnd, &mut p);
            }
            (p.x, p.y)
        });
        let mut q = Query { at, items: self.items.borrow().clone(), ctrl: keys.0 & MK_CONTROL.0 != 0, shift: keys.0 & MK_SHIFT.0 != 0, drop, op: None };
        unsafe { SendMessageW(hwnd, WM_DRAG_QUERY, Some(WPARAM(0)), Some(LPARAM(&mut q as *mut Query as isize))) };
        effect(q.op)
    }
}

impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(&self, data: Ref<IDataObject>, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, out: *mut DROPEFFECT) -> windows::core::Result<()> {
        *self.items.borrow_mut() = data.ok().map(paths).unwrap_or_default();
        let e = self.ask(keys, Some(pt), false);
        unsafe { *out = e };
        Ok(())
    }

    fn DragOver(&self, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, out: *mut DROPEFFECT) -> windows::core::Result<()> {
        let e = self.ask(keys, Some(pt), false);
        unsafe { *out = e };
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        self.ask(MODIFIERKEYS_FLAGS(0), None, false);
        self.items.borrow_mut().clear();
        Ok(())
    }

    fn Drop(&self, data: Ref<IDataObject>, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, out: *mut DROPEFFECT) -> windows::core::Result<()> {
        if let Ok(d) = data.ok() {
            *self.items.borrow_mut() = paths(d);
        }
        self.ask(keys, Some(pt), true);
        self.items.borrow_mut().clear();
        // We run the copy/move ourselves, so the source must not delete anything afterwards.
        unsafe { *out = DROPEFFECT_NONE };
        Ok(())
    }
}

/// Drags `paths` out of the window (modal until the drop). Explorer and other apps receive normal shell items.
pub fn drag_out(hwnd: HWND, paths: &[PathBuf]) {
    let mut pidls: Vec<*const ITEMIDLIST> = Vec::new();
    for p in paths {
        let mut pidl = std::ptr::null_mut();
        if unsafe { SHParseDisplayName(&HSTRING::from(p.as_os_str()), None, &mut pidl, 0, None) }.is_ok() {
            pidls.push(pidl);
        }
    }
    if !pidls.is_empty()
        && let Ok(arr) = unsafe { SHCreateShellItemArrayFromIDLists(&pidls) }
        && let Ok(data) = unsafe { arr.BindToHandler::<_, IDataObject>(None, &BHID_DataObject) }
    {
        let _ = unsafe { SHDoDragDrop(Some(hwnd), &data, None, DROPEFFECT_COPY | DROPEFFECT_MOVE) };
    }
    for p in pidls {
        unsafe { CoTaskMemFree(Some(p as _)) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn explorer_drop_rules() {
        let items = [p(r"C:\a\x.txt"), p(r"C:\a\y.txt")];
        assert_eq!(drop_op(&items, Path::new(r"C:\b"), false, false), Some(Op::Move), "same drive moves");
        assert_eq!(drop_op(&items, Path::new(r"D:\b"), false, false), Some(Op::Copy), "other drive copies");
        assert_eq!(drop_op(&items, Path::new(r"c:\b"), false, false), Some(Op::Move), "drive letters ignore case");
        assert_eq!(drop_op(&items, Path::new(r"C:\b"), true, false), Some(Op::Copy), "Ctrl copies");
        assert_eq!(drop_op(&items, Path::new(r"D:\b"), false, true), Some(Op::Move), "Shift moves");
    }

    #[test]
    fn drops_that_do_nothing() {
        let items = [p(r"C:\a\x.txt")];
        assert_eq!(drop_op(&items, Path::new(r"C:\a"), false, false), None, "same folder");
        assert_eq!(drop_op(&items, Path::new(r"C:\a"), true, false), Some(Op::Copy), "Ctrl in the same folder copies");
        assert_eq!(drop_op(&[p(r"C:\a")], Path::new(r"C:\a\sub"), false, false), None, "folder into itself");
        assert_eq!(drop_op(&[p(r"C:\a")], Path::new(r"C:\a"), true, false), None, "folder onto itself");
        assert_eq!(drop_op(&[], Path::new(r"C:\a"), false, false), None);
    }
}
