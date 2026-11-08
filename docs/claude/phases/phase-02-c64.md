# Phase 2: C64

Part of the project's instructions for Claude Code; the core is `CLAUDE.md` at the repository root, the index of these files is its section "Further documentation".

| # | Phase | Est. | Gate |
| --- | --- | --- | --- |
| 2 | C64 boots: VIC-II, CIA, SID, keyboard, PRG/D64/T64, simplified 1541 | 10 wk | Lorenz suite ≥ 90 % |

## Result

Phase 2 done 2026-10-20: Lorenz 264/264 with ROMs, PAL + NTSC C64.

## Device, freezer and CRT work in this phase

- Device work by phase (part of each gate): **Phase 2:** C64 control ports with digital joystick, keyboard matrix + keymaps, `emu-input` with gamepad and keyboard-as-joystick, 1541 slot with image + copy-on-write backend.
- CRT work by phase: **Phase 2:** `video-crt` skeleton: aspect correction, integer scaling, simple scanline shader; C64 1702/1084S composite profiles without signal stage.
