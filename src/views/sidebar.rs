use crate::app::App;
use crate::draw::Glyph;
use crate::layout::Rect;
use crate::shell::{self, Place};
use std::path::{Component, Path};

const HEADER_H: f32 = 28.0;
const ITEM_H: f32 = 28.0;
const SECTION_GAP: f32 = 12.0;
pub const SECTIONS: [&str; 4] = ["Favorites", "Cloud", "Locations", "Tags"];

#[derive(Debug, Default)]
pub struct Sidebar {
    pub favorites: Vec<Place>,
    pub cloud: Vec<Place>,
    pub drives: Vec<Place>,
    pub tags: Vec<crate::tags::Tag>,
    /// Item under the mouse (index across all sections).
    pub hover: Option<usize>,
    pub hover_header: Option<usize>,
    pub collapsed: [bool; 4],
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
        Sidebar { favorites: shell::favorites(), cloud: shell::cloud(), drives: shell::drives(), tags: crate::tags::Tag::ALL.to_vec(), ..Default::default() }
    }

    fn sections(&self) -> [&[Place]; 3] {
        [&self.favorites, &self.cloud, &self.drives]
    }

    fn lens(&self) -> [usize; 4] {
        [self.favorites.len(), self.cloud.len(), self.drives.len(), self.tags.len()]
    }

    /// Item by index across Favorites, Cloud, Locations (tags come after them).
    pub fn place(&self, i: usize) -> Option<&Place> {
        self.sections().into_iter().flatten().nth(i)
    }

    /// Tag for an index in the Tags section.
    pub fn tag_at(&self, i: usize) -> Option<crate::tags::Tag> {
        let places: usize = self.lens()[..3].iter().sum();
        i.checked_sub(places).and_then(|k| self.tags.get(k).copied())
    }

    fn all(&self) -> Vec<Place> {
        self.sections().into_iter().flatten().cloned().collect()
    }

    /// Visible sections as (title, header rect, section index, first flat item index). Empty sections are skipped.
    fn layout(&self, area: Rect) -> Vec<(&'static str, Rect, usize, usize)> {
        let mut y = area.y + 4.0;
        let mut first = 0;
        let mut out = Vec::new();
        for (s, len) in self.lens().into_iter().enumerate() {
            if len > 0 {
                out.push((SECTIONS[s], Rect::new(area.x + 20.0, y, area.w - 32.0, HEADER_H), s, first));
                y += HEADER_H + if self.collapsed[s] { 0.0 } else { len as f32 * ITEM_H } + SECTION_GAP;
            }
            first += len;
        }
        out
    }

    #[cfg(test)]
    fn headers(&self, area: Rect) -> Vec<(&'static str, Rect)> {
        self.layout(area).into_iter().map(|(t, r, ..)| (t, r)).collect()
    }

    /// Section index of the header under the point.
    pub fn header_at(&self, x: f32, y: f32, area: Rect) -> Option<usize> {
        self.layout(area).into_iter().find(|(_, r, ..)| r.contains(x, y)).map(|(_, _, s, _)| s)
    }

    pub fn item_rects(&self, area: Rect) -> Vec<(usize, Rect)> {
        let lens = self.lens();
        let mut out = Vec::new();
        for (_, header, s, first) in self.layout(area) {
            if self.collapsed[s] {
                continue;
            }
            for k in 0..lens[s] {
                let r = Rect::new(area.x + 12.0, header.bottom() + k as f32 * ITEM_H, area.w - 24.0, ITEM_H);
                if r.bottom() <= area.bottom() {
                    out.push((first + k, r));
                }
            }
        }
        out
    }

    pub fn hit(&self, x: f32, y: f32, area: Rect) -> Option<usize> {
        self.item_rects(area).into_iter().find(|(_, r)| r.contains(x, y)).map(|(i, _)| i)
    }
}

/// The rounded inner panel the sidebar sits on (Sonoma style).
pub fn panel_rect(app: &App) -> Rect {
    let l = &app.layout;
    Rect::new(6.0, 6.0, l.sidebar.w - 12.0, l.pathbar.y - 12.0)
}

pub fn paint(app: &App) {
    let (g, t, s) = (&app.gfx, &app.theme, &app.sidebar);
    let area = app.layout.sidebar;
    let panel = if app.mica { t.text.with_alpha(if t.dark { 0.05 } else { 0.035 }) } else { t.sidebar_bg };
    g.fill_round(panel_rect(app), 10.0, panel);
    for (title, r, sec, _) in s.layout(area) {
        if r.bottom() <= area.bottom() {
            g.text(title, r, &g.fonts.small, t.text_dim);
            if s.hover_header == Some(sec) {
                let glyph = if s.collapsed[sec] { Glyph::Chevron } else { Glyph::SortDown };
                g.glyph(glyph, Rect::new(r.right() - 12.0, r.y + 9.0, 10.0, 10.0), t.text_dim);
            }
        }
    }
    let current = best_match(&s.all(), app.current_dir());
    for (i, r) in s.item_rects(area) {
        if let Some(tag) = s.tag_at(i) {
            if s.hover == Some(i) {
                g.fill_round(r, 6.0, t.hover);
            }
            g.fill_round(Rect::new(r.x + 11.0, r.y + 9.0, 10.0, 10.0), 5.0, crate::theme::Color::rgb(tag.rgb()));
            g.text(tag.name(), Rect::new(r.x + 32.0, r.y, r.w - 38.0, r.h), &g.fonts.body, t.text);
            continue;
        }
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

    fn sample() -> Sidebar {
        Sidebar { favorites: vec![place("a"), place("b")], cloud: vec![place("od")], drives: vec![place("C:\\")], ..Default::default() }
    }

    #[test]
    fn item_hit_maps_rows() {
        let s = sample();
        let area = Rect::new(0.0, 64.0, 200.0, 600.0);
        let rects = s.item_rects(area);
        assert_eq!(rects.len(), 4);
        let (i, r) = rects[3];
        assert_eq!(i, 3);
        assert_eq!(s.hit(r.x + 5.0, r.y + 5.0, area), Some(3));
        assert_eq!(s.hit(30.0, 66.0, area), None, "section header is not an item");
        assert_eq!(s.place(2).unwrap().name, "od");
        assert_eq!(s.place(3).unwrap().path, PathBuf::from("C:\\"));
        assert!(s.place(4).is_none());
    }

    #[test]
    fn header_click_collapses_section() {
        let mut s = sample();
        let area = Rect::new(0.0, 64.0, 200.0, 600.0);
        let fav_header = s.headers(area)[0].1;
        assert_eq!(s.header_at(fav_header.x + 2.0, fav_header.y + 2.0, area), Some(0));
        let cloud_y = s.headers(area)[1].1.y;
        s.collapsed[0] = true;
        assert!(s.item_rects(area).iter().all(|(i, _)| *i >= 2), "favorites hidden");
        assert!(s.headers(area)[1].1.y < cloud_y, "next section moves up");
    }

    #[test]
    fn empty_section_has_no_header() {
        let s = Sidebar { favorites: vec![place("a")], drives: vec![place("C:\\")], ..Default::default() };
        let titles: Vec<&str> = s.headers(Rect::new(0.0, 64.0, 200.0, 600.0)).iter().map(|h| h.0).collect();
        assert_eq!(titles, ["Favorites", "Locations"]);
    }
}
