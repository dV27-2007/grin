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
