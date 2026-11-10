# Phase 22: Acorn BBC Micro

Part of the project's instructions for Claude Code; the core is `CLAUDE.md` at the repository root, the index of these files is its section "Further documentation".

| # | Phase | Est. | Gate |
| --- | --- | --- | --- |
| 22 | Acorn BBC Micro (`machine-bbc`, scheduled 2026-11-01): Model B, B+, Master 128 as models; Video ULA, 6845, SAA5050 teletext, 6522 VIAs, SN76489, 6850 ACIA, cassette, 8271 and 1770 FDCs with SSD/DSD/ADF, sideways ROM/RAM; Electron and Tube later; ROMs user-supplied | 10 wk | all three models boot to BASIC with real ROMs; a DFS disk boots; mode 7 checked against reference pictures |

## Backlog detail

- **Acorn BBC Micro** (added 2026-10-27 on the user's request, Phase 22): Model B, B+ and Master 128 (Electron later) as models of one machine; 6502 at 2 MHz (reuse `cpu-6502`; the Master's 65SC12 as a CMOS variant), Video ULA and 6845 CRTC (reuse `chips-amstrad`'s CRTC) for modes 0–7 including the SAA5050 teletext mode 7, 6522 VIAs (reuse `chips-cbm`) for the keyboard, sound, speech, user port and printer, SN76489 sound (reuse `chips-ti`), 6850 ACIA with cassette and RS-423, 8271 (Model B) and 1770/1772 FDCs with SSD/DSD/ADF images, Tube later, sideways ROMs/RAM and the Master's MOS 3.20 banking; MOS, BASIC and DFS/ADFS ROMs user-supplied.

## Status

Begun 2026-11-10 on the user's request. User choices: I look for a source of the ROMs and show it with its licence before anything is downloaded; Model B first (to BASIC and a DFS disk), then the B+ and the Master 128; both disk controllers, the 8271 (Acorn DFS on the Model B) and the 1770 (B+, Master, 1770 DFS boards); mode 7 is checked against the SAA5050 data sheet (its character set and the control code rules), with own test pages as the reference.

ROMs (the user owns them; his copies in `~/.mimic/roms/bbc`, never in the repository): MOS 1.20 `os12.rom`, BASIC II `basic2.rom`, Acorn DFS 1.20 (8271) `dfs-1.20.rom`, Acorn DFS 2.26 (1770) `dfs-2.26.rom`, the B+ MOS `bplus-mos.rom`, the Master 128 MOS 3.20 set in `master320/`.

Milestones:
Done 2026-11-10: M1 — the Model B boots MOS 1.20 to BASIC II with the user's ROMs ("BBC Computer 32K", "BASIC", the prompt), BASIC computes `PRINT 1+2`, `MODE 2` + `GCOL 0,129:CLG` fills the screen red, MODE 1 text is right (`tests/tests/bbc_boot.rs`, 3 tests; `machine-bbc` 7 tests with the stub MOS; `chips_acorn::VideoUla` 3 tests). The 6850 answers its status at rest until M3 (the MOS polls it in its interrupt handler). Open in M1: the cursor not yet seen on a picture (blink phase?), the hardware wrap-around sizes unchecked by a scrolling test.

Done 2026-11-10: M2 — `chips_acorn::Saa5050` after the Mullard data sheet (July 1982): the English character set of Fig. 11 read dot by dot from the scan (94 automatically, `7` and `?` by eye), the control codes of Table 1 with the teletext set-at/set-after rules, contiguous and separated mosaics (Fig. 9/10), blast-through, hold graphics, double height over two rows, flash 3:1 at ~0.75 Hz, conceal, character rounding to 12 × 20 half dots (7 unit tests). The machine routes teletext through it (the row's scan line from RA, in half lines when the 6845 interlaces). The boot screen shows "BBC Computer 32K", "BASIC" and the cursor; the mode 7 test page (`mode_7_test_page_matches_its_reference`: double height, colours, backgrounds, all mosaics, hold, blast-through, flash, the whole character set) is fixed by its picture hash after checking it by eye. Open in M2: the 6845's display and cursor skew (R8) not modelled, the separated-graphics gap side and the exact rounding half-dot placement taken as described, not compared with a real picture.

Done 2026-11-10: M3 — the 6850 ACIA (`machine-bbc/src/serial.rs`, after the Motorola data sheet: status, control, master reset, receive/transmit/DCD interrupts, overrun, the receiver held in reset while DCD is high) on the IRQ line with the VIAs; the serial ULA at `&FE10` (motor relay bit 7, RS423/cassette bit 6, rates); the cassette recorder as media slot "cassette" (`machine_bbc::SLOT_CASSETTE`, `.uef` plain or gzip and `.wav` through the new `media::uef`; keys play/record/stop/rewind, insert presses PLAY, recording rewrites the tape from where it began and goes back to the medium as an uncompressed UEF on stop, rewind, flush or eject; an empty file is a blank tape). The run-in detector raises DCD after 24 bit times of unbroken carrier and drops it at the first start bit (the hardware guide names the detector but not its time; 24 is this project's choice). `basic_saves_and_loads_on_tape`: `SAVE "T"` on a blank tape (RECORD then RETURN, block counter `T 00 000D`), the UEF written back has leader, `&2A` and the name, `LOAD "T"` and `LIST` show the line again. Sound: `basic_sound_plays_a_tone` (`SOUND 1,-15,100,20` loud for one second, quiet after). The user VIA is there (port lines unconnected). Open in M3: the tape is fed as whole bytes (no tone-level modelling of the serial ULA, so copy-protection loaders that measure tones and the 300-baud bit-3 tone swap are not covered); RS423 has no device behind it; the WAV decoder is checked with synthetic tones only, no real recording yet.

- M1 Model B to BASIC: 6502 at 2 MHz with the 1 MHz bus stretch, memory map and sideways ROM banking (ROMSEL), the 6845 (reuse `chips-amstrad`) and the Video ULA (modes 0–6), the system VIA (keyboard matrix, addressable latch, vsync interrupt), a stub ROM for tests.
- M2 Mode 7: the SAA5050 after its data sheet (character set, graphics, hold graphics, double height, flash, character rounding), own test pages as references.
- M3 Sound and cassette: SN76489 through the slow data bus (reuse `chips-ti`), the 6850 ACIA with the serial ULA and the cassette (WAV/UEF), the user VIA.
- M4 Disks on the Model B: the 8271 with DFS 1.20, SSD/DSD images; a DFS disk boots (`SHIFT+BREAK`).
- M5 B+: 64 KB with shadow RAM, the 1770 (reuse `chips-acorn`) with DFS 2.26.
- M6 Master 128: 65SC12 (`cpu-6502` CMOS variant), MOS 3.20 banking (ACCCON, ROMSEL, private and shadow RAM), CMOS RAM and clock, ADFS.
- M7 Frontends, debugger targets, graphics views, docs, screenshot. Gate.
