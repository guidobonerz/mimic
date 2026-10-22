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

### Video, colour and host conventions

| Topic | Document | Used in |
| --- | --- | --- |
| YUV, square-pixel rates | ITU-R Recommendation BT.601 | `video-crt` |
| PAL/NTSC subcarrier frequencies, colour burst | PAL and NTSC broadcast standards (ITU-R BT.470) | `video-crt/src/signal.rs` |
| Host key names | W3C *UI Events KeyboardEvent code values* | `emu-input`, `emu-desktop`, `emu-preview` |
| VICE binary monitor protocol | VICE manual, chapter "Binary monitor" (API version 2) | `emu-debug/src/vice.rs` |
| Symbol files | VICE monitor label files; KickAssembler `.sym` format | `emu-debug/src/symbols.rs` |
| base64 | RFC 4648 | `emu-debug/src/protocol.rs` |

## Test suites and test data

Downloaded by `tools/fetch-testdata.sh` into `testdata/` (gitignored, never
committed), pinned to exact versions and checked by hash where possible.

| Suite | Source | Version | License / note |
| --- | --- | --- | --- |
| Tom Harte SingleStepTests, 6502 | https://github.com/SingleStepTests/65x02 | commit `2f6980a2d95757486c7bee24355c360e40e2a224` | MIT |
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

## Software dependencies

Third-party Rust crates (see `Cargo.toml` for exact versions): `winit`
(windowing), `wgpu` (GPU rendering), `naga` (WGSL validation in tests),
`cpal` (audio), `gilrs` (gamepads), `tokio` and `tokio-tungstenite` (debug
and preview servers), `futures-util`, `serde`, `serde_json`, `toml`, `png`, `flate2` (gzip test data),
`qoi`, `pollster`, `criterion` (benchmarks). The Python debug client uses
the Python standard library only.

## User-supplied ROMs

Not in the repository (golden rule 6): Commodore KERNAL/BASIC/character
ROMs (C64, VIC-20, PET, C16/Plus/4, C128), drive DOS ROMs (1541, 1571,
1581), Atari XL OS and BASIC, Amstrad CPC ROMs, Amiga Kickstart (1.2, 1.3,
2.0, 3.1; 256 or 512 KB images), the A1000 bootstrap ROM. Users put them
into `roms/<machine>/` and `roms/drives/`.
