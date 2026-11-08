# Phase 18: EACA Colour Genie EG2000

Part of the project's instructions for Claude Code; the core is `CLAUDE.md` at the repository root, the index of these files is its section "Further documentation".

| # | Phase | Est. | Gate |
| --- | --- | --- | --- |
| 18 | EACA Colour Genie EG2000 (`machine-cgenie`, scheduled 2026-11-01): Z80, 6845 CRTC, AY-3-8910, text with a programmable character set, low-resolution graphics, keyboard, cassette, CAS images; floppy (CGD) and RS-232 later; ROMs user-supplied | 4 wk | boots to BASIC with real ROMs; a CAS program loads |

## Backlog detail

- **EACA Colour Genie EG2000** (added 2026-10-27 on the user's request, Phase 18): Z80 at 2.2 MHz (reuse `cpu-z80`), 6845 CRTC (reuse `chips-amstrad`'s CRTC) with 40 × 24 (and 40 × 25) text from a character generator plus a programmable 128-character set in RAM and 160 × 96 (… × 102) low-resolution graphics, 16 colours, AY-3-8910 sound (reuse `chips-amstrad`'s AY), 16/32 KB RAM, memory-mapped keyboard matrix, cassette (CAS images), light pen, joysticks, the floppy controller with the EG2000's DOS (CGD images) and the RS-232/printer cards later; BASIC ROM and the character generator user-supplied.
