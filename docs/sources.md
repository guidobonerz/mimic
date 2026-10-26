# Sources

All outside documents, data and software mimic was built from. The
emulation code is this project's own: hardware behaviour comes from the
documents below and from test suites run against the code, never from
another emulator's source (CLAUDE.md, golden rule 7). Each module names
its sources again in its doc comment; this file collects them.

How this list was made: from the `Source:`/`Sources:` notes in the code,
`tools/fetch-testdata.sh`, the workspace's dependencies and the web pages
looked up while developing. Hardware knowledge in the comments comes from
the published documents named here (as known from their published
contents); pages actually fetched during development are listed in their
own section.

## Hardware documentation

### CPUs

| Topic | Document | Used in |
| --- | --- | --- |
| 6502 instruction set, cycle tables | MOS Technology, *MCS6500 Microcomputer Family Hardware Manual* (appendix A) | `cpu-6502` (`table.rs`, `micro.rs`) |
| 6502 cycle-by-cycle bus activity | John West, Marko Mäkelä, *64doc* | `cpu-6502/src/micro.rs` |
| Undocumented 6502 opcodes | Groepaz, *NMOS 6510 Unintended Opcodes* ("No More Secrets") | `cpu-6502/src/table.rs` |
| 6502 interrupt polling | NESdev wiki, "CPU interrupts" | `cpu-6502/src/micro.rs` |
| 6510 I/O port | MOS 6510 data sheet | `cpu-6502/src/io_port.rs` |
| 6502C SALLY HALT | Avery Lee, *Altirra Hardware Reference Manual*, "6502C" | `cpu-6502/src/lib.rs` |
| Z80 | Zilog, *Z80 CPU User Manual* (UM0080); Sean Young, *The Undocumented Z80 Documented*; boo_boo & Vladimir Kladov, *MEMPTR, esoteric register of the Z80*; David Banks and Peter Helcmanovsky on the Q register / SCF/CCF flags | `cpu-z80` |
| 68000 instruction set, exceptions, bus cycles, prefetch | Motorola, *M68000 Family Programmer's Reference Manual* (M68000PM/AD); Motorola, *M68000 8-/16-/32-Bit Microprocessors User's Manual* (M68000UM/AD, timing tables) | `cpu-m68k` |
| 68000 per-cycle bus order of every instruction (reads, writes, internal clocks, prefetch) | "YACHT — Yet Another Cycle Hunting Table" (https://gist.github.com/cbmeeks/e759c7061d61ec4ac354a7df44a4a8f1) | `cpu-m68k/src/exec.rs`, `ea.rs` |
| 68000 ABCD/SBCD/NBCD flags and undefined V | Flamewing, analysis of the 68000 BCD instructions (as published with his test ROM, 2014) | `cpu-m68k/src/exec.rs` |

### Commodore chips and machines

| Topic | Document | Used in |
| --- | --- | --- |
| VIC-II timing, bad lines, sprites, AEC | Christian Bauer, *The MOS 6567/6569 video controller (VIC-II) and its application in the Commodore 64* (1996) | `chips-cbm/src/vic2.rs` |
| VIC-II palette | Philip "Pepto" Timmermans, *Calculating the color palette of the VIC II* (method and measured luma levels) | `chips-cbm/src/vic2/palette.rs` |
| CIA 6526 | MOS 6526 data sheet; Wolfgang Lorenz, *A Software Model of the CIA6526* | `chips-cbm/src/cia.rs` |
| SID 6581/8580 | MOS 6581 data sheet (envelope rates); published analyses of the envelope counter | `chips-cbm/src/sid.rs` |
| VIA 6522 | MOS 6522 data sheet; Rockwell R6522 data sheet (timing diagrams) | `chips-cbm/src/via.rs` |
| PIA 6520 | MOS 6520 data sheet; Motorola MC6820 data sheet | `chips-cbm/src/pia.rs` |
| CRTC 6545 | MOS 6545 data sheet; Motorola MC6845 data sheet | `chips-cbm/src/crtc.rs` |
| VIC 6560/6561 | *VIC-20 Programmer's Reference Guide*; MOS 6560/6561 data sheet | `chips-cbm/src/vic.rs`, `machine-vic20` |
| TED 7360/8360 | *Commodore Plus/4 User's Guide*; *TED 7360R0 preliminary data sheet*; *Plus/4 Programmer's Reference* | `chips-cbm/src/ted.rs`, `machine-ted` |
| MMU 8722, VDC 8563 | *Commodore 128 Programmer's Reference Guide* (chapters 10, 13); MOS 8722 and 8563 data sheets; C128 schematic 310378 | `chips-cbm/src/mmu.rs`, `vdc.rs`, `machine-c128` |
| C64 memory map, PLA, keyboard, paddles, expansion port | *Commodore 64 Programmer's Reference Guide* (chapter 5, appendix I); C64-Wiki, "Bank Switching" | `chips-cbm/src/pla.rs`, `machine-c64`, `devices-*` |
| PET | Commodore PET schematics and the editor ROM's key decode tables | `machine-pet` |
| Keyboard matrices | Programmer's Reference Guides of each machine; KERNAL key tables (VIC-20 `$EC5E`, Plus/4 `$E026`, C128 `$FA80`) | `machine-*/src/keyboard.rs` |
| 1351 mouse | *Commodore 1351 Mouse User's Manual* | `devices-input/src/mouse1351.rs` |

### Commodore drives and media

| Topic | Document | Used in |
| --- | --- | --- |
| 1541, GCR, D64 | Immers/Neufeld, *Inside Commodore DOS* ("Inside the 1541"); *1541 User's Guide*; 1541 service manual schematic 251854 | `drives-cbm`, `media/src/d64.rs`, `gcr.rs` |
| 1571, D71 | *Commodore 1571 User's Guide*; port bits as the 1571 DOS (310654-05) uses them | `drives-cbm/src/c1541.rs`, `media` |
| 1581, D81, MFM | *Commodore 1581 User's Guide* and service manual (318044); Western Digital WD1770/1772 data sheet | `drives-cbm/src/c1581.rs`, `wd177x.rs`, `media/src/mfm.rs` |
| 8050/8250, D80/D82 | *CBM 8050/8250 user's manual* | `media/src/d64.rs` |
| Serial (IEC) bus protocol | *Commodore 64 Programmer's Reference Guide*, serial bus; *Inside the 1541* | `drives-cbm/src/virtual_drive.rs` |
| IEEE-488 on the PET | PET KERNAL behaviour (e.g. `$F0D8`), IEEE-488 three-wire handshake | `drives-cbm/src/ieee_drive.rs` |
| TAP | TAP format as written by C64S and documented with the VICE tools | `media/src/tap.rs` |
| CRT cartridges | CRT format documentation of the VICE cartridge tools; C64-Wiki | `media/src/crt.rs`, `devices-cartridge/src/banked.rs` |
| EasyFlash | skoe, EasyFlash programmer's documentation; AMD Am29F040B data sheet | `devices-cartridge/src/easyflash.rs`, `flash.rs` |

### Real hardware: flux readers and real drives (`media-backends`, features `flux`, `greaseweazle`, `opencbm`)

| Topic | Document | Used in |
| --- | --- | --- |
| Greaseweazle USB protocol | Keir Fraser, Greaseweazle firmware `inc/cdc_acm_protocol.h` (commands, acks, bus types, flux stream opcodes, N28 encoding, the 10000/9600-baud reset) and host tools `src/greaseweazle/usb.py` (command framing, `<4BI4B3H14x` GET_INFO layout, READ_FLUX with `revs + 1` index pulses, flux stream decoding), https://github.com/keirf/greaseweazle-firmware, https://github.com/keirf/greaseweazle — public domain (Unlicense); read as documentation, no code copied | `media-backends/src/greaseweazle.rs` |
| OpenCBM API | `include/opencbm.h` (function signatures, `CBM_FILE` per platform, `__cdecl`, `cbm_device_type_e`) and the doxygen comments of `lib/cbm.c` (return values of `cbm_driver_open_ex`, `cbm_raw_read`, `cbm_device_status`, `cbm_exec_command` …), https://github.com/OpenCBM/OpenCBM — GPL; only the interface was used, no code copied | `media-backends/src/opencbm/ffi.rs` |
| DOS direct access | *1541 User's Guide*, "Direct access commands" (`#` buffer channel, `U1`/`U2`, `B-P`, error channel); *1571 User's Guide* (`U0>M1`); *1581 User's Guide* | `media-backends/src/opencbm/mod.rs` |
| 1541 bit rates, GCR framing | *Inside the 1541* (read/write electronics, SYNC detection, GCR); 1541 service manual (16 MHz / 13–16 / 4 bit clock per speed zone) | `media-backends/src/flux/gcr.rs` |
| G64 | Peter Schepers, "G64 (Raw GCR binary representation of 1541 disks)", G64.TXT; VICE manual, emulator file formats | `media/src/g64.rs` |
| MFM, IBM track format | WD1772 and NEC µPD765 data sheets (MFM clock rule, `$A1` with missing clock = `$4489`, ID/data address marks, CRC-CCITT); IBM *Diskette General Information Manual* | `media-backends/src/flux/mfm.rs`, `disk.rs` |
| Amiga MFM | Amiga Hardware Reference Manual ch. 8 "Disk"; Laurent Clévy, ADF FAQ | `media-backends/src/flux/mfm.rs` (via `media/src/adf.rs`) |

### Atari 8-bit

| Topic | Document | Used in |
| --- | --- | --- |
| ANTIC, GTIA, POKEY, 6502C, SIO, cartridges, memory map | Avery Lee, *Altirra Hardware Reference Manual* | `chips-atari`, `machine-atari8`, `drives-other/src/sio.rs`, `devices-cartridge/src/atari.rs` |
| ANTIC, colours | Chris Crawford et al., *De Re Atari* (chapter 2, colour appendix) | `chips-atari/src/antic.rs`, `palette.rs` |
| GTIA priority logic | GTIA data sheet (equations as reproduced in the Altirra manual) | `chips-atari/src/gtia.rs` |
| POKEY | POKEY data sheet (C012294) | `chips-atari/src/pokey.rs` |
| Keyboard codes, controllers | *Atari 400/800 (and XL) Technical Reference Notes* | `machine-atari8/src/keyboard.rs`, `devices-input/src/paddles.rs` |
| Memory map, PORTB banking | *Atari 800XL Field Service Manual* | `machine-atari8/src/memory.rs` |
| 130XE: 64 KB extended RAM in four 16 KB banks at $4000–$7FFF, PORTB bit 4 (CPU) and bit 5 (ANTIC) enable them separately, bits 2–3 select the bank | *Atari 130XE Field Service Manual*; Altirra Hardware Reference Manual, "PORTB" / "Extended memory" | `machine-atari8/src/memory.rs` (`Kind::Xe130`) |
| 400/800: 8–48 KB RAM, 10 KB OS-A/OS-B at $D800–$FFFF, nothing at $C000–$CFFF, left/right cartridge slots, PIA port B = joystick ports 3/4 (TRIG2/3, POT4–7), the RESET key wired to ANTIC's /RNMI (NMIST/NMIEN bit 5) | *Atari 400/800 Operating System User's Manual* (memory map, RNMI); *Atari 400/800 Hardware Manual* (PIA and port wiring); Altirra Hardware Reference Manual, "ANTIC" NMIEN/NMIST | `machine-atari8/src/memory.rs` (`Kind::Old`), `lib.rs`, `chips-atari/src/antic.rs` (`reset_key`) |
| Graphics data views of the Atari 8-bit: P/M memory layout from PMBASE (double-line missiles +$180, players +$200 + $80·n; single-line +$300, +$400 + $100·n), display-list walk (blank/JMP/JVB/LMS, 1 KB display-list and 4 KB memory-scan counters, playfield widths), character modes 2–7 with CHBASE/CHACTL | Altirra Hardware Reference Manual, "ANTIC" and "GTIA"; *Atari 400/800 Hardware Manual* (ANTIC/GTIA register descriptions) | `chips-atari/src/antic/gfx.rs` |
| Graphics data view of the Atari 2600 TIA: GRP/ENAM/ENABL, NUSIZ copies and sizes, REFP, CTRLPF, PF0–PF2 bit order, VDEL old/new graphics, object start delays | Steve Wright, *Stella Programmer's Guide* (1979); Andrew Towers, "Atari 2600 TIA Hardware Notes" (2003) | `chips-atari/src/tia/gfx.rs` |
| SIO protocol | *Atari 400/800 OS Manual*, "Serial I/O" | `drives-other/src/sio.rs` |
| XEX | *Atari DOS 2.0S Reference Manual*, "Binary file format" | `media/src/xex.rs` |
| ATR | ATR format as published with the SIO2PC project (Nick Kennedy) | `media/src/atr.rs` |
| CAS | CAS format as documented with the A8CAS tools | `media/src/cas.rs` |
| CAR | CAR format and type numbers from the Atari800 project's and Altirra's documentation | `media/src/car.rs` |

### Amstrad CPC

| Topic | Document | Used in |
| --- | --- | --- |
| Z80 instruction lengths on the CPC (µs/"NOPs"), wait-state rule (Gate Array holds /WAIT on 3 of 4 T-states; accesses stretched, internal cycles not; I/O sampled on TW) | cpcwiki, "Z80", CPC instruction timings (read via the mirror https://oldwiki.cpcwiki.eu/index.php/Z80) | `chips-amstrad/src/gate_array.rs`, `machine-cpc/tests/z80_timing.rs` |
| CRTC types 0–2 (readable registers, status, sync widths, R8, R12/R13 per line on type 1), firmware register values | cpcwiki, "CRTC" (mirror https://oldwiki.cpcwiki.eu/index.php/CRTC); Motorola MC6845 data sheet | `chips-amstrad/src/crtc.rs` |
| Gate Array (PENR/INKR/RMR/MMR, hardware colours, pixel encodings, interrupts, 6128 RAM configurations) | cpcwiki, "Gate Array" (mirror https://oldwiki.cpcwiki.eu/index.php/Gate_Array) | `chips-amstrad/src/gate_array.rs`, `palette.rs`, `machine-cpc/src/memory.rs` |
| 8255 PPI on the CPC (ports, port B links, port C, AY control) | cpcwiki, "8255" (mirror https://oldwiki.cpcwiki.eu/index.php/8255); Intel 8255A data sheet | `chips-amstrad/src/ppi.rs`, `machine-cpc` |
| Keyboard matrix | cpcwiki, "Programming:Keyboard scanning" (mirror) | `machine-cpc/src/keyboard.rs`, `emu-input` keymap `cpc` |
| CDT tape images (TZX 1.20 block format, pulse and pause semantics) | "TZX format" specification, World of Spectrum (format revision 1.20) | `media/src/cdt.rs`, `drives-other/src/cpc_tape.rs` |
| µPD765A floppy controller (commands, phases, MSR, ST0–ST3, end of cylinder without TC) | NEC µPD765 data sheet; cpcwiki, "765 FDC" | `chips-amstrad/src/fdc.rs` |
| DSK / extended DSK disk images | cpcwiki, "Format:DSK disk image file format" (Marco Vieth, Ulrich Doewich, Kevin Thacker) | `media/src/dsk.rs`, `drives-other/src/cpc_drive.rs` |
| CPC joystick port pinout (fire 2 = main button, pin 6; fire 1 = pin 7), JOY() bits | cpcwiki, "Connector:Digital joystick"; magic-cookie.co.uk, "CPC JOY" (https://magic-cookie.co.uk/CPC/joy.html) | `devices-input/src/joystick.rs`, `amx_mouse.rs`, `machine-cpc` |
| AY-3-8912 registers, tone/noise/envelope generators, volume DAC | General Instrument AY-3-8910/8912 data sheet | `chips-amstrad/src/ay.rs` |
| Z80 samples /WAIT on the falling edge of T2 (TW for I/O) | Zilog Z80 CPU User Manual (UM0080), timing diagrams; Thomas Harte, CLK wiki "Amstrad CPC Timing" (search summary, page not reachable) | `cpu-z80/src/lib.rs` (`step_timed`) |

### Amiga

| Topic | Document | Used in |
| --- | --- | --- |
| Custom chip registers, bit layouts | Commodore-Amiga, *Amiga Hardware Reference Manual*, 3rd edition, appendix A–C | `chips-amiga` |
| Beam counters, DMA time-slot allocation, interrupt levels (INTENA/INTREQ), memory map | HRM chapter 7 "System control hardware", memory map appendix | `chips-amiga/src/custom.rs`, `paula.rs`, `machine-amiga/src/memory.rs` |
| Copper MOVE/WAIT/SKIP, danger bit, restart at vertical blank | HRM chapter 2 "Coprocessor hardware" | `chips-amiga/src/copper.rs` |
| Bitplane fetch window (DDFSTRT/DDFSTOP), display window, scrolling, HAM, EHB, dual playfield | HRM chapter 3 "Playfield hardware" | `chips-amiga/src/custom.rs`, `denise.rs` |
| Sprite DMA and display, attached sprites, priorities | HRM chapter 4 "Sprite hardware" | `chips-amiga/src/custom.rs`, `denise.rs` |
| Blitter area, fill and line modes, minterms | HRM chapter 6 "Blitter hardware" | `chips-amiga/src/blitter.rs` |
| 8520 CIA (TOD event counter, one-shot start), CIA port assignments, floppy control and status lines, drive identification | HRM appendix F "8520 CIA"; HRM chapter 8 "Disk" | `chips-cbm/src/cia.rs`, `machine-amiga`, `drives-other/src/amiga_drive.rs` |
| Collision detection (CLXCON, CLXDAT) | HRM chapter 7 "Collision detection" | `chips-amiga/src/denise.rs` |
| Blitter cycle lengths by channel, idle ("-") cycles in the blitter cycle sequences, line-mode pixel time; CPU on even slots, BLITHOG and the three-cycle rule | HRM chapter 6 "Blitter speed" and "Blitter DMA" (read via the Amiga Developer CD 2.1 HTML copy, see below) | `chips-amiga/src/blitter.rs`, `custom.rs` |
| 68000 chip-bus arbitration: a chip-RAM-bound loop runs at its full instruction time with bitplane DMA off (the CPU takes any free slot, odd ones included, contrary to the HRM's "even cycles only"); tick counts under 5-plane and 4-plane DMA with and without per-line Copper MOVEs | Measured with `tools/amiga_bustest.py` on WinUAE (cycle-exact A500: 68000, 512 KB chip + 512 KB slow, Kickstart 1.3, PAL) on 2026-10-22: 5539, 6DD3, 7196, 5EC9 E-clock ticks; observable behaviour only, no code | `chips-amiga/src/custom.rs` (CPU slots), `tests/tests/amiga_boot.rs` (`chip_bus_arbitration_matches_the_bus_test`) |
| Joystick bits in JOYxDAT, POTGO/POTGOR | HRM chapter 8 "Interface hardware" | `machine-amiga`, `chips-amiga/src/paula.rs` |
| Interrupt latency: VERTB set two colour clocks after the vertical strobe at 0/0, the IPL lines changing four colour clocks after an INTREQ/INTENA change, the CPU acting on IPL about six clocks later (logic-analyzer and test-ROM observations on A500/A1000) | vAmiga issue #274 "Test Interrupts/int/int1" (github.com/dirkwhoffmann/vAmiga/issues/274), discussion only, no code; MC68000 User's Manual, interrupt exception processing | `machine-amiga/src/lib.rs` (`IPL_DELAY`), `chips-amiga/src/custom.rs` (`VERTB_DELAY`) |
| BLITHOG off: colour clocks in which a 68000 waiting for its data slot sees the Blitter, bitplane DMA or a Copper fetch on the bus count towards the "three consecutive memory cycles"; its request half-cycle and the fixed slots do not | HRM chapter 6 "Blitter DMA"; measured with `tools/amiga_bustest2.py` on a cycle-exact WinUAE A500 (68000, Kickstart 1.3, PAL) on 2026-10-23, start at line 0: ticks/loops 2084/00AA (BLTPRI off), 1A39/0001 (on), 411A/00FE (5 planes + per-line Copper), 1A53/0128 (D only); observable behaviour only, no code | `chips-amiga/src/custom.rs`, `tests/tests/amiga_boot.rs` (`blitter_bus_arbitration_matches_the_bus_test`) |
| Copper jumps written by the Copper (COPJMP1/COPJMP2 on every line into a 5- or 30-MOVE sub-list and back), with and without 5 lowres planes: the 68000's loop time | Measured with `tools/amiga_bustest3.py` on a cycle-exact WinUAE A500 on 2026-10-23: 588C, 58FB, 71DF, 71D5 E-clock ticks; observable behaviour only, no code | `chips-amiga/src/copper.rs`, `tests/tests/amiga_boot.rs` (`copper_jump_bus_timing_matches_the_bus_test`) |
| Sprite visibility without plane data: no sprite on lines where BPLCON0 has BPU = 0 or bitplane DMA is off, visible again once BPL1DAT arrives (the plane pointers do not advance while BPU = 0); the vAmigaTS `sprbpu1` photographs (OCS and ECS A500s) show the same gaps | `tools/amiga_sprtest.py` booted in WinUAE (OCS A500 configuration), screenshots 2026-10-23; observable behaviour only, no code | `chips-amiga/src/denise.rs` |
| Denise's pixel timing against Agnus: a Copper COLOR00 write at $48 shows from the display window's first pixel (DIWSTRT H $94 = 148), i.e. pixel 2h − 4 of the clock the write happens in; sprites with HSTART $84 and $1AE show one pixel inside windows starting at 148 and ending at 432, i.e. from HSTART + 1 | WinUAE screenshot (cycle-exact A500 configuration) of the frame around Sanity's "Interference" raster tunnel, 2026-10-23; observable behaviour only, no code | `chips-amiga/src/custom.rs` (`DENISE_LAG`), `chips-amiga/src/denise.rs` |
| The 68000 next to Copper lists like Interference's (per-line BPLxPTL sub-lists, 30 COLOR MOVEs on every second or every line) and the Blitter: the cube's blit loop without and with its 1 x 30-word A->D blits, 5 lowres planes | `tools/amiga_bustest4.py`, booted on a cycle-exact WinUAE A500 on 2026-10-23: 3FD6, 42AB, 572B, 7184, 5A59, 65D1, 7ABF, A829 E-clock ticks; observable behaviour only, no code | `chips-amiga/src/custom.rs`, `tests/tests/amiga_boot.rs` (`cpu_blitter_copper_bus_timing_matches_the_bus_test`) |
| Copper wake-up: one Copper cycle passes between a restart (COPJMP, vertical blank) and the first instruction fetch | markwrobel.dk, "Amiga Machine Code Letter XII – Wave" (the no-op MOVE at the start of a Copper list "is there because the Copper start need one extra cycle before the first instruction fetch"), citing English Amiga Board post #1431584 (eab.abime.net/showthread.php?p=1431584); observation only, no code | `chips-amiga/src/copper.rs` (`wake`) |
| Copper wake-up after a satisfied WAIT: one even slot that holds the bus before the next fetch (so `WAIT $FF,$DE` + `WAIT $20,$DE` reaches line $120, not 256) | Measured with `tools/amiga_bustest.py` against WinUAE (cycle-exact A500, 2026-10-22): the 4-plane + per-line-Copper row; seen in Rule 30's kaleidoscope (Andromeda), whose lower part switched 32 lines early without it; observable behaviour only, no code | `chips-amiga/src/copper.rs` (`busy_wake`), `custom.rs` |
| Audio channels: registers, DMA, block-start interrupts, left/right assignment, output filters | HRM chapter 5 "Audio hardware" | `chips-amiga/src/audio.rs`, `machine-amiga` |
| Disk controller: DSKLEN double write, WORDSYNC, DSKSYNC, DSKBYTR, FIFO, ADKCON FAST; drive lines, index | HRM chapter 8 "Disk", appendix A | `chips-amiga/src/disk.rs`, `drives-other/src/amiga_drive.rs` |
| ADF images, AmigaDOS track and sector format (odd/even MFM longs, header and data checksums), boot block checksum | Laurent Clévy, "The .ADF (Amiga Disk File) format FAQ" | `media/src/adf.rs`, `tests/tests/amiga_boot.rs` |
| Keyboard: serial protocol to CIA-A (KCLK/KDAT, bit order, inversion), handshake, power-up stream, raw key codes | HRM chapter 8 "Keyboard", keyboard appendix | `machine-amiga/src/keyboard.rs`, `emu-input` keymap `amiga` |
| Mouse quadrature signals, game port pinout, JOYxDAT counters, JOYTEST | HRM chapter 8 "Controller ports", "Reading the mouse" | `devices-input/src/amiga_mouse.rs`, `chips-amiga/src/custom.rs` |
| Interlace: long/short fields (LOF), field order | HRM chapter 3 "Interlaced mode" | `chips-amiga/src/custom.rs`, `video-crt/src/field.rs` |
| VPA/E-clock bus cycles (CIA accesses, autovector acknowledge) | Motorola, *M68000 User's Manual*, "Synchronous operation" | `machine-amiga` (`e_clock_waits`) |
| Audio attach modes (ADKCON USExV/USExP: a channel's words set the next channel's volume or period, both alternating volume first; the attached channel is silent) | HRM chapter 5 "Producing complex sounds", "Modulating sound", ADKCON in appendix A | `chips-amiga/src/audio.rs`, `custom.rs` |
| Serial port: SERPER (bit period in colour clocks, LONG), SERDAT (data with stop bits, shifted out LSB first after a start bit until empty), SERDATR bits (OVRUN, RBF, TBE, TSRE, RXD), TBE/RBF interrupts, UARTBRK | HRM chapter 8 "Serial port", appendix A | `chips-amiga/src/uart.rs` |
| ADKCON PRECOMP/MFMPREC: write precompensation shifts flux transitions by 0/140/280/560 ns; no effect on a bit-cell model, the bits read back | HRM appendix A, ADKCON | `chips-amiga/src/paula.rs` (doc) |
| A500 output filters: fixed first-order low-pass about 4.9 kHz; LED filter a second-order Butterworth (Sallen-Key) low-pass about 3.3 kHz switched by CIA-A PA1 | HRM chapter 5 "Audio filter"; corner frequencies as commonly derived from the A500 schematic (Rev 6A, audio output stage) | `machine-amiga/src/lib.rs` (`LED_B`, `LED_A`) |
| Zorro II autoconfig: configuration ROM as nibbles at $E80000 (byte n at 4n / 4n+2, upper data bits, all but er_Type inverted), er_Type bits (Zorro II $C0, memory list $20, size code), base address written to $E80048/$E8004A, shut-up at $E8004C, expansion space $200000–$9FFFFF | HRM appendix "Expansion", `libraries/configregs.h` of the Amiga ROM Kernel Reference Manual: Includes and Autodocs (3rd ed.) | `machine-amiga/src/memory.rs` (`Autoconfig`) |
| A1000 writable control store: 256 KB RAM at $FC0000 loaded by the bootstrap ROM from the Kickstart disk, write-protect latch set by a write into the bootstrap ROM's range, the WCS then showing at $F80000–$FFFFFF; 256 KB chip RAM standard | *Amiga 1000* schematics and service documentation (Commodore), the HRM memory map; behaviour also described in the Amiga Hardware Database (A1000 page) | `machine-amiga/src/memory.rs` (`Wcs`) |
| 1 MB chip RAM (Agnus 8372 "Fat Agnus" in A500 rev 6A and A2000), its address range and mirroring; A2000 as a 1 MB chip RAM board without trapdoor RAM | Amiga Hardware Database, A500 and A2000 pages; HRM memory map | `machine-amiga` (`chip` option, models) |
| AGA registers: Alice ids $22/$32 in VPOSR, Lisa id $F8, BPLCON0 BPU3, BPLCON3 BANK/LOCT/PF2OF/SPRES, BPLCON4 BPLAM/ESPRM/OSPRM, BPLCON2 KILLEHB, BPLCON1's extra scroll bits, FMODE fetch widths, BPL7/8PT, CLXCON2, SPRxCTL's extra position bits, HAM8 (planes 1–2 control, 3–8 data), 24-bit palette writes copying the high nibbles to the low ones | amiga-dev.wikidot.com register pages bplcon0, bplcon1, bplcon2, bplcon3, bplcon4, fmode, vposr (read 2026-10-23); HRM appendix C ECS/AGA entries (theflatnet.de hard_c.html). Not in these sources and this project's reading: the fetch order within an AGA fetch unit, DDFSTRT not being aligned to the unit, the HAM8 low two bits of a gun staying from the previous pixel, SPRES 0 meaning 140 ns sprites | `chips-amiga/src/custom.rs`, `chips-amiga/src/denise.rs` |
| ColecoVision memory and I/O map ($80/$C0 controller segment strobes, VDP at $BE/$BF, SN76489 and controller reads at $E0–$FF with A1 selecting the port), 1 KB RAM mirrored over $6000–$7FFF, the VDP interrupt on /NMI, the cartridge header ($AA55 title screen, $55AA direct start, start pointer at $800A) | pvcollib wiki, "Complete Colecovision Specs" (https://github.com/alekmaul/pvcollib/wiki/Complete-Colecovision-Specs, read 2026-10-23); colecoboxart FAQ (http://colecoboxart.com/faq/FAQ05.htm); Wikipedia "ColecoVision" (clock, BIOS size, 12.7 s / 3.3 s title pause) | `machine-coleco` |
| ColecoVision controllers: the DE-9 pinout (pins 5 and 8 the segment commons, 7 and 9 the spinner phases), the Super Action Controller's extra buttons, the Roller Controller using both ports | retronicdesign USBJoystickAdapter wiki, "ColecoVision and ADAM controllers" (read 2026-10-23); AtariAge forum thread on the controller PCBs. Not in these sources and this project's reading: the keypad code table, which bits of the controller byte carry the spinner phases, the interrupt on phase A's rising edge cleared by reading the port, the Driving Module's pedal on the fire line | `devices-input/src/coleco.rs`, `machine-coleco` |
| MegaCart bank switching (last 16 KB fixed at $8000, the bank at $C000 selected by accessing $FFC0–$FFFF) | colecovision.dk, "MegaCart" (https://colecovision.dk/edewaard.htm) for the sizes and the 16 KB halves; the $FFC0 register and the fixed last bank are the scheme as homebrew developers describe it (forum posts), not checked against a board | `devices-cartridge/src/coleco.rs` |
| TMS9918A/TMS9929A: registers, the four display modes, Graphics II table masking, sprites (32, four a line, 5S, coincidence, early clock, magnification), status and interrupt, the read-ahead buffer, 342 clocks per line, 262/313 lines, borders 13/15 and 27/24 (PAL 54/48), the palette from the luminance and colour-difference table | Texas Instruments, *TMS9918A/TMS9928A/TMS9929A Video Display Processors Data Manual* (1982); SMS Power, tms9918a.txt (https://www.smspower.org/uploads/Development/tms9918a.txt) for the frame flag after line 192 | `chips-ti/src/vdp.rs` |
| SN76489: write format, tone periods (period 0 = 1024), the 15-bit noise register with taps 0 and 1 (white) or 0 (periodic), noise rate 3 following tone 2, 2 dB attenuation steps | Texas Instruments SN76489AN datasheet; SMS Power, "SN76489" (https://www.smspower.org/Development/SN76489) | `chips-ti/src/psg.rs` |
| WDC 65C816: instruction set, cycle-by-cycle bus activity (table 5-7), emulation-mode rules, vectors | Western Design Center, *W65C816S* datasheet; David Eyes / Ron Lichty, *Programming the 65816*; cycle details fitted to Tom Harte's SingleStepTests 65816 | `cpu-65816` |
| Sony SPC700: instruction set, flags, DIV/DAA/DAS behaviour, cycle counts, dummy reads of stores | Martin Korth, fullsnes, "SNES APU SPC700 CPU …" (https://problemkaputt.de/fullsnes.htm, read 2026-10-23); bus cycles fitted to Tom Harte's SingleStepTests spc700 | `cpu-spc700` |
| SNES S-DSP: BRR, pitch, Gaussian interpolation (the 512-entry table is reproduced from fullsnes as a hardware constant), ADSR/GAIN rates, volumes, KON/KOFF/FLG, noise, echo/FIR | fullsnes, "SNES APU DSP …", "SNES APU Low Level Timings" (read 2026-10-23) | `chips-nintendo/src/sdsp.rs` |
| SNES S-SMP I/O ports, timers, communication protocol (implemented as this project's own loader — the boot ROM disassembly in fullsnes was not used) | fullsnes, "SNES APU SPC700 I/O Ports", "SNES APU Main CPU Communication Port" (read 2026-10-23) | `machine-snes/src/apu.rs` |
| SNES S-PPU: registers, BG modes, tiles and maps, Mode 7 formula, sprites, windows, colour math, timers and status, VRAM/CGRAM/OAM access | fullsnes, "SNES Picture Processing Unit (PPU)" and subchapters, "SNES Memory … Access" (read 2026-10-23); anomie's register doc for offset-per-tile | `chips-nintendo/src/sppu.rs` |
| SNES system: memory map and access speeds, DMA/HDMA, interrupts, H/V events and line timing, refresh, multiply/divide, controllers (joypad, mouse, Multiplayer 5, Super Scope) | fullsnes, "SNES I/O Map", "SNES Memory …", "SNES DMA …", "SNES PPU Interrupts", "SNES Timing …", "SNES Maths Multiply/Divide", "SNES Controllers …" (read 2026-10-23) | `machine-snes`, `devices-input/src/snes.rs` |
| SNES cartridges: internal header, LoROM/HiROM/ExHiROM mapping, SRAM, copier header, odd ROM sizes | fullsnes, "SNES Cartridge ROM Header", "SNES Cart LoROM/HiROM Mapping" (read 2026-10-23) | `devices-cartridge/src/snes.rs` |
| NES 2A03: the 6502 without decimal mode, interrupt polling and NMI/BRK interaction, DMA (halt, dummy and alignment cycles, get/put cycles, load and reload DMC fetches, DMC during OAM DMA) | NESdev wiki, "CPU", "CPU interrupts", "DMA" (https://www.nesdev.org/wiki/DMA, read 2026-10-23); blargg's cpu_interrupts_v2 and sprdma_and_dmc_dma results; the LXA ($AB) constant $FF from blargg's instr_test-v5 | `cpu-6502` (`Variant::Rp2A03`, `sample_interrupts`), `machine-nes` |
| NES PPU 2C02/2C07: registers, scrolling (v/t/x/w), rendering pipeline, sprite evaluation and overflow bug, frame timing, odd-frame skip, power-up state, I/O latch decay, palette from the composite levels and phases, emphasis | NESdev wiki, "PPU registers", "PPU scrolling", "PPU rendering", "PPU sprite evaluation", "PPU frame timing", "PPU power up state", "PPU palettes", "NTSC video" (read 2026-10-23); dot positions fitted to blargg's ppu_vbl_nmi, vbl_nmi_timing, sprite_overflow_tests, mmc3_test_2; power-up palette from blargg's power_up_palette | `chips-nintendo/src/ppu.rs`, `palette.rs` |
| NES APU: frame counter, length table, envelope, sweep, pulse duty, triangle, noise periods, DMC rates, mixer formulas | NESdev wiki, "APU", "APU Frame Counter", "APU Length Counter", "APU Envelope", "APU Sweep", "APU Pulse", "APU Triangle", "APU Noise", "APU DMC", "APU Mixer" (read 2026-10-23) | `chips-nintendo/src/apu.rs` |
| NES cartridges: iNES and NES 2.0 headers, mirroring, NROM, MMC1, UxROM, CNROM, MMC3 (and MMC3A), bus conflicts | NESdev wiki, "INES", "NES 2.0", "Mirroring", "NROM", "MMC1", "UxROM", "CNROM", "MMC3", "Bus conflict" (read 2026-10-23) | `devices-cartridge/src/nes.rs` |
| NES controllers: standard controller, Four Score signatures, Zapper lines, Arkanoid controller | NESdev wiki, "Standard controller", "Controller reading", "Four Score", "Zapper", "Arkanoid controller" (read 2026-10-23) | `devices-input/src/nes.rs`, `machine-nes` |
| Graphics data views of the TMS9918A/TMS9929A: table addresses, Graphics I/II colour tables and masks, Multicolour, Text, sprite attributes and 16×16 quadrant order | Texas Instruments, *TMS9918A/TMS9928A/TMS9929A Video Display Processors Data Manual* (1982), sections 2.2–2.4 (as for the chip) | `chips-ti/src/vdp/gfx.rs` |
| Graphics data views of the NES PPU: pattern table layout, nametables and attribute quadrants, OAM bytes and 8×16 sprites, palette mirrors, scroll from t and fine x | NESdev wiki, "PPU pattern tables", "PPU nametables", "PPU attribute tables", "PPU OAM", "PPU palettes", "PPU scrolling" | `chips-nintendo/src/ppu/gfx.rs` |
| Graphics data views of the SNES S-PPU: VRAM bit-plane tile format, BGxSC/BGxNBA, Mode 7 map and tile bytes, OBSEL sizes, name select gap and large-object tile stride, OAM high table, CGRAM | fullsnes, "SNES PPU Video Memory (VRAM)", "SNES PPU Sprites (OBJ)", "SNES PPU Color Palette Memory (CGRAM)" (https://problemkaputt.de/fullsnes.htm) | `chips-nintendo/src/sppu/gfx.rs` |
| AGA wide-fetch slots: blocks aligned to the fetch unit, planes in a long unit's last eight clocks (plane 1 last), sprite DMA lost from DDFSTRT on (sprite n when $17 + 4n ≥ DDFSTRT), a sprite keeping its slot against a plane | `tools/amiga_agatest.py` booted in WinUAE (A1200 configuration) by the user, screenshots 2026-10-26 (four bands, then 13 bands DDFSTRT $48…$18); observable behaviour only, no code | `chips-amiga/src/custom.rs` |
| AGA fetch widths and planes: FMODE 1/2 doubles and FMODE 3 quadruples the bitplane bandwidth (hires with more than 16 colours needs FMODE 1+, 64-bit aligned bitmaps with FMODE 3, modulo minus 8), BPLCON1's 35 ns scroll bits and the scroll range of 64 low-resolution pixels; 68020 interrupt recognition (IPL synchronised on two falling clock edges, taken at an instruction boundary) | "AGA" by Randy of Comax (text file, copy read at https://www.stashofcode.fr/code/afficher-sprites-et-bobs-sur-amiga/AGAByRandyOfComax.txt, 2026-10-26); Motorola, *MC68020/MC68EC020 Microprocessors User's Manual*, section 5 "Signal descriptions / interrupt operation". Which slots a wide fetch unit gives to planes 5–8, the 35 ns output switching and `IPL_DELAY_020` are this project's model | `chips-amiga/src/custom.rs`, `chips-amiga/src/denise.rs`, `machine-amiga/src/lib.rs` |
| 68020 instruction set and programming model: full-format extension words, bit field instructions, CAS/CAS2, CHK2/CMP2, PACK/UNPK, MULx.L/DIVx.L, MOVEC control registers, RTD, TRAPcc, LINK.L, EXTB, the M bit with MSP/ISP, exception stack frame formats 0/1/2 and RTE's handling, VBR, misaligned operands | Motorola M68000 Family Programmer's Reference Manual (M68000PM/AD rev. 1), sections 2 (addressing modes), 4 (integer instructions), 6 (exception processing). The timing follows the User's Manual (next row) | `cpu-m68k/src/exec020.rs`, `cpu-m68k/src/ea.rs`, `cpu-m68k/src/lib.rs` |
| 68020 instruction cache (64 long-word entries, direct mapped by A7–A2, tag A31–A8 + FC2 and a valid bit, CACR E/F/CE/C, CAAR, cleared at reset, data never cached) and instruction timing (tables 8.2.1–8.2.18: fetch/fetch immediate/calculate/calculate immediate/jump effective address, MOVE, special-purpose MOVE, arithmetic, immediate, BCD, single-operand, shift/rotate, bit, bit field, branch, control, exception-related, save/restore; best/cache/worst case with read/prefetch/write counts; 3-clock bus cycles; long-word prefetch; bus controller/sequencer concurrency and overlap, section 8.1) | Motorola, *MC68020/MC68EC020 Microprocessors User's Manual* (MC68020UM/AD, first edition, 1992), sections 4 "On-Chip Cache Memory" and 8 "Instruction Execution Timing" (copy read: https://media.nostalgianerd.com/file/mc68020.pdf, 2026-10-25). The cache-case column is the model's data; how the internal clocks lie between the bus cycles is this project's model | `cpu-m68k/src/timing020.rs`, `cpu-m68k/src/lib.rs` |
| A1200 Gayle: id register at $DE1000 read bit-serially in D7 (A1200 id $D0, a write restarts), IDE/status registers at $DA0000–$DAFFFF (unused here: no drive) | Linux kernel `arch/m68k/include/asm/amigayle.h` (register addresses and id constants as hardware facts; GPL text not copied); amiga-dev wiki | `machine-amiga/src/memory.rs` (`Gayle`) |
| ECS registers: VPOSR Agnus ids (8372 $20 PAL / $30 NTSC, LOL bit 7), DENISEID $FC for the 8373, BEAMCON0 bit layout, HTOTAL/VTOTAL meaning under VARBEAMEN, DIWHIGH bit layout (13 stop H8, 10–8 stop V, 5 start H8, 2–0 start V), BLTSIZV/BLTSIZH, BPLCON0 ECSENA/SHRES and the BPLCON3 bits it gates, super-hires with two planes and two colour bits per gun | Amiga Hardware Reference Manual, 3rd edition, appendix C "Custom chip register list, alphabetical" with the ECS entries (https://www.theflatnet.de/pub/cbm/amiga/AmigaDevDocs/hard_c.html, read 2026-10-23); amiga-dev.wikidot.com register pages bplcon0, bplcon3, beamcon0, diwhigh, ddfstrt (read 2026-10-23); that the 2 MB 8375 reports the same id as the 8372A comes from community measurements (xSysInfo issue 61, https://github.com/reinauer/xSysInfo/issues/61). Not in the sources and this project's reading: the super-hires fetch order, which two colour bits survive (the upper ones here), DIWHIGH being reset by DIWSTRT/DIWSTOP writes (the HRM's "if DIWHIGH is written last"), BEAMCON0's power-on PAL bit following the chip's strap | `chips-amiga/src/custom.rs`, `chips-amiga/src/denise.rs`, `chips-amiga/src/blitter.rs`, `chips-amiga/src/regnames.rs` |
| High-density floppy drive: $AAAAAAAA identification with an HD disk, DD identification with a DD disk, 150 rpm for HD disks (twice the bit cells per track at Paula's clock), 22 sectors per track, 1 760 KB ADF | Commodore *A3000/A4000 HD floppy* documentation as summarised by the trackdisk.device autodocs (`TD_GETDRIVETYPE`, `DRIVE3_5_150RPM`) and the ADF FAQ | `drives-other/src/amiga_drive.rs`, `media/src/adf.rs` (`Density`) |
| Extended ADF layouts `UAE--ADF` (sync/length table) and `UAE-1ADF` (reserved, type, length, bit length per track) | Laurent Clévy, "The .ADF (Amiga Disk File) format FAQ", section on the UAE extended ADF; format description only, no code | `media/src/adfx.rs` |
| Sprite behaviour checks: display-window clipping of sprites (shown only after the line's first BPL1DAT, or on lines without planes), sprite/playfield and sprite/sprite collisions (CLXCON/CLXDAT: bit 0 follows the odd planes alone, the odd-plane sprite bits need the even planes too in single-playfield mode, nothing counts outside the window or on lines without bitplane data), playfield priorities, SPRxPOS/CTL/DATA write timing | vAmiga test suite (github.com/dirkwhoffmann/vAmigaTS, GPL-3.0; the test programs are mostly Alpine9000's Amiga examples) — run as test data only with `tests/tests/vamigats.rs`, compared against the repository's photographs of real A500s; no code or images copied into this repository | `tests/tests/vamigats.rs`, `chips-amiga/src/denise.rs` |
| DMS archive layout (file header, track headers, CRC-16/ARC, checksums), compression modes NOCOMP/SIMPLE/QUICK/MEDIUM/DEEP/HEAVY1/HEAVY2 and the track flags (state carry-over, new trees, RLE) | xDMS 1.3.2 by André Rodrigues de la Rocha (public domain, `COPYING`: "xdms is licensed under PUBLIC DOMAIN"), read as the format's documentation; its decoders derive from LZHUF (Haruyasu Yoshizaki, public domain) and UNIX LHA (Masaru Oki); verified with freely distributed DMS archives from Aminet | `media/src/dms.rs` |

### Sinclair ZX81

| Topic | Document | Used in |
| --- | --- | --- |
| Memory map (ROM $0000 with shadow, RAM from $4000, A15 undecoded), keyboard matrix and port $FE, 3.25 MHz clock | Sinclair ZX81 Service Manual | `machine-zx81/src/memory.rs`, `keyboard.rs` |
| The ULA's display mechanism: NOP substitution on M1 fetches with A15 set and D6 clear, the pattern address from I, the character code and the 3-bit line counter LCNTR, the shift register at 6.5 MHz; /INT from refresh address line A6 (R bit 6); the NMI generator (207 T-states per line) and /WAIT while halted; VSYNC on `IN ($FE)`, off on any `OUT`; `OUT ($FD)`/`OUT ($FE)` NMI off/on | Wilf Rigter, "The ZX81 Video Display System" and his ULA notes (as reproduced in the ZX81 community's hardware documentation); Sinclair ZX81 Service Manual (ULA pinout); observable behaviour only, no emulator code | `chips-sinclair/src/ula.rs`, `machine-zx81/src/lib.rs` |
| ROM entry points for the tape traps (NEXT-PROG $0347, HEADER $02FF, SLOW/FAST $0207), system variables D_FILE $400C, E_LINE $4014, the program area from VERSN $4009, character codes | Ian Logan and Frank O'Hara, *The Complete ZX81 ROM Disassembly* (Melbourne House, 1983); the user's `zx81.edition3.sym` label file | `machine-zx81/src/lib.rs`, `tests/tests/zx81_boot.rs` |
| .P file layout: the bytes from $4009 up to E_LINE as the ROM saves them | Logan/O'Hara (SAVE routine); common ZX81 emulator documentation of the ".P" format | `machine-zx81/src/lib.rs` (`PTape`) |
| Character set (64 codes, bit 7 inverse, patterns at $1E00 through I); the display file from D_FILE (leading NEWLINE, 24 lines of up to 32 codes, collapsed lines below 3¼ KB of RAM) for the graphics data views | Sinclair, *ZX81 BASIC Programming* manual (Steven Vickers, 1981), appendix A and chapter 27 "Organization of memory"; Logan/O'Hara (CLS, the character patterns) | `machine-zx81/src/gfx.rs` |

### Sinclair ZX Spectrum

| Topic | Document | Used in |
| --- | --- | --- |
| ULA frame timing (224 × 312 T-states at 3.5 MHz, 228 × 311 at 3.5469 MHz on the 128K), the frame interrupt (32 / 36 T-states), contended memory (the 6, 5, 4, 3, 2, 1, 0, 0 delays from T-state 14335 / 14361, the odd RAM banks on the 128K), I/O contention patterns (N:1 C:3, C:1 C:3, N:4, C:1 ×4), the floating bus (bitmap, attribute, bitmap, attribute, four idle T-states), border, port $FE bits, issue 2/3 EAR feedback | Chris Smith, *The ZX Spectrum ULA: How to Design a Microcomputer* (ZX Design Retro Computer, 2010); comp.sys.sinclair FAQ and the World of Spectrum FAQ (reference pages "Contended memory", "128K timings", "Floating bus"); Ramsoft, "Floating bus technical guide" (https://k1.spdns.de/Develop/Projects/zxsp/Info/Floating%20bus%20technical%20guide.html, read 2026-10-26) | `chips-sinclair/src/spectrum.rs` |
| Internal T-states of every Z80 instruction with the address on the bus (IR after a fetch, the operand or stack address otherwise) | Tom Harte's SingleStepTests Z80 bus traces (checked by `cpu-z80/tests/harte.rs`); the World of Spectrum FAQ contention tables ("pc:4, ir:1 ×2" notation) | `cpu-z80` (`TimedBus::idle`, `Z80::idle_cycles`) |
| Memory map, 128K paging port $7FFD (A15 = 0, A1 = 0; bank, screen, ROM, lock), AY ports $FFFD/$BFFD, the keyboard matrix | Sinclair ZX Spectrum Service Manual; Sinclair ZX Spectrum 128 Service Manual; ZX Spectrum 48K BASIC manual, chapter 23 | `machine-spectrum/src/memory.rs`, `keyboard.rs`, `lib.rs` |
| Timing test: frame length, contended memory and I/O, high-port contention, floating bus, LDIR at the contended boundary, $7FFD/$3FFD reads latching the floating bus on the 128K | Philip Kendall, Fuse Test 2008-03-28 (GPL test program; fetched by `tools/fetch-testdata.sh` from https://zxe.io/depot/software/ZX%20Spectrum/, run only, never committed); listed on the redcode ZXSpectrum wiki (https://github.com/redcode/ZXSpectrum/wiki/Fuse-Test, read 2026-10-26) | `tests/tests/spectrum_boot.rs` (`fuse_test_passes`), `machine-spectrum/src/lib.rs` |
| The ROM loader's entry LD-BYTES $0556 (A flag, IX address, DE length, carry load/verify) and SA/LD-RET $053F, tape block layout (flag, data, XOR checksum), standard pulse timings | Ian Logan and Frank O'Hara, *The Complete Spectrum ROM Disassembly* (Melbourne House, 1983) | `machine-spectrum/src/lib.rs` (`trap`, `fast_load`), `media/src/cdt.rs` |
| TAP and TZX files; the pause after a block begins with an edge (the opposite level, 1 ms if high, then low); "stop the tape" and "stop if in 48K mode" blocks | World of Spectrum, "TZX format 1.20" and "TAP format" descriptions | `media/src/cdt.rs`, `drives-other/src/spectrum_tape.rs` |
| SNA (48K/128K) and Z80 (versions 1–3, hardware modes, page numbering, ED ED run-length coding) snapshots | World of Spectrum, "Z80 file format" and "SNA file format" (worldofspectrum.org/faq/reference/z80format.htm, formats.htm) | `machine-spectrum/src/snapshot.rs` |
| Kempston joystick (port $1F, bits right/left/down/up/fire), Sinclair Interface 2 and cursor joysticks on keys, Kempston mouse ports $FBDF/$FFDF/$FADF | World of Spectrum FAQ (joystick and mouse interfaces); Kempston Mouse Interface manual | `machine-spectrum/src/lib.rs`, `keyboard.rs`, `devices-input/src/kempston_mouse.rs` |
| Multiface 128: `IN ($BF)` pages in, `IN ($3F)` out, hidden after power-on, the red button activates it, works on 48/128/+2; `OUT ($3F)`/`OUT ($BF)` hide and show it and `IN ($BF)` bit 7 is the screen bit, read from the user's MF128 ROM | Romantic Robot, *Multiface 128* manual (OCR by Wilko Schröter, 2004; https://spectrumcomputing.co.uk/pub/sinclair/hardware-info/m/Multiface128_Manual.pdf, read 2026-10-26); the user's `mf128.rom` disassembled with this project's tools | `machine-spectrum/src/memory.rs` |
| Interface 1: shadow ROM paging (in at the fetch from $0008/$1708, out after $0700), ports $E7/$EF/$F7 and their bits, the Microdrive drive-select shift register, GAP/SYNC polling, block lengths (15-byte header, 528-byte record, 12-byte preamble), FORMAT's $FC record filler, RS-232 bit levels | Sinclair, *ZX Interface 1 and Interface 2 Service Manual*; the user's Interface 1 ROM (edition 2) disassembled with this project's tools (SEL-DRIVE $1532, $15AD–$16D9, FORMAT $1B5D, RS-232 $0B88/$0D0C); Ian Logan, *Spectrum Microdrive Book* | `machine-spectrum/src/if1.rs`, `drives-other/src/microdrive.rs` |
| MDR cartridge images (254 × 543 bytes and the write-protect byte) | World of Spectrum, "MDR file format"; https://k1.spdns.de/Develop/Projects/zxsp/Info/File%20Formats/mdr.txt (read 2026-10-26) | `media/src/mdr.rs` |

### Vectrex

| Topic | Document | Used in |
| --- | --- | --- |
| Motorola 6809: registers, condition codes, instruction descriptions, the cycle tables of all three opcode pages and the indexed postbyte, interrupt sequences (NMI arming by S, FIRQ, CWAI, SYNC) | Motorola, "MC6809 8-Bit Microprocessor" data sheet; Motorola, "MC6809-MC6809E Microprocessor Programming Manual" (M6809PM/AD). The undocumented aliases ($01 NEG, $05 LSR, $0B DEC, $4E/$5E CLR, $x2 NEG/COM by carry, $14/$15/$CD halting) are common knowledge in the 6809 community; not checked against a chip | `cpu-6809` |
| Vectrex memory map, VIA wiring (port A DAC, port B: S/H strobe, multiplexer select, BC1/BDIR, comparator, cartridge line, /RAMP from T1), CA1 button 4 / light pen, CA2 /ZERO, CB2 /BLANK from the shift register, the multiplexer channels (Y, zero reference, intensity, sound; controller pots), the AY's port A as the buttons, the 6522 shift-register modes | roadsidethoughts.com, "Vectrex Programmer Notes": "Hardware Description", "Memory Map", "6522 Interface Adapter", "Programmable Sound Generator" (read 2026-10-24) | `machine-vectrex`, `chips-cbm/src/via.rs`, `devices-input/src/vectrex.rs` |
| 6522 shift-out under φ2 (mode 6): one bit per two φ2 cycles, eight bits, then the SR flag | MOS 6522 and Rockwell R6522 data sheets (shift-register timing diagrams) | `chips-cbm/src/via.rs` |
| 64 KB Vectrex cartridges: PB6 selects the 32 KB bank, high (pulled up after reset) the upper half | search summary of homebrew cartridge descriptions (2026-10-24); behaviour only, no code used | `devices-cartridge/src/vectrex.rs` |
| 3D Imager: port 2, sync pulse once a revolution on button 4 (CA1), motor by pulse-width modulation on pin 2, the eye/colour order right blue, green, red, then left | nicole.express, "Fun With 3 Colors: The Vectrex 3-D Imager!" (https://nicole.express/2021/color-and-3d-finally-united.html, read 2026-10-24); FuzzyLogic8192, "Vectrex-3D-Imager-Triangle-Demo" README (https://github.com/FuzzyLogic8192/Vectrex-3D-Imager-Triangle-Demo, read 2026-10-24); 3-D Mine Storm's spin-up loop and interrupt handler, disassembled with this project's tools (AY port A bit 5 drives the motor, the 0→1 edge of bit 7 counts a revolution, CA1 on the rising edge, a turn under $FC00 cycles); motor constants and the six equal sectors are this project's assumptions | `devices-input/src/vectrex.rs` (`Imager`), `machine-vectrex` |
| Executive routines as observed while debugging: `Print_Str` ($F495) moves the beam back with `NEG` on port A (which needs port A reads to return ORA on output pins), the cold-start title loop runs until the frame counter $C825 reaches 2 | disassembly of the user's system ROM with this project's disassembler (not copied) | `machine-vectrex` |

### Apple II

| Topic | Document | Used in |
| --- | --- | --- |
| Disk II track format: address field (D5 AA 96, 4-and-4 volume/track/sector/checksum, DE AA EB), data field (D5 AA AD, 342 + 1 nibbles), self-sync bytes, 6-and-2 encoding and the rule behind its write table, DOS 3.3 sector interleave | Don Worth, Pieter Lechner, *Beneath Apple DOS* (Quality Software), chapter 3 "Disk II formatting" | `media/src/a2disk.rs` |
| ProDOS block-to-sector mapping (block on physical sectors 0/2, 4/6, …), volume directory header (storage type $F, entry length $27, 13 entries per block) | Don Worth, Pieter Lechner, *Beneath Apple ProDOS* (Quality Software), chapters 3 and 4 | `media/src/a2disk.rs` |
| WOZ disk images (header, CRC32, INFO, TMAP, TRKS in versions 1 and 2, FLUX in 2.1) | John K. Morris, "WOZ Disk Image Reference" 1.0 (https://applesaucefdc.com/woz/reference/) and 2.0/2.1 (https://applesaucefdc.com/woz/reference2/) | `media/src/a2disk.rs` |
| CRC-32 as used by WOZ (reflected polynomial $EDB88320) | ISO/IEC 3309 / IEEE 802.3 CRC-32, check value `CBF43926` for "123456789" | `media/src/a2disk.rs` |
| Video scanner (65 cycles, 262 lines, horizontal states $00/$40–$7F, vertical $0FA–$1FF), the address adder (A3–A6 = H5 H4 H3 + V4 V3 V4 V3 + 1101), text/lo-res/hi-res address bits, the floating bus, hi-res bit-7 delay, the colour killer in text mode, language card switches and pre-write rule | Jim Sather, *Understanding the Apple II* (Quality Software, 1983), chapters 3, 5, 8; *Understanding the Apple IIe* (1985), chapters 5 and 8 | `machine-apple2` (`video.rs`, `mem.rs`) |
| I/O map ($C000–$C0FF), soft switches, game I/O and paddles, cassette, keyboard latch and strobe, slot ROM and $C800 space conventions | Apple, *Apple II Reference Manual* (1979) | `machine-apple2`, `devices-input/src/apple.rs`, `drives-other/src/apple_tape.rs` |
| IIe memory switches (80STORE, RAMRD, RAMWRT, INTCXROM, ALTZP, SLOTC3ROM, INTC8ROM), status reads $C011–$C01F, 80-column and double hi-res display, auxiliary memory cards, RESET behaviour | Apple, *Apple IIe Technical Reference Manual* (1985), chapters 2, 4, 7 | `machine-apple2` |
| Disk II card I/O (phases, motor, drive select, Q6/Q7), logic state sequencer commands, stepper phases | Sather, *Understanding the Apple II*, chapter 9 "The Disk II", table 9.1 | `drives-other/src/disk2.rs` |
| P6 PROM address and data line order (A7 S0, A6 S2, A5 S3, A4 /PULSE, A3 Q7, A2 Q6, A1 QA, A0 S1; next state reflected in D7–D4) | Christer Ericson, notes on the P6 state machine ROM (retro.co.za/ccc/apple2/christer/Apple/p6.notes.html, via a search summary 2026-10-24); confirmed here by reading GCR nibbles through the user's 341-0028 image, which only this order does | `drives-other/src/disk2.rs` |
| Hi-res colours as an NTSC monitor shows them (green, violet, orange, blue at about 105°, 285°, 15°, 195°) | common Apple II colour descriptions (Apple II Reference Manual, "High-Resolution Graphics"); the picture phase checked with `video-crt`'s composite stage | `machine-apple2/src/video.rs` |
| Mockingboard: two 6522s at $Cn00/$Cn80, port A data, port B BC1/BDIR//RESET, AY-3-8910 per VIA, IRQ | Sweet Micro Systems, *Mockingboard* manual (programming section) | `machine-apple2/src/mockingboard.rs` |
| RGB values of the 16 lo-res colours for the graphics data views (4-bit RGB per colour; an approximation of an NTSC monitor's picture) and the simple hi-res colour rule (adjacent dots white, lone dots violet/green or blue/orange by column parity and bit 7) | Apple, *Apple IIGS Hardware Reference* (1987), "Video" (the IIGS's colour table for the Apple II modes); Apple II Reference Manual, "High-Resolution Graphics" | `machine-apple2/src/gfx.rs` |
| 65C02 instructions, addressing modes, cycle table, decimal mode; the undefined opcodes' lengths and bus cycles, decimal-mode dummy reads | WDC W65C02S data sheet, table 5-7; Rockwell R65C02 data sheet; Bruce Clark, "Decimal Mode" appendix A (65C02 SBC); Tom Harte, SingleStepTests `65x02/synertek65c02` (MIT) | `cpu-6502` (`Variant::Cmos65C02`) |
| WAV files (RIFF chunks, `fmt `, `data`, PCM sample layouts) | Microsoft/IBM, *Multimedia Programming Interface and Data Specifications 1.0* (1991), "WAVE Form" | `media/src/wav.rs` |

### Macintosh

| What | Source | Used in |
| --- | --- | --- |
| Memory map (RAM, ROM, overlay, SCC $9FFFF8/$BFFFF9, IWM $DFE1FF, VIA $EFE1FE, SCSI $580000 with DACK at A9), VIA port bits, interrupt levels, screen and sound buffer addresses, mouse wiring, 5380 register addresses | Linux/m68k "Mac Plus hardware" notes (https://linux-mac68k.sourceforge.net/devel/plushw.php.html); Apple, *Inside Macintosh* III, "The Macintosh Hardware" (1985); Wikipedia, "Macintosh 128K/512K technical details" | `machine-mac` |
| Video timing (704 × 370 at 15.6672 MHz), video taking every other RAM slot (SE: one in four) | Wikipedia, "Macintosh 128K/512K technical details"; Big Mess o' Wires, Mac Plus/SE memory timing (bigmessowires.com/?p=391, citing MacTech) | `machine-mac/src/video.rs` |
| Real-time clock protocol (command bytes, seconds, 20/256-byte parameter RAM, write protect) | *Inside Macintosh* III, "The Real-Time Clock"; Linux/m68k notes | `machine-mac/src/rtc.rs` |
| M0110/M0110A keyboard: commands (Inquiry $10, Instant $14, Model $16, Test $36), Null/ACK/NAK, transition bytes, $79 keypad prefix, $71 shift, raw codes, bit timing | *Inside Macintosh* III, "The Keyboard and Keypad"; tmk, "Apple M0110 Keyboard Protocol" (github.com/tmk/tmk_keyboard/wiki); the two disagree on the command codes, Inside Macintosh's are used | `machine-mac/src/keyboard.rs` |
| ADB transceiver states (ST0/ST1, /INT), command byte, register 0/3 layouts, default addresses | Linux/m68k `via-macii` notes; Wikipedia, "Apple Desktop Bus"; mcosre.sourceforge.net/docs/adb_intro.html | `machine-mac/src/adb.rs` |
| Z8530 SCC registers, status-modified vector, external/status latches | Zilog, *Z8530 SCC Technical Manual* | `machine-mac/src/scc.rs` |
| NCR 5380 registers, SCSI-1 phases and commands | NCR 5380 data sheet; ANSI X3.131-1986 | `machine-mac/src/scsi.rs` |
| IWM state bits and registers, latch mode, Sony drive sense lines and commands, tachometer (60 pulses a turn), zone speeds | Thomas W., "Emulating the Macintosh floppy drive" (https://thomasw.dev/post/mac-floppy-emu/); CiderPress II "Nibble notes" | `drives-other/src/iwm.rs` |
| 3.5" GCR sector layout (address field, format byte, data field of 699 + 4 bytes, gaps, 2:1 interleave), the data checksum's byte order | CiderPress II, "Nibble notes" and "DiskCopy notes" (ciderpress2.com/formatdoc); Clock Signal (MIT, git.applefritter.com/6502/CLK, commit 5a9f3cf, decoder) | `media/src/macdisk.rs` |
| The 400K drive's speed control: the 128K ROM's speed table, its tachometer measurement and the shift register it steps to make the pulse-width byte | the user's 128K ROM, read by disassembly while it ran (2026-10-24); no code taken from it | `machine-mac` (`PWM_WIDTH`), `drives-other/src/iwm.rs` |
| Boot blocks ('LK', bbEntry, bbVersion $44xx for the Plus/SE ROMs to run the code), MFS master directory block | the user's Plus ROM (disassembly of its boot routine); *Inside Macintosh* II, "The File Manager" | `tests/tests/mac.rs` |
| Screen buffer layout for the graphics data view (512 × 342, 64 bytes a line, MSB left, 1 = black, main buffer $5900 / alternate $D900 below the top of RAM, VIA PA6) | *Inside Macintosh* III, "The Video Interface"; Apple, *Guide to the Macintosh Family Hardware* (2nd ed., 1990), chapter 3 | `machine-mac/src/gfx.rs` |

### Video, colour and host conventions

| Topic | Document | Used in |
| --- | --- | --- |
| YUV, square-pixel rates | ITU-R Recommendation BT.601 | `video-crt` |
| PAL/NTSC subcarrier frequencies, colour burst | PAL and NTSC broadcast standards (ITU-R BT.470) | `video-crt/src/signal.rs` |
| CRT stage: phosphor masks (shadow mask with delta-gun dot triads, aperture grille, slot mask with in-line guns), halation, barrel curvature, vignette, phosphor decay | No outside document consulted (2026-10-25): the effects follow general knowledge of colour picture tubes; the formulas (triangular phosphor weights, 7-tap glow, the curvature mapping, max-based decay) are this project's own, no shader pack or emulator code was used | `video-crt/src/cpu.rs`, `video-crt/src/shaders/crt.wgsl` |
| Host key names | W3C *UI Events KeyboardEvent code values* | `emu-input`, `emu-desktop`, `emu-preview` |
| VICE binary monitor protocol | VICE manual, chapter "Binary monitor" (API version 2) | `emu-debug/src/vice.rs` |
| Symbol files | VICE monitor label files; KickAssembler `.sym` format | `emu-debug/src/symbols.rs` |
| base64 | RFC 4648 | `emu-debug/src/protocol.rs` |
| VICE monitor commands imported as cheats (`>` memory set, `f` fill) | VICE manual, chapter "Monitor" | `emu-cheats/src/import.rs` |
| Cheat database key (CRC-32 of the image) | ISO/IEC 3309 / IEEE 802.3 CRC-32, check value `CBF43926` | `emu-cheats/src/lib.rs` |
| BASIC `POKE` statements (decimal; `$`/`&` hex on the C64 BASIC extensions and the CPC) | Commodore 64 Programmer's Reference Guide (BASIC keywords); Amstrad CPC 6128 User Instructions (Locomotive BASIC, `&` hex) | `emu-cheats/src/import.rs` |
| WebAssembly exports without bindings; AudioWorklet; Gamepad standard mapping; IndexedDB; Pointer Lock | WebAssembly Core Specification 1.0 (W3C); Web Audio API (W3C, AudioWorklet); Gamepad (W3C, "standard" mapping); Indexed Database API 3.0 (W3C); Pointer Lock 2.0 (W3C) | `emu-web/src/abi.rs`, `emu-web/www/mimic.js` |
| Rewind differences (XOR, zero-run lengths as LEB128) | LEB128 as in the DWARF 5 standard, section 7.6 | `emu-core/src/rewind.rs` |
| Light pens: VIC-II LP latch ($D013 = X/2, $D014 = raster, IRQ bit 3, once a frame; control port 1 pin 6 shared with fire) | Christian Bauer, "The MOS 6567/6569 video controller (VIC-II)", section 3.11; *Commodore 64 Programmer's Reference Guide*, appendix I (control port pinout) | `chips-cbm/src/vic2.rs`, `machine-c64`, `machine-c128`, `devices-input/src/lightpen.rs` |
| Light pens: ANTIC PENH (colour clock) / PENV (VCOUNT), trigger line as the light-pen input | Altirra Hardware Reference Manual, "ANTIC" (light pen registers); *De Re Atari*, chapter 8 | `chips-atari/src/antic.rs`, `machine-atari8` |
| Light pens: Agnus LPEN (BPLCON0 bit 3), VPOSR/VHPOSR frozen by a trigger or at the end of the field, released at line 20; pins 6 (LPENTG) and 5 (LPENPR) of game port 0; ECS BEAMCON0 LPENDIS | *Amiga Hardware Reference Manual*, 3rd edition, "Reading the light pen", "Connector pin usage for light pen inputs", appendix C (BEAMCON0) | `chips-amiga/src/custom.rs`, `machine-amiga` |
| Light pens: 6845 LPSTB latching the refresh address into R16 (MA13–8) / R17 (MA7–0); CPC expansion port pin 47 LPEN | Motorola MC6845 data sheet ("Light pen registers"); cpcwiki, "Connector:Expansion port" | `chips-amstrad/src/crtc.rs`, `machine-cpc` |
| Light-pen threshold (Rec. 601 luma) | ITU-R Recommendation BT.601 | `devices-input/src/lightpen.rs` |

### Network devices (C64 Ultimate command interface, WiC64)

Documentation only; the 1541 Ultimate and WiC64 firmware sources are GPL and were not used (golden rule 7).

- Ultimate documentation, "Core UCI Architecture" (https://1541u-documentation.readthedocs.io/en/latest/uci/core_uci_architecture.html) — registers $DF1C–$DF1F, control and status bits, the state machine, queue sizes (`devices-net::uci`).
- Ultimate documentation, "Ultimate DOS Target" (https://1541u-documentation.readthedocs.io/en/latest/uci/ultimate_dos_target.html) — DOS commands, open attributes, file info packet, directory entries, status texts.
- Ultimate documentation, "Network Target" (https://1541u-documentation.readthedocs.io/en/latest/uci/network_target.html) — socket commands, MAC/IP answers, READ_SOCKET/WRITE_SOCKET formats.
- Ultimate documentation, "Control Target" (https://1541u-documentation.readthedocs.io/en/latest/uci/control_target.html) — IDENTIFY, GET_HWINFO.
- WiC64 library README (https://github.com/WiC64-Team/wic64-library) — legacy, standard and extended protocols, status codes, the command list and behaviour (`devices-net::wic64`); command ids from its `wic64.h` constants.
- Forum64, "WiC64 Hardware Infos" (https://www.forum64.de/index.php?thread/119265-wic64-hardware-infos/) and pagetable.com (https://www.pagetable.com/?p=1656) — the user-port wiring: PA2 direction, PC2 and FLAG2 handshake.
- C64 Programmer's Reference Guide, appendix I — user port pinout (`emu-core::port::user_port`); MOS 6526 data sheet — PC pulses after port B accesses.
- Howard Hinnant, "chrono-Compatible Low-Level Date Algorithms" (http://howardhinnant.github.io/date_algorithms.html, public domain) — civil date from days (`emu-core::host::HostTime::local`).
- RFC 9112 (HTTP/1.1 message syntax, chunked transfer coding) — the small HTTP client in `media-backends::net`.

## Test suites and test data

Downloaded by `tools/fetch-testdata.sh` into `testdata/` (gitignored, never
committed), pinned to exact versions and checked by hash where possible.

| Suite | Source | Version | License / note |
| --- | --- | --- | --- |
| Tom Harte SingleStepTests, 6502 and NES 2A03 (`6502/v1`, `nes6502/v1`) | https://github.com/SingleStepTests/65x02 | commit `2f6980a2d95757486c7bee24355c360e40e2a224` | MIT |
| Tom Harte SingleStepTests, 65816 (`v1`, native and emulation files) | https://github.com/SingleStepTests/65816 | commit `307c63f5673c8c6b861c52f71ddb89212a227480` | MIT (~3 GB) |
| Tom Harte SingleStepTests, SPC700 (`v1`) | https://github.com/SingleStepTests/spc700 | commit `67d15f492b2740964abd4efd1229e0ec9c342228` | MIT |
| nes-test-roms collection (blargg's suites, nestest by Kevin Horton, others) — Phase 12 | https://github.com/christopherpow/nes-test-roms (pointed to by the user) | commit `95d8f621ae55cee0d09b91519a8989ae0e64753b`, archive SHA-256 `78aab1d5e5d1fb717fd4200c0525ec08a32e4c7e4094590cb6ace95a8ae81d7f` | no licence in the repository; run only, not committed |
| Tom Harte SingleStepTests, Z80 | https://github.com/SingleStepTests/z80 | commit `ebe1875d48f374bcfd4b505d8eb8ee751568b5f7` | MIT |
| Tom Harte SingleStepTests, 68000 (`68000/v1`) | https://github.com/SingleStepTests/680x0 | commit `e0d5ece9670205cc84a0101081837deb446f86a3` | MIT; three cases skipped as contradictory (`cpu-m68k/tests/harte.rs`, `BROKEN`) |
| Klaus Dormann 6502 functional test | https://github.com/Klaus2m5/6502_65C02_functional_tests | commit `7954e2dbb49c469ea286070bf46cdd71aeb29e4b`, SHA-256 checked | GPL-3.0; run only |
| ZEXDOC / ZEXALL (Frank Cringle) | https://github.com/agn453/ZEXALL | commit `8f71d418bae69a476a5a0e5c6e122c8801b8d9f4`, SHA-256 checked | GPL-2.0; run only |
| Wolfgang Lorenz C64 Emulator Test Suite 2.15 | VICE test programs, https://svn.code.sf.net/p/vice-emu/code/ (`testprogs/general/Lorenz-2.15`) | SVN revision 46290 | public domain |
| Acid800 (Avery Lee) — Phase 5 gate | https://www.virtualdub.org/downloads/Acid800-1.2.7z (listed on https://virtualdub.org/altirra) | 1.2, SHA-256 `4074e6a40db1833e97603e334ca130f5923bff34a0d4f8a83e2ecf9617de1b20` | MIT (per the test sources' headers); run only, not committed |
| Arnold "Acid tests" for the CPC (Kevin Thacker) — Phase 6 gate | ColinPitrat/CPCMisc, `assembly/007_arnold_acid` (https://github.com/ColinPitrat/CPCMisc), assembled locally with pasmo | commit `9e0ad714baee3548c6effc22044861a3f401799b`, SHA-256 `6342175d5694a2cce734bbc311c526160e5bcdb761dabc3e90e81e7c16cac7f7` | test headers: "free to distribute without modification, this copyright header must be retained" (repository GPL-3.0); run only, not committed |
| pasmo Z80 assembler (Julián Albo), to assemble the CPC tests | Debian source package, https://deb.debian.org/debian/pool/main/p/pasmo/pasmo_0.5.3.orig.tar.gz | 0.5.3, SHA-256 `db3c41cac16df5966fcca4719bb218af78224de13785b222d1d8e292fc87f1eb` | GPL-3.0; built and run as a tool only |

## Pages consulted during development

Pages fetched or searched while building (beyond the documents above):

- https://virtualdub.org/altirra — Acid800 releases and archive names.
- Atari 2600 (Phase 6b): Andrew Towers, "Atari 2600 TIA Hardware Notes" (2003, https://www.atarihq.com/danb/files/TIA_HW_Notes.txt) — TIA horizontal and object counters, start delays, copy spacing, HMOVE blanking extension and clock stuffing, GRP/ENABL old/new registers, the CPU-cycle-to-pixel table used in the machine tests; the PAL TIA palette is this project's approximation of the commonly described hue layout (searches only). Kevin Horton, "Cart Information" (https://www.qotile.net/minidig/docs/sizes.txt) — cartridge sizes and bank-switching hotspots; "Atari 2600 Specifications" (https://www.astro.physik.uni-potsdam.de/~mwendt/2600scene/specs.html) — clocks, line timing, joystick/paddle/console-switch bits; TIA register summary (https://hemulator.56k.guru/references/tia) — register map, collision bits, AUDC values; AtariAge forum threads on the RIOT timer (https://forums.atariage.com/topic/316035-what-a-riot-timer-weirdness/, https://forums.atariage.com/topic/133686-please-explain-riot-timmers/; search summaries) — first decrement right after the write, 1-cycle counting after the timer passes zero.
- C16/C116/Plus/4 A/V socket pinout (https://old.pinouts.ru/Audio-Video-Hardware/C16Video_pinout.shtml, community-compiled): luminance and chroma on separate pins besides composite, so the TED's output is S-Video like the C64's 8-pin socket.
- Ultimate 64 "Data Streams" (https://1541u-documentation.readthedocs.io/en/latest/data_streams.html) and REST API (`api/api_calls.rst`) — the idea of an uncompressed indexed video stream with a separate audio stream; the preview's raw stream (`emu-preview::raw`) has its own layout, no compatibility.
- AtariAge forum, "Acid800 - an Atari test suite" (https://forums.atariage.com/topic/171296-acid800-an-atari-test-suite/) — how the suite is run (search result).
- Searches for the CPC wait-state rule: cpcwiki forum threads (not readable, HTTP 403) and the CLK wiki page "Amstrad CPC Timing" (HTTP 502); only their search summaries were used.
- Arnold acid test sources (CPCMisc, see above) as hardware measurements: expected values and comments of `ppi` (port C pull levels &2F, `port_c_input`; port reset by mode set), `psg` (register numbers ≥ 16 deselect the AY), `type1ctrl` (UM6845R status bit 6 after power-on, cleared by reading R16/R17), `cpctest` (RMR ignores bit 5 on the CPC; interrupt timing) and `z80tests/cpu` (LD A,I/LD A,R P/V when an interrupt follows). No test code was copied; the runner patches two test bugs (`crtctest` CRTC-type prompt, `cpctest` GA I/O decode entry without BC) and documents them in `tests/src/cpc_acid.rs`.
- The ACE emulator's published CPC test results (search summary) — note that `crtctest` ignores the typed CRTC type.
- Searches about PPI port B bit 5 (/EXP) on the 6128 (cpcwiki pages HTTP 403; only search summaries): not conclusive, port B kept at &7E.
- Amiga Hardware Reference Manual, blitter chapter pages of the Amiga Developer CD 2.1 HTML copy (https://discmaster.textfiles.com/file/18/Amiga%20Developer%20CD%20v2.1.iso/Reference/HTML/Hardware_Manual_guide/node012A.html and node012B.html) — blitter cycle lengths, BLITHOG, CPU on even cycles.
- Harte 68000 bus transactions (`transactions` in the test data) — the cycle order of `M68000::step_timed`, the address-error and exception frames, and the DIVU/DIVS internal clock counts were fitted to them.
- Harte Z80 per-T-state bus traces (`cycles` in the test data) — the M-cycle placement of `Z80::step_timed` is checked against them.
- The Acid800 1.2 archive's own `README.html` and test sources — how standalone tests report pass/fail (`_testPassed`, `_testFailed`, `_testSkipped` labels in the `.lab` files).
- Acid800 test sources (MIT) as hardware measurements: the expected values and cycle comments of `antic_dmapattern` (playfield and refresh DMA slots), `antic_pfstarttiming`/`antic_pfstoptiming` (DMA window edges), `antic_vscroll`/`antic_vscroldli`, `antic_dlitiming`, `antic_blockednmi`, `antic_linebuffering`, `gtia_pmresize`/`gtia_pmoverlap`/`gtia_pmretrigger` (player width logic and retrigger) and `pokey_sertiming`. The models in `chips-atari` and `cpu-6502` were fitted to these results; no test code was copied.
- Light pens (2026-10-25): Amiga Hardware Reference Manual, light pen pages of the Amiga Developer CD 2.1 HTML copy (https://discmaster.textfiles.com/file/18/Amiga%20Developer%20CD%20v2.1.iso/Reference/HTML/Hardware_Manual_guide/node018A.html and the pin table in node0189/node010E) — LPEN, the VPOSR freeze and its release at line 20, LPENTG/LPENPR pins; cpcwiki, "Connector:Expansion port" (https://www.cpcwiki.eu/index.php/Connector:Expansion_port) — LPEN on pin 47; C64-Wiki "Control Port" (https://www.c64-wiki.com/wiki/Controlport) — pin 6 "Button A / LP"; searches (summaries only) on C64 and Atari light-pen button wiring (a hackaday.io log naming pin 3; atarimania FAQ and COMPUTE! articles on the CX70/CX75, PENH/PENV and LPENH/LPENV shadows) — the pens' button pins stay unverified.
- Real hardware (2026-10-25): the Greaseweazle repositories' `COPYING` (Unlicense, public domain), `inc/cdc_acm_protocol.h` and `src/greaseweazle/usb.py`; OpenCBM's `opencbm/include/opencbm.h` and the doc comments of `opencbm/lib/cbm.c` (interface only, GPL code not used).
- Atari 2600 DPC (2026-10-25): Kevin Horton's description of the DPC chip in Pitfall II (register map $1000–$107F, data fetchers with top/bottom flag compare, music mode of DF5–DF7 clocked by an on-board oscillator of about 20 kHz, amplitude mixing of the three voices, random number generator) and his "Cart Information" v6 (https://www.qotile.net/minidig/docs/sizes.txt); the random number generator's taps (XNOR of bits 7, 5, 4, 3, shifting left) were checked against the 255-byte generator dump appended to the 10 495-byte Pitfall II image. No emulator code was used.

## Software dependencies

Third-party Rust crates (see `Cargo.toml` for exact versions): `winit`
(windowing), `wgpu` (GPU rendering), `naga` (WGSL validation in tests),
`cpal` (audio), `gilrs` (gamepads), `tokio` and `tokio-tungstenite` (debug
and preview servers), `futures-util`, `serde`, `serde_json`, `toml`, `png`, `flate2` (gzip test data),
`qoi`, `pollster`, `criterion` (benchmarks); optional for real hardware:
`serialport` (Greaseweazle) and `libloading` (OpenCBM at run time). The Python debug client uses
the Python standard library only.

## User-supplied ROMs

Not in the repository (golden rule 6): Commodore KERNAL/BASIC/character
ROMs (C64, VIC-20, PET, C16/Plus/4, C128), drive DOS ROMs (1541, 1571,
1581), Atari XL OS and BASIC, Amstrad CPC ROMs, Amiga Kickstart (1.2, 1.3,
2.0, 3.1; 256 or 512 KB images), the A1000 bootstrap ROM, the Vectrex
system ROM (8 KB, or the 4 KB executive alone), the Apple II ROMs, the
Macintosh ROMs (128K, 512K, Plus, SE). Users put them
into `roms/<machine>/` and `roms/drives/`.
