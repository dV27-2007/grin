# Decisions

## D001 — Four-crate critical-path workspace

`terminal-core`, `terminal-pty`, `terminal-renderer`, and `terminal-app` enforce one-way responsibilities at compile time. Optional modules will be app-level dependents, never core dependencies.

## D002 — Winit and wgpu for the native surface

Use `winit` for a small cross-platform event-loop boundary and `wgpu` for portable GPU control. The app uses wait-mode dispatch and change-triggered redraws. Neither choice leaks into terminal state or PTY code, so each remains replaceable.

## D003 — Portable PTY behind a local abstraction

Use `portable-pty` for the M0 shell lifecycle across macOS/Linux/Windows. Only `terminal-pty` sees it. Blocking operations run on named OS threads with bounded queues; no general async runtime is required.

## D004 — Proven streaming VT parser

Use `vte` for byte-sequence parsing while keeping all state semantics local to `terminal-core`. This avoids writing a fragile byte-state machine and permits incremental terminal behavior.

## D005 — Scalable cached glyph rendering on a fixed terminal grid

Use `glyphon`/`cosmic-text` with a persistent system-font database, Swash cache, and GPU atlas. Font metrics determine the primary grid, but terminal cells determine placement: text shaping never wraps a terminal row, non-ASCII graphemes are independently anchored, and continuation cells never emit glyphs. On macOS the legacy `GB18030 Bitmap` face is excluded because it reports non-finite scalable advances.

## D006 — Row-local render preparation

Cache persistent shaping buffers per visible row and invalidate them with compact row signatures. Changed rows alone are rebuilt and reshaped; trailing blank glyphs and default-background rectangles are omitted. GPU/text resources remain persistent, and unchanged redraws bypass preparation.

## D007 — Per-window app runtime with movable tab sessions

The app owns native windows in a `HashMap<WindowId, WindowSession>` and routes each `WindowEvent` through the matching runtime. A detached tab moves its existing `TabSession` into a newly initialized window, preserving its terminal models and live PTY handles; process, parser, and renderer crates remain unaware of window ownership.

## D008 — Versioned multi-window workspace snapshots

Workspace version 2 stores a list of per-window sizes and tab trees under one global theme. Version 1 is decoded explicitly and migrated to a single `WindowState`; unknown or corrupt versions still fall back safely. Snapshots are written by one coalescing background worker with at most one pending value, keeping filesystem I/O off input/render paths. Window position remains deferred rather than relying on platform-specific placement hacks.

## D009 — Event-driven interaction animation

Hover and drag state invalidate only renderer interaction geometry. The short tab-drop animation chains redraw requests for at most 140 ms, then clears its timestamp and returns to the application's normal `ControlFlow::Wait` idle behavior; there is no timer or permanent frame loop.
# Command palette dispatch

Keyboard shortcuts and the Command Palette are frontends over the same central `Action` dispatch path. The palette owns only per-window query/selection state; it does not own terminal or PTY behavior.

The macOS native menu uses `muda` and forwards its callbacks through the winit user-event proxy, then activates the tracked focused window before dispatching the same `Action`.
