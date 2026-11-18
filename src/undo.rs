//! Undo/redo of file changes (Ctrl+Z / Ctrl+Y), including putting recycled items back from the Recycle Bin.
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// Items moved or renamed: (from, to).
    Moved(Vec<(PathBuf, PathBuf)>),
    /// Items sent to the Recycle Bin.
    Recycled(Vec<PathBuf>),
    /// Items that now exist (copies, new folders, restored items); undoing recycles them.
    Created(Vec<PathBuf>),
}

impl Change {
    /// The change that reverses this one.
    pub fn inverse(&self) -> Change {
        match self {
            Change::Moved(pairs) => Change::Moved(pairs.iter().map(|(a, b)| (b.clone(), a.clone())).collect()),
            Change::Recycled(p) => Change::Created(p.clone()),
            Change::Created(p) => Change::Recycled(p.clone()),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Change::Moved(p) => p.is_empty(),
            Change::Recycled(p) | Change::Created(p) => p.is_empty(),
        }
    }
}

const LIMIT: usize = 50;

#[derive(Default, Debug)]
pub struct History {
    undo: Vec<Change>,
    redo: Vec<Change>,
}

impl History {
    /// Records a change the user made; a new change clears what could be redone.
    pub fn push(&mut self, c: Change) {
        if c.is_empty() {
            return;
        }
        self.undo.push(c);
        if self.undo.len() > LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// The change to apply for Ctrl+Z (already inverted).
    pub fn undo(&mut self) -> Option<Change> {
        let c = self.undo.pop()?;
        let inv = c.inverse();
        self.redo.push(c);
        Some(inv)
    }

    /// The change to apply for Ctrl+Y.
    pub fn redo(&mut self) -> Option<Change> {
        let c = self.redo.pop()?;
        self.undo.push(c.clone());
        Some(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn undo_reverses_and_redo_replays() {
        let mut h = History::default();
        h.push(Change::Moved(vec![(p(r"C:\a\x"), p(r"C:\b\x"))]));
        h.push(Change::Recycled(vec![p(r"C:\a\y")]));
        assert_eq!(h.undo(), Some(Change::Created(vec![p(r"C:\a\y")])), "undo recycle = put back");
        assert_eq!(h.undo(), Some(Change::Moved(vec![(p(r"C:\b\x"), p(r"C:\a\x"))])), "undo move = move back");
        assert_eq!(h.undo(), None);
        assert_eq!(h.redo(), Some(Change::Moved(vec![(p(r"C:\a\x"), p(r"C:\b\x"))])));
        assert_eq!(h.redo(), Some(Change::Recycled(vec![p(r"C:\a\y")])));
        assert_eq!(h.redo(), None);
    }

    #[test]
    fn new_change_clears_redo_and_empty_is_ignored() {
        let mut h = History::default();
        h.push(Change::Created(vec![p(r"C:\n")]));
        h.undo();
        h.push(Change::Recycled(Vec::new()));
        assert!(h.redo.len() == 1, "empty change ignored");
        h.push(Change::Recycled(vec![p(r"C:\z")]));
        assert_eq!(h.redo(), None);
    }
}
