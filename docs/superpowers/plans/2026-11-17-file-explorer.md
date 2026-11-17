# Fast Light File Explorer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Finder-inspired, Direct2D-drawn file explorer for Windows that starts in under 200 ms and idles under 25 MB.

**Architecture:** One Win32 top-level window with a custom unified toolbar, drawn entirely with Direct2D/DirectWrite. Pure Rust modules (`listing`, `search`, `layout`, `selection`, `nav`, `format`) hold all logic and carry the unit tests. Win32 glue (`app`, `draw`, `shell`, `views/*`) stays thin. Disk work runs on short-lived worker threads that post results back with `PostMessageW`.

**Tech Stack:** Rust 1.98.1 stable (MSVC toolchain), `windows` crate 0.62.2, std only otherwise.

**Spec:** `docs/superpowers/specs/2026-11-17-file-explorer-design.md`

## Global Constraints

- Windows 10 1903+ and Windows 11, x64. Mica only on Windows 11 (build ≥ 22621); flat colors otherwise.
- Only dependency: `windows = "0.62.2"`. No other crates, including dev-dependencies.
- Single `.exe`, no installer. Crate and binary name: `fexplorer`.
- No network calls, no telemetry, no admin rights, no background service, no saved state.
- Release profile: `opt-level = 3`, `lto = true`, `codegen-units = 1`, `panic = "abort"`, `strip = true`.
- Manifest: per-monitor DPI aware v2, `longPathAware`, Common Controls v6.
- No Apple icons, SF fonts, or traffic lights. Font: Segoe UI Variable, fallback Segoe UI.
- Targets: cold start < 200 ms; idle RAM < 25 MB with a 10k-file folder; idle CPU 0 %; 200k-file search < 1 s warm; 60 fps scroll; UI thread never does disk I/O.
- Animations only: hover fade 100 ms, wheel scroll 120 ms ease-out, column slide 150 ms. A timer runs only while one is active.
- Search debounce 150 ms; batch interval 50 ms.
- Every task ends with `cargo test`, `cargo clippy --all-targets -- -D warnings`, and `cargo build --release` all clean.
- Work on branch `feat/explorer`. Commit after each task only with the user's go-ahead (user's standing rule).

## Review Focus

1. **Access-denied or vanished folder on navigate** (e.g. `C:\System Volume Information`, an unplugged USB drive): show an inline error in the content area and keep the previous folder. Test: `read_folder_denied_returns_err` and `nav_failure_keeps_current` (Task 1, Task 6).
2. **Non-ASCII and odd names** (emoji, CJK, names with trailing dots or no extension, names not valid UTF-8): list, sort, match, and display without panics. Test: `natural_sort_unicode`, `matches_non_utf8_lossy` (Task 1, Task 2).
3. **Paths longer than 260 chars**: search and listing reach them. Test: `search_finds_file_in_deep_tree` builds a 300+ char path (Task 2).
4. **Rapid navigation or retyping**: stale listing or search results never appear. Test: `stale_generation_is_dropped` (Task 6) and `cancel_stops_callbacks` (Task 2).
5. **Huge folder (100k entries)**: sorting stays fast and scrolling stays virtual. Test: `sort_100k_under_100ms` (Task 1) and `visible_rows_bounds` (Task 3).

---

## File Structure

| File | Responsibility |
|---|---|
| `Cargo.toml`, `build.rs`, `app.manifest` | Crate config, release profile, manifest embedding |
| `src/main.rs` | `#![windows_subsystem = "windows"]`, module list, COM init, call `app::run()` |
| `src/listing.rs` | `Entry`, `read_folder`, natural sort, `SortKey` |
| `src/search.rs` | Parallel name search with batching and cancel |
| `src/format.rs` | Size and date strings |
| `src/layout.rs` | `Rect`, window layout, visible rows, row hit test, NC hit test |
| `src/selection.rs` | Multi-select model and type-ahead |
| `src/nav.rs` | Back/forward history and column-view state |
| `src/theme.rs` | `Color`, `Theme` light/dark, `Metrics`, system dark/accent reads |
| `src/draw.rs` | D2D/DWrite factories, render target, brushes, text formats, glyphs, icon bitmap cache |
| `src/shell.rs` | Known folders, drives, free space, type names, icon index, open, `IFileOperation`, clipboard, local time |
| `src/app.rs` | `App` state, window class, wndproc, message routing, worker thread posting |
| `src/views/mod.rs`, `toolbar.rs`, `sidebar.rs`, `list.rs`, `columns.rs`, `pathbar.rs` | Paint and input for each region |

Custom messages (in `app.rs`): `WM_LISTING_DONE = WM_APP + 1`, `WM_SEARCH_BATCH = WM_APP + 2`, `WM_SEARCH_DONE = WM_APP + 3`. Payloads travel as `Box<T>` turned into `LPARAM` with `Box::into_raw`; the receiver rebuilds the box. `HWND` crosses threads as `isize`.

---

### Task 1: Scaffold and folder listing

**Files:**
- Create: `Cargo.toml`, `build.rs`, `app.manifest`, `src/main.rs`, `src/listing.rs`
- Test: inline `#[cfg(test)] mod tests` in `src/listing.rs`

**Interfaces:**
- Produces:
  - `pub struct Entry { pub name: OsString, pub size: u64, pub modified: u64 /* FILETIME ticks */, pub attrs: u32, pub rel_dir: Option<PathBuf> }`
  - `impl Entry { pub fn is_dir(&self) -> bool; pub fn is_hidden(&self) -> bool /* HIDDEN or SYSTEM */; pub fn is_reparse(&self) -> bool; pub fn ext_lower(&self) -> String }`
  - `pub fn entry_from(de: &std::fs::DirEntry) -> io::Result<Entry>`
  - `pub fn read_folder(path: &Path, show_hidden: bool) -> io::Result<Vec<Entry>>`
  - `#[derive(Clone, Copy, PartialEq, Eq)] pub enum SortKey { Name, Modified, Size, Kind }`
  - `pub fn sort_entries(entries: &mut [Entry], key: SortKey, descending: bool)` — folders always first, regardless of `descending`; ties break by natural name.
  - `pub fn natural_cmp(a: &OsStr, b: &OsStr) -> Ordering` — case-insensitive, digit runs compared numerically.

- [ ] **Step 1: Confirm toolchain is MSVC**

Run: `rustc -vV`
Expected: `host: x86_64-pc-windows-msvc`. If not, stop and ask the user.

- [ ] **Step 2: Create the crate files**

`Cargo.toml`: package `fexplorer`, edition 2024, dependency `windows = { version = "0.62.2", features = [...] }` (add features as tasks need them), release profile from Global Constraints.
`app.manifest`: `dpiAwareness` = `PerMonitorV2`, `longPathAware` = `true`, dependency on `Microsoft.Windows.Common-Controls` 6.0.0.0.
`build.rs`:

```rust
fn main() {
    let m = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("app.manifest");
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", m.display());
}
```

`src/main.rs`: `mod listing;` and an empty `fn main() {}` for now.

- [ ] **Step 3: Write the failing tests**

```rust
#[test] fn read_folder_lists_files_and_dirs()      // temp dir with a.txt (5 bytes) and sub/; expect 2 entries, a.txt size 5, sub is_dir
#[test] fn read_folder_hides_hidden_when_asked()   // set FILE_ATTRIBUTE_HIDDEN via attrib +h; show_hidden=false -> absent, true -> present
#[test] fn read_folder_denied_returns_err()        // read_folder(Path::new(r"C:\System Volume Information"), true).is_err()
#[test] fn read_folder_missing_returns_err()       // nonexistent path -> Err
#[test] fn natural_sort_orders_numbers()           // ["file10","File2","file1"] -> ["file1","File2","file10"]
#[test] fn natural_sort_unicode()                  // ["Zeta","émile","日本","alpha","😀"] sorts without panic; "alpha" before "Zeta"
#[test] fn sort_folders_first_both_directions()    // folder "z" stays before file "a" with descending true and false
#[test] fn sort_by_size_and_modified()             // sizes 3,1,2 -> 1,2,3 asc; reversed desc
#[test] fn sort_100k_under_100ms()                 // 100_000 synthetic entries, sort_entries(Name) < 100 ms in --release; #[ignore] in debug via cfg!(debug_assertions) early return
```

Temp dirs: `std::env::temp_dir().join(format!("fx-test-{}-{}", name, std::process::id()))`, removed at test end.

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo test listing`
Expected: compile errors for missing items.

- [ ] **Step 5: Implement `listing.rs`**

Use `std::os::windows::fs::MetadataExt` on `DirEntry::metadata()` (free on Windows, comes from `FindNextFileW`): `file_attributes()`, `file_size()`, `last_write_time()`.

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test listing` then `cargo test --release listing -- sort_100k`
Expected: all PASS.

- [ ] **Step 7: Lint, build, commit**

Run: `cargo clippy --all-targets -- -D warnings` and `cargo build --release`. Expected: clean.
Commit: `feat: scaffold crate and folder listing`

---

### Task 2: Parallel name search

**Files:**
- Create: `src/search.rs`
- Modify: `src/main.rs` (add `mod search;`)

**Interfaces:**
- Consumes: `Entry`, `entry_from` (Task 1).
- Produces:
  - `pub fn name_matches(name: &OsStr, query_lower: &str) -> bool` — `to_string_lossy().to_lowercase().contains(query_lower)`.
  - `pub struct SearchStats { pub matched: usize, pub skipped_denied: usize, pub cancelled: bool }`
  - `pub struct SearchHandle` with `pub fn cancel(&self)`; `Drop` cancels and does not join.
  - `pub fn start_search(root: PathBuf, query: &str, show_hidden: bool, on_batch: impl Fn(Vec<Entry>) + Send + Sync + 'static, on_done: impl FnOnce(SearchStats) + Send + 'static) -> SearchHandle`
  - Each matched `Entry` has `rel_dir = Some(parent relative to root)` (`""` for root itself).

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn matches_case_insensitive()            // name_matches("Invoice_March.PDF", "invoice") == true
#[test] fn matches_non_utf8_lossy()              // OsString::from_wide(&[0xD800, b'a' as u16]) with query "a" -> true, no panic
#[test] fn search_finds_nested_matches()         // tree: a/report.txt, a/b/REPORT2.md, c/other.txt; query "report" -> 2 matches with rel_dir "a" and "a\\b"
#[test] fn search_finds_file_in_deep_tree()      // nested dirs until path > 300 chars, file "deep_target.txt" at the bottom -> found
#[test] fn search_skips_reparse_points()         // junction loop: `mklink /J loop .` inside tree (via cmd /C); search completes, each match once
#[test] fn search_counts_denied()                // root = C:\ is too big; instead: root containing a dir with deny ACL via `icacls dir /deny %USERNAME%:(RX)`; skipped_denied >= 1; restore ACL after
#[test] fn search_respects_show_hidden()         // hidden match excluded when show_hidden=false
#[test] fn cancel_stops_callbacks()              // big synthetic tree (2_000 dirs); cancel right after start; on_done gets cancelled=true; no on_batch after on_done
#[test] #[ignore] fn search_200k_under_1s()      // builds %TEMP%\fx-bench (2_000 dirs x 100 empty files) if missing; one warm-up run; second run of query "f99" completes in < 1 s. Run: cargo test --release -- --ignored search_200k
```

Collect callback output with `Arc<Mutex<Vec<_>>>` and a `std::sync::mpsc` channel to wait for `on_done` (timeout 10 s).

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test search`
Expected: compile errors.

- [ ] **Step 3: Implement `start_search`**

Algorithm (the tests do not pin it, so it is fixed here):

```text
shared: queue Mutex<Vec<PathBuf>> seeded with root, busy: usize, Condvar,
        pending Mutex<(Vec<Entry>, Instant last_flush)>, cancel AtomicBool, counters AtomicUsize
spawn N = available_parallelism() threads:
  loop:
    lock queue; while empty && busy > 0 && !cancel: wait
    if cancel || (empty && busy == 0): notify_all; break
    pop dir; busy += 1; unlock
    read_dir(dir): Err(PermissionDenied) -> skipped_denied += 1
    for each entry (check cancel every 256 entries):
      skip hidden if !show_hidden; if is_dir && !is_reparse -> push to queue + notify_one
      if name_matches -> push to local vec
    append local vec to pending; if last_flush >= 50 ms and pending not empty -> take and on_batch(it)
    lock queue; busy -= 1; notify_all
last thread to exit (AtomicUsize countdown): flush remaining pending unless cancelled, then on_done(stats)
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test search`
Expected: all PASS.

- [ ] **Step 5: Lint, build, commit**

Commit: `feat: parallel scoped name search`

---

### Task 3: Pure UI math — format, layout, selection

**Files:**
- Create: `src/format.rs`, `src/layout.rs`, `src/selection.rs`
- Modify: `src/main.rs`

**Interfaces:**
- Produces:
  - `format.rs`:
    - `pub fn format_size(bytes: u64) -> String` — 1024-based; `0 -> "0 bytes"`, `1 -> "1 byte"`, `900 -> "900 bytes"`, `4096 -> "4 KB"`, `1_258_291 -> "1.2 MB"`, `5_368_709_120 -> "5.0 GB"`. KB has no decimal; MB/GB/TB one decimal.
    - `pub struct LocalTime { pub year: u16, pub month: u8, pub day: u8, pub hour: u8, pub minute: u8 }`
    - `pub fn format_modified(t: LocalTime, today: LocalTime) -> String` — same date `"Today, 09:14"`; previous calendar day `"Yesterday, 18:02"`; else `"Nov 2, 2026"`.
  - `layout.rs`:
    - `#[derive(Clone, Copy, Debug, PartialEq, Default)] pub struct Rect { pub x: f32, pub y: f32, pub w: f32, pub h: f32 }` with `fn contains(&self, x: f32, y: f32) -> bool`.
    - `pub struct Layout { pub toolbar: Rect, pub sidebar: Rect, pub splitter: Rect /* 4 px grab zone */, pub content: Rect, pub header: Rect /* list column headers, 28 px */, pub rows: Rect, pub pathbar: Rect, pub min_btn: Rect, pub max_btn: Rect, pub close_btn: Rect }`
    - `pub fn compute_layout(w: f32, h: f32, sidebar_w: f32, scale: f32) -> Layout` — toolbar 52, pathbar 28, header 28, caption buttons 46×32 each at top-right, all × `scale`; `sidebar_w` clamped to 160–320 × `scale`.
    - `pub fn visible_rows(scroll_y: f32, viewport_h: f32, row_h: f32, count: usize) -> Range<usize>`
    - `pub fn row_at(y_in_rows: f32, scroll_y: f32, row_h: f32, count: usize) -> Option<usize>`
    - `pub fn nc_hit(l: &Layout, interactive: &[Rect], x: f32, y: f32, w: f32, h: f32, border: f32, maximized: bool) -> u32` — returns `HTTOPLEFT..HTBOTTOMRIGHT` for edges (none when maximized), `HTMINBUTTON/HTMAXBUTTON/HTCLOSE` on buttons, `HTCLIENT` inside any `interactive` rect (back, forward, title, view switch, search field), `HTCAPTION` on other toolbar space, `HTCLIENT` otherwise.
  - `selection.rs`:
    - `pub struct Selection` with `new()`, `clear()`, `click(i)`, `ctrl_click(i)`, `shift_click(i)`, `move_focus(delta: isize, extend: bool, count: usize)`, `select_all(count)`, `is_selected(i) -> bool`, `focus() -> Option<usize>`, `selected() -> Vec<usize>` (sorted). Backed by `Vec<usize>` sorted + anchor + focus.
    - `pub struct TypeAhead` with `push(&mut self, ch: char, now: Instant) -> &str` (resets buffer if > 1 s since last key).
    - `pub fn find_prefix(names: impl Iterator<Item = &OsStr>, prefix: &str) -> Option<usize>` — case-insensitive.

- [ ] **Step 1: Write the failing tests**

```rust
// format.rs
#[test] fn size_strings()        // every pair listed in the interface above
#[test] fn date_today_yesterday_other() // today 2026-11-17 09:14 -> "Today, 09:14"; 2026-11-16 18:02 -> "Yesterday, 18:02"; 2026-11-02 -> "Nov 2, 2026"; today 2027-01-01, t 2026-12-31 -> "Yesterday, …"
// layout.rs
#[test] fn layout_regions_tile()        // 1200x800 scale 1, sidebar 200: toolbar.h 52, pathbar.y 772, content.x 200, close_btn.x 1154
#[test] fn sidebar_clamped()            // sidebar_w 50 -> 160; 900 -> 320; scale 2 -> 320..640
#[test] fn visible_rows_bounds()        // scroll 0, h 100, row 24, count 100_000 -> 0..5; scroll 2_399_976 -> 99_999..100_000; count 0 -> 0..0
#[test] fn row_at_hits_and_misses()     // y beyond last row -> None
#[test] fn nc_hit_regions()             // top-left corner -> HTTOPLEFT; maximized corner -> not an edge; close btn -> HTCLOSE; empty toolbar -> HTCAPTION; search rect -> HTCLIENT
// selection.rs
#[test] fn click_ctrl_shift()           // click 2; ctrl_click 5 (anchor=5); shift_click 8 -> selected() == [5,6,7,8] (Explorer behavior: shift replaces selection with anchor..=i)
#[test] fn move_focus_extends()         // focus 3, move +2 extend -> {3,4,5}; clamps at count-1
#[test] fn type_ahead_resets_after_1s() // "a","b" within 1 s -> "ab"; then 1.1 s later "c" -> "c"
#[test] fn find_prefix_case_insensitive()
```

- [ ] **Step 2: Run to verify fail** — `cargo test format layout selection`, expect compile errors.
- [ ] **Step 3: Implement the three modules.** Month names: fixed English abbreviations `Jan…Dec`.
- [ ] **Step 4: Run to verify pass** — same command, all PASS.
- [ ] **Step 5: Lint, build, commit** — `feat: layout, selection and formatting math`

---

### Task 4: Theme, window shell, Direct2D drawing

**Files:**
- Create: `src/theme.rs`, `src/draw.rs`, `src/app.rs`, `src/views/mod.rs`, `src/views/toolbar.rs`
- Modify: `src/main.rs`, `Cargo.toml` (features)

**Interfaces:**
- Consumes: `Layout`, `compute_layout`, `nc_hit` (Task 3).
- Produces:
  - `theme.rs`: `#[derive(Clone, Copy)] pub struct Color { pub r: f32, pub g: f32, pub b: f32, pub a: f32 }` with `fn rgb(hex: u32) -> Color`; `pub struct Theme { pub dark: bool, pub window_bg, sidebar_bg, content_bg, text, text_dim, separator, hover, accent, selection_text, field_bg, close_hover: Color }`; `Theme::light(accent)`, `Theme::dark(accent)`; `pub fn system_prefers_dark() -> bool` (registry `AppsUseLightTheme == 0`); `pub fn accent_color() -> Color` (`DwmGetColorizationColor`, fallback `0x0067C0`). `pub const ROW_H: f32 = 24.0; pub const COLUMN_W: f32 = 220.0;`
  - `draw.rs`: `pub struct Gfx` holding `ID2D1Factory`, `IDWriteFactory`, `Option<ID2D1HwndRenderTarget>`, text formats `body` (13 px), `title` (13 px semibold), `small` (11 px semibold), all with ellipsis trimming and no wrap. Methods: `new() -> Result<Gfx>`, `ensure_target(hwnd, w, h) -> Result<()>`, `resize(w, h)`, `begin()`, `end() -> Result<()>` (on `D2DERR_RECREATE_TARGET` drop target and device-dependent caches), `fill_rect(Rect, Color)`, `fill_round(Rect, radius, Color)`, `line(x1, y1, x2, y2, Color)`, `text(&str, Rect, &IDWriteTextFormat, Color)`, `set_dpi(dpi: f32)` (recreate text formats).
  - Glyphs as `ID2D1PathGeometry` built once: `glyph_back`, `glyph_forward`, `glyph_search`, `glyph_list`, `glyph_columns`, `glyph_folder`, `glyph_home`, `glyph_desktop`, `glyph_doc`, `glyph_download`, `glyph_picture`, `glyph_drive`, `glyph_min`, `glyph_max`, `glyph_restore`, `glyph_close`, `glyph_chevron`. Drawn via `fn glyph(&self, g: Glyph, Rect, Color)` with `pub enum Glyph { … }`, scaled to the rect.
  - `app.rs`: `pub fn run() -> Result<()>`; `pub struct App { hwnd, gfx: Gfx, theme: Theme, scale: f32, sidebar_w: f32, layout: Layout, hover: HoverTarget, … }` stored in `GWLP_USERDATA`.

- [ ] **Step 1: Write the failing tests**

```rust
// theme.rs
#[test] fn rgb_parses()                 // Color::rgb(0x0067C0) == (0.0, 0x67/255, 0xC0/255, 1.0)
#[test] fn themes_contrast()            // relative luminance contrast text vs content_bg >= 7.0 for light and dark; text_dim >= 4.5
```

- [ ] **Step 2: Run to verify fail** — `cargo test theme`.
- [ ] **Step 3: Implement `theme.rs`** with these values (builder may tune within contrast tests):
  - Light: window_bg `F5F5F7`, sidebar_bg `EDEDF0` (transparent when Mica), content_bg `FFFFFF`, text `1D1D1F`, text_dim `6E6E73`, separator `E3E3E6`, hover `0x000000` at a=0.05, field_bg `0x000000` at a=0.06, close_hover `C42B1C`.
  - Dark: window_bg `1E1E1E`, sidebar_bg `252526`, content_bg `1C1C1E`, text `F5F5F7`, text_dim `98989D`, separator `38383A`, hover `0xFFFFFF` at a=0.06, field_bg `0xFFFFFF` at a=0.08, close_hover `C42B1C`.
- [ ] **Step 4: Run to verify pass** — `cargo test theme`.
- [ ] **Step 5: Implement window shell in `app.rs` + `draw.rs`**
  - `main.rs`: `CoInitializeEx(COINIT_APARTMENTTHREADED)`, then `app::run()`.
  - Window class with `CS_HREDRAW | CS_VREDRAW` off (we repaint ourselves), no background brush, `WS_OVERLAPPEDWINDOW`.
  - `WM_NCCALCSIZE` with `wparam == TRUE`: return 0 so the client area covers the caption; when maximized, inset by `GetSystemMetricsForDpi(SM_CXFRAME + SM_CXPADDEDBORDER)` so content is not clipped.
  - `DwmExtendFrameIntoClientArea` margins `{0,0,1,0}` to keep the DWM shadow.
  - `WM_NCHITTEST` → `layout::nc_hit`. Return `HTMAXBUTTON` on the max button so Windows 11 snap flyout appears.
  - Caption buttons: on `WM_NCLBUTTONDOWN/UP` with those hit codes, minimize/maximize/close ourselves.
  - Mica: if build ≥ 22621, `DwmSetWindowAttribute(DWMWA_SYSTEMBACKDROP_TYPE, DWMSBT_MAINWINDOW)` and clear the sidebar area with transparent color; else fill `sidebar_bg`.
  - `DWMWA_USE_IMMERSIVE_DARK_MODE` set from `theme.dark`.
  - `WM_PAINT` → `views::paint(&mut App)`; toolbar draws back/forward glyphs (disabled tint until Task 6), placeholder title "Home", view switch, search field rect, caption buttons with hover.
  - `WM_DPICHANGED`: update `scale`, `gfx.set_dpi`, `SetWindowPos` to the suggested rect.
  - `WM_SETTINGCHANGE` with `lparam` string `"ImmersiveColorSet"`: re-read theme and accent, update dark-mode attribute, invalidate.
  - `WM_ERASEBKGND` → return 1.
  - Splitter: drag on `layout.splitter` updates `sidebar_w` with `SetCapture`; cursor `IDC_SIZEWE`.
- [ ] **Step 6: Manual verification**

Run: `cargo run --release`
Expected: window opens with toolbar, sidebar, empty content, path bar. Drag by toolbar moves it; edges resize; double-click toolbar maximizes; Win+Z / hovering max button shows Snap layouts on Win 11; minimize/maximize/close work; toggling Windows dark mode recolors live; moving between monitors of different DPI keeps text crisp.

- [ ] **Step 7: Measure start and RAM**

Run in PowerShell:
```powershell
$s=[Diagnostics.Stopwatch]::StartNew(); $p=Start-Process .\target\release\fexplorer.exe -PassThru; $null=$p.WaitForInputIdle(5000); $s.ElapsedMilliseconds; Start-Sleep 2; (Get-Process -Id $p.Id).PrivateMemorySize64/1MB
```
Expected: < 200 ms and < 15 MB at this stage. Record numbers in the commit message.

- [ ] **Step 8: Lint, build, commit** — `feat: unified-toolbar window with Direct2D rendering`

---

### Task 5: Shell services

**Files:**
- Create: `src/shell.rs`
- Modify: `src/main.rs`, `src/draw.rs` (icon cache), `Cargo.toml` (features)

**Interfaces:**
- Consumes: `Entry` (Task 1), `LocalTime` (Task 3), `Gfx` (Task 4).
- Produces:
  - `pub struct Place { pub name: String, pub path: PathBuf, pub glyph: Glyph }`
  - `pub fn favorites() -> Vec<Place>` — Home (`FOLDERID_Profile`), Desktop, Documents, Downloads, Pictures via `SHGetKnownFolderPath`; skip any that fail.
  - `pub fn drives() -> Vec<Place>` — `GetLogicalDrives` + `GetDriveTypeW` in {`DRIVE_FIXED`, `DRIVE_REMOVABLE`}; name `"{label} ({L}:)"` from `GetVolumeInformationW`, fallback `"Local Disk (C:)"`.
  - `pub fn free_space(path: &Path) -> Option<u64>` — `GetDiskFreeSpaceExW`.
  - `pub fn type_name(e: &Entry, cache: &mut HashMap<String, String>) -> String` — `"Folder"` for dirs; else `SHGetFileInfoW(SHGFI_TYPENAME | SHGFI_USEFILEATTRIBUTES)` keyed by `ext_lower()`.
  - `pub fn icon_index(e: &Entry, cache: &mut HashMap<String, i32>) -> i32` — `SHGFI_SYSICONINDEX | SHGFI_USEFILEATTRIBUTES | SHGFI_SMALLICON`, keyed by extension.
  - `pub fn to_local(filetime: u64) -> LocalTime` and `pub fn now_local() -> LocalTime`.
  - In `draw.rs`: `pub fn icon(&mut self, index: i32) -> Option<&ID2D1Bitmap>` — get `HICON` via `ImageList_GetIcon` on the system small image list (from `SHGetImageList(SHIL_SMALL)`; use `SHIL_LARGE` when scale ≥ 1.5), convert with WIC `CreateBitmapFromHICON` → `IWICFormatConverter` to `GUID_WICPixelFormat32bppPBGRA` → `CreateBitmapFromWicBitmap`; `DestroyIcon` after. Cache `HashMap<i32, ID2D1Bitmap>`, cleared on target recreate.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn favorites_exist()        // non-empty; every path.is_dir(); first is Home
#[test] fn drives_include_system()  // contains a path equal to %SystemDrive%\
#[test] fn free_space_positive()    // free_space(Path::new("C:\\")).unwrap() > 0
#[test] fn type_name_cached()       // "a.txt" entry -> non-empty, second call hits cache (cache.len()==1); dir -> "Folder"
#[test] fn icon_index_by_ext()      // two .txt entries -> same index, cache.len()==1
#[test] fn to_local_roundtrip()     // a FILETIME for 2026-11-17 12:00 UTC converts to a LocalTime on 2026-11-17 or adjacent day
```

- [ ] **Step 2: Run to verify fail** — `cargo test shell`.
- [ ] **Step 3: Implement `shell.rs` and the icon cache.**
- [ ] **Step 4: Run to verify pass** — `cargo test shell`.
- [ ] **Step 5: Lint, build, commit** — `feat: known folders, drives, type names and icons`

---

### Task 6: Navigation and list view

**Files:**
- Create: `src/nav.rs`, `src/views/list.rs`
- Modify: `src/app.rs`, `src/views/mod.rs`, `src/views/toolbar.rs`

**Interfaces:**
- Consumes: `read_folder`, `sort_entries`, `SortKey` (1); `format_*` (3); `visible_rows`, `row_at`, `Selection`, `TypeAhead`, `find_prefix` (3); `Gfx`, `Theme` (4); `type_name`, `icon_index`, `to_local`, `now_local` (5).
- Produces:
  - `nav.rs`: `pub struct History { … }` with `new(start: PathBuf)`, `current(&self) -> &Path`, `visit(&mut self, p: PathBuf)` (clears forward), `back(&mut self) -> Option<&Path>`, `forward(&mut self) -> Option<&Path>`, `can_back()`, `can_forward()`, `parent(&self) -> Option<PathBuf>`.
  - `pub struct Generation(AtomicU64)` with `next(&self) -> u64` and `is_current(&self, g: u64) -> bool`.
  - `app.rs`: `fn navigate(&mut self, path: PathBuf, push_history: bool)` — bump generation, spawn worker `read_folder`, post `WM_LISTING_DONE` with `Box<(u64, PathBuf, io::Result<Vec<Entry>>)>`. On receipt: drop if stale; on `Err` show inline error banner in content and do **not** change `current`; on `Ok` set entries, sort, reset selection and scroll, and commit history only now.
  - `views/list.rs`: `pub struct ListView { pub entries: Vec<Entry>, pub sort: (SortKey, bool), pub scroll_y: f32, pub sel: Selection, pub type_ahead: TypeAhead, pub show_where: bool }` with `paint(&self, gfx, theme, rect_header, rect_rows, …)`, `on_click(x, y, mods) -> ListAction`, `on_key(vk, mods) -> ListAction`, `on_char(ch)`, `on_wheel(delta)`. `pub enum ListAction { None, Open(usize), Redraw }`.
  - Columns and widths at scale 1: Name (flex, min 200), Date Modified 170, Size 90, Kind 140. Name cell: 16 px icon (folder glyph for dirs, system icon otherwise) + 8 px gap + name. Sort arrow chevron on the active header.

- [ ] **Step 1: Write the failing tests**

```rust
// nav.rs
#[test] fn history_back_forward()     // visit a,b,c; back->b; back->a; forward->b; visit d clears forward
#[test] fn parent_of_root_is_none()   // History::new("C:\\").parent() == None
#[test] fn stale_generation_is_dropped() // g1 = next(); g2 = next(); !is_current(g1); is_current(g2)
#[test] fn nav_failure_keeps_current()   // simulate: History at a; a failed listing for b must not call visit; current() == a (test the App-free helper `fn apply_listing(h: &mut History, gen_ok: bool, result: io::Result<..>, target, push) -> Outcome`)
```

Put `apply_listing` in `nav.rs` as a pure function returning `pub enum Outcome { Stale, Failed(String), Loaded }` so this is testable without a window.

- [ ] **Step 2: Run to verify fail** — `cargo test nav`.
- [ ] **Step 3: Implement `nav.rs`, list view, and wiring in `app.rs`.**
  - Keys: Enter → open (dir navigates; file `shell::open` added in Task 9, stub logs nothing for now); Alt+Left/Right; Alt+Up; Backspace = Back; F5 refresh; Up/Down/PageUp/PageDown/Home/End with Shift extend; Ctrl+A.
  - Mouse: click/ctrl/shift select; double-click opens; header click sorts (same column toggles direction); hover row tint.
  - Scroll: wheel moves `scroll_y` by 3 rows per notch, clamped. Smooth animation lands in Task 10; jump directly now.
  - Toolbar back/forward reflect `can_back/can_forward`; title shows folder display name (drive → `"Local Disk (C:)"` from `drives()`).
- [ ] **Step 4: Run to verify pass** — `cargo test`.
- [ ] **Step 5: Manual verification**

`cargo run --release`: opens in Home with entries sorted folders-first; double-click a folder navigates; Back/Forward/Alt+Up work; `C:\Windows\System32` (≈ 5k entries) shows instantly and scrolls smoothly; typing `no` jumps to the first name starting with "no"; navigating to `C:\System Volume Information` (via Task 7 address entry, or temporarily hardcoded) shows the inline error and keeps the previous folder.

- [ ] **Step 6: Lint, build, commit** — `feat: navigation history and virtual list view`

---

### Task 7: Sidebar, toolbar title entry, path bar

**Files:**
- Create: `src/views/sidebar.rs`, `src/views/pathbar.rs`
- Modify: `src/app.rs`, `src/views/toolbar.rs`, `src/views/mod.rs`

**Interfaces:**
- Consumes: `favorites`, `drives`, `free_space` (5); `History` (6); `Gfx`, `Theme` (4).
- Produces:
  - `sidebar.rs`: `pub struct Sidebar { pub favorites: Vec<Place>, pub drives: Vec<Place>, pub hover: Option<usize> }` with `paint(...)`, `hit(x, y, rect) -> Option<PathBuf>`. Section headers "Favorites" and "Locations" in `small` format, `text_dim`. Item rows 28 px, glyph 16 px tinted accent, rounded 6 px highlight for current location (path equals or is an ancestor, longest match wins).
  - Drives refresh on `WM_DEVICECHANGE` (`DBT_DEVICEARRIVAL` / `DBT_DEVICEREMOVECOMPLETE`).
  - `pathbar.rs`: `pub fn crumbs(path: &Path) -> Vec<(String, PathBuf)>` (pure, tested), `paint(...)`, `hit(x, y) -> Option<PathBuf>`. Right side text: `"{n} items, {free} available"` (`format_size`).
  - Address entry: a borderless `EDIT` child (`ES_AUTOHSCROLL`) shown over the title rect on Ctrl+L or title double-click, prefilled with the full path, all selected. Enter → `navigate`; Esc or focus loss → hide. Colors via `WM_CTLCOLOREDIT` returning a brush of `field_bg` blended on `window_bg`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn crumbs_drive_path()  // r"C:\Users\jaswa\Docs" -> ["C:", "Users", "jaswa", "Docs"] with cumulative paths, first path "C:\\"
#[test] fn crumbs_unc_path()    // r"\\server\share\a" -> ["\\\\server\\share", "a"]
```

- [ ] **Step 2: Run to verify fail** — `cargo test pathbar`.
- [ ] **Step 3: Implement sidebar, path bar, address entry.**
- [ ] **Step 4: Run to verify pass** — `cargo test`.
- [ ] **Step 5: Manual verification** — sidebar clicks navigate and highlight; plugging a USB drive adds it; path bar crumbs navigate; Ctrl+L, type `C:\Windows`, Enter navigates; invalid path shows the inline error; Esc cancels.
- [ ] **Step 6: Lint, build, commit** — `feat: sidebar, path bar and address entry`

---

### Task 8: Column view

**Files:**
- Create: `src/views/columns.rs`
- Modify: `src/nav.rs`, `src/app.rs`, `src/views/toolbar.rs`, `src/views/mod.rs`

**Interfaces:**
- Consumes: `read_folder`, `sort_entries` (1); `Generation`, `History` (6); `ListView` drawing helpers (6).
- Produces:
  - `nav.rs`: `pub struct Column { pub path: PathBuf, pub entries: Option<Vec<Entry>> /* None = loading */, pub selected: Option<usize>, pub scroll_y: f32 }` and `pub struct Columns { pub cols: Vec<Column>, pub scroll_x: f32 }` with:
    - `open_root(&mut self, path: PathBuf)` — one column.
    - `select(&mut self, col: usize, row: usize) -> Option<PathBuf>` — set selection, truncate columns right of `col`; if the row is a dir, push a loading column for it and return its path to load.
    - `loaded(&mut self, path: &Path, entries: Vec<Entry>)` — fill the matching column only.
    - `current_dir(&self) -> &Path` — deepest column's path.
    - `drop_offscreen(&mut self, viewport_w: f32, col_w: f32)` — columns fully left of `scroll_x` keep path and selection but set `entries = None` (reload when scrolled back).
  - `views/columns.rs`: paint columns of `COLUMN_W × scale`, each a virtual list with chevrons on folders; an info panel column when a file is selected (icon 64 px, name, kind, size, created/modified). Horizontal scroll keeps the newest column visible.
  - View switcher: Ctrl+1 list, Ctrl+2 columns; toolbar segmented control. Switching to columns opens the root at `History::current()`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn select_dir_pushes_column()   // root with dirs a,b; select(0, a) -> returns a path; cols.len()==2 and cols[1].entries.is_none()
#[test] fn select_truncates_right()     // 3 columns; select(0, b) -> cols.len()==2
#[test] fn select_file_no_push()        // file row -> returns None; cols.len() unchanged after truncate
#[test] fn loaded_ignores_unknown()     // loaded(path not in cols) changes nothing
```

- [ ] **Step 2: Run to verify fail** — `cargo test nav`.
- [ ] **Step 3: Implement.** Column loads reuse the Task 6 worker pattern with their own `Generation` per column index.
- [ ] **Step 4: Run to verify pass** — `cargo test`.
- [ ] **Step 5: Manual verification** — Ctrl+2 shows columns; clicking folders opens columns to the right; Left/Right arrows move between columns; selecting a file shows the info panel; Back/Forward and sidebar still work.
- [ ] **Step 6: Lint, build, commit** — `feat: column view`

---

### Task 9: Search UI

**Files:**
- Modify: `src/app.rs`, `src/views/toolbar.rs`, `src/views/list.rs`, `src/views/pathbar.rs`

**Interfaces:**
- Consumes: `start_search`, `SearchHandle`, `SearchStats` (2); `ListView` (6); `Columns` (8).
- Produces:
  - Search `EDIT` child inside the search field rect, placeholder painted by us: `"Search in {folder name}"`.
  - `App` fields: `search: Option<SearchHandle>`, `search_gen: u64`, `pre_search_view: View`.
  - On `EN_CHANGE`: `SetTimer(ID_SEARCH_DEBOUNCE, 150)`; on timer or Enter: cancel previous, empty query → leave search mode and restore the folder listing and view; else `start_search(current_dir, query, show_hidden, …)` whose callbacks `PostMessageW` `WM_SEARCH_BATCH` / `WM_SEARCH_DONE` with `Box<(u64, Vec<Entry>)>` / `Box<(u64, SearchStats)>`.
  - Batch receipt (current gen only): append to list entries; keep current sort by inserting batch then re-sorting only if count < 50k, else append unsorted until done and sort once at done.
  - List shows the **Where** column (relative folder, 200 px) while `show_where`. Context action "Show in Folder" navigates to `current_dir.join(rel_dir)` and selects the name.
  - Path bar shows `"Searching… {n}"` while running, then `"{n} results"` plus `", {k} folders skipped"` when `skipped_denied > 0`.
  - Ctrl+F focuses the field and selects its text. Esc in the field clears it and cancels. Navigation cancels search.
  - Search forces list view; restores previous view when cleared.

- [ ] **Step 1: Write the failing test**

```rust
// app-free helper in search.rs
#[test] fn display_status_strings() // status_text(running=true, 412, 0) == "Searching… 412"; (false, 3, 0) == "3 results"; (false, 1, 2) == "1 result, 2 folders skipped"
```

Add `pub fn status_text(running: bool, matched: usize, skipped: usize) -> String` in `search.rs`.

- [ ] **Step 2: Run to verify fail** — `cargo test search`.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run to verify pass** — `cargo test`.
- [ ] **Step 5: Manual + performance verification**
  - Typing in Home finds files in subfolders, results stream in; typing more cancels and restarts; Esc restores the folder.
  - Timing: `cargo test --release -- --ignored search_200k` passes (engine < 1 s). Then in the app, search `f99` in `%TEMP%\fx-bench`; results must appear with no UI stutter.
- [ ] **Step 6: Lint, build, commit** — `feat: scoped search UI`

---

### Task 10: File actions and context menu

**Files:**
- Modify: `src/shell.rs`, `src/app.rs`, `src/views/list.rs`, `src/views/columns.rs`

**Interfaces:**
- Consumes: `Selection`, `ListView`, `Columns`, `History`.
- Produces in `shell.rs` (all take `hwnd` as owner for OS dialogs):
  - `pub fn open(path: &Path) -> Result<()>` — `ShellExecuteW(…, "open", …)`; error dialog via `MessageBoxW` on failure.
  - `pub fn rename(hwnd, path: &Path, new_name: &str) -> Result<()>` — `IFileOperation::RenameItem`. Reject empty names and names containing `\/:*?"<>|` before calling, with an inline message.
  - `pub fn new_folder(hwnd, dir: &Path) -> Result<PathBuf>` — `NewItem` with name `"New folder"`, OS resolves conflicts via `FOF_RENAMEONCOLLISION`; return the final path from the sink, or re-list and pick the newest `New folder*`.
  - `pub fn delete(hwnd, paths: &[PathBuf], permanent: bool, confirm: bool) -> Result<()>` — `FOFX_RECYCLEONDELETE` unless `permanent`; `FOF_NOCONFIRMATION` when `!confirm`. The UI always passes `confirm = true`.
  - `pub fn clipboard_put(hwnd, paths: &[PathBuf], cut: bool) -> Result<()>` — `CF_HDROP` (`DROPFILES` + double-null wide list) and `"Preferred DropEffect"` = `DROPEFFECT_MOVE` or `DROPEFFECT_COPY`.
  - `pub fn clipboard_get(hwnd) -> Option<(Vec<PathBuf>, bool /*cut*/)>` — via `DragQueryFileW`.
  - `pub fn paste(hwnd, items: &[PathBuf], cut: bool, dest: &Path) -> Result<()>` — `CopyItems` or `MoveItems`, then `PerformOperations`. After a cut-paste, clear the clipboard.
- Inline rename: an `EDIT` child placed over the name cell; selects the stem (without extension) on start; Enter commits, Esc cancels, focus loss commits.
- Context menu via `CreatePopupMenu` + `TrackPopupMenu`: on items → Open, Show in Folder (search only), Rename, Copy, Cut, Delete; on empty space → Paste (disabled if clipboard empty), New Folder, a check item Show Hidden Files.
- After every action, refresh the current folder (or update rows in place during search: remove deleted, rename renamed).

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn rename_rejects_bad_names()   // validate_name("") Err; "a:b" Err; "ok.txt" Ok  — pure pub fn validate_name(&str) -> Result<(), &'static str>
#[test] fn rename_roundtrip()           // temp file a.txt -> rename(HWND::default(), …, "b.txt"); b.txt exists, a.txt gone
#[test] fn new_folder_creates()         // in temp dir twice -> "New folder" and "New folder (2)"
#[test] fn delete_permanent_removes()   // delete(.., permanent=true, confirm=false) -> file gone
#[test] fn clipboard_roundtrip()        // put [a,b] cut=true -> get returns same paths and cut==true
#[test] fn paste_copy_into_dir()        // copy a.txt into dest -> dest\a.txt exists, source remains
```

Tests that touch the clipboard or `IFileOperation` call `CoInitializeEx` themselves and run with `--test-threads=1`.

- [ ] **Step 2: Run to verify fail** — `cargo test shell -- --test-threads=1`.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run to verify pass** — `cargo test -- --test-threads=1`.
- [ ] **Step 5: Manual verification** — copy in our app, paste in Windows Explorer and the reverse; cut-paste moves; Del sends to Recycle Bin (restore works); Shift+Del asks and deletes; F2 renames with stem selected; Ctrl+Shift+N creates and enters rename; Show Hidden toggles; actions in search results update rows.
- [ ] **Step 6: Lint, build, commit** — `feat: file actions, clipboard and context menu`

---

### Task 11: Animations, polish, and target verification

**Files:**
- Modify: `src/app.rs`, `src/views/*.rs`

**Interfaces:**
- Produces: `pub struct Anim { from: f32, to: f32, start: Instant, dur: Duration }` in `layout.rs` with `value(now) -> f32` (ease-out cubic) and `done(now) -> bool`. One `ID_ANIM` timer at 16 ms, started when any `Anim` is active, killed when none are.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn anim_eases_and_ends()   // value at 0 == from; at dur == to; at half > linear midpoint (ease-out); done after dur
```

- [ ] **Step 2: Run to verify fail** — `cargo test layout`.
- [ ] **Step 3: Implement** hover fade (100 ms), smooth wheel scroll (120 ms, retarget on repeated notches), column slide (150 ms on `scroll_x`). Respect `SPI_GETCLIENTAREAANIMATION`: if off, jump instantly.
- [ ] **Step 4: Run to verify pass** — `cargo test`.
- [ ] **Step 5: Verify every target from Global Constraints**

```powershell
cargo build --release
$s=[Diagnostics.Stopwatch]::StartNew(); $p=Start-Process .\target\release\fexplorer.exe -PassThru; $null=$p.WaitForInputIdle(5000); "start ms: $($s.ElapsedMilliseconds)"
```
Then navigate to a 10k-file folder (`$env:TEMP\fx-bench\..` built with 100 dirs × 100 files flat into one folder), wait 10 s idle, and run:
```powershell
$q=Get-Process fexplorer; "private MB: $($q.PrivateMemorySize64/1MB)"; $c1=$q.TotalProcessorTime; Start-Sleep 10; $q.Refresh(); "idle cpu ms over 10s: $(($q.TotalProcessorTime-$c1).TotalMilliseconds)"
```
Expected: start < 200 ms; private memory < 25 MB; idle CPU ms ≈ 0 (< 20 ms over 10 s). Scroll 10k rows: no visible stutter (check with the Windows Game Bar FPS counter or `PresentMon`, ≥ 60 fps). Navigate 30 times back and forth and search 10 times; private memory must return within 2 MB of the first idle reading.

- [ ] **Step 6: Lint, build, commit** — `feat: animations and verified performance targets`. Put the measured numbers in the commit body.
