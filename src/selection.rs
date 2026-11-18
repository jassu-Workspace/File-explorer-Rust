use std::ffi::OsStr;
use std::time::{Duration, Instant};

/// Sorted selected indexes plus anchor (for Shift) and focus (keyboard cursor).
#[derive(Default, Debug, Clone)]
pub struct Selection {
    sel: Vec<usize>,
    anchor: Option<usize>,
    focus: Option<usize>,
}

impl Selection {
    pub fn new() -> Selection {
        Selection::default()
    }
    pub fn clear(&mut self) {
        *self = Selection::default();
    }
    pub fn click(&mut self, i: usize) {
        self.sel = vec![i];
        self.anchor = Some(i);
        self.focus = Some(i);
    }
    pub fn ctrl_click(&mut self, i: usize) {
        match self.sel.binary_search(&i) {
            Ok(p) => {
                self.sel.remove(p);
            }
            Err(p) => self.sel.insert(p, i),
        }
        self.anchor = Some(i);
        self.focus = Some(i);
    }
    pub fn shift_click(&mut self, i: usize) {
        let a = *self.anchor.get_or_insert(i);
        self.sel = (a.min(i)..=a.max(i)).collect();
        self.focus = Some(i);
    }
    pub fn move_focus(&mut self, delta: isize, extend: bool, count: usize) {
        if count == 0 {
            return;
        }
        let cur = self.focus.unwrap_or(0) as isize;
        let next = (cur + delta).clamp(0, count as isize - 1) as usize;
        if extend { self.shift_click(next) } else { self.click(next) }
    }
    /// Replaces the selected set (drag-box selection); focus and anchor stay.
    pub fn set(&mut self, mut idx: Vec<usize>) {
        idx.sort_unstable();
        idx.dedup();
        self.sel = idx;
    }
    /// Moves focus (and the Shift anchor) to `i`, e.g. after rows were reordered.
    pub fn set_focus(&mut self, i: Option<usize>) {
        self.focus = i;
        self.anchor = i;
    }
    pub fn select_all(&mut self, count: usize) {
        self.sel = (0..count).collect();
    }
    pub fn is_selected(&self, i: usize) -> bool {
        self.sel.binary_search(&i).is_ok()
    }
    pub fn focus(&self) -> Option<usize> {
        self.focus
    }
    pub fn selected(&self) -> Vec<usize> {
        self.sel.clone()
    }
}

const TYPE_AHEAD_RESET: Duration = Duration::from_secs(1);

#[derive(Default, Debug)]
pub struct TypeAhead {
    buf: String,
    last: Option<Instant>,
}

impl TypeAhead {
    pub fn push(&mut self, ch: char, now: Instant) -> &str {
        if self.last.is_none_or(|l| now.duration_since(l) > TYPE_AHEAD_RESET) {
            self.buf.clear();
        }
        self.buf.push(ch);
        self.last = Some(now);
        &self.buf
    }
}

pub fn find_prefix<'a>(mut names: impl Iterator<Item = &'a OsStr>, prefix: &str) -> Option<usize> {
    let p = prefix.to_lowercase();
    names.position(|n| n.to_string_lossy().to_lowercase().starts_with(&p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::time::{Duration, Instant};

    #[test]
    fn click_ctrl_shift() {
        let mut s = Selection::new();
        s.click(2);
        s.ctrl_click(5);
        assert_eq!(s.selected(), [2, 5]);
        s.shift_click(8);
        assert_eq!(s.selected(), [5, 6, 7, 8]);
        s.ctrl_click(6);
        assert_eq!(s.selected(), [5, 7, 8]);
        assert_eq!(s.focus(), Some(6));
    }

    #[test]
    fn move_focus_extends() {
        let mut s = Selection::new();
        s.click(3);
        s.move_focus(2, true, 10);
        assert_eq!(s.selected(), [3, 4, 5]);
        s.move_focus(100, false, 10);
        assert_eq!(s.selected(), [9]);
        s.move_focus(-100, false, 10);
        assert_eq!(s.selected(), [0]);
        let mut e = Selection::new();
        e.move_focus(1, false, 0);
        assert!(e.selected().is_empty());
    }

    #[test]
    fn set_replaces_selection_keeping_focus() {
        let mut s = Selection::new();
        s.click(4);
        s.set(vec![7, 2, 7]);
        assert_eq!(s.selected(), [2, 7]);
        assert_eq!(s.focus(), Some(4));
    }

    #[test]
    fn select_all_and_clear() {
        let mut s = Selection::new();
        s.select_all(4);
        assert_eq!(s.selected(), [0, 1, 2, 3]);
        assert!(s.is_selected(3));
        s.clear();
        assert!(s.selected().is_empty());
        assert_eq!(s.focus(), None);
    }

    #[test]
    fn type_ahead_resets_after_1s() {
        let mut t = TypeAhead::default();
        let t0 = Instant::now();
        t.push('a', t0);
        assert_eq!(t.push('b', t0 + Duration::from_millis(500)), "ab");
        assert_eq!(t.push('c', t0 + Duration::from_millis(1600)), "c");
    }

    #[test]
    fn find_prefix_case_insensitive() {
        let names = ["alpha", "Notes.txt", "nothing"].map(OsStr::new);
        assert_eq!(find_prefix(names.into_iter(), "no"), Some(1));
        assert_eq!(find_prefix(names.into_iter(), "NOTH"), Some(2));
        assert_eq!(find_prefix(names.into_iter(), "zz"), None);
    }
}
