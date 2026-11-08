# Phase 23: Atari ST family

Part of the project's instructions for Claude Code; the core is `CLAUDE.md` at the repository root, the index of these files is its section "Further documentation".

| # | Phase | Est. | Gate |
| --- | --- | --- | --- |
| 23 | Atari ST family (`machine-atarist`, scheduled 2026-11-01): 520ST/1040ST, STF/STFM, Mega ST, STE as models; GLUE/MMU/Shifter, MFP 68901, YM2149, STE DMA sound, ACIA with IKBD, WD1772 with ST/MSA/STX, ACSI, MIDI; then TT030 and Falcon030 with a 68030 `cpu-m68k` model; TOS user-supplied, EmuTOS optional | 20 wk | TOS and EmuTOS boot to the desktop on ST and STE; a user-supplied disk runs; TT and Falcon as a gate of their own after the 68030 |

## Backlog detail

- **Atari ST/TT series** (added 2026-10-24 on the user's request, TT and Falcon added 2026-10-27, Phase 23): 520ST/1040ST, STF/STFM, Mega ST, STE (and Mega STE later) as models of one machine; 68000 (reuse `cpu-m68k`), GLUE/MMU/Shifter video (low/medium/high resolution, STE hardware scrolling and 4096-colour palette), MFP 68901, YM2149 (reuse `chips-amstrad` AY), STE DMA sound, ACIA 6850 with the IKBD (HD6301 keyboard/mouse/joystick controller), WD1772 FDC with ST/MSA/STX images, ACSI hard disks, MIDI; TOS ROMs user-supplied (EmuTOS as a free optional download). Later the **TT030** (68030 at 32 MHz — new `cpu-m68k` model with MMU and data cache — TT RAM, TT video modes with 256 colours and the 1280 × 960 monochrome mode, second MFP, SCSI 5380 (reuse the Mac's), SCC 8530 (reuse the Mac's), VME bus) and the **Falcon030** (68030 at 16 MHz, VIDEL video with true colour, DSP 56001, SDMA/CODEC sound, IDE, SCSI) as further models; TOS 3.x/4.x user-supplied.
