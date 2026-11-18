//! Explorer tools: Properties, Open with, Open in Terminal, Copy as path, New text document, folder picker,
//! and the full Windows context menu ("Show more options").
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree, IDataObject};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    BHID_DataObject, BHID_SFUIObject, CMF_EXTENDEDVERBS, CMF_NORMAL, CMINVOKECOMMANDINFO, FOS_PICKFOLDERS, FileOpenDialog, IContextMenu, IContextMenu3, IFileOpenDialog, IShellItem,
    IShellItemArray, OAIF_ALLOW_REGISTRATION, OAIF_EXEC, OPENASINFO, SHCreateItemFromParsingName, SHCreateShellItemArrayFromIDLists,
    SHMultiFileProperties, SHOpenWithDialog, SHParseDisplayName, SIGDN_FILESYSPATH, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::{CreatePopupMenu, HMENU, DestroyMenu, SW_SHOWNORMAL, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu};
use windows::core::{HSTRING, PCSTR, PCWSTR, w};

/// Shell items for `paths` (one array, so menus and properties see them together).
pub fn shell_items(paths: &[PathBuf]) -> Option<IShellItemArray> {
    let mut pidls: Vec<*const ITEMIDLIST> = Vec::new();
    for p in paths {
        let mut pidl = std::ptr::null_mut();
        if unsafe { SHParseDisplayName(&HSTRING::from(p.as_os_str()), None, &mut pidl, 0, None) }.is_ok() {
            pidls.push(pidl);
        }
    }
    let arr = (!pidls.is_empty()).then(|| unsafe { SHCreateShellItemArrayFromIDLists(&pidls) }.ok()).flatten();
    for p in pidls {
        unsafe { CoTaskMemFree(Some(p as _)) };
    }
    arr
}

pub fn data_object(paths: &[PathBuf]) -> Option<IDataObject> {
    unsafe { shell_items(paths)?.BindToHandler(None, &BHID_DataObject).ok() }
}

/// The Windows Properties sheet for one or more items.
pub fn properties(paths: &[PathBuf]) {
    if let Some(d) = data_object(paths) {
        let _ = unsafe { SHMultiFileProperties(&d, 0) };
    }
}

pub fn open_with(hwnd: HWND, path: &Path) {
    let file = HSTRING::from(path.as_os_str());
    let info = OPENASINFO { pcszFile: PCWSTR(file.as_ptr()), pcszClass: PCWSTR::null(), oaifInFlags: OAIF_ALLOW_REGISTRATION | OAIF_EXEC };
    let _ = unsafe { SHOpenWithDialog(Some(hwnd), &info) };
}

/// Windows Terminal in `dir`, or Command Prompt when Terminal is not installed.
pub fn open_terminal(hwnd: HWND, dir: &Path) {
    let d = HSTRING::from(dir.as_os_str());
    // The folder goes in as the working directory, so no quoting of the path is needed.
    let r = unsafe { ShellExecuteW(Some(hwnd), w!("open"), w!("wt.exe"), w!("-d ."), &d, SW_SHOWNORMAL) };
    if r.0 as isize <= 32 {
        unsafe { ShellExecuteW(Some(hwnd), w!("open"), w!("cmd.exe"), None, &d, SW_SHOWNORMAL) };
    }
}

/// Explorer's "Copy as path" text: each path in quotes, one per line.
pub fn path_text(paths: &[PathBuf]) -> String {
    paths.iter().map(|p| format!("\"{}\"", p.display())).collect::<Vec<_>>().join("\r\n")
}

/// Creates "New Text Document.txt" (or "New Text Document (n).txt") in `dir` and returns its path.
pub fn new_text_file(dir: &Path) -> std::io::Result<PathBuf> {
    for n in 1..10_000 {
        let name = if n == 1 { "New Text Document.txt".to_string() } else { format!("New Text Document ({n}).txt") };
        let p = dir.join(name);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&p) {
            Ok(_) => return Ok(p),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("too many new text documents"))
}

/// Folder picker for Copy to / Move to. `None` when cancelled.
pub fn pick_folder(hwnd: HWND, title: &str, start: &Path) -> Option<PathBuf> {
    unsafe {
        let d: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        d.SetOptions(d.GetOptions().ok()? | FOS_PICKFOLDERS).ok()?;
        let _ = d.SetTitle(&HSTRING::from(title));
        if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(start.as_os_str()), None) {
            let _ = d.SetFolder(&item);
        }
        d.Show(Some(hwnd)).ok()?;
        let p = d.GetResult().ok()?.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as _));
        s.map(PathBuf::from)
    }
}

thread_local! {
    /// The Windows menu while it is open, for forwarding owner-drawn submenu messages.
    static ACTIVE: std::cell::RefCell<Option<IContextMenu3>> = const { std::cell::RefCell::new(None) };
}

/// Forwards menu drawing messages to an open Windows context menu. Returns the result when it handled one.
pub fn forward_menu_msg(msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    use windows::Win32::UI::WindowsAndMessaging::{WM_DRAWITEM, WM_INITMENUPOPUP, WM_MEASUREITEM, WM_MENUCHAR};
    if !matches!(msg, WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR) {
        return None;
    }
    let cm = ACTIVE.with(|a| a.borrow().clone())?;
    let mut out = LRESULT(0);
    unsafe { cm.HandleMenuMsg2(msg, wp, lp, Some(&mut out)) }.ok().map(|_| out)
}

/// Shows the full Windows context menu for `paths` (same folder) at a screen point and runs the chosen command.
pub fn shell_menu(hwnd: HWND, paths: &[PathBuf], at: POINT) {
    const FIRST: u32 = 1;
    let Some((cm, menu)) = build_shell_menu(paths, FIRST) else { return };
    unsafe {
        use windows::core::Interface;
        ACTIVE.with(|a| *a.borrow_mut() = cm.cast::<IContextMenu3>().ok());
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, at.x, at.y, None, hwnd, None).0 as u32;
        ACTIVE.with(|a| *a.borrow_mut() = None);
        if cmd >= FIRST {
            let info = CMINVOKECOMMANDINFO {
                cbSize: size_of::<CMINVOKECOMMANDINFO>() as u32,
                hwnd,
                // MAKEINTRESOURCE: the verb is the command offset.
                lpVerb: PCSTR((cmd - FIRST) as usize as *const u8),
                nShow: SW_SHOWNORMAL.0,
                ..Default::default()
            };
            let _ = cm.InvokeCommand(&info);
        }
        let _ = DestroyMenu(menu);
    }
}

/// The Windows context menu for `paths`, filled into a new popup menu with ids from `first`.
fn build_shell_menu(paths: &[PathBuf], first: u32) -> Option<(IContextMenu, HMENU)> {
    unsafe {
        let cm = shell_items(paths)?.BindToHandler::<_, IContextMenu>(None, &BHID_SFUIObject).ok()?;
        let menu = CreatePopupMenu().ok()?;
        if cm.QueryContextMenu(menu, 0, first, 0x7FFF, CMF_NORMAL | CMF_EXTENDEDVERBS).is_err() {
            let _ = DestroyMenu(menu);
            return None;
        }
        Some((cm, menu))
    }
}

/// "Displaced item" properties of Recycle Bin items: the folder an item was deleted from (2) and when (3).
const FMTID_DISPLACED: windows::core::GUID = windows::core::GUID::from_u128(0x9b174b33_40ff_11d2_a27e_00c04fc30871);

/// Puts recycled items back where they were (newest match per path). Returns the paths it could not find.
pub fn restore_from_bin(paths: &[PathBuf]) -> Vec<PathBuf> {
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::UI::Shell::{BHID_EnumItems, FOLDERID_RecycleBinFolder, IEnumShellItems, IShellItem2, KF_FLAG_DEFAULT, SHGetKnownFolderItem, SIGDN_NORMALDISPLAY};
    use windows::core::Interface;
    let from_key = PROPERTYKEY { fmtid: FMTID_DISPLACED, pid: 2 };
    let when_key = PROPERTYKEY { fmtid: FMTID_DISPLACED, pid: 3 };
    let take = |p: windows::core::PWSTR| unsafe {
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as _));
        s
    };
    // (original path, deleted time, bin item) for everything in the bin.
    let mut items: Vec<(PathBuf, String, u64, IShellItem)> = Vec::new();
    unsafe {
        let Ok(bin) = SHGetKnownFolderItem::<IShellItem>(&FOLDERID_RecycleBinFolder, KF_FLAG_DEFAULT, None) else { return paths.to_vec() };
        let Ok(en) = bin.BindToHandler::<_, IEnumShellItems>(None, &BHID_EnumItems) else { return paths.to_vec() };
        loop {
            let mut one = [None];
            if en.Next(&mut one, None).is_err() {
                break;
            }
            let Some(item) = one[0].take() else { break };
            let Ok(i2) = item.cast::<IShellItem2>() else { continue };
            let (Ok(from), Ok(name)) = (i2.GetString(&from_key), item.GetDisplayName(SIGDN_NORMALDISPLAY)) else { continue };
            let when = i2.GetFileTime(&when_key).map_or(0, |t| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64);
            items.push((PathBuf::from(take(from)), take(name).to_lowercase(), when, item));
        }
    }
    let mut missing = Vec::new();
    for p in paths {
        let Some(parent) = p.parent() else { continue };
        // The bin shows the original full path, or just the name; either may hide the extension.
        let full = p.to_string_lossy().to_lowercase();
        let bare = p.with_extension("").to_string_lossy().to_lowercase();
        let name = p.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
        let stem = p.file_stem().unwrap_or_default().to_string_lossy().to_lowercase();
        let hit = items
            .iter()
            .filter(|(from, shown, ..)| from.as_os_str().eq_ignore_ascii_case(parent.as_os_str()) && [&full, &bare, &name, &stem].contains(&shown))
            .max_by_key(|i| i.2);
        let ok = hit.is_some_and(|(.., item)| unsafe {
            let Ok(cm) = item.BindToHandler::<_, IContextMenu>(None, &BHID_SFUIObject) else { return false };
            let info = CMINVOKECOMMANDINFO { cbSize: size_of::<CMINVOKECOMMANDINFO>() as u32, lpVerb: PCSTR(c"undelete".as_ptr() as *const u8), ..Default::default() };
            cm.InvokeCommand(&info).is_ok()
        });
        if !ok {
            missing.push(p.clone());
        }
    }
    missing
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_as_path_quotes_each_line() {
        let t = path_text(&[PathBuf::from(r"C:\a b\x.txt"), PathBuf::from(r"D:\y")]);
        assert_eq!(t, "\"C:\\a b\\x.txt\"\r\n\"D:\\y\"");
    }

    #[test]
    fn windows_menu_has_items() {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
        }
        let d = std::env::temp_dir().join(format!("fx-menu-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("a.txt");
        std::fs::write(&f, "x").unwrap();
        let (_cm, menu) = build_shell_menu(&[f], 1).expect("menu");
        let n = unsafe { windows::Win32::UI::WindowsAndMessaging::GetMenuItemCount(Some(menu)) };
        assert!(n > 3, "{n} items");
        unsafe {
            let _ = DestroyMenu(menu);
        }
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn recycle_then_restore_puts_the_file_back() {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
        }
        let d = std::env::temp_dir().join(format!("fx-restore-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("bring-me-back.txt");
        std::fs::write(&f, "hello").unwrap();
        let job = crate::transfer::build_job(crate::transfer::Op::Recycle, std::slice::from_ref(&f), None, &Default::default()).unwrap();
        crate::transfer::run(job, Default::default(), |_| {});
        assert!(!f.exists(), "recycled");
        assert!(restore_from_bin(std::slice::from_ref(&f)).is_empty());
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "hello");
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn new_text_file_picks_a_free_name() {
        let d = std::env::temp_dir().join(format!("fx-tools-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(new_text_file(&d).unwrap(), d.join("New Text Document.txt"));
        assert_eq!(new_text_file(&d).unwrap(), d.join("New Text Document (2).txt"));
        std::fs::remove_dir_all(d).unwrap();
    }
}
