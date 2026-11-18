# Finder Layout v2 — Design and Plan

Date: 2026-11-17
Status: draft, awaiting approval
Builds on: `2026-11-17-file-explorer-design.md` (v1, implemented on branch `feat/explorer`)
Reference: the user's macOS Finder screenshot (Sonoma, icon view, "Library" folder)

## 1. What changed in the requirements

| Item | v1 | v2 |
|---|---|---|
| Memory budget | < 25 MB idle | **75–100 MB** allowed. Soft target 75 MB, hard cap 100 MB. |
| Layout | Finder-inspired, list + columns | **Same layout as the screenshot**: icon grid by default, labeled toolbar, 4 view modes, Tags in the sidebar |
| Distribution | `cargo run` | An installable app, with clear install steps |

What stays the same: Rust, Win32 + Direct2D, offline, no admin rights, no background service, no telemetry.

## 2. Screenshot → component map

Each region of the screenshot, and what we change to match it.

| # | Region in screenshot | Today (v1) | v2 change | Kind |
|---|---|---|---|---|
| 1 | Floating window, large rounded corners, soft shadow | Square-ish window, Win 11 default corners | Ask Windows 11 for rounded corners (`DWMWA_WINDOW_CORNER_PREFERENCE = ROUND`). Shadow already comes from DWM. | Edit |
| 2 | Traffic-light buttons, top-left | Windows min/max/close, top-right | **Keep Windows buttons top-right** (see Decision D1). | Keep |
| 3 | Sidebar as a full-height translucent panel, inset from the window edge | Full-height sidebar with Mica | Draw the sidebar as a rounded inner panel (8 px margin, 10 px radius) over Mica, like Sonoma. | Edit |
| 4 | Sidebar sections: Favorites, iCloud, Locations, Tags | Favorites, Locations | Four sections: **Favorites, Cloud, Locations, Tags**. Section headers collapse on click. | Edit + New |
| 5 | Favorites items: AirDrop, Recents, Applications, Desktop, Documents, Downloads | Home, Desktop, Documents, Downloads, Pictures | **Recents, Applications, Desktop, Documents, Downloads, Home, Pictures**. AirDrop has no Windows equivalent; sharing goes through the Share button. | Edit |
| 6 | iCloud: iCloud Drive, Shared | — | **Cloud: OneDrive** (if installed), plus any other synced roots Windows reports. | New |
| 7 | Locations: MacBook Air, Test, Network | Drives | Drives (fixed, removable) + **Network** (mapped drives and recent network locations). | Edit |
| 8 | Tags: colored dots (Red, Orange, Yellow, Green, Blue, Purple, Gray) + All Tags | — | Same seven colors plus custom tags. Click a tag to list all files with it. | New |
| 9 | Toolbar: Back/Forward pair with caption | Back, Forward icons | Same buttons, grouped as one pill, with a small caption under it. | Edit |
| 10 | Folder title next to Back/Forward | Title | Keep. | Keep |
| 11 | View control: 4 segments (Icon, List, Columns, Gallery) with "View" caption | 2 segments (List, Columns) | **4 segments**: Icon, List, Columns, Gallery. | Edit + New |
| 12 | Group button | — | Group by: None, Kind, Date Modified, Size, Name. Adds section headers in all views. | New |
| 13 | Share button | — | Opens the Windows share sheet for the selection. | New |
| 14 | Add Tags button | — | Popover with color tags and a text box for custom tags. | New |
| 15 | Action (⋯) button | Right-click menu only | Same menu as right-click, from the toolbar. | Edit |
| 16 | Search: a magnifier icon that expands into a field | Always-visible field | Icon by default; expands into a field on click or Ctrl+F; collapses when empty. | Edit |
| 17 | Toolbar captions under each control ("View", "Group", "Share"…) | No captions | "Icon and text" toolbar mode. Toolbar height 52 → 64 px. A setting switches to icon-only. | Edit |
| 18 | **Icon view grid** (content area): big light-blue folder icons, names wrapped to 2 lines, centered | — | New default view (see §4). | New |
| 19 | Selection: grey rounded box behind the icon, accent pill behind the name | Accent row in list view | Same style in icon view. List and column views keep their row style. | New |
| 20 | No path bar or status bar visible | Path bar always shown | Path bar **off by default**, like Finder. Toggle with Ctrl+Alt+P or the Action menu. | Edit |
| 21 | Light theme, white content, soft grey sidebar | Follows Windows theme | Keep following Windows. Light theme colors are tuned toward the screenshot. | Edit |

## 3. Decisions (my calls — please confirm or change)

- **D1. Window buttons stay Windows-style, top-right.** Apple's traffic lights are Apple's trade dress, and Windows Snap layouts only appear on a real Windows max button. Everything else in the layout matches the screenshot.
- **D2. Our own icons, not Apple's.** The light-blue folder is drawn by us as vector art in the same flat style. File icons come from Windows. Apple's icon files and the SF Pro font are not used (license).
- **D3. GPU rendering comes back.** The 75–100 MB budget allows the GPU render target (measured 47 MB on your Intel GPU), which scrolls large grids and thumbnails more smoothly. If GPU setup fails, the app falls back to WARP software rendering.
- **D4. Tags live in a small local file**, `%APPDATA%\Files\tags.tsv`, keyed by each file's NTFS file ID, so a tag follows the file when it is renamed or moved on the same drive. Other apps cannot see these tags. (Alternative: NTFS alternate data streams travel with the file, but are lost on copy to USB/FAT32, cloud folders, or email. Rejected for reliability.)
- **D5. "Applications"** lists Start-menu shortcuts (all-users + current user), shown as an icon grid. Opening one launches the app.
- **D6. "Recents"** reads the Windows recent-items folder (`shell:recent`), which Windows already maintains. We store nothing extra.
- **D7. Icon view is the new default view.** The last-used view is remembered per folder in `%APPDATA%\Files\views.tsv` (small, capped at 2,000 folders).
- **D8. Gallery view uses Windows' own thumbnail cache** (`IShellItemImageFactory`). We do not decode images ourselves.

## 4. Icon view (the screenshot's main area)

- Cell: 104 × 96 DIP. Icon 64 DIP. Name: 2 lines max, centered, ellipsis in the middle for long names.
- Columns = floor(content width / cell width); leftover space spread evenly between cells.
- Virtualized: only visible rows are drawn and only visible thumbnails are requested.
- Folders: our vector folder (light blue, darker tab). Files: Windows icon at 48/64 px; images and PDFs show a thumbnail when "Show previews" is on.
- Selection: click, Ctrl+click, Shift+click, rubber-band drag-box (new; Finder supports it in this view).
- Keyboard: arrows move in 2D, Home/End, type-to-select, Enter opens, F2 renames under the icon.
- Icon size slider (Ctrl+Plus/Minus): 48–128 DIP.

## 5. Gallery view

- Large preview of the selected item (thumbnail up to 512 px) on top; a horizontal strip of thumbnails below; an info panel on the right (Kind, Size, Created, Modified, Dimensions for images).
- Arrow keys move through the strip.

## 6. Memory budget (100 MB hard cap)

| Part | Budget |
|---|---|
| GPU render target, D2D/DirectWrite, Windows UI libraries | ~47 MB (measured) |
| Folder listing + search results (up to ~100k items) | ≤ 15 MB |
| Thumbnail cache (LRU, evicts oldest) | ≤ 20 MB |
| Icons, fonts, tag/view stores | ≤ 5 MB |
| Headroom | ~13 MB |
| **Total** | **≤ 100 MB** |

The thumbnail cache is the only part that grows with use, and it is capped. If GPU memory pushes us over, the cache cap shrinks first.

New targets: idle ≤ 75 MB with a 10k-file folder in icon view; peak ≤ 100 MB during gallery browsing; idle CPU 0 %; 60 fps scrolling in every view; cold start ≤ 300 ms (GPU device creation costs ~100 ms).

## 7. Installing it on your laptop

### Today (works now, no installer)

The app is one portable file with no dependencies.

```bash
cargo build --release
```

Then copy `target\release\fexplorer.exe` to any folder, for example `%LOCALAPPDATA%\Programs\Files\`, and double-click it. To find it in Start, right-click the exe → **Show more options → Pin to Start**.

Windows SmartScreen may warn "Windows protected your PC" the first time, because the exe is not code-signed. Click **More info → Run anyway**. This is normal for self-built apps.

### Planned (part of this plan)

1. **App identity in the exe:** an app icon, the name "Files", and a version number (Windows resource file, built in `build.rs`).
2. **`install.ps1` (per-user, no admin):**
   - copies the exe to `%LOCALAPPDATA%\Programs\Files\`
   - creates a Start menu shortcut and an optional desktop shortcut
   - registers an uninstaller in **Settings → Apps → Installed apps**
   - optional: adds **"Open in Files"** to the right-click menu of folders (in Windows Explorer and on the desktop)
3. **`uninstall.ps1`** removes everything above, including `%APPDATA%\Files\` on request.
4. **Release zip:** `Files-<version>-x64.zip` containing the exe, `install.ps1`, `uninstall.ps1`, and a README.
5. **Command line:** `fexplorer.exe "C:\some\folder"` opens that folder (needed for the context-menu entry).

Not planned: replacing Windows Explorer as the system default (Win+E, desktop, taskbar). That needs registry changes that can break the shell, and Windows updates often undo them. "Open in Files" gives the same benefit safely. A signed MSIX/winget package can come later; it needs a code-signing certificate (paid).

## 8. Non-goals (v2)

- AirDrop or any network sharing of our own (Windows share sheet covers it).
- Quick Look (Space to preview). Gallery view covers previews. Can be added later.
- iCloud integration. OneDrive and other Windows sync folders appear as normal folders.
- Tabs, multiple windows, Finder-style "Smart Folders".
- Any change to the search engine (still name search, scoped to the current folder tree).

## 9. Implementation plan

Each task is test-first, ends green (`cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo build --release`), and gets its own commit. Pure logic goes in testable modules; drawing is checked by screenshot.

| # | Task | Main files | Tests that prove it |
|---|---|---|---|
| 1 | **GPU rendering with WARP fallback**, rounded window corners | `draw.rs`, `app.rs` | Unit: target-type fallback logic. Manual: memory with GPU, scroll fps. |
| 2 | **Toolbar v2**: grouped Back/Forward, 4-segment view control, Group/Share/Tags/Action buttons, collapsible search, captions, 64 px height | `views/toolbar.rs`, `layout.rs` | `toolbar_rects` no-overlap at 800–2560 px widths; captions hidden in icon-only mode; search collapses when empty. |
| 3 | **Sidebar v2**: inset rounded panel, collapsible sections, Recents/Applications/Cloud/Network | `views/sidebar.rs`, `shell.rs` | `favorites()` includes Recents/Applications when present; OneDrive found via known folder; section collapse hit-testing. |
| 4 | **Icon view grid**: layout math, virtualization, 2-line names, selection style, 2D keys, icon size | `views/grid.rs` (new), `layout.rs` | `grid_cols`, `cell_at`, `visible_cells`, 2D arrow movement, name wrapping into ≤ 2 lines, size clamps 48–128. |
| 5 | **Rubber-band selection** in icon view | `views/grid.rs`, `selection.rs` | Cells intersecting a drag rect are selected; Ctrl adds to the selection. |
| 6 | **Thumbnails + LRU cache** (Windows thumbnail cache, 20 MB cap) | `thumbs.rs` (new) | LRU evicts oldest past the byte cap; worker returns stale-generation results that are dropped. |
| 7 | **Gallery view** | `views/gallery.rs` (new) | Strip layout math; selection follows arrows; info fields formatted. |
| 8 | **Group by** with section headers in icon/list views | `listing.rs`, `views/*` | `group_entries` by Kind/Date buckets (Today, Yesterday, Previous 7 Days, …)/Size/Name; headers counted in layout. |
| 9 | **Tags**: local store keyed by file ID, Add Tags popover, sidebar tag filter, colored dots in all views | `tags.rs` (new), `views/*` | Store round-trip; tag survives rename (same file ID); unknown IDs pruned; filter lists only tagged files. |
| 10 | **Share** (Windows share sheet) and **Action** menu | `shell.rs`, `app.rs` | Menu content per selection state; share invoked with the selected paths (manual check for the sheet). |
| 11 | **Per-folder view memory** and path bar toggle | `prefs.rs` (new) | Store capped at 2,000 entries; last view restored on revisit. |
| 12 | **Packaging**: exe icon + version, command-line folder argument, `install.ps1` / `uninstall.ps1`, release zip | `build.rs`, `app.manifest`, `packaging/` | Arg parsing tests; install → shortcut + uninstall entry exist; uninstall removes them (run in a test user profile path). |
| 13 | **Verify targets** (§6) and the v1 targets that still apply | — | Benchmark scripts: idle ≤ 75 MB, peak ≤ 100 MB, 0 % idle CPU, ≤ 16 ms/frame in every view. |

Suggested order: 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9 → 10 → 11 → 12 → 13. Tasks 9–11 are independent of 6–7 and can swap.

## 10. Open items carried from v1

- No screen-reader support (UI Automation) yet. Icon and gallery views make this gap bigger. Recommended as its own project after v2.
- Context menus are light-themed in dark mode.
- Column view has no rename or type-to-select.
