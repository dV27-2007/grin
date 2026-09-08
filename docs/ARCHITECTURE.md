# Architecture

Grin's critical path is deliberately small and one-way:

```text
native input ───────────────> bounded PTY command queue ──> shell
shell output ─> PTY reader ─> bounded event queue ────────> VT parser/grid
                                                              │
                                                              └─> redraw request ─> GPU
```

The native event loop uses wait mode. A PTY producer coalesces wake-ups; the app drains a bounded batch of queued output, mutates terminal state, and requests one redraw when the state generation changes. Remaining output schedules another event-loop turn so sustained output cannot monopolize input indefinitely. A frame is never scheduled merely because time passed.

## Crates

- `terminal-core` is deterministic, platform-independent terminal state. It owns cells, attributes, cursor, resize, bounded scrollback, and the `vte` parser integration.
- `terminal-pty` owns `portable-pty` and all blocking operations. Reader, controller, and child-wait threads communicate through bounded channels. The caller supplies a wake callback, keeping this crate UI-agnostic.
- `terminal-renderer` owns `wgpu`, `glyphon`, the surface, sparse rectangle pipeline, persistent font/glyph caches, and row-local shaping buffers. Terminal cell coordinates remain authoritative across fallback fonts.
- `terminal-app` owns `winit`, creates components, maps input and native clipboard/selection/search actions, propagates resize, and handles process/render failures.

Optional features must be outside this graph's reverse dependency path. They communicate with app-level coordination asynchronously and may not intercept or gate the terminal pipeline.

## Prompt 1 constraints

The terminal path includes common shell/TUI VT behavior, alternate screen, Unicode width/combining cells, scalable fallback rendering, selection/clipboard, search, and bounded scrollback. It deliberately excludes configuration, tabs, panes, widgets, plugins, and external integrations. Horizontal reflow, complete VT conformance, and mouse-reporting protocols remain future terminal-core work.
