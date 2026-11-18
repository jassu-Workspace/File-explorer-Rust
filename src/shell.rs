use crate::draw::Glyph;
use crate::format::LocalTime;
use crate::listing::Entry;
use std::collections::HashMap;
use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
use windows::Win32::UI::Shell::{
    FOLDERID_CommonPrograms, FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Pictures, FOLDERID_Profile, FOLDERID_Recent,
    FOLDERID_SkyDrive, KF_FLAG_DEFAULT, SHFILEINFOW, SHGFI_FLAGS,
    SHGFI_SMALLICON, SHGFI_SYSICONINDEX, SHGFI_TYPENAME, SHGFI_USEFILEATTRIBUTES, SHGetFileInfoW, SHGetKnownFolderPath,
};
use windows::core::{GUID, PCWSTR};

const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;
const DRIVE_REMOTE: u32 = 4;

#[derive(Clone, Debug)]
pub struct Place {
    pub name: String,
    pub path: PathBuf,
    pub glyph: Glyph,
}

pub fn wide(p: &Path) -> Vec<u16> {
    p.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn known(id: &GUID) -> Option<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let path = PathBuf::from(OsString::from_wide(p.as_wide()));
        CoTaskMemFree(Some(p.0 as _));
        Some(path)
    }
}

fn places(list: &[(&str, &GUID, Glyph)]) -> Vec<Place> {
    list.iter().filter_map(|&(name, id, glyph)| known(id).filter(|p| p.is_dir()).map(|path| Place { name: name.into(), path, glyph })).collect()
}

pub fn home() -> Option<PathBuf> {
    known(&FOLDERID_Profile)
}

/// Finder's Favorites, mapped to Windows folders.
pub fn favorites() -> Vec<Place> {
    places(&[
        ("Recents", &FOLDERID_Recent, Glyph::Clock),
        // shortcut: all-users Start menu only; per-user app shortcuts live in FOLDERID_Programs. Merge both when a virtual folder exists.
        ("Applications", &FOLDERID_CommonPrograms, Glyph::Grid),
        ("Desktop", &FOLDERID_Desktop, Glyph::Desktop),
        ("Documents", &FOLDERID_Documents, Glyph::Doc),
        ("Downloads", &FOLDERID_Downloads, Glyph::Download),
        ("Home", &FOLDERID_Profile, Glyph::Home),
        ("Pictures", &FOLDERID_Pictures, Glyph::Picture),
    ])
}

/// Cloud-synced roots (iCloud's place in Finder).
pub fn cloud() -> Vec<Place> {
    places(&[("OneDrive", &FOLDERID_SkyDrive, Glyph::Cloud)])
}

/// Name shown to the user: shortcut extensions are always hidden (like Explorer); other extensions only when
/// "show file name extensions" is off.
pub fn display_name(e: &Entry, show_ext: bool) -> String {
    let name = e.name.to_string_lossy();
    if e.is_dir() {
        return name.into_owned();
    }
    let lower = name.to_ascii_lowercase();
    let base = match [".lnk", ".url"].iter().find(|x| lower.ends_with(*x)) {
        Some(x) => name[..name.len() - x.len()].to_string(),
        None => name.into_owned(),
    };
    if show_ext {
        return base;
    }
    // A leading dot is a name, not an extension (".gitignore" stays).
    match base.rfind('.') {
        Some(i) if i > 0 => base[..i].to_string(),
        _ => base,
    }
}

pub fn drives() -> Vec<Place> {
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .filter_map(|i| {
            let letter = (b'A' + i) as char;
            let root = PathBuf::from(format!("{letter}:\\"));
            let w = wide(&root);
            let kind = unsafe { GetDriveTypeW(PCWSTR(w.as_ptr())) };
            if kind == DRIVE_REMOTE {
                // No volume query: a disconnected network drive can block for seconds.
                return Some(Place { name: format!("Network Drive ({letter}:)"), path: root, glyph: Glyph::Drive });
            }
            if kind != DRIVE_FIXED && kind != DRIVE_REMOVABLE {
                return None;
            }
            let mut label = [0u16; 261];
            let ok = unsafe { GetVolumeInformationW(PCWSTR(w.as_ptr()), Some(&mut label), None, None, None, None) }.is_ok();
            let len = label.iter().position(|&c| c == 0).unwrap_or(0);
            let label = if ok && len > 0 { String::from_utf16_lossy(&label[..len]) } else { "Local Disk".into() };
            Some(Place { name: format!("{label} ({letter}:)"), path: root, glyph: Glyph::Drive })
        })
        .collect()
}

pub fn free_space(path: &Path) -> Option<u64> {
    let w = wide(path);
    let mut free = 0u64;
    unsafe { GetDiskFreeSpaceExW(PCWSTR(w.as_ptr()), Some(&mut free), None, None) }.ok()?;
    Some(free)
}

/// SHGetFileInfoW by extension only (no disk access).
fn file_info(e: &Entry, flags: SHGFI_FLAGS) -> SHFILEINFOW {
    let name: Vec<u16> = if e.is_dir() {
        "folder".encode_utf16().chain(Some(0)).collect()
    } else {
        format!("x.{}", e.ext_lower()).encode_utf16().chain(Some(0)).collect()
    };
    let attrs = if e.is_dir() { FILE_ATTRIBUTE_DIRECTORY } else { FILE_ATTRIBUTE_NORMAL };
    let mut info = SHFILEINFOW::default();
    unsafe { SHGetFileInfoW(PCWSTR(name.as_ptr()), attrs, Some(&mut info), size_of::<SHFILEINFOW>() as u32, flags | SHGFI_USEFILEATTRIBUTES) };
    info
}

pub fn type_name(e: &Entry, cache: &mut HashMap<String, String>) -> String {
    if e.is_dir() {
        return "Folder".into();
    }
    cache
        .entry(e.ext_lower())
        .or_insert_with(|| {
            let info = file_info(e, SHGFI_TYPENAME);
            let len = info.szTypeName.iter().position(|&c| c == 0).unwrap_or(0);
            let s = String::from_utf16_lossy(&info.szTypeName[..len]);
            if s.is_empty() { "File".into() } else { s }
        })
        .clone()
}

pub fn icon_index(e: &Entry, cache: &mut HashMap<String, i32>) -> i32 {
    let key = if e.is_dir() { "\\dir".to_string() } else { e.ext_lower() };
    *cache.entry(key).or_insert_with(|| file_info(e, SHGFI_SYSICONINDEX | SHGFI_SMALLICON).iIcon)
}

fn local_from(st: SYSTEMTIME) -> LocalTime {
    LocalTime { year: st.wYear, month: st.wMonth as u8, day: st.wDay as u8, hour: st.wHour as u8, minute: st.wMinute as u8 }
}

pub fn to_local(filetime: u64) -> LocalTime {
    let ft = FILETIME { dwLowDateTime: filetime as u32, dwHighDateTime: (filetime >> 32) as u32 };
    let (mut utc, mut local) = (SYSTEMTIME::default(), SYSTEMTIME::default());
    unsafe {
        if FileTimeToSystemTime(&ft, &mut utc).is_err() || SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_err() {
            return LocalTime { year: 1601, month: 1, day: 1, hour: 0, minute: 0 };
        }
    }
    local_from(local)
}

pub fn now_local() -> LocalTime {
    local_from(unsafe { GetLocalTime() })
}


// ---- File actions -------------------------------------------------------------------------------

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND, POINT};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, RegisterClipboardFormatW, SetClipboardData};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_HDROP;
use windows::Win32::UI::Shell::{
    DROPFILES, DragQueryFileW, FILEOPERATION_FLAGS, FOF_ALLOWUNDO, FOF_NOCONFIRMMKDIR, FOF_NOERRORUI, FOF_RENAMEONCOLLISION,
    FOFX_ADDUNDORECORD, FileOperation, HDROP, IFileOperation, IShellItem, SHCreateItemFromParsingName,
    ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, w};

const DROPEFFECT_COPY: u32 = 1;
const DROPEFFECT_MOVE: u32 = 2;

/// Rejects names Windows cannot store. Returns a message for the user.
pub fn validate_name(name: &str) -> Result<(), &'static str> {
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5",
        "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if name.trim().is_empty() {
        return Err("A name can't be empty.");
    }
    if name.chars().any(|c| matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || (c as u32) < 32) {
        return Err(r#"A name can't contain \ / : * ? " < > |"#);
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Err("A name can't end with a dot or a space.");
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        return Err("That name is reserved by Windows.");
    }
    Ok(())
}

pub fn open(hwnd: Option<HWND>, path: &Path) -> windows::core::Result<()> {
    let r = unsafe { ShellExecuteW(hwnd, w!("open"), &HSTRING::from(path.as_os_str()), None, None, SW_SHOWNORMAL) };
    // ShellExecute returns a value > 32 on success.
    if r.0 as isize > 32 { Ok(()) } else { Err(windows::core::Error::from_thread()) }
}

fn item(path: &Path) -> windows::core::Result<IShellItem> {
    unsafe { SHCreateItemFromParsingName(&HSTRING::from(path.as_os_str()), None) }
}

fn file_op(hwnd: Option<HWND>, flags: FILEOPERATION_FLAGS) -> windows::core::Result<IFileOperation> {
    let op: IFileOperation = unsafe { CoCreateInstance(&FileOperation, None, CLSCTX_ALL)? };
    unsafe {
        op.SetOperationFlags(flags | FOFX_ADDUNDORECORD)?;
        if let Some(h) = hwnd {
            op.SetOwnerWindow(h)?;
        }
    }
    Ok(op)
}

/// Runs the queued operations. A user cancel is not an error.
fn perform(op: &IFileOperation) -> windows::core::Result<()> {
    match unsafe { op.PerformOperations() } {
        Err(e) if e.code() == windows::Win32::Foundation::E_ABORT => Ok(()),
        r => r,
    }
}

pub fn rename(hwnd: Option<HWND>, path: &Path, new_name: &str) -> windows::core::Result<()> {
    validate_name(new_name).map_err(|m| windows::core::Error::new(windows::Win32::Foundation::E_INVALIDARG, m))?;
    let op = file_op(hwnd, FOF_ALLOWUNDO | FOF_NOERRORUI)?;
    unsafe { op.RenameItem(&item(path)?, &HSTRING::from(new_name), None)? };
    perform(&op)
}

/// Creates "New folder", or "New folder (n)" when taken, and returns its path.
pub fn new_folder(hwnd: Option<HWND>, dir: &Path) -> windows::core::Result<PathBuf> {
    let name = (1..)
        .map(|n| if n == 1 { "New folder".to_string() } else { format!("New folder ({n})") })
        .find(|n| !dir.join(n).exists())
        .unwrap_or_default();
    let op = file_op(hwnd, FOF_ALLOWUNDO | FOF_RENAMEONCOLLISION | FOF_NOCONFIRMMKDIR)?;
    unsafe { op.NewItem(&item(dir)?, FILE_ATTRIBUTE_DIRECTORY.0, &HSTRING::from(name.as_str()), None, None)? };
    perform(&op)?;
    Ok(dir.join(name))
}

struct Clipboard;

impl Clipboard {
    fn open(hwnd: Option<HWND>) -> windows::core::Result<Clipboard> {
        // Another app may hold the clipboard for a moment; retry briefly.
        for _ in 0..10 {
            if unsafe { OpenClipboard(hwnd) }.is_ok() {
                return Ok(Clipboard);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        unsafe { OpenClipboard(hwnd) }.map(|_| Clipboard)
    }
}

impl Drop for Clipboard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

fn drop_effect_format() -> u32 {
    unsafe { RegisterClipboardFormatW(w!("Preferred DropEffect")) }
}

/// Copies bytes into a movable global block for the clipboard.
fn global_bytes(bytes: &[u8]) -> windows::core::Result<HGLOBAL> {
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len())?;
        let p = GlobalLock(h) as *mut u8;
        if p.is_null() {
            let _ = GlobalFree(Some(h));
            return Err(windows::core::Error::from_thread());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        let _ = GlobalUnlock(h);
        Ok(h)
    }
}

/// Puts files on the clipboard the way Explorer does (CF_HDROP + Preferred DropEffect).
pub fn clipboard_put(hwnd: Option<HWND>, paths: &[PathBuf], cut: bool) -> windows::core::Result<()> {
    let header = DROPFILES { pFiles: size_of::<DROPFILES>() as u32, pt: POINT::default(), fNC: false.into(), fWide: true.into() };
    let mut bytes = unsafe { std::slice::from_raw_parts(&header as *const DROPFILES as *const u8, size_of::<DROPFILES>()) }.to_vec();
    for p in paths {
        for u in p.as_os_str().encode_wide().chain(Some(0)) {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
    }
    bytes.extend_from_slice(&[0, 0]);
    let effect = if cut { DROPEFFECT_MOVE } else { DROPEFFECT_COPY };
    let files = global_bytes(&bytes)?;
    let fx = global_bytes(&effect.to_le_bytes())?;
    let _cb = Clipboard::open(hwnd)?;
    unsafe {
        EmptyClipboard()?;
        // On success the clipboard owns the memory.
        SetClipboardData(CF_HDROP.0 as u32, Some(HANDLE(files.0)))?;
        SetClipboardData(drop_effect_format(), Some(HANDLE(fx.0)))?;
    }
    Ok(())
}

/// Files on the clipboard and whether they were cut.
pub fn clipboard_get(hwnd: Option<HWND>) -> Option<(Vec<PathBuf>, bool)> {
    let _cb = Clipboard::open(hwnd).ok()?;
    unsafe {
        let h = GetClipboardData(CF_HDROP.0 as u32).ok()?;
        let drop = HDROP(h.0);
        let n = DragQueryFileW(drop, u32::MAX, None);
        let paths: Vec<PathBuf> = (0..n)
            .map(|i| {
                let len = DragQueryFileW(drop, i, None) as usize;
                let mut buf = vec![0u16; len + 1];
                DragQueryFileW(drop, i, Some(&mut buf));
                PathBuf::from(OsString::from_wide(&buf[..len]))
            })
            .collect();
        let cut = GetClipboardData(drop_effect_format()).ok().is_some_and(|h| {
            let p = GlobalLock(HGLOBAL(h.0)) as *const u32;
            let v = if p.is_null() { 0 } else { p.read_unaligned() };
            let _ = GlobalUnlock(HGLOBAL(h.0));
            v & DROPEFFECT_MOVE != 0
        });
        (!paths.is_empty()).then_some((paths, cut))
    }
}

pub fn clipboard_clear(hwnd: Option<HWND>) {
    if let Ok(_cb) = Clipboard::open(hwnd) {
        unsafe {
            let _ = EmptyClipboard();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::listing::Entry;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    fn file(name: &str) -> Entry {
        Entry { name: name.into(), size: 0, modified: 0, created: 0, attrs: 0x20, rel_dir: None }
    }

    #[test]
    fn favorites_exist() {
        let f = favorites();
        let names: Vec<&str> = f.iter().map(|p| p.name.as_str()).collect();
        assert!(f.iter().all(|p| p.path.is_dir()));
        assert_eq!(names.first(), Some(&"Recents"));
        assert!(names.contains(&"Applications") && names.contains(&"Home"), "{names:?}");
        assert_eq!(home().unwrap(), f.iter().find(|p| p.name == "Home").unwrap().path);
    }

    #[test]
    fn display_name_hides_shortcut_extensions() {
        let e = |n: &str| Entry { name: n.into(), size: 0, modified: 0, created: 0, attrs: 0x20, rel_dir: None };
        assert_eq!(display_name(&e("Report.docx.lnk"), true), "Report.docx");
        assert_eq!(display_name(&e("Site.URL"), true), "Site");
        assert_eq!(display_name(&e("notes.txt"), true), "notes.txt");
        assert_eq!(display_name(&e("notes.txt"), false), "notes");
        assert_eq!(display_name(&e("Report.docx.lnk"), false), "Report");
        assert_eq!(display_name(&e(".gitignore"), false), ".gitignore");
        assert_eq!(display_name(&e("archive.tar.gz"), false), "archive.tar");
        let dir = Entry { attrs: 0x10, ..e("folder.lnk") };
        assert_eq!(display_name(&dir, false), "folder.lnk", "folders keep their name");
    }

    #[test]
    fn drives_include_system() {
        let sys = PathBuf::from(format!("{}\\", std::env::var("SystemDrive").unwrap()));
        assert!(drives().iter().any(|d| d.path == sys), "{:?}", drives().iter().map(|d| &d.path).collect::<Vec<_>>());
    }

    #[test]
    fn free_space_positive() {
        assert!(free_space(Path::new("C:\\")).unwrap() > 0);
        assert!(free_space(Path::new(r"Q:\nope\never")).is_none());
    }

    #[test]
    fn type_name_cached() {
        let mut cache = HashMap::new();
        let a = type_name(&file("a.txt"), &mut cache);
        assert!(!a.is_empty());
        assert_eq!(type_name(&file("b.TXT"), &mut cache), a);
        assert_eq!(cache.len(), 1);
        let dir = Entry { attrs: 0x10, ..file("x") };
        assert_eq!(type_name(&dir, &mut cache), "Folder");
    }

    #[test]
    fn icon_index_by_ext() {
        let mut cache = HashMap::new();
        let a = icon_index(&file("a.txt"), &mut cache);
        assert_eq!(icon_index(&file("b.txt"), &mut cache), a);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn to_local_roundtrip() {
        // 2026-11-17 12:00:00 UTC in FILETIME ticks.
        let unix = 1_794_916_800u64;
        let ft = (unix + 11_644_473_600) * 10_000_000;
        let t = to_local(ft);
        assert_eq!((t.year, t.month), (2026, 11));
        assert!((16..=18).contains(&t.day));
        assert!(now_local().year >= 2026);
    }
}

#[cfg(test)]
mod action_tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};

    fn com() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("fx-act-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn rename_rejects_bad_names() {
        assert!(validate_name("").is_err());
        assert!(validate_name("   ").is_err());
        assert!(validate_name("a:b").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("CON").is_err());
        assert!(validate_name("ends.").is_err());
        assert!(validate_name("ok.txt").is_ok());
    }

    #[test]
    fn rename_roundtrip() {
        com();
        let d = tmp("rename");
        fs::write(d.join("a.txt"), "x").unwrap();
        rename(None, &d.join("a.txt"), "b.txt").unwrap();
        assert!(d.join("b.txt").exists() && !d.join("a.txt").exists());
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn new_folder_creates() {
        com();
        let d = tmp("newdir");
        assert_eq!(new_folder(None, &d).unwrap(), d.join("New folder"));
        assert_eq!(new_folder(None, &d).unwrap(), d.join("New folder (2)"));
        assert!(d.join("New folder (2)").is_dir());
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    #[ignore = "replaces the user's clipboard; run with --ignored"]
    fn clipboard_roundtrip() {
        let paths = vec![PathBuf::from(r"C:\a b\one.txt"), PathBuf::from(r"C:\two.txt")];
        clipboard_put(None, &paths, true).unwrap();
        assert_eq!(clipboard_get(None), Some((paths.clone(), true)));
        clipboard_put(None, &paths[..1], false).unwrap();
        assert_eq!(clipboard_get(None), Some((paths[..1].to_vec(), false)));
    }
}
