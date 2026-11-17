# Fast Light File Explorer — Design

Date: 2026-11-17
Status: draft, awaiting approval

## Intent

Windows Explorer is slow and messy, and finding a file inside a folder tree is painful.
Build a replacement that looks premium and calm, in the design spirit of Apple's Finder.
It must start fast, use very little RAM and CPU, and run fully offline.

The user browses folder by folder and roughly knows where a file is.
When stuck, the user searches inside the current folder and its subfolders, by file name.

## Goals

- A professional, high-end look: clean layout, quiet colors, generous spacing, crisp text.
- Finder-style structure: sidebar, unified toolbar, list view, column view, path bar.
- Browse folders fast: open a folder of 10k files with no visible delay.
- Search by file name inside the current folder subtree, with results streaming in.
- Basic file actions: open, rename, new folder, copy, cut, paste, delete.
- Light and dark mode, following the Windows setting.
- Performance targets on a typical laptop:
  - Cold start to usable window: under 200 ms.
  - Idle RAM: under 25 MB with a 10k-file folder open.
  - Idle CPU: 0 %. No timers run while idle.
  - Search of a 200k-file tree (warm disk cache): under 1 s to finish.
  - 60 fps scrolling. The UI thread never blocks on disk I/O.

## Non-goals (v1)

- Pixel copy of Finder. No Apple icons, SF fonts, or Apple artwork. No traffic-light window buttons.
- Icon grid view, gallery view, Quick Look preview, tags.
- Whole-drive index (NTFS MFT / USN journal).
- Search inside file contents.
- Tabs, thumbnails.
- The Windows shell right-click menu (IContextMenu, "Open with", 7-Zip entries).
- Drag and drop, rubber-band (drag-box) selection.
- Live auto-refresh from file system watchers. F5 refreshes; our own actions refresh automatically.
- Cross-platform support.

## Anti-goals (failure even if it works)

- Any background service, tray process, or startup entry.
- Any network call or telemetry.
- Needing admin rights.
- Memory that grows over a long session.
- Animations that make the app feel slower than Explorer.

## Constraints

- Windows 10 1903+ and Windows 11, x64. Windows 11 gets the Mica backdrop; Windows 10 gets flat colors.
- Rust stable (1.98.1 installed). Single `.exe`, no installer.
- Only dependency: Microsoft's `windows` crate (platform bindings). Everything else is std.
- Apple's assets and fonts are not used. The look is our own, Finder-inspired.

## Approaches considered

### Rendering (decided)

| Approach | RAM idle | Effort | Look | Verdict |
|---|---|---|---|---|
| Win32 window + Direct2D/DirectWrite custom drawing | ~15–25 MB | Highest | Full control, GPU text and shapes | **Picked by user** |
| egui, restyled | ~40–70 MB | Medium | Custom, weaker text polish | Rejected: heavier, harder to reach "high-end" |
| Win32 standard controls, recolored | ~10 MB | Lowest | Still looks like Windows; no column view | Rejected: fails the look goal |
| Tauri / Electron | 100+ MB | Medium | Web | Rejected early: defeats "very light" |

The user first picked plain native Win32, then the Finder-style look requirement changed the choice to Direct2D custom drawing.

## Design

### Window

```
+--------------------------------------------------------------------------------+
| [<] [>]  Documents              [≡ list | ▥ columns]   [🔍 Search      ]  _ □ X |
+------------------+-------------------------------------------------------------+
| Favorites        | Name                     Date Modified      Size    Kind    |
|   ⌂ Home         | ▸ 📁 invoices            Nov 2, 2026        --      Folder  |
|   ▭ Desktop      |   📄 report.pdf          Nov 15, 2026      1.2 MB   PDF     |
|   📄 Documents   |   📄 notes.txt           Today, 09:14       4 KB    Text    |
|   ⬇ Downloads    |                                                             |
| Locations        |                                                             |
|   💽 C:          |                                                             |
|   💽 D:          |                                                             |
+------------------+-------------------------------------------------------------+
| 💽 C: › Users › jaswa › Documents                        1,204 items, 38 GB free |
+--------------------------------------------------------------------------------+
```

- One top-level `HWND`. No title bar strip: the toolbar is the title bar (unified toolbar).
  - Handle `WM_NCCALCSIZE` to remove the caption; handle `WM_NCHITTEST` to make empty toolbar space draggable.
  - Keep the standard Windows minimize, maximize, close buttons on the right, drawn by us with Windows' hover colors. Snap layouts work through `HTMAXBUTTON` hit testing.
- Windows 11: Mica backdrop (`DWMWA_SYSTEMBACKDROP_TYPE`) behind the sidebar. The content area is opaque.
- Dark title bar and borders via `DWMWA_USE_IMMERSIVE_DARK_MODE`.
- The sidebar width is draggable (160–320 px).

### Rendering

- `ID2D1HwndRenderTarget` for drawing, DirectWrite for text with **Segoe UI Variable** (falls back to Segoe UI).
- Draw only on `WM_PAINT`. Invalidate only on input, data change, or an active animation.
- Lists are virtual: draw only visible rows. Row height 24 px at 100 % scale, scaled by DPI.
- Animations (only these): hover fade (100 ms), smooth wheel scroll (120 ms ease-out), column view slide (150 ms). A timer runs only while an animation is active.
- Per-monitor DPI aware v2. Recreate text formats on `WM_DPICHANGED`.
- Handle `D2DERR_RECREATE_TARGET` by rebuilding device resources.

### Text input

- Search field, address entry, and rename box use **native `EDIT` child windows**, borderless, colored with `WM_CTLCOLOREDIT`.
- This gives IME, selection, undo, and clipboard support for free. We draw the rounded field background around them.

### Theme

- Read `AppsUseLightTheme` from `HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize`.
- Repaint on `WM_SETTINGCHANGE` with `"ImmersiveColorSet"`.
- Selection color uses the Windows accent color (`DwmGetColorizationColor` or `UISettings`).
- All colors live in one `Theme` struct with a light and a dark instance.

### Sidebar

- Favorites: Home, Desktop, Documents, Downloads, Pictures (from `SHGetKnownFolderPath`).
- Locations: fixed and removable drives from `GetLogicalDrives`. Shows label and letter.
- Small monochrome glyphs drawn as Direct2D geometry, so they are sharp at every DPI and theme.
- Click navigates. The current location is highlighted.

### Toolbar

- Back, Forward (history stack).
- Current folder name in semibold. Double-click it, or press Ctrl+L, to type a path (native EDIT).
- View switcher: List | Columns.
- Search field. Typing starts a scoped search (see Search).

### List view

- Columns: Name, Date Modified, Size, Kind. Click a header to sort; click again to reverse.
- Default sort: folders first, then name, natural and case-insensitive (`file2` before `file10`).
- Dates use relative words for today and yesterday, otherwise a short date.
- Selection: click, Ctrl+click, Shift+click, arrow keys, Ctrl+A.
- Type-to-select: typing letters jumps to the first matching name (reset after 1 s).
- Zebra rows off; a subtle hover highlight; accent-colored selection.

### Column view

- Each folder opens in a new column to the right, Finder-style. Columns are 220 px, horizontally scrollable.
- Left/Right arrows move between columns; Up/Down move within one.
- Selecting a file shows a small info column: icon, name, kind, size, dates. No content preview.
- Each column is a virtual list. Columns scrolled out of view free their data.

### Path bar

- Bottom strip: clickable breadcrumb of the current path, item count, and free space on the drive.

### Icons

- File icons from `SHGetFileInfoW` with `SHGFI_USEFILEATTRIBUTES | SHGFI_SYSICONINDEX` (by extension, no disk access).
- Convert each icon once to a Direct2D bitmap with WIC `CreateBitmapFromHICON`. Cache by system icon index.
- Folders use one drawn folder glyph in the accent tint, for a consistent premium look.
- The cache is bounded by the number of distinct icon indexes, so it does not grow without limit.

### Folder listing

- A worker thread reads the folder with `std::fs::read_dir` and posts the finished `Vec<Entry>` with `PostMessageW`.
- A generation counter (`AtomicU64`) tags every job. The UI drops results from old generations.
- `Entry`: name, size (`u64`), modified time (`u64` FILETIME), attributes (`u32`). Search results also hold the parent path relative to the search root.
- Hidden and system files: hidden by default, with a toggle in the context menu.

### Search

- Scope: the current folder and all subfolders. The search field shows the scope ("Search in Documents").
- Match: case-insensitive substring of the name. An empty query exits search mode.
- Starts 150 ms after the last keystroke, or immediately on Enter.
- Parallel walk using std only:
  - A shared directory queue (`Mutex<Vec<PathBuf>>` + `Condvar`).
  - `available_parallelism()` worker threads pop a directory, read it, push subdirectories, test names.
  - Matches reach the UI in batches, at most every 50 ms, via `PostMessageW`.
  - Workers stop when the queue is empty and no worker is busy.
- Cancel: a new query, Esc, or navigation bumps the generation counter. Workers check it per directory.
- Skip reparse points (junctions, symlinks) to avoid loops.
- Skip folders that deny access. The path bar shows the skipped count.
- Results use list view with an extra **Where** column (relative folder). Enter opens; a menu item opens the containing folder.
- Worker threads exist only during a search.

### File actions

- Open: `ShellExecuteW` default verb. Folders navigate in-app.
- Rename (F2): EDIT overlay, then `IFileOperation::RenameItem`.
- New folder (Ctrl+Shift+N): `IFileOperation::NewItem`, then start rename.
- Delete (Del): `IFileOperation` with `FOFX_RECYCLEONDELETE`. Shift+Del is permanent; the OS confirms.
- Copy / Cut / Paste: clipboard format `CF_HDROP` + `Preferred DropEffect`, compatible with Explorer. Paste uses `CopyItems` / `MoveItems`.
- `IFileOperation` gives OS progress, conflict, and undo dialogs.
- Right-click opens our own menu (`TrackPopupMenu`): Open, Rename, Copy, Cut, Paste, Delete, New Folder, Show Hidden Files.
- Actions work in search results too. Affected rows update in place.

### Keyboard

| Key | Action |
|---|---|
| Enter | Open selected |
| F2 | Rename |
| Alt+Left / Alt+Right | Back / Forward |
| Alt+Up | Parent folder |
| Ctrl+L | Type a path |
| Ctrl+F | Focus search |
| Esc | Cancel search, clear field |
| Ctrl+1 / Ctrl+2 | List view / Column view |
| F5 | Refresh |
| Del / Shift+Del | Recycle / permanent delete |
| Ctrl+C / X / V | Copy / Cut / Paste |
| Ctrl+Shift+N | New folder |
| Ctrl+A | Select all |
| Letters | Type-to-select |

Enter opens (Windows convention), not rename (Finder convention), because the user works on Windows.

### Platform details

- Manifest: per-monitor DPI aware v2, `longPathAware`, Common Controls v6.
- COM apartment-threaded on the UI thread.
- Release profile: `opt-level = 3`, `lto = true`, `codegen-units = 1`, `panic = "abort"`, `strip = true`.
- Start folder: Home.

### Code layout (proposal)

- `src/main.rs` — window, message loop, input routing.
- `src/theme.rs` — colors, metrics, light/dark detection.
- `src/draw.rs` — Direct2D/DirectWrite resources and drawing helpers.
- `src/views.rs` — sidebar, toolbar, list view, column view, path bar.
- `src/listing.rs` — `Entry`, folder read, sort. Pure, testable.
- `src/search.rs` — parallel walker and matcher. Pure, testable.
- `src/shell.rs` — known folders, icons, open, `IFileOperation`, clipboard.

### Testing

- Unit tests for `listing` and `search` on temp directory trees: match rule, natural sort, case folding, reparse-point skip, cancel.
- Unit tests for view math: visible row range, hit testing, column scroll.
- A manual checklist for UI, theme switch, DPI change, and file actions.
- Measure RAM and CPU with `Get-Process` against the targets.

## Builder's calls (decided during write-up)

- Search debounce 150 ms; UI batch interval 50 ms.
- Hidden files off by default, toggle in the context menu.
- Start folder is Home. No saved state between runs in v1 (window size, view, column widths reset).
- The address bar accepts any path Windows accepts, including UNC. An invalid path shows an inline error and keeps the current folder.
- No custom favorites in v1; the sidebar list is fixed.
- Folder glyph is drawn by us; file icons come from Windows.
- Windows caption buttons stay; no macOS traffic lights.
- Enter opens, F2 renames.
- Search results always show in list view. If column view is active, it switches to list during search and back when the search clears.

## Left to the builder

- Exact message numbers, struct layout, and hit-test code.
- Exact color values and spacing, inside the Theme struct.
- Error wording.
