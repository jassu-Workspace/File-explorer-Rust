# Explorer Parity v3 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans. Steps are test-first where there is logic; UI steps are checked by screenshot.

**Goal:** Windows File Explorer's features with Finder visuals, and a custom animated transfer system that replaces Windows' copy, conflict and delete dialogs.

**Architecture:** Pure logic (settings resolution, view presets, transfer bookkeeping, conflict planning, undo stack, animation curves) lives in testable modules. Win32/COM glue (IFileOperation with a progress sink on a worker thread, OLE drag and drop, shell context menu, Recycle Bin) stays thin. All animation is Direct2D, driven by the existing animation timer.

**Tech Stack:** Rust 1.98.1, `windows` 0.62.2 (+ `windows-core` 0.62.2 for `#[implement]`), existing modules.

**Spec:** `docs/superpowers/specs/2026-11-18-explorer-parity-v3-design.md`

## Global Constraints

- Targets stay: idle ≤ 75 MB (10k folder, icon view), peak ≤ 100 MB, 0 % idle CPU, ≤ 16 ms CPU per animated frame.
- No Windows progress, conflict or delete-confirmation dialogs during transfers (`FOF_SILENT | FOF_NOCONFIRMATION`); our UI replaces all of them. Permanent delete (Shift+Del) still asks first, in our own card.
- Every animation honours "Show animations in Windows" (instant when off).
- Settings follow Windows unless overridden in Files' Settings; overrides persist in `prefs.tsv`.
- No admin rights; nothing runs in the background when the window is closed.
- Each task ends with `cargo test`, `cargo clippy --all-targets -- -D warnings`, release build, and a commit.

## Review Focus

1. A transfer cancelled midway leaves no half-written destination file (IFileOperation handles per-file atomicity; verify cancel test).
2. Conflict answers "Replace" never touch files the user chose to Skip.
3. Undo of a move puts files back where they were even if the window navigated away.
4. Drag and drop onto the same folder is a no-op, not a duplicate copy.
5. Single-click mode never opens a file on the first click of a drag or a drag-box.

## Tasks

| # | Task | Files | Tests |
|---|---|---|---|
| 1 | **Settings**: read Windows (`SHGetSettings`: double-click, extensions, hidden, system files), overrides in prefs, `Settings` resolver; single-click mode (hover-select, underline, click opens); extensions hidden when off; Settings sheet (Ctrl+,) drawn in-app with toggles and choices; panel style A/B/C; animation strength | `settings.rs` (new), `app.rs`, `views/settings.rs` (new) | resolver precedence; prefs round trip; single-click hit rules (drag threshold) |
| 2 | **View sizes and sorting**: View menu (Extra large 192, Large 128, Medium 64, Small 32 icons, Details, Columns, Gallery); Sort menu (Name, Date modified, Date created, Type, Size; ascending/descending) in every view and in right-click; Date created read with the listing; status line "N items · M selected · size" | `listing.rs`, `views/grid.rs`, `app.rs`, `views/pathbar.rs` | preset sizes; created-date sort; selection size sum |
| 3 | **Transfer engine**: `Transfer` jobs (copy, move, recycle, delete, new-folder unchanged) run on a worker STA thread with `IFileOperation` + `IFileOperationProgressSink`; progress posted to the UI (bytes, items, current name, speed/ETA moving average); cancel; per-item errors collected; conflict pre-scan → `ConflictPlan` (replace / skip / keep both, apply-to-all) → item lists and flags; paste, Copy to/Move to, delete and drop all route here | `transfer.rs` (new), `shell.rs`, `app.rs` | ETA/speed averaging; conflict planning; job state machine; real copy/move/cancel on temp dirs |
| 4 | **Transfer UI and animations**: panel styles A (pill + ring), B (card + shimmer bar), C (card + flipping stack); launch flight from selection to destination; drive-to-drive particle strip (cross-drive copies, click panel to toggle); completion check; conflict card with spring; error row shake with Retry/Skip; **Recycle Bin**: bin pops up bottom-right, lid opens, files arc in, lid closes, Undo pill 5 s | `views/transfer.rs` (new), `layout.rs` (curves), `app.rs` | bezier path points; spring curve; panel layout; particle speed mapping |
| 5 | **Drag and drop**: drag source (`SHDoDragDrop` with a shell data object) from icon/list/column views; drop target (`IDropTarget`) on content, folders, sidebar places and drives; Explorer rules (same drive = move, other drive = copy, Ctrl copy, Shift move, Alt shortcut); drop highlight; drops route to the transfer engine | `dragdrop.rs` (new), `app.rs` | effect rules; same-folder no-op |
| 6 | **Explorer tools**: Copy to… / Move to… (folder picker), Copy as path (Ctrl+Shift+C), Select none / Invert selection, New → Text document, Properties (Alt+Enter), Open with…, Open in Terminal, full Windows right-click menu ("Show more options") with submenus, New window (Ctrl+N), Close (Ctrl+W) | `shell.rs`, `app.rs` | path text formatting; invert selection |
| 7 | **Undo/redo** (Ctrl+Z / Ctrl+Y) for rename, move, copy, new folder, recycle (restore from Recycle Bin); **Pin to Favorites** (add, remove, persists); **This PC** (drives with usage bars); **Recycle Bin** location (browse, restore, empty) | `undo.rs` (new), `recycle.rs` (new), `views/sidebar.rs`, `app.rs` | undo stack inverse operations; favorites persistence |
| 8 | **Pending fixes**: dark popup menus; rename in column and gallery views; per-user apps merged into Applications; custom tag names; type-to-select in column view; save errors shown | various | apps merge; custom tag store round trip |
| 9 | **Ship**: rebuild, reinstall over Files 0.2.0 as 0.3.0, verify targets, final review | packaging | benchmark scripts |
