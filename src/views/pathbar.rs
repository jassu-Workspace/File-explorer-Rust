use crate::app::App;
use crate::draw::Glyph;
use crate::format::format_size;
use crate::layout::Rect;
use std::path::{Component, Path, PathBuf};

const GAP: f32 = 20.0;

/// Breadcrumbs: (label, cumulative path). The first crumb is the drive or UNC share.
pub fn crumbs(path: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let mut acc = PathBuf::new();
    for c in path.components() {
        match c {
            Component::Prefix(p) => {
                acc.push(p.as_os_str());
                let label = p.as_os_str().to_string_lossy().trim_end_matches('\\').to_string();
                out.push((label, PathBuf::new()));
            }
            Component::RootDir => {
                acc.push("\\");
                if let Some(last) = out.last_mut() {
                    last.1 = acc.clone();
                }
            }
            Component::Normal(n) => {
                acc.push(n);
                out.push((n.to_string_lossy().into_owned(), acc.clone()));
            }
            _ => {}
        }
    }
    out
}

/// Places crumbs left to right; when they do not fit, drops leading ones so the current folder stays visible.
/// Returns (crumb index, x, width).
pub fn crumb_layout(widths: &[f32], start_x: f32, max_x: f32, gap: f32) -> Vec<(usize, f32, f32)> {
    let mut first = 0;
    while first + 1 < widths.len() {
        let total: f32 = widths[first..].iter().sum::<f32>() + gap * (widths.len() - first - 1) as f32;
        if start_x + total <= max_x {
            break;
        }
        first += 1;
    }
    let mut x = start_x;
    (first..widths.len())
        .map(|i| {
            let r = (i, x, widths[i]);
            x += widths[i] + gap;
            r
        })
        .collect()
}

type Crumbs = Vec<(String, PathBuf)>;
type Placed = Vec<(usize, f32, f32)>;

fn layout_crumbs(app: &App) -> (Crumbs, Placed, Rect) {
    let bar = app.layout.pathbar;
    let cs = crumbs(app.current_dir());
    let widths: Vec<f32> = cs.iter().map(|c| app.gfx.measure(&c.0, &app.gfx.fonts.body)).collect();
    let status = Rect::new(bar.right() - 260.0, bar.y, 248.0, bar.h);
    let placed = crumb_layout(&widths, bar.x + 34.0, status.x - 12.0, GAP);
    (cs, placed, status)
}

pub fn status_text(app: &App) -> String {
    if let Some(s) = app.status_override() {
        return s;
    }
    let n = app.item_count();
    let items = if n == 1 { "1 item".to_string() } else { format!("{n} items") };
    match app.free {
        Some(f) => format!("{items}, {} available", format_size(f)),
        None => items,
    }
}

pub fn paint(app: &App) {
    let (g, t) = (&app.gfx, &app.theme);
    let bar = app.layout.pathbar;
    let (cs, placed, status) = layout_crumbs(app);
    g.glyph(Glyph::Drive, Rect::new(bar.x + 12.0, bar.y + (bar.h - 14.0) / 2.0, 14.0, 14.0), t.text_dim);
    for (k, &(i, x, w)) in placed.iter().enumerate() {
        let last = k + 1 == placed.len();
        g.text(&cs[i].0, Rect::new(x, bar.y, w + 2.0, bar.h), &g.fonts.body, if last { t.text } else { t.text_dim });
        if !last {
            g.glyph(Glyph::Chevron, Rect::new(x + w + 5.0, bar.y + (bar.h - 10.0) / 2.0, 10.0, 10.0), t.text_dim);
        }
    }
    g.text(&status_text(app), status, &g.fonts.body_right, t.text_dim);
}

pub fn hit(app: &App, x: f32, y: f32) -> Option<PathBuf> {
    let bar = app.layout.pathbar;
    if !bar.contains(x, y) {
        return None;
    }
    let (cs, placed, _) = layout_crumbs(app);
    placed.into_iter().find(|&(_, cx, w)| x >= cx && x < cx + w).map(|(i, ..)| cs[i].1.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn crumbs_drive_path() {
        let c = crumbs(Path::new(r"C:\Users\jaswa\Docs"));
        let names: Vec<_> = c.iter().map(|c| c.0.as_str()).collect();
        assert_eq!(names, ["C:", "Users", "jaswa", "Docs"]);
        assert_eq!(c[0].1, PathBuf::from("C:\\"));
        assert_eq!(c[2].1, PathBuf::from(r"C:\Users\jaswa"));
    }

    #[test]
    fn crumbs_unc_path() {
        let c = crumbs(Path::new(r"\\server\share\a"));
        let names: Vec<_> = c.iter().map(|c| c.0.as_str()).collect();
        assert_eq!(names, [r"\\server\share", "a"]);
        assert_eq!(c[1].1, PathBuf::from(r"\\server\share\a"));
    }

    #[test]
    fn crumb_layout_keeps_tail_when_narrow() {
        // widths 50 each, gap 20: all four need 50*4 + 20*3 = 260.
        let all = crumb_layout(&[50.0; 4], 0.0, 300.0, 20.0);
        assert_eq!(all.iter().map(|c| c.0).collect::<Vec<_>>(), [0, 1, 2, 3]);
        assert_eq!(all[1].1, 70.0);
        let tail = crumb_layout(&[50.0; 4], 0.0, 130.0, 20.0);
        assert_eq!(tail.iter().map(|c| c.0).collect::<Vec<_>>(), [2, 3]);
        assert_eq!(crumb_layout(&[500.0], 0.0, 100.0, 20.0).len(), 1, "last crumb always shown");
    }
}
