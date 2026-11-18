//! Small persistent settings in `%APPDATA%\Files\prefs.tsv`: the view per folder, and app flags under `@` keys.
use std::path::PathBuf;

/// Key -> value pairs, most recently set last; keys are case-insensitive. Oldest entries drop past `cap`.
pub struct Prefs {
    items: Vec<(String, String)>,
    cap: usize,
}

impl Prefs {
    pub fn new(cap: usize) -> Prefs {
        Prefs { items: Vec::new(), cap }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        let k = key.to_lowercase();
        self.items.iter().rev().find(|(ik, _)| *ik == k).map(|(_, v)| v.as_str())
    }

    pub fn set(&mut self, key: &str, value: &str) {
        let k = key.to_lowercase();
        self.items.retain(|(ik, _)| *ik != k);
        self.items.push((k, value.to_string()));
        if self.items.len() > self.cap {
            let extra = self.items.len() - self.cap;
            self.items.drain(..extra);
        }
    }

    pub fn to_tsv(&self) -> String {
        self.items.iter().map(|(k, v)| format!("{k}\t{v}\n")).collect()
    }

    pub fn from_tsv(text: &str, cap: usize) -> Prefs {
        let mut p = Prefs::new(cap);
        for (k, v) in text.lines().filter_map(|l| l.split_once('\t')) {
            p.set(k, v);
        }
        p
    }

    fn file() -> Option<PathBuf> {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Files").join("prefs.tsv"))
    }

    pub fn load() -> Prefs {
        Prefs::file().and_then(|f| std::fs::read_to_string(f).ok()).map_or_else(|| Prefs::new(2000), |s| Prefs::from_tsv(&s, 2000))
    }

    /// Writes atomically (temp file + rename).
    pub fn save(&self) -> std::io::Result<()> {
        let Some(f) = Prefs::file() else { return Ok(()) };
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

    #[test]
    fn set_get_case_insensitive() {
        let mut p = Prefs::new(10);
        p.set(r"C:\Users\Me", "list");
        assert_eq!(p.get(r"c:\users\me"), Some("list"));
        p.set(r"C:\USERS\ME", "columns");
        assert_eq!(p.get(r"C:\Users\Me"), Some("columns"));
        assert_eq!(p.get(r"C:\other"), None);
    }

    #[test]
    fn cap_drops_least_recently_set() {
        let mut p = Prefs::new(2);
        p.set("a", "1");
        p.set("b", "2");
        p.set("a", "3"); // a becomes newest
        p.set("c", "4");
        assert_eq!(p.get("b"), None);
        assert_eq!((p.get("a"), p.get("c")), (Some("3"), Some("4")));
    }

    #[test]
    fn tsv_roundtrip() {
        let mut p = Prefs::new(10);
        p.set(r"C:\x y", "gallery");
        p.set("@pathbar", "1");
        let back = Prefs::from_tsv(&(p.to_tsv() + "no tab here\n"), 10);
        assert_eq!(back.get(r"c:\x y"), Some("gallery"));
        assert_eq!(back.get("@pathbar"), Some("1"));
    }
}
