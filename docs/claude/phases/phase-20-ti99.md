# Phase 20: TI-99/4A

Part of the project's instructions for Claude Code; the core is `CLAUDE.md` at the repository root, the index of these files is its section "Further documentation".

| # | Phase | Est. | Gate |
| --- | --- | --- | --- |
| 20 | Texas Instruments TI-99/4A (`machine-ti99`, scheduled 2026-11-01): new `cpu-tms9900`, TMS9918A, TMS9919 sound, TMS9901, GROM with the GPL interpreter's data, cartridges, 32 KB expansion with wait states, cassette; Peripheral Expansion Box with disks and the speech synthesizer later; ROMs and cartridges user-supplied | 8 wk | TI BASIC starts with real ROMs; TMS9900 instruction tests green; a user-supplied cartridge runs |

## Backlog detail

- **Texas Instruments TI-99/4A** (added 2026-10-27 on the user's request, Phase 20): TMS9900 16-bit CPU at 3 MHz (new `cpu-tms9900`: workspace registers in RAM, context switching, CRU bit-serial I/O), TMS9918A/9929A video (reuse `chips-ti`; the video RAM is the only large RAM, the CPU sees 256 bytes of scratchpad plus the 16-bit console ROM), TMS9919/SN94624 sound (reuse the SN76489 of `chips-ti`), TMS9901 (interrupts, keyboard, joysticks, cassette), GROM (the Graphics ROMs with their auto-incrementing address and the GPL interpreter), cartridges (GROM + ROM, the 8 KB banking of later carts), 32 KB expansion RAM with its wait states, cassette, Peripheral Expansion Box with the TI disk controller (DSK images) and the Speech Synthesizer later; console ROM, GROMs and cartridge images user-supplied.
