//! The Windows share sheet for selected files (Finder's Share button).
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use windows::ApplicationModel::DataTransfer::{DataRequestedEventArgs, DataTransferManager};
use windows::Foundation::TypedEventHandler;
use windows::Storage::{IStorageItem, StorageFile, StorageFolder};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::IDataTransferManagerInterop;
use windows::core::{HSTRING, Interface, Result, factory};
use windows_collections::IIterable;

/// Title shown at the top of the share sheet.
pub fn share_title(paths: &[PathBuf]) -> String {
    match paths {
        [] => String::new(),
        [one] => one.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        many => format!("{} items", many.len()),
    }
}

fn storage_item(p: &std::path::Path) -> Result<IStorageItem> {
    let h = HSTRING::from(p.as_os_str());
    if p.is_dir() { StorageFolder::GetFolderFromPathAsync(&h)?.join()?.cast() } else { StorageFile::GetFileFromPathAsync(&h)?.join()?.cast() }
}

/// Registered once per window; `show` sets the paths the next share request hands out.
pub struct Share {
    interop: IDataTransferManagerInterop,
    paths: Arc<Mutex<Vec<PathBuf>>>,
}

impl Share {
    pub fn register(hwnd: HWND) -> Result<Share> {
        let interop = factory::<DataTransferManager, IDataTransferManagerInterop>()?;
        let dtm: DataTransferManager = unsafe { interop.GetForWindow(hwnd)? };
        let paths = Arc::new(Mutex::new(Vec::<PathBuf>::new()));
        let shared = paths.clone();
        dtm.DataRequested(&TypedEventHandler::<DataTransferManager, DataRequestedEventArgs>::new(move |_, args| {
            let Some(args) = args.as_ref() else { return Ok(()) };
            let paths = shared.lock().map(|p| p.clone()).unwrap_or_default();
            let data = args.Request()?.Data()?;
            data.Properties()?.SetTitle(&HSTRING::from(share_title(&paths)))?;
            let items: Vec<Option<IStorageItem>> = paths.iter().filter_map(|p| storage_item(p).ok()).map(Some).collect();
            data.SetStorageItems(&IIterable::<IStorageItem>::from(items), true)
        }))?;
        Ok(Share { interop, paths })
    }

    pub fn show(&self, hwnd: HWND, paths: Vec<PathBuf>) -> Result<()> {
        if let Ok(mut p) = self.paths.lock() {
            *p = paths;
        }
        unsafe { self.interop.ShowShareUIForWindow(hwnd) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn share_titles() {
        assert_eq!(share_title(&[PathBuf::from(r"C:\x\alpha.txt")]), "alpha.txt");
        assert_eq!(share_title(&[PathBuf::from(r"C:\a"), PathBuf::from(r"C:\b"), PathBuf::from(r"C:\c")]), "3 items");
        assert_eq!(share_title(&[]), "");
    }
}
