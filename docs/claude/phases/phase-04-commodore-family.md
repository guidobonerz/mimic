# Phase 4: Commodore family

Part of the project's instructions for Claude Code; the core is `CLAUDE.md` at the repository root, the index of these files is its section "Further documentation".

| # | Phase | Est. | Gate |
| --- | --- | --- | --- |
| 4 | Commodore family: VIC-20, PET, C16/Plus/4 (TED), C128 (8502+Z80, VDC, MMU), 1571/1581, IEC bus | 12 wk | All boot to BASIC |

## Result

Phase 4 done 2026-10-21: VIC-20, PET, C16/Plus/4 and C128 boot to BASIC with real ROMs; the items under `docs/claude/phases/phase-04-commodore-family.md` are held back.

## Deferred from Phase 4

Held back on 2026-10-21 so Phase 5 can start; pick them up in a later phase. They are not part of the Phase 4 gate.

- **Freezer cartridges (C64/C128):** Action Replay, Atomic/Nordic Power, Retro Replay and Nordic Replay exist on the C64 since 2026-11-01 (`devices-cartridge::freezer`, without flash writes; the freeze button is a method, simplified to NMI + Ultimax). Open: Final Cartridge III, Super Snapshot, the C128's bus hooks, the button from desktop UI, hotkey and the debug command `press_cartridge_button`; tests for freeze/restart round trips.
- **Cheat-file import:** POKE lists and VICE monitor commands into `cheats/<machine>/<game-hash>.toml`.
- **True-emulated 2031** (IEEE-488 drive with its own CPU, `dos2031` ROM); the PET has the virtual IEEE drive only.
- **Input replay files** in `tests/replays/` for end-to-end game tests (need game images, user-supplied).
- **Preview:** marking the current raster line in partial frames.
- **VICE adapter:** D64 autostart through the VICE monitor protocol.
- **Signal stage on the GPU:** composite/S-Video decoding runs on the CPU (`video_crt::signal`) in every frontend; a WGSL version is optional.

When finishing a phase, update "Current phase" above.

## Device, freezer and CRT work in this phase

- Device work by phase (part of each gate): **Phase 4:** 1351 mouse, paddles, virtual drive mode, host-directory backend, 1571/1581, datasette.
- Memory freezer by phase: **Phase 4 (deferred, see `docs/claude/phases/phase-04-commodore-family.md`):** C64/C128 freezer cartridges (Action Replay, Retro Replay, Final Cartridge III), cheat-file import.
- CRT work by phase: **Phase 4:** PAL delay-line + NTSC composite signal stage (C64/VIC-20/Plus/4), C128 VDC on RGB profile, PET monochrome.
