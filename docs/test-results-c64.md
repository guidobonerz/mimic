# C64 test results

Run on 2026-10-28, VICE test programs and Lorenz with real ROMs again on
2026-10-30 after the VIC-II and CIA work (release build, user ROMs in
`roms/`, test data from `tools/fetch-testdata.sh` and
`tools/fetch_vice_testprogs.py`).

## Overview

| Suite | What it checks | Result | Pass rate | Run time |
| --- | --- | --- | --- | --- |
| Tom Harte SingleStepTests `6502` (all, `EMU_HARTE_FULL=1`) | NMOS 6502 core of the 6510: every opcode incl. undocumented ones, registers, memory, every bus cycle | 2 560 000 / 2 560 000 | 100 % | 16 s |
| Klaus Dormann 6502 functional test | all documented instructions, flags, decimal mode | passed | 100 % | 1 s |
| Lorenz 2.15, real ROMs (`lorenz_suite`) | CPU, CIA, VIC-II timing on the C64 | 264 / 264 | 100 % | 9.4 min |
| Lorenz 2.15, stub ROMs (`lorenz_suite_stub`, CI) | the same without ROMs (3 tests need real ROMs) | 261 / 261 (3 skipped) | 100 % | 4.4 min |
| VICE test programs (`c64-testlist.in`) | CIA, CPU, VIC-II, SID, interrupts, drives, expansions | 874 / 1240 (305 failed, 61 errors, 303 skipped) | 70.5 % | 36.3 min |

Trend: VICE test programs 850 → 874 (+24, none newly failing: all of
`CPU/sha`, `CPU/shs`, `CPU/shxy`, `CPU/ane`; every other area unchanged), Lorenz 264 → 264 and Acid800
54 → 54 on 2026-10-30 with the CPU change. Before that: 840 → 850 (+10 since the previous run of
2026-10-30, none newly failing: `CIA/ciavarious` cia3, cia3a, cia4, cia8,
`cia-timer-oldcias`, `dd0dtest` on both CIA models, `CIA/tod/hzsync6`,
`interrupts/cia-int` irq-new and nmi-new; 680 → 850 since 2026-10-28); Lorenz
with real ROMs 264 → 264 (no change). The
Harte (every 10th test, all green after the CPU change) and Lorenz stub-ROM rows
are from 2026-10-28. The VICE test programs were at 676 / 1240
on 2026-10-26.

## VICE test programs by area

| Area | Pass | Fail | Error | Skip | Pass rate (pass / pass+fail+error) |
| --- | ---: | ---: | ---: | ---: | ---: |
| general (incl. Lorenz 2.15: 314/314) | 319 | 5 | 0 | 1 | 98.5 % |
| CPU | 115 | 0 | 0 | 5 | 100 % |
| CIA | 160 | 28 | 0 | 3 | 85.1 % |
| interrupts | 22 | 17 | 0 | 4 | 56.4 % |
| VICII | 215 | 58 | 0 | 88 | 78.8 % |
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
| **total** | **874** | **305** | **61** | **303** | **70.5 %** |

Largest open blocks: `SID/wb_testsuite` (110 failing, SID waveform
sampling), `CIA/shiftregister` (28, `cia-sdr-icr`),
`VICII/videomode` (28), `drive/viavarious` (19), `interrupts/irqdma`
(17); the 48 errors under
`C64` are programs that do not finish. Skipped: tests that need hardware
or options the runner does not set up (REU and GEO-RAM tests, mice,
interactive and analyser programs).

Details per program: `target/tmp/vice-testprogs.txt`, Lorenz:
`target/tmp/lorenz.txt`.

## Changes of 2026-10-30, CPU

`cpu-6502`, 91 → 115 of 115: when RDY holds the cycle before the write,
SHA/SHX/SHY/TAS store the value without `& (H+1)`, while a crossed address
keeps it (`CPU/sha`, `CPU/shs`, `CPU/shxy`). ANE: constant $EF on the C64 and
C128 (the plain NMOS default stays $EE for Harte's data), bit 0 clear when RDY
held the operand read (`CPU/ane`). Savestate version 4. The check runs only
on the RDY path of `tick`, so the normal cycle costs nothing extra.

## Changes of 2026-10-28/29

VIC-II (`chips-cbm/src/vic2.rs`), 112 → 215:

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
  (`colorfetchbug`);
- collisions of a cycle's last four pixels reach $D01E/$D01F one cycle
  later (`spritecollisions/*-collision-cycle`);
- a sprite row fetched in this cycle is available from pixel 3
  (`spritex/testsuite`, Krestage 3);
- light pen: CIA 1 PB4 driven low pulls LP; the edge is sampled in the
  next cycle with that cycle's X (8565/8562: minus 2) and raster line;
  edges in the last line and up to cycle 2 of line 0 are lost, and a line
  held low into the frame latches in cycle 2 of line 0; RASTER reads the
  last line in cycle 1 of line 0 (`lp-trigger`, `lplatency`,
  `split-tests/lightpen` with its dumps of real chips; the `fldscroll`
  programs sync on the light pen);
- 6567R8: sprite fetches at 59/61/63/65/2/4/6/8 (one cycle before Bauer's
  diagram; measured by `phi1timing`), sprite DMA check in cycles 56/57;
  a sprite is not triggered in its own p-access cycle (`spritex` NTSC);
- the idle access of the cycle in which a bad line is made in idle state
  (DMA delay) reads $38FF (6569/6567R8) or $3807 (8565/8562)
  (`vsp-tester`, measured on real machines). This costs
  `colorfetchbug/main` one cell in line $30, whose reference picture x64sc
  made without that effect;
- without DMA a sprite's data latches take the bus in its slot ($FF, the
  idle byte, $FF), shown by a sprite at X >= $164 in the line its DMA
  starts (`sb_sprite_fetch` readme; `sbsprf24-164` 11 -> 7 pixels).

Mode changes in the middle of a line (`videomode`, `modesplit`,
`vicii_timing`, 38 of the 58 failing VIC-II entries) show transition
pixels that depend on the multicolour pair phase; their references were
crafted by hand ("unsafe reference"), so no model was fitted to them.

CIA (`chips-cbm/src/cia.rs`), 98 → 160: TOD digits as binary counters
with carries by comparison, AM/PM flip on writing 12, clock stopped at
power-up and reset, hours relatch only after tenths, divider reset only on
restarting a stopped clock, 0–6 divider, alarm on the comparator's rising
edge. Serial shift register: the last half-bit of a byte takes no timer
underflow of its own; four cycles after the 15th underflow CNT rises, the
interrupt flag is set and the next byte can start at the next underflow
(`cia-sdr-init`, `cia-sdr-delay`, `cia-sdr-load` match the real-hardware
values of their readme exactly). For phase 2 of `cia-sdr-icr` (the shifter
switched to input and back to output with state left over) the CIA keeps an
internal output signal: a timer A underflow with CNT low sets it one and
three cycles later and clears it after two, an underflow with CNT high or
an idle one clears it after four, the last half-bit sets it after three; on
switching SP to output a set signal sets SP. A further underflow while the
finished byte waits restarts the four-cycle wait once (with latch 0 the
byte never completes). Phase 1 matches the reference for every latch
checked, phase 2 for latch 19, 37, 39; single values still differ at latch
1, 2, 40, 64 and 100 — stops at the end of the first half-bit, where the
reference generator (`generate.c`) assumes SP = 0 — so 28 `cia-sdr-icr`
entries (the `*_7f` ones stop at those values) still fail.

Still failing and looked at: `VICII/videomode` (mode-change pixels per
combination, references marked unsafe), `fldscroll-20`, the 8565 variants
of the multicolour splits in `VICII/spritesplit`,
`spritex/demusinterruptus`, `spritegap`, `CIA/shiftregister/cia-sdr-icr`.

ICR reads, from `cia-timer` (dumps of real chips), `ciavarious` and
`dd0dtest`. Old 6526: a read in the cycle before a Timer B underflow loses
the flag, the interrupt still comes and reads as $80; a read right after one
that returned an enabled flag shows bit 7 alone; a source masked in the cycle
of its flag by the second of two writes in a row (`INC $DD0D,X`, test 11)
raises no interrupt, while a single write there still does
(`interrupts/cia-int`, reference of a real 6526; only these two tests separate
the cases, so the two-write condition is inferred from them, not a measured
signal). 6526A: a read right after a read sees the
same flags again, and the output comes a cycle late when the flag is set right
after a read. `CIA/tod/hzsync6`: the TOD divider is a ring of six states, so
switching to 50 Hz in the sixth makes a tenth of 6 + 5 ticks. `dd0dtest`
checks these through `INC $DD0D,X`; its readme says the tests were not written
for the new model cycle by cycle, so the 6526A rule is the one that fits the
tests' numbers, not a measured signal.
