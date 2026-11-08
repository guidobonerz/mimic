# Phase 5: Atari 800XL

Part of the project's instructions for Claude Code; the core is `CLAUDE.md` at the repository root, the index of these files is its section "Further documentation".

| # | Phase | Est. | Gate |
| --- | --- | --- | --- |
| 5 | Atari 800XL: ANTIC, GTIA, POKEY, PIA, ATR/XEX/CAS — debugger unchanged | 8 wk | Acid800 mostly green |

## Result

Phase 5 done 2026-10-21: Acid800 52/57 on PAL and NTSC; 54/57 since 2026-10-25 (`antic_hiresbug`: players collide in vertical blank after a hires last line; `antic_hscrolbug`: playfield DMA follows ANTIC's DMA clock, Altirra HRM 4.12); 57/57 (1 skipped: no 65C816 board) on 800XL, 65XE and 130XE, PAL and NTSC, since 2026-11-02, also booted as the full suite from `acid800.atr` on the 800XL (Passed 57, Failed 0, Skipped 1); 800: 56/57 (`mmu_xlbanking` tests XL banking the machine does not have, correctly fails); 400: 56/56 (2 skipped): the machine keeps the data bus's last byte for ANTIC's fetches in cycles without a bus (`antic_virtdma`) and GTIA's player/missile latches, which load it a cycle after a slot ANTIC left free (`gtia_phantomdma`; only player 0's cycle is tested, a held CPU's repeated read does not reach the bus byte, vertical blank assumed free of it); a GTIA mode still on at the end of cycle 15 turns that line's hires pixels into PF0-PF3 (`gtia_psuedomodee`.

## Device, freezer and CRT work in this phase

- Device work by phase (part of each gate): **Phase 5/6:** Atari joystick/paddles/SIO drives; CPC dual joystick, AMX mouse, FDC drives.
- CRT work by phase: **Phase 5/6:** Atari NTSC artifact colors (gate includes an artifacting test image); CPC CTM644 and GT65 profiles.

## Backlog detail

- **Atari 8-bit models** (Phase 9; configuration of `machine-atari8`, Phase 5 ships the 800XL only): **400** (and 800: OS-A/B, four joystick ports, no PORTB banking, RESET as NMI), **65XE** (800XL hardware in XE case), **130XE** (128 KB, PORTB banking with separate CPU/ANTIC access).
