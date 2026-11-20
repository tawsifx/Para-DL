# paradl-ui

Progress display and opt-in ratatui TUI for paraDL. **Consumes** typed values
from [`paradl-events`](../paradl-events) and renders them as plain text or an
interactive terminal screen; it never emits JSONL (the single-emitter rule).

## Role

| Surface | Feature | TTY required |
|---|---|---|
| `ProgressDisplay` (simple line + full renderable) | always | no |
| `event_summary` | always | no |
| `format_size` / `format_speed` | always | no |
| `tui::TaskState` / `tui::TaskStore` / `tui::apply_event` | `tui` | no |
| `tui::TuiApp` / `tui::render_app` / `tui::run_tui` | `tui` | only `run_tui` |

There is no extension trait in this crate: consumers call the render functions
directly with a `paradl_events::Event` (or a `ProgressEvent`). All fallible
entry points return `paradl_core::Error` / `paradl_core::ExitCode`.

## Feature flags

- `tui` (default: **off**) — compiles `ratatui` and the interactive app.
  With the feature off the crate has no TTY or system-tool dependency.

## Examples

```sh
# Plain progress display (default build)
cargo run -p paradl-ui --example plain_progress

# Render a TUI frame into an in-memory buffer (no TTY needed)
cargo run -p paradl-ui --features tui --example tui_preview
```

## Golden parity

`tests/golden.rs` and `tests/golden_tui.rs` assert byte-exact strings captured
from the READ-ONLY Python oracles `src/paradl/ui/display.py` and
`src/paradl/ui/tui.py`: `format_size`, `format_speed`, `_format_simple_line`,
`_build_renderable`, `_TaskState.progress_pct/progress_bar/status_text/to_row`.
`tests/tui_render.rs` verifies the ratatui layout into a `TestBackend` buffer.
