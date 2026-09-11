# Next Tasks

1. Design and implement the Command Palette at the app/terminal boundary.
2. Add Smart History without coupling terminal-core or terminal-pty to optional product systems.
3. Add event-driven autocomplete that never blocks terminal input or PTY processing.
4. Replace full-width scrollback rows with a measured sparse/trimmed representation while preserving selection and search semantics.
5. Reduce cold GPU/font initialization toward the 100 ms first-frame target.
6. Runtime-test the terminal path on Linux and Windows.

Stage-1 tabs, panes, detach, and multi-window persistence are complete. Do not redesign them while starting the next product feature.
# Next

- Build later Stage-2 features on the action registry and command-palette infrastructure without coupling them to terminal input or rendering.
