//! File transfers (copy, move, recycle, delete) with our own progress and conflict handling instead of Windows' dialogs.
//! `build_job` plans a transfer (pure); `run` executes it on the calling thread through `IFileOperation`.
use crate::format::format_size;
use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use windows::Win32::Foundation::E_ABORT;
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx};
use windows::Win32::UI::Shell::{
    FILEOPERATION_FLAGS, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOCONFIRMMKDIR, FOF_NOERRORUI, FOF_RENAMEONCOLLISION, FOF_SILENT, FOFX_ADDUNDORECORD,
    FOFX_RECYCLEONDELETE, FileOperation, IFileOperation, IFileOperationProgressSink, IFileOperationProgressSink_Impl, IShellItem,
    SHCreateItemFromParsingName,
};
use windows::core::{HRESULT, HSTRING, PCWSTR, Ref, implement};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    Copy,
    Move,
    Recycle,
    Delete,
}

/// The user's answer for one name clash.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Choice {
    Replace,
    Skip,
    KeepBoth,
}

/// A planned transfer: `plain` items have no clash; `replace` overwrite; `keep_both` get a new name.
#[derive(Clone, Debug, Default)]
pub struct Job {
    pub op: Option<Op>,
    pub plain: Vec<PathBuf>,
    pub replace: Vec<PathBuf>,
    pub keep_both: Vec<PathBuf>,
    pub dest: Option<PathBuf>,
}

impl Job {
    pub fn is_empty(&self) -> bool {
        self.plain.is_empty() && self.replace.is_empty() && self.keep_both.is_empty()
    }
    pub fn count(&self) -> usize {
        self.plain.len() + self.replace.len() + self.keep_both.len()
    }
}

fn same_dir(item: &Path, dest: &Path) -> bool {
    item.parent().is_some_and(|p| p.as_os_str().eq_ignore_ascii_case(dest.as_os_str()))
}

/// Items whose name already exists in `dest` (copies into their own folder are not clashes: they become "name - Copy").
pub fn find_conflicts(items: &[PathBuf], dest: &Path, exists: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
    items.iter().filter(|i| !same_dir(i, dest) && i.file_name().is_some_and(|n| exists(&dest.join(n)))).cloned().collect()
}

/// Plans a transfer from the selection and the user's conflict answers.
pub fn build_job(op: Op, items: &[PathBuf], dest: Option<&Path>, answers: &HashMap<PathBuf, Choice>) -> Result<Job, String> {
    let mut job = Job { op: Some(op), dest: dest.map(Path::to_path_buf), ..Default::default() };
    for item in items {
        if let (Some(d), Op::Copy | Op::Move) = (dest, op) {
            let (il, dl) = (item.to_string_lossy().to_lowercase(), d.to_string_lossy().to_lowercase());
            if dl == il || dl.starts_with(&format!("{il}\\")) {
                return Err(format!("“{}” can’t be put inside itself.", item.file_name().unwrap_or_default().to_string_lossy()));
            }
            if same_dir(item, d) {
                if op == Op::Copy {
                    job.keep_both.push(item.clone());
                }
                continue; // a move onto itself does nothing
            }
        }
        match answers.get(item) {
            Some(Choice::Replace) => job.replace.push(item.clone()),
            Some(Choice::KeepBoth) => job.keep_both.push(item.clone()),
            Some(Choice::Skip) => {}
            None => job.plain.push(item.clone()),
        }
    }
    Ok(job)
}

/// Total bytes of files and folder trees (for "1.2 GB of 4.8 GB").
pub fn total_bytes(items: &[PathBuf]) -> u64 {
    fn walk(p: &Path) -> u64 {
        match std::fs::symlink_metadata(p) {
            Ok(m) if m.is_dir() && !m.file_type().is_symlink() => std::fs::read_dir(p).map(|rd| rd.flatten().map(|e| walk(&e.path())).sum()).unwrap_or(0),
            Ok(m) => m.len(),
            Err(_) => 0,
        }
    }
    items.iter().map(|p| walk(p)).sum()
}

/// Readable reason for a failed item. The copy engine's own codes have no system message text.
pub fn reason(hr: HRESULT) -> String {
    use windows::Win32::UI::Shell::*;
    let known = match hr {
        COPYENGINE_E_SHARING_VIOLATION_SRC | COPYENGINE_E_SHARING_VIOLATION_DEST => "It’s open in another app.",
        COPYENGINE_E_ACCESS_DENIED_SRC | COPYENGINE_E_ACCESS_DENIED_DEST => "Access is denied.",
        COPYENGINE_E_DISK_FULL | COPYENGINE_E_DISK_FULL_CLEAN => "There isn’t enough space on the disk.",
        COPYENGINE_E_FILE_TOO_LARGE => "The file is too large for the destination drive.",
        COPYENGINE_E_PATH_TOO_DEEP_SRC => "The path is too long.",
        COPYENGINE_E_NET_DISCONNECT_SRC => "The network location is no longer available.",
        _ => "",
    };
    if !known.is_empty() {
        return known.into();
    }
    let m = windows::core::Error::from(hr).message().trim().to_string();
    if m.is_empty() { format!("Error 0x{:08X}.", hr.0 as u32) } else { m }
}

#[derive(Debug)]
pub enum Update {
    Started { items: usize, bytes: u64 },
    Progress { fraction: f64, current: String },
    Failed { path: PathBuf, message: String },
    Done { cancelled: bool },
}

/// Progress bookkeeping for the panel: speed is a moving average over the last 2 s of samples.
#[derive(Clone, Debug)]
pub struct Progress {
    pub items: usize,
    pub bytes: u64,
    pub fraction: f64,
    samples: VecDeque<(f64, f64)>,
}

pub fn eta_text(secs: f64) -> String {
    match secs.ceil() as u64 {
        s if s < 60 => format!("about {s} s left"),
        s if s < 3600 => format!("about {} min left", s.div_ceil(60)),
        s => format!("about {} h left", s.div_ceil(3600)),
    }
}

impl Progress {
    pub fn new(items: usize, bytes: u64) -> Progress {
        Progress { items, bytes, fraction: 0.0, samples: VecDeque::new() }
    }

    /// Records progress `fraction` at `t` seconds after the start.
    pub fn sample(&mut self, t: f64, fraction: f64) {
        self.fraction = fraction.clamp(0.0, 1.0);
        self.samples.push_back((t, self.fraction * self.bytes as f64));
        while self.samples.len() > 2 && self.samples.front().is_some_and(|s| t - s.0 > 2.0) {
            self.samples.pop_front();
        }
    }

    /// Bytes per second over the sample window.
    pub fn speed(&self) -> f64 {
        match (self.samples.front(), self.samples.back()) {
            (Some(a), Some(b)) if b.0 > a.0 => (b.1 - a.1) / (b.0 - a.0),
            _ => 0.0,
        }
    }

    pub fn eta_secs(&self) -> Option<f64> {
        let s = self.speed();
        (s > 0.0).then(|| (1.0 - self.fraction) * self.bytes as f64 / s)
    }

    /// "12 of 340 · 162.0 MB of 4.5 GB · 81.0 MB/s · about 55 s left"
    pub fn line(&self) -> String {
        let done_items = ((self.fraction * self.items as f64 + 1e-9).floor() as usize).min(self.items);
        let mut s = format!("{done_items} of {}", self.items);
        if self.bytes > 0 {
            s.push_str(&format!(" · {} of {}", format_size((self.fraction * self.bytes as f64) as u64), format_size(self.bytes)));
        }
        let speed = self.speed();
        if speed > 0.0 {
            s.push_str(&format!(" · {}/s", format_size(speed as u64)));
        }
        if let Some(e) = self.eta_secs() {
            s.push_str(&format!(" · {}", eta_text(e)));
        }
        s
    }
}

type Emit = Arc<Mutex<Box<dyn FnMut(Update) + Send>>>;

/// Receives Windows' per-item and progress callbacks for one batch.
#[implement(IFileOperationProgressSink)]
struct Sink {
    emit: Emit,
    cancel: Arc<AtomicBool>,
    /// This batch's share of the whole job: progress maps to base + span * (so_far / total).
    base: f64,
    span: f64,
    last: Cell<Option<Instant>>,
    current: Mutex<String>,
}

impl Sink {
    fn send(&self, u: Update) {
        if let Ok(mut f) = self.emit.lock() {
            f(u);
        }
    }
    fn pre(&self, item: Ref<IShellItem>) -> windows::core::Result<()> {
        if self.cancel.load(SeqCst) {
            return Err(E_ABORT.into());
        }
        if let Some(i) = item.as_ref()
            && let Ok(name) = unsafe { i.GetDisplayName(windows::Win32::UI::Shell::SIGDN_NORMALDISPLAY) }
        {
            let n = unsafe { name.to_string() }.unwrap_or_default();
            unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(name.0 as _)) };
            if let Ok(mut c) = self.current.lock() {
                *c = n;
            }
        }
        Ok(())
    }
    fn post(&self, item: Ref<IShellItem>, hr: HRESULT) -> windows::core::Result<()> {
        if hr.is_err() && hr != E_ABORT && hr != HRESULT::from_win32(1223) {
            let path = item.as_ref().and_then(|i| unsafe { i.GetDisplayName(windows::Win32::UI::Shell::SIGDN_FILESYSPATH) }.ok()).map(|p| {
                let s = unsafe { p.to_string() }.unwrap_or_default();
                unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as _)) };
                PathBuf::from(s)
            });
            self.send(Update::Failed { path: path.unwrap_or_default(), message: reason(hr) });
        }
        Ok(())
    }
}

impl IFileOperationProgressSink_Impl for Sink_Impl {
    fn StartOperations(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn FinishOperations(&self, _: HRESULT) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreRenameItem(&self, _: u32, i: Ref<IShellItem>, _: &PCWSTR) -> windows::core::Result<()> {
        self.pre(i)
    }
    fn PostRenameItem(&self, _: u32, i: Ref<IShellItem>, _: &PCWSTR, hr: HRESULT, _: Ref<IShellItem>) -> windows::core::Result<()> {
        self.post(i, hr)
    }
    fn PreMoveItem(&self, _: u32, i: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR) -> windows::core::Result<()> {
        self.pre(i)
    }
    fn PostMoveItem(&self, _: u32, i: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR, hr: HRESULT, _: Ref<IShellItem>) -> windows::core::Result<()> {
        self.post(i, hr)
    }
    fn PreCopyItem(&self, _: u32, i: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR) -> windows::core::Result<()> {
        self.pre(i)
    }
    fn PostCopyItem(&self, _: u32, i: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR, hr: HRESULT, _: Ref<IShellItem>) -> windows::core::Result<()> {
        self.post(i, hr)
    }
    fn PreDeleteItem(&self, _: u32, i: Ref<IShellItem>) -> windows::core::Result<()> {
        self.pre(i)
    }
    fn PostDeleteItem(&self, _: u32, i: Ref<IShellItem>, hr: HRESULT, _: Ref<IShellItem>) -> windows::core::Result<()> {
        self.post(i, hr)
    }
    fn PreNewItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostNewItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR, _: &PCWSTR, _: u32, _: HRESULT, _: Ref<IShellItem>) -> windows::core::Result<()> {
        Ok(())
    }
    fn UpdateProgress(&self, total: u32, so_far: u32) -> windows::core::Result<()> {
        // At most ~30 updates a second reach the UI thread.
        let now = Instant::now();
        if self.last.get().is_some_and(|t| now.duration_since(t).as_millis() < 33) && so_far < total {
            return Ok(());
        }
        self.last.set(Some(now));
        let f = if total == 0 { 1.0 } else { so_far as f64 / total as f64 };
        let current = self.current.lock().map(|c| c.clone()).unwrap_or_default();
        self.send(Update::Progress { fraction: self.base + self.span * f, current });
        if self.cancel.load(SeqCst) { Err(E_ABORT.into()) } else { Ok(()) }
    }
    fn ResetTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn PauseTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn ResumeTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
}

/// Runs one batch of items through IFileOperation with `flags`. Returns false when the user cancelled.
#[allow(clippy::too_many_arguments)]
fn batch(op: Op, items: &[PathBuf], dest: Option<&Path>, flags: FILEOPERATION_FLAGS, emit: &Emit, cancel: &Arc<AtomicBool>, base: f64, span: f64) -> bool {
    if items.is_empty() {
        return true;
    }
    let item = |p: &Path| unsafe { SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(p.as_os_str()), None) };
    let fail = |p: &Path, e: windows::core::Error| {
        if let Ok(mut f) = emit.lock() {
            f(Update::Failed { path: p.to_path_buf(), message: reason(e.code()) });
        }
    };
    let result = (|| -> windows::core::Result<()> {
        let fo: IFileOperation = unsafe { CoCreateInstance(&FileOperation, None, CLSCTX_ALL)? };
        unsafe { fo.SetOperationFlags(flags)? };
        let sink: IFileOperationProgressSink = Sink { emit: emit.clone(), cancel: cancel.clone(), base, span, last: Cell::new(None), current: Mutex::new(String::new()) }.into();
        let cookie = unsafe { fo.Advise(&sink)? };
        let d = dest.map(item).transpose()?;
        for p in items {
            match item(p) {
                Ok(i) => unsafe {
                    match op {
                        Op::Copy => fo.CopyItem(&i, d.as_ref(), None, None)?,
                        Op::Move => fo.MoveItem(&i, d.as_ref(), None, None)?,
                        Op::Recycle | Op::Delete => fo.DeleteItem(&i, None)?,
                    }
                },
                Err(e) => fail(p, e),
            }
        }
        let r = unsafe { fo.PerformOperations() };
        let _ = unsafe { fo.Unadvise(cookie) };
        r
    })();
    match result {
        Err(e) if e.code() == E_ABORT || e.code() == HRESULT::from_win32(1223) || cancel.load(SeqCst) => false,
        Err(e) => {
            fail(dest.unwrap_or(Path::new("")), e);
            true
        }
        Ok(()) => !cancel.load(SeqCst),
    }
}

/// Executes a planned job on this thread (call from a worker). Windows shows no UI: progress, errors and the
/// final state arrive through `emit`.
pub fn run(job: Job, cancel: Arc<AtomicBool>, emit: impl FnMut(Update) + Send + 'static) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let emit: Emit = Arc::new(Mutex::new(Box::new(emit)));
    let send = |u: Update| {
        if let Ok(mut f) = emit.lock() {
            f(u);
        }
    };
    let all: Vec<PathBuf> = job.plain.iter().chain(&job.replace).chain(&job.keep_both).cloned().collect();
    let op = job.op.unwrap_or(Op::Copy);
    let bytes = if matches!(op, Op::Copy | Op::Move) { total_bytes(&all) } else { 0 };
    send(Update::Started { items: all.len(), bytes });
    for p in all.iter().filter(|p| std::fs::symlink_metadata(p).is_err()) {
        send(Update::Failed { path: p.clone(), message: "This item no longer exists.".into() });
    }
    let present = |v: &[PathBuf]| v.iter().filter(|p| std::fs::symlink_metadata(p).is_ok()).cloned().collect::<Vec<_>>();
    let quiet = FOF_SILENT | FOF_NOERRORUI | FOF_NOCONFIRMMKDIR | FOFX_ADDUNDORECORD;
    let base_flags = match op {
        Op::Recycle => quiet | FOF_NOCONFIRMATION | FOF_ALLOWUNDO | FOFX_RECYCLEONDELETE,
        Op::Delete => quiet | FOF_NOCONFIRMATION,
        _ => quiet | FOF_ALLOWUNDO,
    };
    // Plain items and replacements overwrite silently; keep-both items get Windows' "name (2)" naming.
    let first: Vec<PathBuf> = present(&[job.plain.clone(), job.replace.clone()].concat());
    let second = present(&job.keep_both);
    let n = (first.len() + second.len()).max(1) as f64;
    let split = first.len() as f64 / n;
    let ok = batch(op, &first, job.dest.as_deref(), base_flags | FOF_NOCONFIRMATION, &emit, &cancel, 0.0, split)
        && batch(op, &second, job.dest.as_deref(), base_flags | FOF_RENAMEONCOLLISION, &emit, &cancel, split, 1.0 - split);
    if ok {
        send(Update::Progress { fraction: 1.0, current: String::new() });
    }
    send(Update::Done { cancelled: !ok });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("fx-xfer-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn conflicts_and_plan() {
        let src = [PathBuf::from(r"C:\s\a.txt"), PathBuf::from(r"C:\s\b.txt"), PathBuf::from(r"C:\s\c.txt")];
        let dest = Path::new(r"C:\d");
        let exists = |p: &Path| p.ends_with("a.txt") || p.ends_with("b.txt");
        assert_eq!(find_conflicts(&src, dest, exists), [src[0].clone(), src[1].clone()]);
        let answers = HashMap::from([(src[0].clone(), Choice::Replace), (src[1].clone(), Choice::Skip)]);
        let job = build_job(Op::Copy, &src, Some(dest), &answers).unwrap();
        assert_eq!(job.replace, [src[0].clone()]);
        assert_eq!(job.plain, [src[2].clone()]);
        assert!(job.keep_both.is_empty(), "skipped b is nowhere");
    }

    #[test]
    fn same_folder_rules() {
        let src = [PathBuf::from(r"C:\d\a.txt")];
        let dest = Path::new(r"C:\d");
        assert_eq!(build_job(Op::Copy, &src, Some(dest), &HashMap::new()).unwrap().keep_both, src, "copy here makes 'a - Copy'");
        assert!(find_conflicts(&src, dest, |_| true).is_empty(), "no prompt for copying into the same folder");
        assert!(build_job(Op::Move, &src, Some(dest), &HashMap::new()).unwrap().is_empty(), "moving onto itself does nothing");
        let err = build_job(Op::Move, &[PathBuf::from(r"C:\d")], Some(Path::new(r"C:\d\sub")), &HashMap::new());
        assert!(err.is_err(), "a folder cannot go inside itself");
    }

    #[test]
    fn speed_eta_and_status_line() {
        let mut p = Progress::new(340, 4_800_000_000);
        p.sample(0.0, 0.0);
        p.sample(1.0, 0.0177);
        p.sample(2.0, 0.0354);
        let speed = p.speed();
        assert!((speed - 84_960_000.0).abs() < 1e6, "{speed}");
        let eta = p.eta_secs().unwrap();
        assert!((54.0..56.0).contains(&eta), "{eta}");
        assert_eq!(p.line(), "12 of 340 · 162.0 MB of 4.5 GB · 81.0 MB/s · about 55 s left");
        let mut one = Progress::new(1, 100);
        one.sample(0.0, 0.6);
        assert!(one.line().starts_with("0 of 1"), "an item counts once it is fully done");
        assert_eq!(eta_text(30.0), "about 30 s left");
        assert_eq!(eta_text(150.0), "about 3 min left");
    }

    fn run_sync(job: Job, cancel: bool) -> Vec<Update> {
        let updates = Arc::new(Mutex::new(Vec::new()));
        let u = updates.clone();
        run(job, Arc::new(AtomicBool::new(cancel)), move |x| u.lock().unwrap().push(x));
        Arc::try_unwrap(updates).unwrap().into_inner().unwrap()
    }

    #[test]
    fn copies_with_progress_and_moves() {
        let d = tmp("copy");
        fs::create_dir_all(d.join("dst")).unwrap();
        fs::write(d.join("big.bin"), vec![7u8; 2 << 20]).unwrap();
        fs::write(d.join("small.txt"), "x").unwrap();
        let job = build_job(Op::Copy, &[d.join("big.bin"), d.join("small.txt")], Some(&d.join("dst")), &HashMap::new()).unwrap();
        let ups = run_sync(job, false);
        assert!(matches!(ups.first(), Some(Update::Started { items: 2, bytes }) if *bytes == (2 << 20) + 1), "{:?}", ups.first());
        assert!(ups.iter().any(|u| matches!(u, Update::Progress { .. })));
        assert!(matches!(ups.last(), Some(Update::Done { cancelled: false })));
        assert_eq!(fs::read(d.join("dst").join("big.bin")).unwrap().len(), 2 << 20);
        let job = build_job(Op::Move, &[d.join("small.txt")], Some(&d.join("dst")), &HashMap::from([(d.join("small.txt"), Choice::Replace)])).unwrap();
        run_sync(job, false);
        assert!(!d.join("small.txt").exists() && d.join("dst").join("small.txt").exists());
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn replace_skip_keep_both_on_disk() {
        let d = tmp("conflict");
        fs::create_dir_all(d.join("dst")).unwrap();
        for n in ["r.txt", "s.txt", "k.txt"] {
            fs::write(d.join(n), "new").unwrap();
            fs::write(d.join("dst").join(n), "old").unwrap();
        }
        let items: Vec<PathBuf> = ["r.txt", "s.txt", "k.txt"].iter().map(|n| d.join(n)).collect();
        let answers = HashMap::from([(items[0].clone(), Choice::Replace), (items[1].clone(), Choice::Skip), (items[2].clone(), Choice::KeepBoth)]);
        run_sync(build_job(Op::Copy, &items, Some(&d.join("dst")), &answers).unwrap(), false);
        let read = |n: &str| fs::read_to_string(d.join("dst").join(n)).unwrap();
        assert_eq!(read("r.txt"), "new");
        assert_eq!(read("s.txt"), "old");
        assert_eq!(read("k.txt"), "old");
        assert_eq!(fs::read_dir(d.join("dst")).unwrap().count(), 4, "keep both added a renamed copy");
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn cancel_before_start_copies_nothing() {
        let d = tmp("cancel");
        fs::create_dir_all(d.join("dst")).unwrap();
        fs::write(d.join("a.txt"), "a").unwrap();
        let ups = run_sync(build_job(Op::Copy, &[d.join("a.txt")], Some(&d.join("dst")), &HashMap::new()).unwrap(), true);
        assert!(matches!(ups.last(), Some(Update::Done { cancelled: true })));
        assert!(!d.join("dst").join("a.txt").exists());
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn failure_reasons_are_readable() {
        use windows::Win32::UI::Shell::COPYENGINE_E_SHARING_VIOLATION_SRC;
        assert!(reason(COPYENGINE_E_SHARING_VIOLATION_SRC).contains("open in another app"));
        assert_eq!(reason(HRESULT(0x8027_7777_u32 as i32)), "Error 0x80277777.");
        assert!(!reason(HRESULT::from_win32(5)).is_empty(), "system codes keep their message");
    }

    #[test]
    fn delete_permanently_and_report_missing() {
        let d = tmp("del");
        fs::write(d.join("x.txt"), "x").unwrap();
        let ups = run_sync(build_job(Op::Delete, &[d.join("x.txt"), d.join("missing.txt")], None, &HashMap::new()).unwrap(), false);
        assert!(!d.join("x.txt").exists());
        assert!(ups.iter().any(|u| matches!(u, Update::Failed { path, .. } if path.ends_with("missing.txt"))), "{ups:?}");
        fs::remove_dir_all(d).unwrap();
    }
}
