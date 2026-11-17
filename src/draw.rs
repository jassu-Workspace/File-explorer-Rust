use crate::layout::Rect;
use crate::theme::Color;
use std::collections::HashMap;
use windows::Win32::Foundation::{D2DERR_RECREATE_TARGET, HWND};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_BEZIER_SEGMENT, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_BEGIN_HOLLOW,
    D2D1_FIGURE_END_CLOSED, D2D1_FIGURE_END_OPEN, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory, WICBitmapDitherTypeNone, WICBitmapPaletteTypeMedianCut,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::UI::Controls::{IImageList, ILD_NORMAL};
use windows::Win32::UI::Shell::{SHGetImageList, SHIL_LARGE, SHIL_SMALL};
use windows::Win32::UI::WindowsAndMessaging::DestroyIcon;
use windows::core::{Result, w};
use windows_numerics::{Matrix3x2, Vector2};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Glyph {
    Back,
    Forward,
    Search,
    List,
    Columns,
    Folder,
    Home,
    Desktop,
    Doc,
    Download,
    Picture,
    Drive,
    Min,
    Max,
    Restore,
    Close,
    Chevron,
    SortUp,
    SortDown,
}

/// Path data in a 16x16 box: `M x y`, `L x y`, `C x1 y1 x2 y2 x y`, `Z`, `F` (filled figure).
fn glyph_path(g: Glyph) -> &'static str {
    match g {
        Glyph::Back => "M 10 3 L 5 8 L 10 13",
        Glyph::Forward => "M 6 3 L 11 8 L 6 13",
        Glyph::Search => "M 11.5 7 C 11.5 9.5 9.5 11.5 7 11.5 C 4.5 11.5 2.5 9.5 2.5 7 C 2.5 4.5 4.5 2.5 7 2.5 C 9.5 2.5 11.5 4.5 11.5 7 Z M 10.3 10.3 L 14 14",
        Glyph::List => "M 3 4 L 13 4 M 3 8 L 13 8 M 3 12 L 13 12",
        Glyph::Columns => "M 2 3 L 14 3 L 14 13 L 2 13 Z M 6 3 L 6 13 M 10 3 L 10 13",
        Glyph::Folder => "F M 1.5 4.5 C 1.5 3.6 2.1 3 3 3 L 6.2 3 L 7.7 4.5 L 13 4.5 C 13.9 4.5 14.5 5.1 14.5 6 L 14.5 12 C 14.5 12.9 13.9 13.5 13 13.5 L 3 13.5 C 2.1 13.5 1.5 12.9 1.5 12 Z",
        Glyph::Home => "M 2.5 7.5 L 8 2.5 L 13.5 7.5 M 4 6.5 L 4 13.5 L 12 13.5 L 12 6.5",
        Glyph::Desktop => "M 2 3 L 14 3 L 14 11 L 2 11 Z M 5.5 14 L 10.5 14 M 8 11 L 8 14",
        Glyph::Doc => "M 4 2 L 9.5 2 L 12.5 5 L 12.5 14 L 4 14 Z M 9.5 2 L 9.5 5 L 12.5 5",
        Glyph::Download => "M 8 2 L 8 10.5 M 4.5 7 L 8 10.5 L 11.5 7 M 3 13.5 L 13 13.5",
        Glyph::Picture => "M 2 3 L 14 3 L 14 13 L 2 13 Z M 2 11 L 6 7 L 9 10 L 11 8 L 14 11",
        Glyph::Drive => "M 2 9 L 14 9 L 14 13 L 2 13 Z M 3.5 9 L 5 4 L 11 4 L 12.5 9 M 11 11 L 11.2 11",
        Glyph::Min => "M 4 8 L 12 8",
        Glyph::Max => "M 4.5 4.5 L 11.5 4.5 L 11.5 11.5 L 4.5 11.5 Z",
        Glyph::Restore => "M 4.5 6.5 L 9.5 6.5 L 9.5 11.5 L 4.5 11.5 Z M 6.5 6.5 L 6.5 4.5 L 11.5 4.5 L 11.5 9.5 L 9.5 9.5",
        Glyph::Close => "M 4.5 4.5 L 11.5 11.5 M 11.5 4.5 L 4.5 11.5",
        Glyph::Chevron => "M 6 4 L 10 8 L 6 12",
        Glyph::SortUp => "M 4 10 L 8 6 L 12 10",
        Glyph::SortDown => "M 4 6 L 8 10 L 12 6",
    }
}

const ALL_GLYPHS: [Glyph; 19] = [
    Glyph::Back,
    Glyph::Forward,
    Glyph::Search,
    Glyph::List,
    Glyph::Columns,
    Glyph::Folder,
    Glyph::Home,
    Glyph::Desktop,
    Glyph::Doc,
    Glyph::Download,
    Glyph::Picture,
    Glyph::Drive,
    Glyph::Min,
    Glyph::Max,
    Glyph::Restore,
    Glyph::Close,
    Glyph::Chevron,
    Glyph::SortUp,
    Glyph::SortDown,
];

fn v(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

fn build_geometry(f: &ID2D1Factory, path: &str) -> Result<(ID2D1PathGeometry, bool)> {
    let geo = unsafe { f.CreatePathGeometry()? };
    let sink = unsafe { geo.Open()? };
    let toks: Vec<&str> = path.split_whitespace().collect();
    let filled = toks.first() == Some(&"F");
    let begin = if filled { D2D1_FIGURE_BEGIN_FILLED } else { D2D1_FIGURE_BEGIN_HOLLOW };
    let n = |i: usize| -> f32 { toks.get(i).and_then(|t| t.parse().ok()).unwrap_or(0.0) };
    let mut i = filled as usize;
    let mut open = false;
    while i < toks.len() {
        unsafe {
            match toks[i] {
                "M" => {
                    if open {
                        sink.EndFigure(D2D1_FIGURE_END_OPEN);
                    }
                    sink.BeginFigure(v(n(i + 1), n(i + 2)), begin);
                    open = true;
                    i += 3;
                }
                "L" => {
                    sink.AddLine(v(n(i + 1), n(i + 2)));
                    i += 3;
                }
                "C" => {
                    let s = D2D1_BEZIER_SEGMENT { point1: v(n(i + 1), n(i + 2)), point2: v(n(i + 3), n(i + 4)), point3: v(n(i + 5), n(i + 6)) };
                    sink.AddBezier(&s);
                    i += 7;
                }
                "Z" => {
                    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                    open = false;
                    i += 1;
                }
                _ => i += 1,
            }
        }
    }
    unsafe {
        if open {
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
        }
        sink.Close()?;
    }
    Ok((geo, filled))
}

pub struct Fonts {
    pub body: IDWriteTextFormat,
    pub body_right: IDWriteTextFormat,
    pub title: IDWriteTextFormat,
    pub small: IDWriteTextFormat,
    pub big: IDWriteTextFormat,
}

fn text_format(dw: &IDWriteFactory, size: f32, weight: DWRITE_FONT_WEIGHT, align: DWRITE_TEXT_ALIGNMENT) -> Result<IDWriteTextFormat> {
    unsafe {
        let f = dw.CreateTextFormat(w!("Segoe UI Variable Text"), None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!(""))?;
        f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        f.SetTextAlignment(align)?;
        let sign = dw.CreateEllipsisTrimmingSign(&f)?;
        let trim = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
        f.SetTrimming(&trim, &sign)?;
        Ok(f)
    }
}

pub struct Gfx {
    factory: ID2D1Factory,
    dw: IDWriteFactory,
    pub fonts: Fonts,
    glyphs: HashMap<Glyph, (ID2D1PathGeometry, bool)>,
    rt: Option<ID2D1HwndRenderTarget>,
    brush: Option<ID2D1SolidColorBrush>,
    icons: HashMap<i32, Option<ID2D1Bitmap>>,
    wic: Option<IWICImagingFactory>,
    dpi: f32,
}

fn rf(r: Rect) -> D2D_RECT_F {
    D2D_RECT_F { left: r.x, top: r.y, right: r.x + r.w, bottom: r.y + r.h }
}

fn cf(c: Color) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: c.r, g: c.g, b: c.b, a: c.a }
}

impl Gfx {
    pub fn new() -> Result<Gfx> {
        let factory: ID2D1Factory = unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)? };
        let dw: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
        let fonts = Fonts {
            body: text_format(&dw, 13.0, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_LEADING)?,
            body_right: text_format(&dw, 13.0, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_TRAILING)?,
            title: text_format(&dw, 13.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_LEADING)?,
            small: text_format(&dw, 11.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_LEADING)?,
            big: text_format(&dw, 15.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_CENTER)?,
        };
        let mut glyphs = HashMap::new();
        for g in ALL_GLYPHS {
            glyphs.insert(g, build_geometry(&factory, glyph_path(g))?);
        }
        Ok(Gfx { factory, dw, fonts, glyphs, rt: None, brush: None, icons: HashMap::new(), wic: None, dpi: 96.0 })
    }

    /// Creates the render target on first use or after device loss.
    pub fn ensure_target(&mut self, hwnd: HWND, px_w: u32, px_h: u32) -> Result<()> {
        if self.rt.is_some() {
            return Ok(());
        }
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: self.dpi,
            dpiY: self.dpi,
            // Software (WARP): GPU drivers add ~35 MB private memory per process; WARP keeps us under 25 MB.
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            ..Default::default()
        };
        let hprops = D2D1_HWND_RENDER_TARGET_PROPERTIES {
            hwnd,
            pixelSize: D2D_SIZE_U { width: px_w.max(1), height: px_h.max(1) },
            presentOptions: D2D1_PRESENT_OPTIONS_NONE,
        };
        let rt = unsafe { self.factory.CreateHwndRenderTarget(&props, &hprops)? };
        unsafe { rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE) };
        self.brush = Some(unsafe { rt.CreateSolidColorBrush(&cf(Color::TRANSPARENT), None)? });
        self.rt = Some(rt);
        Ok(())
    }

    pub fn resize(&mut self, px_w: u32, px_h: u32) {
        if let Some(rt) = &self.rt {
            let _ = unsafe { rt.Resize(&D2D_SIZE_U { width: px_w.max(1), height: px_h.max(1) }) };
        }
    }

    pub fn set_dpi(&mut self, dpi: f32) {
        self.dpi = dpi;
        if let Some(rt) = &self.rt {
            unsafe { rt.SetDpi(dpi, dpi) };
        }
    }

    pub fn has_target(&self) -> bool {
        self.rt.is_some()
    }

    pub fn begin(&self, clear: Color) {
        if let Some(rt) = &self.rt {
            unsafe {
                rt.BeginDraw();
                rt.SetTransform(&Matrix3x2::identity());
                rt.Clear(Some(&cf(clear)));
            }
        }
    }

    /// Ends the frame. On device loss drops the target and device-bound caches so the next paint rebuilds them.
    pub fn end(&mut self) -> Result<()> {
        let Some(rt) = &self.rt else { return Ok(()) };
        match unsafe { rt.EndDraw(None, None) } {
            Err(e) if e.code() == D2DERR_RECREATE_TARGET => {
                self.rt = None;
                self.brush = None;
                self.icons.clear();
                Ok(())
            }
            r => r,
        }
    }

    fn brush(&self, c: Color) -> Option<(&ID2D1HwndRenderTarget, &ID2D1SolidColorBrush)> {
        let (rt, b) = (self.rt.as_ref()?, self.brush.as_ref()?);
        unsafe { b.SetColor(&cf(c)) };
        Some((rt, b))
    }

    pub fn fill_rect(&self, r: Rect, c: Color) {
        if let Some((rt, b)) = self.brush(c) {
            unsafe { rt.FillRectangle(&rf(r), b) };
        }
    }

    pub fn fill_round(&self, r: Rect, radius: f32, c: Color) {
        if let Some((rt, b)) = self.brush(c) {
            let rr = D2D1_ROUNDED_RECT { rect: rf(r), radiusX: radius, radiusY: radius };
            unsafe { rt.FillRoundedRectangle(&rr, b) };
        }
    }

    pub fn line(&self, x1: f32, y1: f32, x2: f32, y2: f32, c: Color) {
        if let Some((rt, b)) = self.brush(c) {
            unsafe { rt.DrawLine(v(x1, y1), v(x2, y2), b, 1.0, None) };
        }
    }

    pub fn text(&self, s: &str, r: Rect, f: &IDWriteTextFormat, c: Color) {
        if let Some((rt, b)) = self.brush(c) {
            let w: Vec<u16> = s.encode_utf16().collect();
            unsafe { rt.DrawText(&w, f, &rf(r), b, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL) };
        }
    }

    pub fn measure(&self, s: &str, f: &IDWriteTextFormat) -> f32 {
        let w: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            let Ok(layout) = self.dw.CreateTextLayout(&w, f, 10_000.0, 100.0) else { return 0.0 };
            let mut m = DWRITE_TEXT_METRICS::default();
            if layout.GetMetrics(&mut m).is_ok() { m.widthIncludingTrailingWhitespace } else { 0.0 }
        }
    }

    /// Draws a 16x16-unit glyph scaled into `r` (square, centered).
    pub fn glyph(&self, g: Glyph, r: Rect, c: Color) {
        let Some((rt, b)) = self.brush(c) else { return };
        let Some((geo, filled)) = self.glyphs.get(&g) else { return };
        let s = r.w.min(r.h) / 16.0;
        let (ox, oy) = (r.x + (r.w - 16.0 * s) / 2.0, r.y + (r.h - 16.0 * s) / 2.0);
        let m = Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: ox, M32: oy };
        unsafe {
            rt.SetTransform(&m);
            if *filled {
                rt.FillGeometry(geo, b, None);
            } else {
                // Stroke width is in glyph units; 1.3 DIP on screen at any size.
                rt.DrawGeometry(geo, b, 1.3 / s, None);
            }
            rt.SetTransform(&Matrix3x2::identity());
        }
    }

    pub fn push_clip(&self, r: Rect) {
        if let Some(rt) = &self.rt {
            unsafe { rt.PushAxisAlignedClip(&rf(r), D2D1_ANTIALIAS_MODE_ALIASED) };
        }
    }

    pub fn pop_clip(&self) {
        if let Some(rt) = &self.rt {
            unsafe { rt.PopAxisAlignedClip() };
        }
    }

    pub fn target(&self) -> Option<&ID2D1HwndRenderTarget> {
        self.rt.as_ref()
    }

    /// System image-list icon as a D2D bitmap, converted once per index and cached until device loss.
    /// `large` picks the 32 px list for high-DPI screens.
    pub fn icon(&mut self, index: i32, large: bool) -> Option<ID2D1Bitmap> {
        let key = if large { index | (1 << 30) } else { index };
        if let Some(b) = self.icons.get(&key) {
            return b.clone();
        }
        let b = self.load_icon(index, large).ok();
        self.icons.insert(key, b.clone());
        b
    }

    fn load_icon(&mut self, index: i32, large: bool) -> Result<ID2D1Bitmap> {
        let rt = self.rt.as_ref().ok_or_else(windows::core::Error::empty)?;
        if self.wic.is_none() {
            self.wic = Some(unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)? });
        }
        let wic = self.wic.as_ref().unwrap();
        unsafe {
            let list: IImageList = SHGetImageList((if large { SHIL_LARGE } else { SHIL_SMALL }) as i32)?;
            let hicon = list.GetIcon(index, ILD_NORMAL.0)?;
            let src = wic.CreateBitmapFromHICON(hicon);
            let _ = DestroyIcon(hicon);
            let conv = wic.CreateFormatConverter()?;
            conv.Initialize(&src?, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut)?;
            rt.CreateBitmapFromWicBitmap(&conv, None)
        }
    }

    pub fn bitmap(&self, bmp: &ID2D1Bitmap, r: Rect) {
        if let Some(rt) = &self.rt {
            unsafe { rt.DrawBitmap(bmp, Some(&rf(r)), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None) };
        }
    }
}
