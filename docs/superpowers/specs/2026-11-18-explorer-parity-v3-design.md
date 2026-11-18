# Explorer Parity v3 — Design and Plan

Date: 2026-11-18
Status: draft, awaiting approval
Builds on: v1 and v2 (branch `feat/explorer`, installed as Files 0.2.0)

## Intent

The user wants everything Windows File Explorer can do (copying, moving, sorting, organizing, view sizes, settings) with Finder's look and animations. Visuals stay as v2. This plan closes the functional gap and finishes the items left pending in v1/v2.

## What already matches Explorer

Navigation (back, forward, up, address bar, breadcrumbs), scoped search, open, rename, new folder, copy/cut/paste (works with Explorer), delete to Recycle Bin, permanent delete, show hidden files, sort by column (list view), group by, icon/list/column/gallery views, icon zoom, thumbnails, sidebar with known folders and drives, right-click menu, share, per-folder view memory.

## Gaps, in priority order

### A. Copy, move and organize (the user's first ask)

| # | Feature | How | Size |
|---|---|---|---|
| A1 | **Drag and drop** inside the app and to/from Explorer, the desktop and other apps (move on same drive, copy across drives, Ctrl = copy, Shift = move, like Explorer) | OLE drag source + drop target (`SHDoDragDrop`, `IDropTarget`) | L |
| A2 | **Copy to… / Move to…** with a folder picker | `IFileOpenDialog` (pick folder) + existing `IFileOperation` | S |
| A3 | **Undo / redo** of the last copy, move, rename, delete (Ctrl+Z / Ctrl+Y) | Our own undo stack of completed operations (Explorer's undo store is not a public API) | M |
| A4 | **Pin folders to Favorites** (and unpin, reorder) | Stored in prefs; sidebar "Add to Favorites" / "Remove" | S |
| A5 | **Copy as path** (Ctrl+Shift+C), **Select all / Select none / Invert selection** | Clipboard text; selection model | S |
| A6 | **New → Text document / other templates** | Shell "New" items via `IFileOperation::NewItem` | S |
| A7 | **Conflict handling** when pasting/dropping onto existing names (Replace, Skip, Keep both) | Already given by `IFileOperation`'s dialog; verify and expose | S |

### B. Sort and view (the user's second ask)

| # | Feature | How | Size |
|---|---|---|---|
| B1 | **View sizes like Explorer**: Extra large, Large, Medium, Small icons, List, Details, plus Columns and Gallery | Icon presets 192 / 128 / 64 / 32 px; Small icons and List use a compact row-flow layout (icon left of name); Details = current list view | M |
| B2 | **Sort menu in every view**: Name, Date modified, Type, Size, Date created; Ascending / Descending | Toolbar View/Sort menus + right-click "Sort by"; date created read with the listing | S |
| B3 | **Choose and resize columns in Details** (add Date created, drag column edges) | Column chooser on header right-click; drag to resize | M |
| B4 | **Status line**: "12 items · 3 selected · 4.2 MB" | Path bar status shows selection count and total size | S |

### C. Explorer settings

| # | Setting | Default |
|---|---|---|
| C1 | Show file name extensions | On (Finder shows them; Explorer hides) |
| C2 | Show hidden items (exists) | Off |
| C3 | Show protected system files | Off |
| C4 | Open each folder in the same window / new window (Ctrl+N opens a new window) | Same window |
| C5 | **Click items as follows** — *Single-click to open (point to select)*, or *Double-click to open (single-click to select)*, like Explorer's Folder Options | **Follow Windows** (this laptop: single-click) |
| C6 | Show path bar (exists), show thumbnails, folder-first sort | On |
| C7 | Start folder: Home / Recents / This PC | Home |

A **Settings** window (Ctrl+,) lists these; everything persists in `prefs.tsv`.

**Follow Windows by default.** C1, C2, C3 and C5 start from the user's own Explorer settings (read with `SHGetSettings` at startup and when Windows broadcasts a settings change). On this laptop that means: single-click to open, extensions shown, hidden files hidden. Changing one in our Settings window overrides it for Files only; "Use Windows setting" puts it back.

**Single-click mode** (Explorer's "point to select"): hovering an item selects it after a short pause, the name is underlined under the pointer, one click opens it. Ctrl/Shift+click still multi-select. Drag and the drag box still work.

## F. Custom file-transfer experience (replaces the Windows copy dialog)

Today copy, move, delete and drag-and-drop show Windows' own progress and conflict dialogs. v3 replaces them with our own animated UI.

**How it works**
- Every transfer runs on a background thread through `IFileOperation` with Windows' progress UI turned off (`FOF_SILENT`) and our `IFileOperationProgressSink` receiving progress: current file, bytes done/total, files done/total.
- The UI thread draws the transfer panel at 60 fps while something is moving; the timer stops when nothing is.
- **Conflicts** are checked before the operation starts. Our own conflict card asks Replace / Skip / Keep both (with "Do this for all"), showing both files' thumbnails, sizes and dates. The answers become the operation's flags and item list, so Windows never shows its dialog.
- **Cancel** works at any time (the sink returns `E_ABORT`); a cancelled move leaves already-moved files in place, like Explorer.
- Several transfers can run at once; each gets its own row in the panel.
- Speed and time remaining are a moving average over the last 2 s.
- Errors (access denied, disk full, file in use) appear in the transfer row with Retry / Skip, not as modal dialogs.
- Respects "Show animations in Windows" (already wired): with it off, the same UI appears without motion.

**Animation set (to finalize together; see the design preview)**
- **Launch:** selected items lift and fly along a curve to the destination (sidebar item, folder, or drive), shrinking into it; the destination gives a small "breathe".
- **Transfer panel:** a pill at the bottom of the window that expands into a card: progress ring or bar with a shimmer, current file, `12 of 340 · 1.2 GB of 4.8 GB · 85 MB/s · about 40 s left`.
- **Drive to drive:** a source → destination strip with particles whose speed follows the real throughput.
- **Completion:** the ring fills, a check mark draws itself, the pill shrinks away; the new files fade in at the destination.
- **Delete to Recycle Bin:** items shrink and fade with a short slide; undo appears in the panel for 5 s.
- **Conflict card:** slides up with a spring; the chosen option highlights; "apply to all" collapses the remaining conflicts.
- **Error:** the row shakes once and turns red, with Retry / Skip.

**Budget:** animations are Direct2D only (no video, no extra libraries); ≤ 3 ms CPU per frame while animating; 0 % when idle.

Order: F comes right after A1 (drag and drop), because drag, copy, move and paste all route through the new transfer engine.

### D. Explorer-only menus and tools

| # | Feature | How | Size |
|---|---|---|---|
| D1 | **Full Windows right-click menu** ("Show more options": Open with, Send to, Compress to ZIP, 7-Zip, Git, antivirus scan, ...) | Shell `IContextMenu` hosted in our menu, with `IContextMenu3` message forwarding | M |
| D2 | **Properties** (Alt+Enter) | `SHObjectProperties` | S |
| D3 | **Open in Terminal / Open with…** | `wt.exe -d`, `SHOpenWithDialog` | S |
| D4 | **This PC** view: drives with free-space bars | A virtual "This PC" location | S |
| D5 | **Recycle Bin**: browse, restore, empty | Shell folder via `IShellFolder` enumeration | M |

### E. Pending items from v1/v2

| # | Item | Plan |
|---|---|---|
| E1 | Context menus are light in dark mode | Ask Windows for dark menus (uxtheme `SetPreferredAppMode`; undocumented but used by Notepad++ and others; falls back silently) |
| E2 | Rename only in icon/list view | Add F2 rename to column and gallery views |
| E3 | Applications misses per-user apps | Merge the user's Start-menu folder into the Applications listing |
| E4 | Tag/setting save errors are silent after rename/move/delete | Show them in the error banner |
| E5 | Custom tag names | "New Tag…" in the tag menu, with a name box |
| E6 | Type-to-select missing in column view | Add it |
| E7 | **Screen reader support** (UI Automation) | Separate project (see Non-goals) |

## Non-goals for v3 (and why)

- **Tabs.** A big change to the window model; worth doing once A–D are solid. Ctrl+N new windows cover most of the need.
- **Screen reader support (E7).** The custom-drawn views need a full UI Automation provider (items, selection, scrolling patterns). It deserves its own design and testing with Narrator. It is the most important gap after v3, not a nice-to-have.
- **Preview pane, Details pane.** Gallery view covers previews.
- **OneDrive status icons, network discovery, Libraries.** Virtual-shell features with little value for the user's workflow.

## Order of work

1. B1, B2, B4 (views and sort — immediately visible)
2. A1, A2, A5, A6, A7 (copy/move/organize)
3. D1, D2, D3 (Windows menus, properties, terminal)
4. C (settings window)
5. A3 (undo), A4 (pin favorites), B3 (columns), D4, D5
6. E1–E6
7. Rebuild, reinstall over Files 0.2.0, verify targets (idle ≤ 75 MB, peak ≤ 100 MB, 0 % idle CPU, ≤ 16 ms/frame)

Each item is test-first where it has logic, verified by screenshot where it is UI, committed on its own, and the installed copy is updated at the end.
