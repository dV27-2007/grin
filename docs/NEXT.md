# Next Tasks

1. Replace full-width scrollback rows with a measured sparse/trimmed representation while preserving 10,000-line selection and search semantics.
2. Reduce cold GPU/font initialization toward the 100 ms first-frame target without giving up system fallback coverage.
3. Add captured VT conformance fixtures for resize edge cases, origin mode, wide-cell edits, and additional real-world TUI sequences.
4. Decide and implement wrapped-line reflow semantics for horizontal resize.
5. Add next/previous search navigation and harden selections when old scrollback is evicted.
6. Runtime-test the terminal path on Linux and Windows and isolate any platform-specific input/font differences.

Prompt 1's production terminal core is complete. Do not start tabs, panes, widgets, plugins, or external integrations unless the next prompt explicitly requests them.
