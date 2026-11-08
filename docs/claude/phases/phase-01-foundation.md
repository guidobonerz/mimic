# Phase 1: Foundation

Part of the project's instructions for Claude Code; the core is `CLAUDE.md` at the repository root, the index of these files is its section "Further documentation".

| # | Phase | Est. | Gate |
| --- | --- | --- | --- |
| 1 | Foundation: workspace, CI, scheduler, 6502 core, bus, savestates, headless runner, minimal debug server (pause/step/regs) | 6 wk | 6502 tests green |

## Result

Phase 1 done 2026-10-20: 6502 Harte + Dormann green, CI green.

## Device, freezer and CRT work in this phase

- Device work by phase (part of each gate): **Phase 1:** `PortDevice`, `MediaSlot`, `MediaBackend`, `InputEvent` and the input queue defined in `emu-core` (even if only stubbed) — they shape the `Machine` trait.
- Memory freezer by phase: **Phase 1:** freeze bitmaps share the watchpoint mechanism in `DebugHooks`; `Freeze` in savestate format.
- CRT work by phase: **Phase 1:** `FrameRef` with full overscan + signal metadata (pixel clock, line timing, standard, output type, field parity).
