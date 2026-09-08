# Current State

## Works

- Rust workspace with isolated core, PTY, renderer, and app crates.
- Native `winit` window and Retina-aware `wgpu`/`glyphon` text pipeline with measured Menlo metrics, system fallback, and a persistent glyph atlas.
- Default system shell launched in a `portable-pty` session.
- PTY output parsed incrementally into a resizable grid with 10,000 lines of bounded scrollback.
- Unicode grapheme cells, combining marks, wide/continuation cells, 16/256/truecolor SGR, styles, cursor shapes, scrolling regions, insert/delete/erase operations, and primary/alternate screens.
- Terminal-cell coordinates are authoritative: non-ASCII fallback segments are anchored at their owning columns and text-engine wrapping is disabled. The incompatible macOS `GB18030 Bitmap` face is excluded in favor of scalable CJK fallback.
- Text/special/control keyboard input sent to the shell.
- Mouse selection, native copy/paste, bracketed paste, scrollback wheel/page navigation, latest-match search, OSC titles, OSC 8 hyperlinks, and graceful shell-exit state.
- Window size mapped to terminal rows/columns and propagated to the PTY.
- Event loop waits while idle; PTY wake-ups are coalesced and redraw occurs on state change.
- Renderer caches persistent per-row shaping buffers by row signature, trims blank tails, emits sparse background/cursor geometry, and skips preparation on unchanged redraws. Font discovery, atlas/cache, text renderer, GPU buffers, and staging vectors persist.
- Debug builds print startup diagnostics; release builds expose opt-in `GRIN_PROFILE_STARTUP`, `GRIN_PROFILE_INPUT`, and `GRIN_PROFILE_RENDER` diagnostics.
- Unit tests cover core parser/grid/scroll/resize/color/Unicode/alternate-screen logic, finite fallback layout, renderer sizing/color conversion, input mapping, and a real shell/PTY round trip.
- Native release smoke-tested on macOS 26.6.2 with normal/fast input, the mixed Unicode fixture, maximized resize, Vim alternate screen, 20,000 output lines, scrollback, selection/copy/paste, search, and shell exit.

## Release measurements (macOS 26.6.2)

- Fresh idle: 0.0% CPU and 95,776 KiB RSS (about 93.5 MiB).
- Instrumented startup: window 98 ms, GPU/font ready 134 ms, PTY ready 137 ms, first submitted frame 146 ms, first shell output 361 ms. A separate Retina-scale run submitted its first frame at 323 ms.
- Warm changed-row preparation: typically 0.2-0.7 ms. Input event through echoed frame submission: 0.4-8.3 ms; release typing was visually immediate.
- `yes test | head -n 10000`: shell reported 0.020 s; command-to-CUA capture was 565 ms. `seq 1 10000`: 0.014 s and 467 ms respectively. Input remained responsive immediately afterward.
- Filling the 10,000-line scrollback raised RSS to 171,152 KiB. `vmmap` attributed 79.6 MiB allocated/84.0 MiB resident to the small-object heap (full-width `Cell` rows) and 26.9 MiB to `IOSurface`; this is documented rather than hidden by reducing history or using unsafe compression.

## Crate map

- `crates/terminal-core`: pure terminal model and VT performer.
- `crates/terminal-pty`: PTY child plus bounded background I/O.
- `crates/terminal-renderer`: GPU surface, sparse rectangle pipeline, scalable glyph shaping/raster cache, and fixed-grid row-segment cache.
- `crates/terminal-app`: native event loop and orchestration (`grin` binary).

## Known limitations

- Horizontal resize truncates/extends rows rather than reflowing wrapped history.
- Bidirectional and joining-script shaping does not span separate terminal cells; fallback glyph selection/rasterization is supported, but the fixed terminal grid remains authoritative.
- Search exposes the most recent match only; mouse-reporting protocols and focus reporting are not implemented.
- There is no persisted font/theme configuration, tab, pane, widget, or plugin system.
- Windows/Linux are architectural targets but have not been runtime-tested in this repository.
- Fresh idle meets the memory target, but a saturated 10,000-line full-cell scrollback exceeds 100 MiB RSS. Cold first-frame time also remains above the 100 ms target.
