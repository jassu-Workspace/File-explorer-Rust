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

pub fn apply_listing(h: &mut History, gen_ok: bool, err: Option<&io::Error>, target: PathBuf, kind: NavKind) -> Outcome {
    if !gen_ok {
        return Outcome::Stale;
    }
    if let Some(e) = err {
        let why = match e.kind() {
            io::ErrorKind::PermissionDenied => "Access is denied".to_string(),
            io::ErrorKind::NotFound => "Folder not found".to_string(),
            _ => e.to_string(),
        };
        return Outcome::Failed(format!("Can't open {}: {why}.", target.display()));
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
