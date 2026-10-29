# C64 test results

Run on 2026-10-28, VICE test programs and Lorenz with real ROMs again on
2026-10-29 after the VIC-II and CIA work (release build, user ROMs in
`roms/`, test data from `tools/fetch-testdata.sh` and
`tools/fetch_vice_testprogs.py`).

## Overview

| Suite | What it checks | Result | Pass rate | Run time |
| --- | --- | --- | --- | --- |
| Tom Harte SingleStepTests `6502` (all, `EMU_HARTE_FULL=1`) | NMOS 6502 core of the 6510: every opcode incl. undocumented ones, registers, memory, every bus cycle | 2 560 000 / 2 560 000 | 100 % | 16 s |
| Klaus Dormann 6502 functional test | all documented instructions, flags, decimal mode | passed | 100 % | 1 s |
| Lorenz 2.15, real ROMs (`lorenz_suite`) | CPU, CIA, VIC-II timing on the C64 | 264 / 264 | 100 % | 7.7 min |
| Lorenz 2.15, stub ROMs (`lorenz_suite_stub`, CI) | the same without ROMs (3 tests need real ROMs) | 261 / 261 (3 skipped) | 100 % | 4.4 min |
| VICE test programs (`c64-testlist.in`) | CIA, CPU, VIC-II, SID, interrupts, drives, expansions | 780 / 1240 (399 failed, 61 errors, 303 skipped) | 62.9 % | 26.5 min |

Trend: VICE test programs 680 → 780 (+100 since the run of 2026-10-28,
none newly failing); Lorenz with real ROMs 264 → 264 (no change). The
Harte, Dormann and Lorenz stub-ROM rows are from 2026-10-28 and were not
run again (the CPU core did not change). The VICE test programs were at 676 / 1240
on 2026-10-26.

## VICE test programs by area

| Area | Pass | Fail | Error | Skip | Pass rate (pass / pass+fail+error) |
| --- | ---: | ---: | ---: | ---: | ---: |
| general (incl. Lorenz 2.15: 314/314) | 318 | 6 | 0 | 1 | 98.1 % |
| CPU | 91 | 24 | 0 | 5 | 79.1 % |
| CIA | 112 | 76 | 0 | 3 | 59.6 % |
| interrupts | 19 | 20 | 0 | 4 | 48.7 % |
| VICII | 197 | 76 | 0 | 88 | 72.2 % |
| SID | 14 | 155 | 0 | 22 | 8.3 % |
| C64 | 12 | 10 | 48 | 25 | 17.1 % |
| drive | 6 | 32 | 13 | 4 | 11.8 % |
| testbench (self test) | 8 | 0 | 0 | 0 | 100 % |
| vdrive | 2 | 0 | 0 | 1 | 100 % |
| memory-expansions | 1 | 0 | 0 | 5 | 100 % |
| REU | 0 | 0 | 0 | 115 | – |
| GEO-RAM | 0 | 0 | 0 | 5 | – |
| mouse, propmouse, keypad, testjoy | 0 | 0 | 0 | 15 | – |
| others (audio-io, c64-cpm, crtemulation, digimax, plus60k, plus256k, sfx_soundsampler, userportrtc) | 0 | 0 | 0 | 10 | – |
| **total** | **780** | **399** | **61** | **303** | **62.9 %** |

Largest open blocks: `SID/wb_testsuite` (110 failing, SID waveform
sampling), `CIA/shiftregister` (68, mostly `cia-sdr-icr`),
`VICII/videomode` (28), `drive/viavarious` (19), `interrupts/irqdma`
(17); the 48 errors under
`C64` are programs that do not finish. Skipped: tests that need hardware
or options the runner does not set up (REU and GEO-RAM tests, mice,
interactive and analyser programs).

Details per program: `target/tmp/vice-testprogs.txt`, Lorenz:
`target/tmp/lorenz.txt`.

## Changes of 2026-10-28/29

VIC-II (`chips-cbm/src/vic2.rs`), 112 → 197:

- types 8565/8562 (`--option vicii=6569|8565|6567|8562`, the runner picks
  8565/8562 unless an entry asks for the old VIC-II) with the grey dot;
- colour registers act four pixels before the cycle's own pixels, the
  first of them in the old colour (6569) or light grey (8565);
- $D01B from the cycle's pixel 2, $D01D from 2, $D01C from 3; the sprite
  shifter moves one bit per pixel, multicolour switched on mid-sprite
  latches one step later at the start of a step;
- vertical border compare in every cycle before the CPU's write; while the
  vertical border flip-flop is set the sequencer loads nothing;
- a sprite disabled again before cycle 58 is not shown but keeps its DMA;
- a sprite X write takes effect from the cycle's pixel 4 (`ss-xpos`);
- sprite crunching: $D017 cleared in cycle 15 with the expansion flip-flop
  clear sets MCBASE = ($2A & (MCBASE & MC)) | ($15 & (MCBASE | MC)) and
  skips the step of cycle 16 (`spritecrunch`; fixes the digits in the upper
  border of lft's demo "Nine");
- the Y expansion flip-flop turns in cycle 55 after the CPU's write
  (`spritecrunch2`);
- the display logic sees a bad line one cycle after BA (display state from
  the next g-access), a c-access needs BA low since phi1, and a bad line
  made after cycle 54 no longer switches the display on (`dmadelay`,
  `colorfetchbug`, `fldscroll`);
- in the three cycles after BA falls a c-access takes its colour from the
  low nibble of the CPU's data bus; a CPU stopped by RDY keeps repeating its
  read (`Cpu6502::stalled_read`), so it is the data of that read
  (`colorfetchbug`).

CIA (`chips-cbm/src/cia.rs`), 98 → 112: TOD digits as binary counters
with carries by comparison, AM/PM flip on writing 12, clock stopped at
power-up and reset, hours relatch only after tenths, divider reset only on
restarting a stopped clock, 0–6 divider, alarm on the comparator's rising
edge.

Still failing and looked at: `VICII/videomode` (mode-change pixels per
combination, references marked unsafe), `VICII/fldscroll` (the lower part;
the readme of `dmadelay` describes VMLI as a 40-bit shift register), the
8565 variants of the multicolour splits in `VICII/spritesplit`, `CIA/tod/hzsync6` (one line-clock tick
longer than the reference), the old-6526 ICR timing of `ciavarious` 3/3a/4/8
and `cia-timer-oldcias`, `CIA/shiftregister/cia-sdr-icr`.
