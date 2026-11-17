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
    FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Pictures, FOLDERID_Profile, KF_FLAG_DEFAULT, SHFILEINFOW, SHGFI_FLAGS,
    SHGFI_SMALLICON, SHGFI_SYSICONINDEX, SHGFI_TYPENAME, SHGFI_USEFILEATTRIBUTES, SHGetFileInfoW, SHGetKnownFolderPath,
};
use windows::core::{GUID, PCWSTR};

const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;

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

pub fn favorites() -> Vec<Place> {
    [
        ("Home", &FOLDERID_Profile, Glyph::Home),
        ("Desktop", &FOLDERID_Desktop, Glyph::Desktop),
        ("Documents", &FOLDERID_Documents, Glyph::Doc),
        ("Downloads", &FOLDERID_Downloads, Glyph::Download),
        ("Pictures", &FOLDERID_Pictures, Glyph::Picture),
    ]
    .into_iter()
    .filter_map(|(name, id, glyph)| known(id).filter(|p| p.is_dir()).map(|path| Place { name: name.into(), path, glyph }))
    .collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::listing::Entry;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    fn file(name: &str) -> Entry {
        Entry { name: name.into(), size: 0, modified: 0, attrs: 0x20, rel_dir: None }
    }

    #[test]
    fn favorites_exist() {
        let f = favorites();
        assert!(!f.is_empty());
        assert!(f.iter().all(|p| p.path.is_dir()));
        assert_eq!(f[0].name, "Home");
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
