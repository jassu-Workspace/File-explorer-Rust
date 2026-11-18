use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NavKind {
    Visit,
    Back,
    Forward,
    Refresh,
}

/// Back/forward stacks. Changes are committed only after the target folder loads,
/// so a folder that fails to open never enters history.
#[derive(Debug)]
pub struct History {
    back: Vec<PathBuf>,
    forward: Vec<PathBuf>,
    current: PathBuf,
}

impl History {
    pub fn new(start: PathBuf) -> History {
        History { back: Vec::new(), forward: Vec::new(), current: start }
    }
    pub fn current(&self) -> &Path {
        &self.current
    }
    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }
    pub fn can_forward(&self) -> bool {
        !self.forward.is_empty()
    }
    pub fn back_target(&self) -> Option<&Path> {
        self.back.last().map(PathBuf::as_path)
    }
    pub fn forward_target(&self) -> Option<&Path> {
        self.forward.last().map(PathBuf::as_path)
    }
    pub fn parent(&self) -> Option<PathBuf> {
        self.current.parent().map(Path::to_path_buf)
    }
    pub fn commit(&mut self, kind: NavKind, target: PathBuf) {
        match kind {
            NavKind::Visit if target != self.current => {
                self.back.push(std::mem::replace(&mut self.current, target));
                self.forward.clear();
            }
            NavKind::Back if self.back.last() == Some(&target) => {
                self.back.pop();
                self.forward.push(std::mem::replace(&mut self.current, target));
            }
            NavKind::Forward if self.forward.last() == Some(&target) => {
                self.forward.pop();
                self.back.push(std::mem::replace(&mut self.current, target));
            }
            _ => {}
        }
    }
}

/// Tags async jobs; results from older jobs are dropped.
#[derive(Default, Debug)]
pub struct Generation(AtomicU64);

impl Generation {
    pub fn next(&self) -> u64 {
        self.0.fetch_add(1, SeqCst) + 1
    }
    pub fn is_current(&self, g: u64) -> bool {
        self.0.load(SeqCst) == g
    }
}

#[derive(Debug)]
pub enum Outcome {
    Stale,
    Failed(String),
    Loaded,
}

#[derive(Debug)]
pub struct Column {
    pub path: PathBuf,
    /// `None` while loading, or after being freed off-screen.
    pub entries: Option<Vec<crate::listing::Entry>>,
    pub selected: Option<usize>,
    pub scroll_y: f32,
}

/// Finder-style column view: each selected folder opens a column to the right.
#[derive(Debug, Default)]
pub struct Columns {
    pub cols: Vec<Column>,
    pub scroll_x: f32,
    pub focus_col: usize,
}

impl Columns {
    pub fn open_root(&mut self, path: PathBuf) {
        self.cols = vec![Column { path, entries: None, selected: None, scroll_y: 0.0 }];
        self.scroll_x = 0.0;
        self.focus_col = 0;
    }

    /// Selects a row; returns the folder to load when the row is a folder.
    pub fn select(&mut self, col: usize, row: usize) -> Option<PathBuf> {
        let c = self.cols.get_mut(col)?;
        let e = c.entries.as_ref()?.get(row)?;
        let (is_dir, path) = (e.is_dir(), c.path.join(&e.name));
        c.selected = Some(row);
        self.cols.truncate(col + 1);
        self.focus_col = col;
        if !is_dir {
            return None;
        }
        self.cols.push(Column { path: path.clone(), entries: None, selected: None, scroll_y: 0.0 });
        Some(path)
    }

    pub fn loaded(&mut self, path: &Path, entries: Vec<crate::listing::Entry>) {
        if let Some(c) = self.cols.iter_mut().find(|c| c.path == path) {
            c.entries = Some(entries);
        }
    }

    pub fn current_dir(&self) -> &Path {
        self.cols.last().map_or(Path::new(""), |c| c.path.as_path())
    }

    /// Frees entries of columns scrolled fully out on the left; path and selection stay so they can reload.
    pub fn drop_offscreen(&mut self, _viewport_w: f32, col_w: f32) {
        for (i, c) in self.cols.iter_mut().enumerate() {
            if (i as f32 + 1.0) * col_w <= self.scroll_x {
                c.entries = None;
            }
        }
    }
}

pub fn error_message(target: &Path, e: &io::Error) -> String {
    let why = match e.kind() {
        io::ErrorKind::PermissionDenied => "Access is denied".to_string(),
        io::ErrorKind::NotFound => "Folder not found".to_string(),
        _ => e.to_string(),
    };
    format!("Can't open {}: {why}.", target.display())
}

pub fn apply_listing(h: &mut History, gen_ok: bool, err: Option<&io::Error>, target: PathBuf, kind: NavKind) -> Outcome {
    if !gen_ok {
        return Outcome::Stale;
    }
    if let Some(e) = err {
        return Outcome::Failed(error_message(&target, e));
    }
    h.commit(kind, target);
    Outcome::Loaded
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::path::{Path, PathBuf};

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn history_back_forward() {
        let mut h = History::new(p(r"C:\a"));
        h.commit(NavKind::Visit, p(r"C:\b"));
        h.commit(NavKind::Visit, p(r"C:\c"));
        assert_eq!(h.back_target(), Some(Path::new(r"C:\b")));
        h.commit(NavKind::Back, p(r"C:\b"));
        h.commit(NavKind::Back, p(r"C:\a"));
        assert_eq!(h.current(), Path::new(r"C:\a"));
        assert!(!h.can_back());
        assert_eq!(h.forward_target(), Some(Path::new(r"C:\b")));
        h.commit(NavKind::Forward, p(r"C:\b"));
        assert_eq!(h.current(), Path::new(r"C:\b"));
        h.commit(NavKind::Visit, p(r"C:\d"));
        assert!(!h.can_forward());
        assert_eq!(h.back_target(), Some(Path::new(r"C:\b")));
    }

    #[test]
    fn visit_same_folder_is_not_a_new_entry() {
        let mut h = History::new(p(r"C:\a"));
        h.commit(NavKind::Visit, p(r"C:\a"));
        assert!(!h.can_back());
    }

    #[test]
    fn parent_of_root_is_none() {
        assert_eq!(History::new(p("C:\\")).parent(), None);
        assert_eq!(History::new(p(r"C:\a\b")).parent(), Some(p(r"C:\a")));
    }

    #[test]
    fn stale_generation_is_dropped() {
        let g = Generation::default();
        let g1 = g.next();
        let g2 = g.next();
        assert!(!g.is_current(g1));
        assert!(g.is_current(g2));
    }

    #[test]
    fn nav_failure_keeps_current() {
        let mut h = History::new(p(r"C:\a"));
        let err = io::Error::from(io::ErrorKind::PermissionDenied);
        assert!(matches!(apply_listing(&mut h, true, Some(&err), p(r"C:\b"), NavKind::Visit), Outcome::Failed(_)));
        assert_eq!(h.current(), Path::new(r"C:\a"));
        assert!(matches!(apply_listing(&mut h, false, None, p(r"C:\b"), NavKind::Visit), Outcome::Stale));
        assert_eq!(h.current(), Path::new(r"C:\a"));
        assert!(matches!(apply_listing(&mut h, true, None, p(r"C:\b"), NavKind::Visit), Outcome::Loaded));
        assert_eq!(h.current(), Path::new(r"C:\b"));
    }

    #[test]
    fn failure_message_is_readable() {
        let mut h = History::new(p(r"C:\a"));
        let err = io::Error::from(io::ErrorKind::PermissionDenied);
        let Outcome::Failed(m) = apply_listing(&mut h, true, Some(&err), p(r"C:\b"), NavKind::Visit) else { panic!() };
        assert!(m.contains(r"C:\b") && m.to_lowercase().contains("access"), "{m}");
    }
}

#[cfg(test)]
mod column_tests {
    use super::*;
    use crate::listing::Entry;
    use std::path::{Path, PathBuf};

    fn e(name: &str, dir: bool) -> Entry {
        Entry { name: name.into(), size: 0, modified: 0, created: 0, attrs: if dir { 0x10 } else { 0x20 }, rel_dir: None }
    }

    fn root() -> Columns {
        let mut c = Columns::default();
        c.open_root(PathBuf::from(r"C:\r"));
        c.loaded(Path::new(r"C:\r"), vec![e("a", true), e("b", true), e("f.txt", false)]);
        c
    }

    #[test]
    fn select_dir_pushes_column() {
        let mut c = root();
        assert_eq!(c.select(0, 0), Some(PathBuf::from(r"C:\r\a")));
        assert_eq!(c.cols.len(), 2);
        assert!(c.cols[1].entries.is_none());
        assert_eq!(c.current_dir(), Path::new(r"C:\r\a"));
    }

    #[test]
    fn select_truncates_right() {
        let mut c = root();
        c.select(0, 0);
        c.loaded(Path::new(r"C:\r\a"), vec![e("deep", true)]);
        c.select(1, 0);
        assert_eq!(c.cols.len(), 3);
        assert_eq!(c.select(0, 1), Some(PathBuf::from(r"C:\r\b")));
        assert_eq!(c.cols.len(), 2);
        assert_eq!(c.cols[1].path, PathBuf::from(r"C:\r\b"));
    }

    #[test]
    fn select_file_no_push() {
        let mut c = root();
        c.select(0, 0);
        assert_eq!(c.select(0, 2), None);
        assert_eq!(c.cols.len(), 1);
        assert_eq!(c.cols[0].selected, Some(2));
        assert_eq!(c.current_dir(), Path::new(r"C:\r"));
    }

    #[test]
    fn loaded_ignores_unknown() {
        let mut c = root();
        c.loaded(Path::new(r"C:\elsewhere"), vec![e("x", false)]);
        assert_eq!(c.cols.len(), 1);
        assert_eq!(c.cols[0].entries.as_ref().unwrap().len(), 3);
    }

    #[test]
    fn select_out_of_range_is_ignored() {
        let mut c = root();
        assert_eq!(c.select(0, 99), None);
        assert_eq!(c.select(5, 0), None);
        assert_eq!(c.cols[0].selected, None);
    }

    #[test]
    fn drop_offscreen_frees_left_columns() {
        let mut c = root();
        c.select(0, 0);
        c.loaded(Path::new(r"C:\r\a"), vec![e("deep", true)]);
        c.scroll_x = 300.0;
        c.drop_offscreen(500.0, 220.0);
        assert!(c.cols[0].entries.is_none(), "column 0 ends at 220 < scroll_x 300");
        assert!(c.cols[1].entries.is_some());
        assert_eq!(c.cols[0].selected, Some(0), "selection survives");
    }
}
