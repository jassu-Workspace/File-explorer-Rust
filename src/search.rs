use crate::listing::{Entry, entry_from};
use std::ffi::OsStr;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const BATCH_EVERY: Duration = Duration::from_millis(50);

pub fn name_matches(name: &OsStr, query_lower: &str) -> bool {
    name.to_string_lossy().to_lowercase().contains(query_lower)
}

/// Path-bar text for a search in progress or finished.
pub fn status_text(running: bool, matched: usize, skipped: usize) -> String {
    if running {
        return format!("Searching… {matched}");
    }
    let mut s = match matched {
        0 => "No results".to_string(),
        1 => "1 result".to_string(),
        n => format!("{n} results"),
    };
    match skipped {
        0 => {}
        1 => s.push_str(", 1 folder skipped"),
        k => s.push_str(&format!(", {k} folders skipped")),
    }
    s
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SearchStats {
    pub matched: usize,
    pub skipped_denied: usize,
    pub cancelled: bool,
}

pub struct SearchHandle {
    cancel: Arc<AtomicBool>,
}

impl SearchHandle {
    pub fn cancel(&self) {
        self.cancel.store(true, SeqCst);
    }
}

impl Drop for SearchHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

type BatchFn = Box<dyn Fn(Vec<Entry>) + Send + Sync>;
type DoneFn = Box<dyn FnOnce(SearchStats) + Send>;

struct Queue {
    dirs: Vec<PathBuf>,
    busy: usize,
}

struct Shared {
    root: PathBuf,
    query: String,
    show_hidden: bool,
    queue: Mutex<Queue>,
    cv: Condvar,
    pending: Mutex<(Vec<Entry>, Instant)>,
    cancel: Arc<AtomicBool>,
    matched: AtomicUsize,
    denied: AtomicUsize,
    alive: AtomicUsize,
    on_batch: BatchFn,
    on_done: Mutex<Option<DoneFn>>,
}

pub fn start_search(
    root: PathBuf,
    query: &str,
    show_hidden: bool,
    on_batch: impl Fn(Vec<Entry>) + Send + Sync + 'static,
    on_done: impl FnOnce(SearchStats) + Send + 'static,
) -> SearchHandle {
    let n = std::thread::available_parallelism().map_or(4, |n| n.get());
    let cancel = Arc::new(AtomicBool::new(false));
    let shared = Arc::new(Shared {
        queue: Mutex::new(Queue { dirs: vec![root.clone()], busy: 0 }),
        root,
        query: query.to_lowercase(),
        show_hidden,
        cv: Condvar::new(),
        pending: Mutex::new((Vec::new(), Instant::now())),
        cancel: cancel.clone(),
        matched: AtomicUsize::new(0),
        denied: AtomicUsize::new(0),
        alive: AtomicUsize::new(n),
        on_batch: Box::new(on_batch),
        on_done: Mutex::new(Some(Box::new(on_done))),
    });
    for _ in 0..n {
        let s = shared.clone();
        std::thread::spawn(move || worker(&s));
    }
    SearchHandle { cancel }
}

fn worker(s: &Shared) {
    loop {
        let dir = {
            let mut q = s.queue.lock().unwrap();
            while q.dirs.is_empty() && q.busy > 0 && !s.cancel.load(SeqCst) {
                q = s.cv.wait_timeout(q, Duration::from_millis(50)).unwrap().0;
            }
            if s.cancel.load(SeqCst) || q.dirs.is_empty() {
                s.cv.notify_all();
                break;
            }
            q.busy += 1;
            q.dirs.pop().unwrap()
        };
        scan(s, &dir);
        s.queue.lock().unwrap().busy -= 1;
        s.cv.notify_all();
    }
    if s.alive.fetch_sub(1, SeqCst) == 1 {
        let cancelled = s.cancel.load(SeqCst);
        let rest = std::mem::take(&mut s.pending.lock().unwrap().0);
        if !cancelled && !rest.is_empty() {
            (s.on_batch)(rest);
        }
        let stats = SearchStats { matched: s.matched.load(SeqCst), skipped_denied: s.denied.load(SeqCst), cancelled };
        if let Some(done) = s.on_done.lock().unwrap().take() {
            done(stats);
        }
    }
}

fn scan(s: &Shared, dir: &PathBuf) {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => {
            if e.kind() == ErrorKind::PermissionDenied {
                s.denied.fetch_add(1, SeqCst);
            }
            return;
        }
    };
    let rel = dir.strip_prefix(&s.root).map(PathBuf::from).unwrap_or_default();
    let mut hits = Vec::new();
    for (i, de) in rd.enumerate() {
        if i % 256 == 0 && s.cancel.load(SeqCst) {
            return;
        }
        let Ok(mut e) = de.and_then(|de| entry_from(&de)) else { continue };
        if !s.show_hidden && e.is_hidden() {
            continue;
        }
        if e.is_dir() && !e.is_reparse() {
            s.queue.lock().unwrap().dirs.push(dir.join(&e.name));
            s.cv.notify_one();
        }
        if name_matches(&e.name, &s.query) {
            e.rel_dir = Some(rel.clone());
            hits.push(e);
        }
    }
    if hits.is_empty() {
        return;
    }
    s.matched.fetch_add(hits.len(), SeqCst);
    let ready = {
        let mut p = s.pending.lock().unwrap();
        p.0.append(&mut hits);
        if p.1.elapsed() >= BATCH_EVERY {
            p.1 = Instant::now();
            Some(std::mem::take(&mut p.0))
        } else {
            None
        }
    };
    if let Some(b) = ready
        && !s.cancel.load(SeqCst)
    {
        (s.on_batch)(b);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::listing::Entry;
    use std::ffi::{OsStr, OsString};
    use std::fs;
    use std::os::windows::ffi::OsStringExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("fx-search-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    /// Runs a search to completion; returns (matches, stats, batches seen after done).
    fn run(root: &Path, q: &str, hidden: bool) -> (Vec<Entry>, SearchStats) {
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = got.clone();
        let (tx, rx) = mpsc::channel();
        let _h = start_search(root.to_path_buf(), q, hidden, move |b| g.lock().unwrap().extend(b), move |s| tx.send(s).unwrap());
        let stats = rx.recv_timeout(Duration::from_secs(10)).expect("search finished");
        let v = std::mem::take(&mut *got.lock().unwrap());
        (v, stats)
    }

    #[test]
    fn matches_case_insensitive() {
        assert!(name_matches(OsStr::new("Invoice_March.PDF"), "invoice"));
        assert!(!name_matches(OsStr::new("report.txt"), "invoice"));
    }

    #[test]
    fn matches_non_utf8_lossy() {
        let bad = OsString::from_wide(&[0xD800, b'a' as u16]);
        assert!(name_matches(&bad, "a"));
    }

    #[test]
    fn search_finds_nested_matches() {
        let d = tmp("nested");
        fs::create_dir_all(d.join("a").join("b")).unwrap();
        fs::create_dir_all(d.join("c")).unwrap();
        fs::write(d.join("a").join("report.txt"), "").unwrap();
        fs::write(d.join("a").join("b").join("REPORT2.md"), "").unwrap();
        fs::write(d.join("c").join("other.txt"), "").unwrap();
        let (mut v, s) = run(&d, "report", true);
        v.sort_by(|x, y| x.name.cmp(&y.name));
        assert_eq!(v.len(), 2);
        assert_eq!(s.matched, 2);
        assert!(!s.cancelled);
        assert_eq!(v[0].name, "REPORT2.md");
        assert_eq!(v[0].rel_dir.as_deref(), Some(Path::new(r"a\b")));
        assert_eq!(v[1].rel_dir.as_deref(), Some(Path::new("a")));
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn search_finds_file_in_deep_tree() {
        let d = tmp("deep");
        let mut p = d.clone();
        while p.as_os_str().len() < 320 {
            p.push("deep_folder_segment");
        }
        fs::create_dir_all(&p).unwrap();
        fs::write(p.join("deep_target.txt"), "").unwrap();
        let (v, _) = run(&d, "deep_target", true);
        assert_eq!(v.len(), 1);
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn search_skips_reparse_points() {
        let d = tmp("junction");
        fs::write(d.join("hit.txt"), "").unwrap();
        let ok = Command::new("cmd").args(["/C", "mklink", "/J"]).arg(d.join("loop")).arg(&d).output().unwrap().status.success();
        assert!(ok, "mklink /J failed");
        let (v, _) = run(&d, "hit", true);
        assert_eq!(v.len(), 1);
        fs::remove_dir(d.join("loop")).unwrap();
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn search_counts_denied() {
        let d = tmp("denied");
        let locked = d.join("locked");
        fs::create_dir(&locked).unwrap();
        fs::write(d.join("x.txt"), "").unwrap();
        let user = std::env::var("USERNAME").unwrap();
        assert!(Command::new("icacls").arg(&locked).args(["/deny", &format!("{user}:(RX)")]).output().unwrap().status.success());
        let (v, s) = run(&d, "x", true);
        let _ = Command::new("icacls").arg(&locked).args(["/remove:d", &user]).output();
        assert_eq!(v.len(), 1);
        assert!(s.skipped_denied >= 1, "{s:?}");
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn search_respects_show_hidden() {
        let d = tmp("hid");
        let f = d.join("match_hidden.txt");
        fs::write(&f, "").unwrap();
        assert!(Command::new("attrib").arg("+h").arg(&f).status().unwrap().success());
        assert_eq!(run(&d, "match", false).0.len(), 0);
        assert_eq!(run(&d, "match", true).0.len(), 1);
        let _ = Command::new("attrib").arg("-h").arg(&f).status();
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn cancel_stops_callbacks() {
        let d = tmp("cancel");
        for i in 0..2000 {
            let s = d.join(format!("d{i}"));
            fs::create_dir(&s).unwrap();
            fs::write(s.join("f.txt"), "").unwrap();
        }
        let done = Arc::new(Mutex::new(false));
        let late = Arc::new(Mutex::new(false));
        let (dn, lt) = (done.clone(), late.clone());
        let d2 = done.clone();
        let (tx, rx) = mpsc::channel();
        let h = start_search(d.clone(), "f", true, move |_| if *dn.lock().unwrap() { *lt.lock().unwrap() = true }, move |s| {
            *d2.lock().unwrap() = true;
            tx.send(s).unwrap();
        });
        h.cancel();
        let s = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(s.cancelled);
        assert!(!*late.lock().unwrap());
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    #[ignore]
    fn search_200k_under_1s() {
        let root = std::env::temp_dir().join("fx-bench");
        if !root.join("d1999").join("f99.txt").exists() {
            for i in 0..2000 {
                let s = root.join(format!("d{i}"));
                fs::create_dir_all(&s).unwrap();
                for j in 0..100 {
                    fs::write(s.join(format!("f{j}.txt")), "").unwrap();
                }
            }
        }
        run(&root, "f99", true);
        let t = Instant::now();
        let (v, _) = run(&root, "f99", true);
        assert_eq!(v.len(), 2000);
        assert!(t.elapsed() < Duration::from_secs(1), "took {:?}", t.elapsed());
    }
}

#[cfg(test)]
mod status_tests {
    use super::*;

    #[test]
    fn display_status_strings() {
        assert_eq!(status_text(true, 412, 0), "Searching… 412");
        assert_eq!(status_text(false, 3, 0), "3 results");
        assert_eq!(status_text(false, 1, 2), "1 result, 2 folders skipped");
        assert_eq!(status_text(false, 0, 1), "No results, 1 folder skipped");
    }
}
