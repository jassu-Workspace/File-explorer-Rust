use crate::app::App;
use crate::layout::Rect;
use crate::shell::{self, Place};
use std::path::{Component, Path};

const HEADER_H: f32 = 28.0;
const ITEM_H: f32 = 28.0;
const SECTION_GAP: f32 = 12.0;

#[derive(Debug, Default)]
pub struct Sidebar {
    pub favorites: Vec<Place>,
    pub drives: Vec<Place>,
    pub hover: Option<usize>,
}

fn lower_components(p: &Path) -> Vec<String> {
    p.components()
        .filter(|c| !matches!(c, Component::CurDir))
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect()
}

/// The item whose path is the longest component-wise, case-insensitive prefix of `current`.
pub fn best_match(items: &[Place], current: &Path) -> Option<usize> {
    let cur = lower_components(current);
    items
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let pc = lower_components(&p.path);
            cur.starts_with(&pc).then_some((i, pc.len()))
        })
        .max_by_key(|&(_, len)| len)
        .map(|(i, _)| i)
}

impl Sidebar {
    pub fn load() -> Sidebar {
        Sidebar { favorites: shell::favorites(), drives: shell::drives(), hover: None }
    }

    pub fn place(&self, i: usize) -> Option<&Place> {
        match i.checked_sub(self.favorites.len()) {
            None => self.favorites.get(i),
            Some(d) => self.drives.get(d),
        }
    }

    fn all(&self) -> Vec<Place> {
        self.favorites.iter().chain(&self.drives).cloned().collect()
    }

    /// Section header rects: (title, rect).
    fn headers(&self, area: Rect) -> [(&'static str, Rect); 2] {
        let fav = Rect::new(area.x + 16.0, area.y + 4.0, area.w - 24.0, HEADER_H);
        let loc_y = fav.bottom() + self.favorites.len() as f32 * ITEM_H + SECTION_GAP;
        [("Favorites", fav), ("Locations", Rect::new(area.x + 16.0, loc_y, area.w - 24.0, HEADER_H))]
    }

    pub fn item_rects(&self, area: Rect) -> Vec<(usize, Rect)> {
        let [(_, fav), (_, loc)] = self.headers(area);
        let row = |y: f32| Rect::new(area.x + 8.0, y, area.w - 16.0, ITEM_H);
        let favs = (0..self.favorites.len()).map(|i| (i, row(fav.bottom() + i as f32 * ITEM_H)));
        let n = self.favorites.len();
        let drives = (0..self.drives.len()).map(move |i| (n + i, row(loc.bottom() + i as f32 * ITEM_H)));
        favs.chain(drives).filter(|(_, r)| r.bottom() <= area.bottom()).collect()
    }

    pub fn hit(&self, x: f32, y: f32, area: Rect) -> Option<usize> {
        self.item_rects(area).into_iter().find(|(_, r)| r.contains(x, y)).map(|(i, _)| i)
    }
}

pub fn paint(app: &App) {
    let (g, t, s) = (&app.gfx, &app.theme, &app.sidebar);
    let area = app.layout.sidebar;
    for (title, r) in s.headers(area) {
        if r.bottom() <= area.bottom() {
            g.text(title, r, &g.fonts.small, t.text_dim);
        }
    }
    let current = best_match(&s.all(), app.current_dir());
    for (i, r) in s.item_rects(area) {
        let Some(p) = s.place(i) else { continue };
        if current == Some(i) {
            g.fill_round(r, 6.0, t.text.with_alpha(if t.dark { 0.10 } else { 0.08 }));
        } else if s.hover == Some(i) {
            g.fill_round(r, 6.0, t.hover);
        }
        g.glyph(p.glyph, Rect::new(r.x + 8.0, r.y + 6.0, 16.0, 16.0), t.accent);
        g.text(&p.name, Rect::new(r.x + 32.0, r.y, r.w - 38.0, r.h), &g.fonts.body, t.text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw::Glyph;
    use crate::shell::Place;
    use std::path::PathBuf;

    fn place(p: &str) -> Place {
        Place { name: p.into(), path: PathBuf::from(p), glyph: Glyph::Doc }
    }

    #[test]
    fn best_match_picks_longest_prefix() {
        let items = [place(r"C:\Users\me"), place(r"C:\Users\me\Documents"), place("C:\\")];
        assert_eq!(best_match(&items, Path::new(r"C:\Users\me\Documents\x")), Some(1));
        assert_eq!(best_match(&items, Path::new(r"c:\users\ME")), Some(0));
        assert_eq!(best_match(&items, Path::new(r"C:\Windows")), Some(2));
        assert_eq!(best_match(&items, Path::new(r"D:\x")), None);
        assert_eq!(best_match(&items, Path::new(r"C:\Users\meow")), Some(2), "component match, not string prefix");
    }

    #[test]
    fn item_hit_maps_rows() {
        let s = Sidebar { favorites: vec![place("a"), place("b")], drives: vec![place("C:\\")], hover: None };
        let area = Rect::new(0.0, 52.0, 200.0, 600.0);
        let rects = s.item_rects(area);
        assert_eq!(rects.len(), 3);
        let (i, r) = rects[2];
        assert_eq!(i, 2);
        assert_eq!(s.hit(r.x + 5.0, r.y + 5.0, area), Some(2));
        assert_eq!(s.hit(5.0, 53.0, area), None, "section header is not an item");
        assert_eq!(s.place(2).unwrap().path, PathBuf::from("C:\\"));
    }
}
