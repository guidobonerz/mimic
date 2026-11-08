# Phase 21: Sinclair QL

Part of the project's instructions for Claude Code; the core is `CLAUDE.md` at the repository root, the index of these files is its section "Further documentation".

| # | Phase | Est. | Gate |
| --- | --- | --- | --- |
| 21 | Sinclair QL (`machine-ql`, scheduled 2026-11-01): 68008 as a `cpu-m68k` variant, ZX8301, ZX8302, 8049 IPC, Microdrives with MDV images, RS-232; expansions later; QDOS or Minerva ROM | 8 wk | QDOS and Minerva boot to SuperBASIC; an MDV cartridge loads |

## Backlog detail

- **Sinclair QL** (added 2026-10-27 on the user's request, Phase 21): 68008 (8-bit bus variant of `cpu-m68k`, 4-clock bus cycles on a byte bus), ZX8301 (video with modes 4 and 8, 512 × 256 and 256 × 256, display DMA from RAM, master clock) and ZX8302 (RTC, serial, Microdrive and network interface, IPC link), the 8049 IPC (keyboard, sound, joysticks; its program as a user-supplied ROM or a functional model), two internal Microdrives (reuse `drives-other::microdrive` with the QL's sector format and MDV images), RS-232, Trump Card/Gold Card and floppy expansions later; QDOS / Minerva ROMs user-supplied (Minerva is free).
