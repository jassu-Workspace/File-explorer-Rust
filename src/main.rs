#![windows_subsystem = "windows"]

mod app;
mod dragdrop;
mod draw;
mod edit;
mod format;
mod layout;
mod listing;
mod motion;
mod nav;
mod prefs;
mod search;
mod selection;
mod share;
mod settings;
mod shell;
mod theme;
mod tags;
mod thumbs;
mod transfer;
mod views;

use windows::Win32::System::Ole::OleInitialize;

fn main() -> windows::core::Result<()> {
    // IFileOperation, the shell and drag and drop need OLE (an STA) on the UI thread.
    unsafe { OleInitialize(None)? };
    app::run()
}
