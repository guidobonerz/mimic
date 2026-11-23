# Phase 41: Lobo MAX-80

Part of the project's instructions for Claude Code; the core is `CLAUDE.md` at the repository root, the index of these files is its section "Further documentation".

| # | Phase | Est. | Gate |
| --- | --- | --- | --- |
| 41 | Lobo MAX-80 (`machine-max80`, added 2026-11-21 on the user's request and begun the same day: the machine built, Lobo's boot ROM boots LDOS 5.1.3 and CP/M 2.2 from 5.25-inch and 8-inch disks, typing works and programs run under both; the serial ports, the printer and the SASI hard disk open): Lobo Systems' Z80 computer of 1982 that runs both the TRS-80's LDOS and CP/M; Z80B at 5.07 MHz (reuse `cpu-z80`), 128 KB RAM in four banked chunks, one 4 KB block with the boot ROM, video and font RAM, keyboard and device registers that moves through the address space, text screen from an HD46505 CRTC with the font in RAM (80 × 24 and the TRS-80's 64 × 16), MB8876 floppy controller (the FD1793 of `drives-other` on an inverted bus) for 5.25-inch and 8-inch drives with ImageDisk (IMD), DMK and JV images, Z80 PIO with the MSM5832 clock chip, tick interrupt, beeper; two serial ports (Z80 SIO), parallel printer port and the SASI hard disk port as later steps; boot ROM and system disks user-supplied | 4 wk | the boot ROM starts LDOS and CP/M from the user's disk images, typing works, and a program loads and runs under each |

## Status

2026-11-21: added to the phase list on the user's request ("put the max80 in the phase list") and begun the same day ("beginne mit dem max80").

**The gate is met with test disks** (the user's own disks are still to come): Lobo's boot ROM starts LDOS 5.1.3 and Lobo's CP/M 2.2, typed commands load and run programs under both, and CP/M's PIP writes a disk that boots again. `tests/tests/max80_boot.rs`, 6 tests, all pass (first run, nothing earlier to compare with):

| Check | Result |
| --- | --- |
| Diagnostic disk (lobo_max80, MIT) boots from 5.25-inch MFM and 8-inch FM | menu, 128K found, the boot device named right |
| Its main RAM test | banking PASS, March U PASS on A1, A2, B1, B2, retention PASS |
| Its video timing (from the display enable against the tick) | 263.9 lines a frame, 59.97 Hz, 15.83 kHz |
| Its clock chip test | digits in range, seconds counting, 61 ticks a second |
| Its memory timing | video/font RAM 1.00 T, slow RAM 2.00 T an access (a real machine: 1.02 and 2.00) |
| `ram=64k` | the diagnostic finds 64K |
| LDOS 5.1.3 (5.25-inch, 80 tracks, two sides, DMK) | `LDOS Ready`, `DIR :0` and `FREE` run |
| Lobo's CP/M 2.2 (8-inch master, IMD) | boots, STAT runs, PIP copies a file, the written image boots and lists it |
| Savestate during the boot | continues identically |

Booting as well, tried by hand: the 8-inch LDOS 5.1 disk, the 5.25-inch CP/M 2.2 (the WordStar disk), xcpmz80 and the monitor disk of the lobo_max80 project.

What was built:

- **`machine-max80`**: the memory map (`lib.rs`: four 32 KB chunks banked by the PIO's port A, the 4 KB block VLATCH moves, with the write that also lands in the RAM below), the device registers (`io.rs`: MLATCH, VLATCH, SYSFLG, USTAT, DIP switches, CRTC registers, PIO with the MSM5832 clock, the tick, the beep, the floppy controller on its inverted bus), the picture (`video.rs`: font RAM, both character ranges, double width), the keyboard (`keyboard.rs`), wait states by place, savestates, four debugger memory spaces (the processor's view, RAM as fitted, video RAM, font RAM), a built-in stub program in place of the ROM (`stub.rs`, `roms=stub`) for tests and the registry.
- **ImageDisk images** in `media::dmk` (read and written back; 8-inch tracks at 500 kbit/s) and **the doubled clock** of an 8-inch connector in `drives-other::wd17xx` (`Fdc::fast`).
- Host keymap `max80`, ROM role `boot` (folder `max80`), the `.imd`/`.dmk`/`.jv1`/`.jv3` extensions for the frontends.

Where the facts came from: Lobo's own manuals were not found online. Everything is from the documentation and comments of the lobo_max80 emulator and its diagnostic disk (MIT; they cite the Technical Reference Manual, the schematics, Lobo's boot ROM and BIOS sources and measurements on a machine). No code was taken. See `docs/sources.md`.

Who decided what: the user asked for the machine and to begin. Claude's choices, not asked: an own crate instead of a model of `machine-trs80`; 128 KB as the default; the DIP switches following the disk in drive 0 (`boot=auto`); the clock chip starting at 1984-01-01 with emulated time (no host clock, for determinism); a green picture; fetching Lobo's boot ROM from the lobo_max80 repository into `~/.mimic/roms/max80/` and Lobo's CP/M and LDOS disks from the Internet Archive into `testdata/max80/` for local runs (neither is free; neither is in the repository or the fetch script).

## Open

- **The user's own disks and a look by hand** in the desktop frontend (keyboard mapping, sound).
- Serial ports: the SIO takes its registers and reports an empty transmitter; no lines, no receive, no interrupts, the COM8116 rate is only stored.
- Printer: always ready, the bytes go nowhere (the generic printer of `devices-printer` is not connected).
- SASI hard disk port (boot device 7) and Lobo's UVC controller: nothing answers.
- The clock chip cannot be set (the TIME SET button on the back is not built).
- Picture: drawn whole at the end of each field, so changes inside a field do not show (the homebrew demos `demo` and `crtctrix` need them); no CRTC cursor (Lobo's systems do not use it), no interlace; the picture is the displayed area centred on a fixed canvas, not placed by the sync registers.
- Floppy: the chip's delayed register writes, head load and settle times are not modelled; LDOS's FORMAT and CP/M 3 (two 8-inch disks at hand locally) are not tried; a 3.5-inch drive on the 8-inch connector is not offered.
- Drive kind is the image's (an 8-inch image is an 8-inch drive); an empty drive counts as 5.25-inch.
- The expansion bus: no cards.
- The guide PDFs are not rebuilt.
