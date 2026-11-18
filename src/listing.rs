use std::cmp::Ordering;
use std::ffi::OsString;
use std::io;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

const ATTR_HIDDEN: u32 = 0x2;
const ATTR_SYSTEM: u32 = 0x4;
const ATTR_DIRECTORY: u32 = 0x10;
const ATTR_REPARSE: u32 = 0x400;

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: OsString,
    pub size: u64,
    /// FILETIME ticks (100 ns since 1601, UTC).
    pub modified: u64,
    pub attrs: u32,
    /// Parent folder relative to the search root; `None` for plain listings.
    pub rel_dir: Option<PathBuf>,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.attrs & ATTR_DIRECTORY != 0
    }
    pub fn is_hidden(&self) -> bool {
        self.attrs & (ATTR_HIDDEN | ATTR_SYSTEM) != 0
    }
    pub fn is_reparse(&self) -> bool {
        self.attrs & ATTR_REPARSE != 0
    }
    pub fn ext_lower(&self) -> String {
        if self.is_dir() {
            return String::new();
        }
        Path::new(&self.name).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
    }
}

/// On Windows `DirEntry::metadata` comes from the FindNextFileW data: no extra syscall.
pub fn entry_from(de: &std::fs::DirEntry) -> io::Result<Entry> {
    let m = de.metadata()?;
    Ok(Entry { name: de.file_name(), size: m.file_size(), modified: m.last_write_time(), attrs: m.file_attributes(), rel_dir: None })
}

pub fn read_folder(path: &Path, show_hidden: bool) -> io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    for de in std::fs::read_dir(path)? {
        let Ok(e) = de.and_then(|de| entry_from(&de)) else { continue };
        if show_hidden || !e.is_hidden() {
            out.push(e);
        }
    }
    Ok(out)
}

/// A run of entries that share a group label (Finder's "Group by").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub label: String,
    pub range: std::ops::Range<usize>,
}

/// Stable-sorts entries by `key` = (rank, label) and returns the contiguous groups.
/// Order inside a group is the order the entries already had (the user's sort).
pub fn group_entries(entries: &mut Vec<Entry>, mut key: impl FnMut(&Entry) -> (i64, String)) -> Vec<Group> {
    let mut keyed: Vec<((i64, String), Entry)> = entries.drain(..).map(|e| (key(&e), e)).collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    let mut groups: Vec<Group> = Vec::new();
    for (i, (k, e)) in keyed.into_iter().enumerate() {
        match groups.last_mut() {
            Some(g) if g.label == k.1 => g.range.end = i + 1,
            _ => groups.push(Group { label: k.1, range: i..i + 1 }),
        }
        entries.push(e);
    }
    groups
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortKey {
    Name,
    Modified,
    Size,
    Kind,
}

/// Natural order on lowercased names: digit runs compare by numeric value.
fn nat_str(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.as_bytes(), b.as_bytes());
    loop {
        match (x.first(), y.first()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let n = x.iter().take_while(|c| c.is_ascii_digit()).count();
                let m = y.iter().take_while(|c| c.is_ascii_digit()).count();
                let (dx, dy) = (trim_zeros(&x[..n]), trim_zeros(&y[..m]));
                let o = dx.len().cmp(&dy.len()).then(dx.cmp(dy));
                if o != Ordering::Equal {
                    return o;
                }
                x = &x[n..];
                y = &y[m..];
            }
            (Some(c), Some(d)) => {
                if c != d {
                    return c.cmp(d);
                }
                x = &x[1..];
                y = &y[1..];
            }
        }
    }
}

fn trim_zeros(d: &[u8]) -> &[u8] {
    let z = d.iter().take_while(|&&c| c == b'0').count();
    &d[z.min(d.len().saturating_sub(1))..]
}

/// Folders always come first; ties break by natural name.
pub fn sort_entries(entries: &mut [Entry], key: SortKey, descending: bool) {
    // Lowercase each name once; comparing allocated keys per call is too slow for 100k rows.
    let names: Vec<String> = entries.iter().map(|e| e.name.to_string_lossy().to_lowercase()).collect();
    let exts: Vec<String> = if key == SortKey::Kind { entries.iter().map(Entry::ext_lower).collect() } else { Vec::new() };
    let mut idx: Vec<usize> = (0..entries.len()).collect();
    idx.sort_by(|&i, &j| {
        let (a, b) = (&entries[i], &entries[j]);
        b.is_dir().cmp(&a.is_dir()).then_with(|| {
            let o = match key {
                SortKey::Name => Ordering::Equal,
                SortKey::Modified => a.modified.cmp(&b.modified),
                SortKey::Size => a.size.cmp(&b.size),
                SortKey::Kind => exts[i].cmp(&exts[j]),
            }
            .then_with(|| nat_str(&names[i], &names[j]));
            if descending { o.reverse() } else { o }
        })
    });
    // Apply the permutation in place by following cycles.
    let mut done = vec![false; idx.len()];
    for start in 0..idx.len() {
        let mut cur = start;
        while !done[cur] {
            done[cur] = true;
            let next = idx[cur];
            if next != start && !done[next] {
                entries.swap(cur, next);
            }
            cur = next;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::Instant;

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("fx-test-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn names(v: &[Entry]) -> Vec<String> {
        v.iter().map(|e| e.name.to_string_lossy().into_owned()).collect()
    }

    fn fake(name: &str, dir: bool, size: u64, modified: u64) -> Entry {
        Entry { name: name.into(), size, modified, attrs: if dir { 0x10 } else { 0x20 }, rel_dir: None }
    }

    #[test]
    fn read_folder_lists_files_and_dirs() {
        let d = tmp("list");
        fs::write(d.join("a.txt"), b"hello").unwrap();
        fs::create_dir(d.join("sub")).unwrap();
        let v = read_folder(&d, true).unwrap();
        assert_eq!(v.len(), 2);
        let a = v.iter().find(|e| e.name == "a.txt").unwrap();
        assert_eq!(a.size, 5);
        assert!(!a.is_dir());
        assert!(v.iter().find(|e| e.name == "sub").unwrap().is_dir());
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn read_folder_hides_hidden_when_asked() {
        let d = tmp("hidden");
        let f = d.join("secret.txt");
        fs::write(&f, b"x").unwrap();
        assert!(Command::new("attrib").arg("+h").arg(&f).status().unwrap().success());
        assert!(read_folder(&d, false).unwrap().is_empty());
        assert_eq!(read_folder(&d, true).unwrap().len(), 1);
        let _ = Command::new("attrib").arg("-h").arg(&f).status();
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn read_folder_denied_returns_err() {
        assert!(read_folder(Path::new(r"C:\System Volume Information"), true).is_err());
    }

    #[test]
    fn read_folder_missing_returns_err() {
        assert!(read_folder(Path::new(r"C:\definitely\not\here\fx"), true).is_err());
    }

    #[test]
    fn natural_sort_orders_numbers() {
        let mut v = vec![fake("file10", false, 0, 0), fake("File2", false, 0, 0), fake("file1", false, 0, 0)];
        sort_entries(&mut v, SortKey::Name, false);
        assert_eq!(names(&v), ["file1", "File2", "file10"]);
    }

    #[test]
    fn natural_sort_unicode() {
        let mut v: Vec<Entry> = ["Zeta", "émile", "日本", "alpha", "😀"].iter().map(|n| fake(n, false, 0, 0)).collect();
        sort_entries(&mut v, SortKey::Name, false);
        let n = names(&v);
        assert!(n.iter().position(|x| x == "alpha") < n.iter().position(|x| x == "Zeta"));
        assert_eq!(n.len(), 5);
    }

    #[test]
    fn sort_folders_first_both_directions() {
        for desc in [false, true] {
            let mut v = vec![fake("a", false, 0, 0), fake("z", true, 0, 0)];
            sort_entries(&mut v, SortKey::Name, desc);
            assert_eq!(names(&v), ["z", "a"], "descending={desc}");
        }
    }

    #[test]
    fn sort_by_size_and_modified() {
        let mut v = vec![fake("a", false, 3, 1), fake("b", false, 1, 3), fake("c", false, 2, 2)];
        sort_entries(&mut v, SortKey::Size, false);
        assert_eq!(names(&v), ["b", "c", "a"]);
        sort_entries(&mut v, SortKey::Size, true);
        assert_eq!(names(&v), ["a", "c", "b"]);
        sort_entries(&mut v, SortKey::Modified, false);
        assert_eq!(names(&v), ["a", "c", "b"]);
    }

    #[test]
    fn sort_by_kind_uses_extension() {
        let mut v = vec![fake("b.txt", false, 0, 0), fake("a.pdf", false, 0, 0), fake("c.TXT", false, 0, 0)];
        sort_entries(&mut v, SortKey::Kind, false);
        assert_eq!(names(&v), ["a.pdf", "b.txt", "c.TXT"]);
    }

    #[test]
    fn grouping_is_stable_and_contiguous() {
        let mut v = vec![fake("b1", false, 0, 0), fake("a1", false, 0, 0), fake("b2", false, 0, 0), fake("a2", false, 0, 0)];
        let groups = group_entries(&mut v, |e| (0, e.name.to_string_lossy()[..1].to_uppercase()));
        assert_eq!(names(&v), ["a1", "a2", "b1", "b2"], "within a group the existing order stays");
        assert_eq!(groups.iter().map(|g| (g.label.as_str(), g.range.clone())).collect::<Vec<_>>(), [("A", 0..2), ("B", 2..4)]);
        assert!(group_entries(&mut Vec::new(), |_| (0, String::new())).is_empty());
    }

    #[test]
    fn group_rank_beats_label() {
        let mut v = vec![fake("x", false, 1, 0), fake("y", false, 2, 0)];
        let groups = group_entries(&mut v, |e| (-(e.size as i64), format!("s{}", e.size)));
        assert_eq!(groups[0].label, "s2");
    }

    #[test]
    fn sort_100k_under_100ms() {
        if cfg!(debug_assertions) {
            return; // timing only meaningful in --release
        }
        let mut v: Vec<Entry> = (0..100_000u64).map(|i| fake(&format!("file{}", (i * 7919) % 100_000), i % 10 == 0, i, i)).collect();
        let t = Instant::now();
        sort_entries(&mut v, SortKey::Name, false);
        assert!(t.elapsed().as_millis() < 100, "took {:?}", t.elapsed());
    }
}
