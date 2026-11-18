//! Finder-style color tags, stored locally in `%APPDATA%\Files\tags.tsv` (one `path<TAB>Color,Color` line per file).
//! Keyed by path, case-insensitively. Renames, moves and deletes done in this app carry tags along.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Tag {
    Red,
    Orange,
    Yellow,
    Green,
    Blue,
    Purple,
    Gray,
}

impl Tag {
    pub const ALL: [Tag; 7] = [Tag::Red, Tag::Orange, Tag::Yellow, Tag::Green, Tag::Blue, Tag::Purple, Tag::Gray];

    pub fn name(self) -> &'static str {
        match self {
            Tag::Red => "Red",
            Tag::Orange => "Orange",
            Tag::Yellow => "Yellow",
            Tag::Green => "Green",
            Tag::Blue => "Blue",
            Tag::Purple => "Purple",
            Tag::Gray => "Gray",
        }
    }

    /// Finder's tag colors.
    pub fn rgb(self) -> u32 {
        match self {
            Tag::Red => 0xFF3B30,
            Tag::Orange => 0xFF9500,
            Tag::Yellow => 0xFFCC00,
            Tag::Green => 0x28CD41,
            Tag::Blue => 0x007AFF,
            Tag::Purple => 0xAF52DE,
            Tag::Gray => 0x8E8E93,
        }
    }

    fn parse(s: &str) -> Option<Tag> {
        Tag::ALL.into_iter().find(|t| t.name() == s)
    }
}

/// path (lowercased) -> (original path, sorted tags)
#[derive(Default, Debug)]
pub struct Tags {
    map: BTreeMap<String, (PathBuf, Vec<Tag>)>,
}

fn key(p: &Path) -> String {
    p.to_string_lossy().to_lowercase()
}

/// True when `child` equals `parent` or sits inside it (component-wise, case-insensitive).
fn within(child: &str, parent: &str) -> bool {
    child == parent || child.strip_prefix(parent).is_some_and(|rest| rest.starts_with('\\'))
}

impl Tags {
    pub fn of(&self, p: &Path) -> Vec<Tag> {
        self.map.get(&key(p)).map(|e| e.1.clone()).unwrap_or_default()
    }

    /// Adds `tag` to all paths, or removes it when every path already has it (Finder's toggle).
    pub fn toggle(&mut self, paths: &[PathBuf], tag: Tag) {
        let all = paths.iter().all(|p| self.of(p).contains(&tag));
        for p in paths {
            let e = self.map.entry(key(p)).or_insert_with(|| (p.clone(), Vec::new()));
            e.1.retain(|t| *t != tag);
            if !all {
                e.1.push(tag);
                e.1.sort();
            }
        }
        self.map.retain(|_, e| !e.1.is_empty());
    }

    /// Moves tags of `old` (and of everything inside it) to `new`.
    pub fn renamed(&mut self, old: &Path, new: &Path) {
        let (ok, nk) = (key(old), new.to_string_lossy().into_owned());
        // Skip by characters of the original-case path: lowercasing can change byte lengths.
        let old_chars = old.to_string_lossy().chars().count();
        let moved: Vec<String> = self.map.keys().filter(|k| within(k, &ok)).cloned().collect();
        for k in moved {
            if let Some((orig, tags)) = self.map.remove(&k) {
                let rest: String = orig.to_string_lossy().chars().skip(old_chars).collect();
                let path = PathBuf::from(format!("{nk}{rest}"));
                self.map.insert(key(&path), (path, tags));
            }
        }
    }

    /// Forgets tags of `p` and everything inside it.
    pub fn deleted(&mut self, p: &Path) {
        let k = key(p);
        self.map.retain(|m, _| !within(m, &k));
    }

    pub fn files(&self, tag: Tag) -> Vec<PathBuf> {
        self.map.values().filter(|e| e.1.contains(&tag)).map(|e| e.0.clone()).collect()
    }

    pub fn prune(&mut self, exists: impl Fn(&Path) -> bool) {
        self.map.retain(|_, e| exists(&e.0));
    }

    pub fn to_tsv(&self) -> String {
        self.map.values().map(|(p, t)| format!("{}\t{}\n", p.display(), t.iter().map(|t| t.name()).collect::<Vec<_>>().join(","))).collect()
    }

    pub fn from_tsv(text: &str) -> Tags {
        let mut t = Tags::default();
        for line in text.lines() {
            let Some((path, list)) = line.split_once('\t') else { continue };
            let tags: Vec<Tag> = list.split(',').filter_map(Tag::parse).collect();
            if !tags.is_empty() {
                let p = PathBuf::from(path);
                t.map.insert(key(&p), (p, tags));
            }
        }
        t
    }

    fn file() -> Option<PathBuf> {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Files").join("tags.tsv"))
    }

    /// Loads the store and drops tags of files that no longer exist.
    pub fn load() -> Tags {
        let mut t = Tags::file().and_then(|f| std::fs::read_to_string(f).ok()).map(|s| Tags::from_tsv(&s)).unwrap_or_default();
        t.prune(|p| p.exists());
        t
    }

    /// Writes atomically (temp file + rename) so a crash cannot leave a half-written store.
    pub fn save(&self) -> std::io::Result<()> {
        let Some(f) = Tags::file() else { return Ok(()) };
        if let Some(dir) = f.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = f.with_extension("tsv.tmp");
        std::fs::write(&tmp, self.to_tsv())?;
        std::fs::rename(&tmp, &f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn toggle_adds_and_removes() {
        let mut t = Tags::default();
        t.toggle(&[p(r"C:\a.txt"), p(r"C:\b.txt")], Tag::Red);
        assert_eq!(t.of(Path::new(r"c:\A.TXT")), vec![Tag::Red], "paths match case-insensitively");
        t.toggle(&[p(r"C:\a.txt"), p(r"C:\b.txt")], Tag::Red);
        assert!(t.of(Path::new(r"C:\a.txt")).is_empty(), "all had it, so toggling removes it");
        t.toggle(&[p(r"C:\a.txt")], Tag::Blue);
        t.toggle(&[p(r"C:\a.txt"), p(r"C:\b.txt")], Tag::Blue);
        assert_eq!(t.of(Path::new(r"C:\b.txt")), vec![Tag::Blue], "mixed selection: toggling adds to all");
    }

    #[test]
    fn tsv_roundtrip_skips_bad_lines() {
        let mut t = Tags::default();
        t.toggle(&[p(r"C:\x y\é.txt")], Tag::Green);
        t.toggle(&[p(r"C:\x y\é.txt")], Tag::Purple);
        let text = t.to_tsv();
        let back = Tags::from_tsv(&(text + "garbage line\nC:\\z\tNotAColor\n"));
        assert_eq!(back.of(Path::new(r"C:\x y\é.txt")), vec![Tag::Green, Tag::Purple]);
        assert!(back.of(Path::new(r"C:\z")).is_empty());
    }

    #[test]
    fn rename_and_delete_follow_folders() {
        let mut t = Tags::default();
        t.toggle(&[p(r"C:\d\inner.txt"), p(r"C:\d")], Tag::Yellow);
        t.toggle(&[p(r"C:\dx.txt")], Tag::Yellow);
        t.renamed(Path::new(r"C:\d"), Path::new(r"C:\e"));
        assert_eq!(t.of(Path::new(r"C:\e\inner.txt")), vec![Tag::Yellow]);
        assert_eq!(t.of(Path::new(r"C:\e")), vec![Tag::Yellow]);
        assert_eq!(t.of(Path::new(r"C:\dx.txt")), vec![Tag::Yellow], "a sibling with a shared name prefix is untouched");
        t.deleted(Path::new(r"C:\e"));
        assert!(t.of(Path::new(r"C:\e\inner.txt")).is_empty());
        assert_eq!(t.files(Tag::Yellow), vec![p(r"C:\dx.txt")]);
    }

    #[test]
    fn prune_drops_missing_files() {
        let mut t = Tags::default();
        t.toggle(&[p(r"C:\keep"), p(r"C:\gone")], Tag::Gray);
        t.prune(|p| p.ends_with("keep"));
        assert_eq!(t.files(Tag::Gray), vec![p(r"C:\keep")]);
    }
}
