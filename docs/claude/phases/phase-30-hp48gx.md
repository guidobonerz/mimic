# Phase 30: HP 48GX

Part of the project's instructions for Claude Code; the core is `CLAUDE.md` at the repository root, the index of these files is its section "Further documentation".

| # | Phase | Est. | Gate |
| --- | --- | --- | --- |
| 30 | HP 48GX (`machine-hp48`, added 2026-11-05 on the user's request): Saturn at 4 MHz (reuse `cpu-saturn`) in the Yorke chip (memory controller with configurable modules, timers, display controller for the 131 × 64 LCD, keyboard, serial port and IR), 128 KB RAM plus two card slots (RAM cards as media), Kermit/XModem over the serial port to a host folder (through `emu-core::host`); 48SX and 48G as later models; ROM user-supplied | 10 wk | the 48GX boots to its stack with a real ROM (revision R or M); keys, menus and a program run; a RAM card in slot 1 keeps its data |

## Backlog detail

- **HP calculators** (added 2026-11-05 on the user's request, Phases 28–30): HP 22S and HP 42S (Pioneer series, one machine with two models) and HP 48GX (Saturn in the Yorke chip). All need a new `cpu-saturn` (4-bit nibble-addressed CPU with 64-bit registers); the picture is the calculator's face with its LCD, keys are clicked on it or typed on the host keyboard (like the IMSAI's front panel and the KIM-1's keypad). ROMs user-supplied (the 48GX ROM is distributed by HP-licensed sources for emulators; check when the phase starts).
