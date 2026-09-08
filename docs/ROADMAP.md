# Roadmap

- **Stage 0 — vertical slice:** native window, GPU cells, shell PTY round-trip, basic VT state, resize, cursor, idle event-driven behavior.
- **Stage 1 — terminal correctness:** Unicode width/shaping, robust VT modes, alternate screen, scroll regions, selection/clipboard, scrollback viewport, and terminal-focused performance measurements.
- **Stage 2 — workspace ergonomics:** tabs, panes, sessions, searchable history, command palette, and minimal configuration/themes.
- **Stage 3 — extensibility:** isolated plugin SDK and composable widgets, with resource budgets and crash containment.
- **Stage 4 — integrations:** opt-in Git, processes/ports, Docker, SSH, Kubernetes, project tasks, and structured viewers. None may enter the critical terminal path.

Advance stages only by explicit product decision; correctness and measured performance precede breadth.

