# Phase 10: ZX81 and TRS-80

Part of the project's instructions for Claude Code; the core is `CLAUDE.md` at the repository root, the index of these files is its section "Further documentation".

| # | Phase | Est. | Gate |
| --- | --- | --- | --- |
| 10 | Z80 home computers: Sinclair ZX81 (`machine-zx81`: CPU-generated display, ULA line counting, 1/16 KB RAM, tape via ear/mic, .P files) and Tandy TRS-80 Model I/III (`machine-trs80`: text screen with block graphics, keyboard matrix, cassette, Expansion Interface with FDC 1771/1793 and DMK/JV1/JV3 images) | 4 wk | both boot to BASIC with real ROMs; a ZX81 .P program and a TRS-80 disk load |

## Status

The ZX81 passes its gate (`tests/tests/zx81_boot.rs`). `machine-trs80` is not begun: user choices so far are Model I + III with the Expansion Interface FDC (1771/1793, JV1/JV3/DMK read and write), an own fallback charset and an optional character ROM; it waits for ROMs in `roms/trs80/`.

## Backlog detail

- **Sinclair ZX81** (Phase 10): reuses `cpu-z80`; the CPU itself generates the display (NMI/HALT-driven, ULA counts lines), 1 KB/16 KB RAM, tape via ear/mic; ROM user-supplied.
- **Tandy TRS-80 Model I/III** (Phase 10): reuses `cpu-z80`; memory-mapped text screen with block graphics, keyboard matrix in the address space, cassette, Expansion Interface / FDC 1771/1793 floppies (DMK/JV1/JV3 images); ROMs user-supplied.
