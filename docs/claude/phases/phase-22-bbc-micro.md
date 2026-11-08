# Phase 22: Acorn BBC Micro

Part of the project's instructions for Claude Code; the core is `CLAUDE.md` at the repository root, the index of these files is its section "Further documentation".

| # | Phase | Est. | Gate |
| --- | --- | --- | --- |
| 22 | Acorn BBC Micro (`machine-bbc`, scheduled 2026-11-01): Model B, B+, Master 128 as models; Video ULA, 6845, SAA5050 teletext, 6522 VIAs, SN76489, 6850 ACIA, cassette, 8271 and 1770 FDCs with SSD/DSD/ADF, sideways ROM/RAM; Electron and Tube later; ROMs user-supplied | 10 wk | all three models boot to BASIC with real ROMs; a DFS disk boots; mode 7 checked against reference pictures |

## Backlog detail

- **Acorn BBC Micro** (added 2026-10-27 on the user's request, Phase 22): Model B, B+ and Master 128 (Electron later) as models of one machine; 6502 at 2 MHz (reuse `cpu-6502`; the Master's 65SC12 as a CMOS variant), Video ULA and 6845 CRTC (reuse `chips-amstrad`'s CRTC) for modes 0–7 including the SAA5050 teletext mode 7, 6522 VIAs (reuse `chips-cbm`) for the keyboard, sound, speech, user port and printer, SN76489 sound (reuse `chips-ti`), 6850 ACIA with cassette and RS-423, 8271 (Model B) and 1770/1772 FDCs with SSD/DSD/ADF images, Tube later, sideways ROMs/RAM and the Master's MOS 3.20 banking; MOS, BASIC and DFS/ADFS ROMs user-supplied.
