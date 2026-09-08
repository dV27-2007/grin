# Engineering Guide

## Product invariants

- Grin is a native, terminal-first application. The shell and PTY remain useful offline and without optional features.
- Keep terminal input, PTY processing, state updates, and rendering independent of Git, Docker, Kubernetes, widgets, plugins, project detection, animation, and network services.
- Optional systems may depend on the app/terminal boundary; terminal crates must never depend on optional systems.
- Terminal input and rendering must survive optional-module failure or disablement.
- Targets: cold start under 100 ms, idle CPU approximately 0%, idle RAM under 100 MB, imperceptible typing latency, and 120 Hz-capable rendering when active.
- The event loop sleeps while idle. Never introduce a continuous redraw, polling loop, or timer without a measured need.
- Use bounded queues. Never block the UI/event thread on PTY I/O or optional work. Do not hold locks across expensive work.

## Architecture boundaries

- `terminal-core`: cells, grid, cursor, VT parsing, scrollback, change generation. No window, GPU, or process dependencies.
- `terminal-pty`: shell/PTY lifecycle, blocking reader/controller/waiter threads, bounded event and command channels. No UI or renderer dependencies.
- `terminal-renderer`: GPU resources and terminal viewport drawing. Reads `terminal-core`; never owns shell logic.
- `terminal-app`: thin native event-loop orchestration, input encoding, resize propagation, and failure handling.
- Dependency direction: `terminal-app -> {terminal-pty, terminal-renderer, terminal-core}` and `terminal-renderer -> terminal-core`.

## Coding rules

- Stable Rust; format with `cargo fmt`.
- Prefer small modules, explicit ownership, data-oriented state, low allocation in hot paths, and predictable control flow.
- Avoid global mutable state, heavyweight async runtimes, speculative abstractions, and large traits.
- Do not add a dependency without a concrete current need. Record consequential choices in `docs/DECISIONS.md`.
- Runtime-critical paths should return useful errors; do not scatter `unwrap`/`expect`. An `expect` is acceptable only for an OS invariant with a clear message.
- Write meaningful tests for pure state/parser/resize logic. Do not test trivial accessors.
- Keep platform-specific behavior behind narrow boundaries and document support gaps in `docs/STATE.md`.
- Measure before micro-optimizing, but reject architectures that obviously busy-spin, block input, or create unbounded work.

## Commands

```sh
cargo fmt --all --check
cargo test -p terminal-core
cargo test -p terminal-renderer
cargo test -p terminal-pty
cargo check --workspace
cargo test --workspace
cargo run -p terminal-app
```

Run targeted checks first. Workspace-wide tests are appropriate before milestone handoff, not after every small edit.

## Codex operating contract

For every future task:

1. Read `AGENTS.md`.
2. Read `docs/STATE.md`.
3. Read `docs/NEXT.md` only if needed.
4. Inspect only files relevant to the requested change.
5. Use `rg` and targeted file inspection before opening large files.
6. Do not recursively read the repository without a concrete reason.
7. Never inspect `.git`, `target`, `node_modules`, vendor, or cache directories unless debugging them.
8. Do not repeat architecture explanations already documented.
9. Keep diffs narrow; do not refactor unrelated code.
10. Preserve user work and unrelated uncommitted changes.
11. Do not add dependencies without a concrete need.
12. Prefer targeted checks/tests before workspace-wide expensive checks.
13. Once appropriate tests pass, do not rerun them without a reason.
14. Update `docs/STATE.md` only when project state materially changes.
15. Update `docs/NEXT.md` when immediate priorities change.
16. Keep the final response concise: changed, verified, remaining.
17. Do not write long tutorials unless requested.

Optimize for useful implementation per token. Never enter Stage 1 or add optional product systems unless the user explicitly requests it.

