use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::core::w;

pub const ROW_H: f32 = 24.0;
pub const COLUMN_W: f32 = 220.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const TRANSPARENT: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };

    pub fn rgb(hex: u32) -> Color {
        let c = |s: u32| ((hex >> s) & 0xFF) as f32 / 255.0;
        Color { r: c(16), g: c(8), b: c(0), a: 1.0 }
    }
    pub fn with_alpha(self, a: f32) -> Color {
        Color { a, ..self }
    }
    /// Multiplies the alpha (keeps translucent theme colors translucent while fading).
    pub fn fade(self, k: f32) -> Color {
        Color { a: self.a * k, ..self }
    }
    /// This color composited over an opaque background (for GDI, which has no alpha).
    pub fn over(self, bg: Color) -> Color {
        let m = |f: f32, b: f32| f * self.a + b * (1.0 - self.a);
        Color { r: m(self.r, bg.r), g: m(self.g, bg.g), b: m(self.b, bg.b), a: 1.0 }
    }
    /// GDI COLORREF (0x00BBGGRR).
    pub fn to_colorref(self) -> u32 {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
        c(self.r) | (c(self.g) << 8) | (c(self.b) << 16)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub dark: bool,
    pub window_bg: Color,
    pub sidebar_bg: Color,
    pub content_bg: Color,
    pub text: Color,
    pub text_dim: Color,
    pub separator: Color,
    pub hover: Color,
    pub accent: Color,
    pub selection_text: Color,
    pub field_bg: Color,
    pub close_hover: Color,
}

impl Theme {
    pub fn light(accent: Color) -> Theme {
        Theme {
            dark: false,
            window_bg: Color::rgb(0xF5F5F7),
            sidebar_bg: Color::rgb(0xEDEDF0),
            content_bg: Color::rgb(0xFFFFFF),
            text: Color::rgb(0x1D1D1F),
            text_dim: Color::rgb(0x6E6E73),
            separator: Color::rgb(0xE3E3E6),
            hover: Color::rgb(0x000000).with_alpha(0.05),
            accent,
            selection_text: Color::rgb(0xFFFFFF),
            field_bg: Color::rgb(0x000000).with_alpha(0.06),
            close_hover: Color::rgb(0xC42B1C),
        }
    }
    pub fn dark(accent: Color) -> Theme {
        Theme {
            dark: true,
            window_bg: Color::rgb(0x1E1E1E),
            sidebar_bg: Color::rgb(0x252526),
            content_bg: Color::rgb(0x1C1C1E),
            text: Color::rgb(0xF5F5F7),
            text_dim: Color::rgb(0x98989D),
            separator: Color::rgb(0x38383A),
            hover: Color::rgb(0xFFFFFF).with_alpha(0.06),
            accent,
            selection_text: Color::rgb(0xFFFFFF),
            field_bg: Color::rgb(0xFFFFFF).with_alpha(0.08),
            close_hover: Color::rgb(0xC42B1C),
        }
    }
    pub fn system() -> Theme {
        if system_prefers_dark() { Theme::dark(accent_color()) } else { Theme::light(accent_color()) }
    }
}

fn reg_dword(key: windows::core::PCWSTR, value: windows::core::PCWSTR) -> Option<u32> {
    let mut data = 0u32;
    let mut size = 4u32;
    let r = unsafe { RegGetValueW(HKEY_CURRENT_USER, key, value, RRF_RT_REG_DWORD, None, Some(&mut data as *mut u32 as _), Some(&mut size)) };
    r.is_ok().then_some(data)
}

pub fn system_prefers_dark() -> bool {
    reg_dword(w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"), w!("AppsUseLightTheme")) == Some(0)
}

/// The user's accent color. Stored as ABGR in the registry.
pub fn accent_color() -> Color {
    match reg_dword(w!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent"), w!("AccentColorMenu")) {
        Some(abgr) => Color::rgb(((abgr & 0xFF) << 16) | (abgr & 0xFF00) | ((abgr >> 16) & 0xFF)),
        None => Color::rgb(0x0067C0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lum(c: Color) -> f32 {
        let f = |v: f32| if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
        0.2126 * f(c.r) + 0.7152 * f(c.g) + 0.0722 * f(c.b)
    }

    fn contrast(a: Color, b: Color) -> f32 {
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn rgb_parses() {
        let c = Color::rgb(0x0067C0);
        assert_eq!((c.r, c.g, c.b, c.a), (0.0, 0x67 as f32 / 255.0, 0xC0 as f32 / 255.0, 1.0));
    }

    #[test]
    fn themes_contrast() {
        for t in [Theme::light(Color::rgb(0x0067C0)), Theme::dark(Color::rgb(0x4CC2FF))] {
            assert!(contrast(t.text, t.content_bg) >= 7.0, "dark={}", t.dark);
            assert!(contrast(t.text_dim, t.content_bg) >= 4.5, "dark={}", t.dark);
            assert!(contrast(t.text, t.window_bg) >= 7.0, "dark={}", t.dark);
        }
    }
}

#[cfg(test)]
mod blend_tests {
    use super::*;

    #[test]
    fn over_blends() {
        let c = Color::rgb(0x000000).with_alpha(0.5).over(Color::rgb(0xFFFFFF));
        assert!((c.r - 0.5).abs() < 1e-6 && c.a == 1.0);
        assert_eq!(Color::rgb(0x112233).to_colorref(), 0x332211);
    }
}
