#![windows_subsystem = "windows"]
// shortcut: allow dead_code while modules land ahead of their UI wiring, remove in Task 11.
#![allow(dead_code)]

mod app;
mod draw;
mod edit;
mod format;
mod layout;
mod listing;
mod nav;
mod search;
mod selection;
mod shell;
mod theme;
mod views;

use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};

fn main() -> windows::core::Result<()> {
    // IFileOperation and the shell need an STA on the UI thread.
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
    app::run()
}
