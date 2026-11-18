//! Explorer-style settings: start from the user's Windows Explorer settings, with Files-only overrides in prefs.
use crate::prefs::Prefs;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::UI::Shell::{SHELLFLAGSTATE, SHGetSettings, SSF_DOUBLECLICKINWEBVIEW};
use windows::core::w;

const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClickMode {
    /// Explorer's "Single-click to open an item (point to select)".
    Single,
    Double,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PanelStyle {
    Pill,
    Card,
    Stack,
}

/// The user's Windows Explorer settings.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WinDefaults {
    pub double_click: bool,
    pub show_ext: bool,
    pub show_hidden: bool,
    pub show_system: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    Click,
    Ext,
    Hidden,
    System,
    Panel,
}

impl Key {
    fn pref(self) -> &'static str {
        match self {
            Key::Click => "@click",
            Key::Ext => "@ext",
            Key::Hidden => "@hidden",
            Key::System => "@system",
            Key::Panel => "@panel",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Settings {
    pub click: ClickMode,
    pub show_ext: bool,
    pub show_hidden: bool,
    pub show_system: bool,
    pub panel: PanelStyle,
}

/// Sets (or with `None`, removes) a Files-only override.
pub fn set_override(p: &mut Prefs, key: Key, value: Option<&str>) {
    p.set(key.pref(), value.unwrap_or(""));
}

/// Drops the overrides of settings that come from Windows (the panel style is ours and stays).
pub fn reset_to_windows(p: &mut Prefs) {
    for k in [Key::Click, Key::Ext, Key::Hidden, Key::System] {
        set_override(p, k, None);
    }
}

impl Settings {
    pub fn resolve(win: WinDefaults, p: &Prefs) -> Settings {
        let flag = |k: Key, default: bool| match p.get(k.pref()) {
            Some("1") => true,
            Some("0") => false,
            _ => default,
        };
        Settings {
            click: match p.get(Key::Click.pref()) {
                Some("single") => ClickMode::Single,
                Some("double") => ClickMode::Double,
                _ if win.double_click => ClickMode::Double,
                _ => ClickMode::Single,
            },
            show_ext: flag(Key::Ext, win.show_ext),
            show_hidden: flag(Key::Hidden, win.show_hidden),
            show_system: flag(Key::System, win.show_system),
            panel: match p.get(Key::Panel.pref()) {
                Some("card") => PanelStyle::Card,
                Some("stack") => PanelStyle::Stack,
                _ => PanelStyle::Pill,
            },
        }
    }

    /// Explorer's rules: hidden items need "show hidden"; hidden + system ("protected") items need "show protected files".
    pub fn visible(&self, attrs: u32) -> bool {
        let (h, s) = (attrs & FILE_ATTRIBUTE_HIDDEN != 0, attrs & FILE_ATTRIBUTE_SYSTEM != 0);
        match (h, s) {
            (true, true) => self.show_system,
            (true, false) => self.show_hidden,
            _ => true,
        }
    }
}

/// True when a click (press and release on one item) should open it.
pub fn click_opens(mode: ClickMode, moved_px: f32, drag_threshold_px: f32, modifiers: bool) -> bool {
    mode == ClickMode::Single && moved_px < drag_threshold_px && !modifiers
}

fn advanced(name: windows::core::PCWSTR) -> Option<u32> {
    let mut data = 0u32;
    let mut size = 4u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced"),
            name,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as _),
            Some(&mut size),
        )
    };
    r.is_ok().then_some(data)
}

/// Reads the user's Explorer settings (Folder Options > General and View).
pub fn read_windows() -> WinDefaults {
    let mut sfs = SHELLFLAGSTATE::default();
    unsafe { SHGetSettings(&mut sfs, SSF_DOUBLECLICKINWEBVIEW.0) };
    // fDoubleClickInWebView is bit 5 of the flag word.
    let double_click = sfs._bitfield & (1 << 5) != 0;
    WinDefaults {
        double_click,
        // Windows hides extensions unless HideFileExt is 0.
        show_ext: advanced(w!("HideFileExt")) == Some(0),
        show_hidden: advanced(w!("Hidden")) == Some(1),
        show_system: advanced(w!("ShowSuperHidden")) == Some(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prefs::Prefs;

    fn win() -> WinDefaults {
        WinDefaults { double_click: false, show_ext: true, show_hidden: false, show_system: false }
    }

    #[test]
    fn windows_values_apply_without_overrides() {
        let s = Settings::resolve(win(), &Prefs::new(10));
        assert_eq!(s.click, ClickMode::Single);
        assert!(s.show_ext && !s.show_hidden && !s.show_system);
        assert_eq!(s.panel, PanelStyle::Pill);
    }

    #[test]
    fn overrides_win_and_reset_restores_windows() {
        let mut p = Prefs::new(10);
        set_override(&mut p, Key::Click, Some("double"));
        set_override(&mut p, Key::Ext, Some("0"));
        set_override(&mut p, Key::Panel, Some("stack"));
        let s = Settings::resolve(win(), &p);
        assert_eq!((s.click, s.show_ext, s.panel), (ClickMode::Double, false, PanelStyle::Stack));
        reset_to_windows(&mut p);
        let s = Settings::resolve(win(), &p);
        assert_eq!((s.click, s.show_ext), (ClickMode::Single, true));
        assert_eq!(s.panel, PanelStyle::Stack, "panel style is ours, not a Windows setting");
    }

    #[test]
    fn single_click_opens_only_without_drag_or_modifiers() {
        assert!(click_opens(ClickMode::Single, 2.0, 4.0, false));
        assert!(!click_opens(ClickMode::Single, 9.0, 4.0, false), "moved: it was a drag");
        assert!(!click_opens(ClickMode::Single, 0.0, 4.0, true), "Ctrl/Shift click selects");
        assert!(!click_opens(ClickMode::Double, 0.0, 4.0, false));
    }

    #[test]
    fn visibility_rules_match_explorer() {
        let (hidden, system, protected) = (0x2, 0x4, 0x6);
        let s = |h, sys| Settings { show_hidden: h, show_system: sys, ..Settings::resolve(win(), &Prefs::new(1)) };
        assert!(!s(false, false).visible(hidden) && s(true, false).visible(hidden));
        assert!(!s(true, false).visible(protected), "hidden+system needs the protected-files setting");
        assert!(s(true, true).visible(protected));
        assert!(s(false, false).visible(0x20) && s(true, false).visible(system));
    }
}
