# Phase 37: Sega Mega Drive / Genesis

Part of the project's instructions for Claude Code; the core is `CLAUDE.md` at the repository root, the index of these files is its section "Further documentation".

| # | Phase | Est. | Gate |
| --- | --- | --- | --- |
| 37 | Sega Mega Drive / Genesis (`machine-megadrive`, added 2026-11-19 on the user's request, scheduled, not begun): 68000 at 7.67 MHz (reuse `cpu-m68k`) with the Z80 as sound CPU (reuse `cpu-z80`, bus request and reset from the 68000), the VDP 315-5313 (two scroll planes and the window, 80 sprites with link list, H/V interrupts, DMA from 68000 memory, VRAM fill and copy, shadow/highlight, H32/H40 and interlace) in `chips-sega`, YM2612 FM and the SN76489 (reuse `chips-ti`), TMSS, PAL/NTSC and region, 3- and 6-button pads, SRAM and EEPROM saves, the SSF2 mapper; Mega-CD and 32X later as their own steps | 14 wk | commercial games boot and play with sound on PAL and NTSC; a VDP test ROM's checks pass |
