# Phase 6b: Atari 2600

Part of the project's instructions for Claude Code; the core is `CLAUDE.md` at the repository root, the index of these files is its section "Further documentation".

| # | Phase | Est. | Gate |
| --- | --- | --- | --- |
| 6b | Atari 2600 (NTSC, PAL): 6507, TIA (video, sound, inputs), RIOT 6532, cartridge bank switching (2K/4K, F8/F6/F4 + Superchip, FE, E0, E7, 3F, FA), joystick, paddles, keypad, driving controller, console switches | 6 wk | own TIA/RIOT test programs green; user-supplied games run correctly |

## Result

Phase 6b done 2026-10-21: Atari 2600 NTSC/PAL with TIA, RIOT, all planned bank-switching schemes and controllers; 24 games of a ROM collection run with correct scheme detection; DPC (Pitfall II) added 2026-10-25 (picture and three-voice music; `EMU_PITFALL2=path` test), Supercharger and SECAM later.

## Backlog detail

- **Atari 2600 (VCS)**: now Phase 6b (see the phase table): `machine-atari2600` with the 6507 (cpu-6502 variant), TIA and RIOT 6532 in `chips-atari`, bank-switching schemes in `devices-cartridge::atari2600`. DPC (Pitfall II) done 2026-10-25 (`devices-cartridge::atari2600::dpc`; music oscillator taken as the documented ~20 kHz); the `auto` model (default) picks NTSC or PAL from the program's field lengths. Later: Starpath Supercharger, SECAM.
