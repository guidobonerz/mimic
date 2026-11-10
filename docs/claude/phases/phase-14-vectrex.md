# Phase 14: Vectrex

Part of the project's instructions for Claude Code; the core is `CLAUDE.md` at the repository root, the index of these files is its section "Further documentation".

| # | Phase | Est. | Gate |
| --- | --- | --- | --- |
| 14 | Vectrex (`machine-vectrex`): new `cpu-6809`, VIA 6522 (reuse `chips-cbm`), AY-3-8912 (reuse `chips-amstrad`), vector display with a vector frame type next to `FrameRef` (beam segments with intensity; phosphor decay in `video-crt`; debugger/preview carry the segments and a rasterised picture — core trait extension, ask first), cartridges, controllers | 6 wk | built-in Mine Storm plays; 6809 instruction tests green |

## Status

User choices: core extension `Machine::vectors()` (beam segments with intensity, default none) next to a rasterised `FrameRef`, so debugger, preview and tests stay unchanged; the system ROM user-supplied in `~/.mimic/roms/vectrex/` plus an own stub; two controllers, overlays, light pen, 3D Imager; the 6809 with own per-instruction tests (no Harte set exists). Done: `cpu-6809`, `emu_core::vector`, `machine-vectrex`, `devices-input::vectrex` (controller, light pen), `devices-cartridge::vectrex` (PB6 banking), VIA shift-out modes 5/6. The 4 KB executive and Scramble run; Gravitrex (John Dondzila, 2002, 32 KB) reaches its title and, with button 4, the planet screen (2026-11-03, headless; sound and play not checked). The built-in Mine Storm plays with the 8 KB system ROM (the user's `Mine Storm (1982).vec`, in `~/.mimic/roms/vectrex/` since 2026-11-04, picked over the 4 KB executive): GCE title, PLAYER 1, mine field 1; button 4 fires and the joystick turns the ship (`mine_storm_plays`, `EMU_ROM_DIR`). Its loop is not locked to the 30 000-cycle picture window, so some windows hold no ship (segment counts vary 150–320 a window): the real game's rate also drops with the number of objects, not compared. Open: overlays, phosphor persistence in `video-crt`, beam segments in the debug protocol and the preview.

## Backlog detail

- **Vectrex** (Phase 14): 6809 CPU (new `cpu-6809`), VIA 6522 (reuse from `chips-cbm`), AY-3-8912 (reuse from `chips-amstrad`), and a vector display: needs a vector frame type next to `FrameRef` (beam lines with intensity, phosphor decay in `video-crt`) — the debugger/preview contract then carries vectors or a rasterized picture; overlays; console panel: POWER, RESET.
