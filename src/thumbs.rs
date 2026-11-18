//! Thumbnails from Windows' own thumbnail cache, made on one background thread and kept in a byte-capped LRU.
use crate::app::{SendHwnd, WM_THUMB_DONE};
use std::collections::HashMap;
use std::hash::Hash;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
use std::sync::{Arc, mpsc};
use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{BITMAP, GetObjectW, HBITMAP};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_THUMBNAILONLY};
use windows::core::HSTRING;

/// Least-recently-used map that evicts by total byte size.
pub struct Lru<K, V> {
    map: HashMap<K, (V, usize, u64)>,
    bytes: usize,
    cap: usize,
    tick: u64,
}

impl<K: Hash + Eq + Clone, V> Lru<K, V> {
    pub fn new(cap: usize) -> Self {
        Lru { map: HashMap::new(), bytes: 0, cap, tick: 0 }
    }

    pub fn get(&mut self, k: &K) -> Option<&V> {
        self.tick += 1;
        let t = self.tick;
        self.map.get_mut(k).map(|e| {
            e.2 = t;
            &e.0
        })
    }

    pub fn insert(&mut self, k: K, v: V, bytes: usize) {
        if let Some(old) = self.map.remove(&k) {
            self.bytes -= old.1;
        }
        if bytes > self.cap {
            return;
        }
        while self.bytes + bytes > self.cap {
            // shortcut: O(n) scan for the oldest entry; fine for a few hundred thumbnails.
            let Some(oldest) = self.map.iter().min_by_key(|(_, e)| e.2).map(|(k, _)| k.clone()) else { break };
            if let Some(e) = self.map.remove(&oldest) {
                self.bytes -= e.1;
            }
        }
        self.tick += 1;
        self.bytes += bytes;
        self.map.insert(k, (v, bytes, self.tick));
    }

    #[cfg(test)]
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.bytes = 0;
    }
}

/// Thumbnail pixel size to request for a drawn size (in device pixels).
pub fn bucket(px: f32) -> u32 {
    [64, 128, 256].into_iter().find(|&b| px <= b as f32).unwrap_or(512)
}

/// True when any pixel has non-zero alpha. Some thumbnails come back with all-zero alpha and must be drawn opaque.
pub fn has_alpha(bgra: &[u8]) -> bool {
    bgra.as_chunks::<4>().0.iter().any(|p| p[3] != 0)
}

pub type ThumbKey = (PathBuf, u32);

/// Learns, per file extension, whether Windows has thumbnails for it, so types like .txt stop costing a
/// shell round trip for every file. Reset when the folder changes.
#[derive(Default)]
pub struct ExtLearner {
    seen: HashMap<String, (u32, u32)>, // (successes, failures)
}

impl ExtLearner {
    const GIVE_UP_AFTER: u32 = 3;

    pub fn should_request(&self, ext: &str) -> bool {
        self.seen.get(ext).is_none_or(|&(ok, fail)| ok > 0 || fail < Self::GIVE_UP_AFTER)
    }

    pub fn record(&mut self, ext: &str, ok: bool) {
        let e = self.seen.entry(ext.to_string()).or_default();
        if ok { e.0 += 1 } else { e.1 += 1 }
    }

    pub fn reset(&mut self) {
        self.seen.clear();
    }
}

/// Result posted to the UI thread. `bitmap` is an HBITMAP handle (owned by the receiver) and whether it has alpha.
pub struct ThumbDone {
    pub generation: u64,
    pub key: ThumbKey,
    pub bitmap: Option<(isize, bool)>,
}

pub struct Thumbnailer {
    tx: mpsc::Sender<(u64, ThumbKey)>,
    generation: Arc<AtomicU64>,
}

impl Thumbnailer {
    pub fn start(hwnd: SendHwnd) -> Thumbnailer {
        let (tx, rx) = mpsc::channel::<(u64, ThumbKey)>();
        let generation = Arc::new(AtomicU64::new(0));
        let current = generation.clone();
        std::thread::spawn(move || {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            // Blocks on recv while idle: no CPU use.
            for (g, key) in rx {
                if g != current.load(SeqCst) {
                    continue; // the user moved on; skip stale work
                }
                let bitmap = make(&key).ok();
                hwnd.post(WM_THUMB_DONE, Box::new(ThumbDone { generation: g, key, bitmap }));
            }
        });
        Thumbnailer { tx, generation }
    }

    pub fn request(&self, key: ThumbKey) {
        let _ = self.tx.send((self.generation.load(SeqCst), key));
    }

    /// Drops queued requests from the previous folder.
    pub fn new_generation(&self) {
        self.generation.fetch_add(1, SeqCst);
    }

    pub fn is_current(&self, g: u64) -> bool {
        self.generation.load(SeqCst) == g
    }
}

fn make(key: &ThumbKey) -> windows::core::Result<(isize, bool)> {
    unsafe {
        let item: IShellItemImageFactory = SHCreateItemFromParsingName(&HSTRING::from(key.0.as_os_str()), None)?;
        let s = key.1 as i32;
        let h = item.GetImage(SIZE { cx: s, cy: s }, SIIGBF_THUMBNAILONLY)?;
        let mut bm = BITMAP::default();
        GetObjectW(h.into(), size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as _));
        let alpha = !bm.bmBits.is_null()
            && bm.bmBitsPixel == 32
            && has_alpha(std::slice::from_raw_parts(bm.bmBits as *const u8, (bm.bmWidthBytes * bm.bmHeight) as usize));
        Ok((h.0 as isize, alpha))
    }
}

/// Frees an HBITMAP the receiver will not use.
pub fn free(h: isize) {
    unsafe {
        let _ = windows::Win32::Graphics::Gdi::DeleteObject(HBITMAP(h as *mut _).into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lru_evicts_least_recent_past_cap() {
        let mut c = Lru::new(100);
        c.insert("a", 1, 40);
        c.insert("b", 2, 40);
        assert_eq!(c.get(&"a"), Some(&1), "touch a so b is the oldest");
        c.insert("c", 3, 40);
        assert!(c.get(&"b").is_none());
        assert!(c.get(&"a").is_some() && c.get(&"c").is_some());
        assert_eq!(c.bytes(), 80);
    }

    #[test]
    fn lru_replace_updates_bytes_and_oversize_is_dropped() {
        let mut c = Lru::new(100);
        c.insert("a", 1, 40);
        c.insert("a", 2, 10);
        assert_eq!(c.get(&"a"), Some(&2));
        assert_eq!(c.bytes(), 10);
        c.insert("huge", 9, 500);
        assert!(c.get(&"huge").is_none(), "an item bigger than the cap is not kept");
        c.clear();
        assert_eq!(c.bytes(), 0);
    }

    #[test]
    fn size_buckets() {
        assert_eq!(bucket(10.0), 64);
        assert_eq!(bucket(64.0), 64);
        assert_eq!(bucket(65.0), 128);
        assert_eq!(bucket(200.0), 256);
        assert_eq!(bucket(900.0), 512);
    }

    #[test]
    fn stops_asking_for_types_that_never_have_thumbnails() {
        let mut l = ExtLearner::default();
        assert!(l.should_request("txt"));
        for _ in 0..3 {
            l.record("txt", false);
        }
        assert!(!l.should_request("txt"), "three failures and no success");
        l.record("jpg", false);
        l.record("jpg", false);
        l.record("jpg", true);
        l.record("jpg", false);
        assert!(l.should_request("jpg"), "one success keeps the type alive");
        l.reset();
        assert!(l.should_request("txt"));
    }

    #[test]
    fn alpha_detection() {
        assert!(!has_alpha(&[1, 2, 3, 0, 4, 5, 6, 0]), "all-zero alpha means opaque pixels");
        assert!(has_alpha(&[1, 2, 3, 0, 4, 5, 6, 255]));
        assert!(!has_alpha(&[]));
    }
}
