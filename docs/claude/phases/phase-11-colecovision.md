# Phase 11: ColecoVision

Part of the project's instructions for Claude Code; the core is `CLAUDE.md` at the repository root, the index of these files is its section "Further documentation".

| # | Phase | Est. | Gate |
| --- | --- | --- | --- |
| 11 | ColecoVision (`machine-coleco`; begun 2026-10-23): Z80, TMS9918A (`chips-ti`, 16 KB VRAM, sprites, all modes), SN76489, keypad controllers with joystick, cartridge ROMs; console panel POWER/RESET | 4 wk | BIOS boots; user-supplied cartridges run; TMS9918A mode and sprite tests against reference pictures |

## Status

User choices: build with an own stub BIOS first (the user adds the BIOS to `~/.mimic/roms/coleco/` later), NTSC and PAL, carts up to 32 KB plus MegaCart, all four controller kinds (standard, Super Action, Driving Module, Roller Controller). Done: `chips-ti` (TMS9918A/TMS9929A: all four modes, sprites with the four-per-line limit, 5S and coincidence, status/IRQ, read-ahead buffer; SN76489: tones, noise with the 15-bit register, attenuation), `devices-input::coleco` (`PortKind::ColecoControl`; segments, keypad codes, quadrature axes played out one step per 1/400 s), `devices-cartridge::coleco` (plain, MegaCart), `machine-coleco` (memory/I/O map, VDP → NMI, spinner → INT, panel POWER/RESET, keypads on the host keyboard, debugger with a VRAM space, savestates), stub BIOS, `tests/tests/coleco_boot.rs`. The real BIOS and a user cartridge run (Jumpman Junior, 16 KB, 2026-11-03: title, music, keypad, joystick, play). Open: more user cartridges (`EMU_COLECO_CARTS`); TMS9918A reference pictures from test programs; unverified details (keypad code table, the spinner interrupt wiring and which bits carry the phases, the Driving Module's pedal line, the MegaCart scheme, the PAL CPU clock); VRAM access timing (the minimum time between CPU accesses is not enforced); the 4/16K bit; the VDP draws a line from the registers as they are at its start.

## Backlog detail

- **ColecoVision** (Phase 11): reuses `cpu-z80`; TMS9918A video (16 KB VRAM, sprites), SN76489 sound, keypad controllers with joystick; BIOS user-supplied; console panel: POWER, RESET.
