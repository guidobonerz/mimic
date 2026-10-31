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
| VICE test programs (`c64-testlist.in`) | CIA, CPU, VIC-II, SID, interrupts, drives, expansions | 1073 / 1240 (106 failed, 61 errors, 303 skipped) | 86.5 % | 50.2 min |

Trend: VICE test programs 1056 → 1073 (full run 2026-10-31, none newly failing: `interrupts/` 22 → 39, all of `irqdma`), Lorenz 264 → 264, Acid800 54 → 54, NES 116 → 116, Harte 6502/2A03/65C02 complete green after the CPU change. Before that: 913 → 1056 (full run 2026-10-31, none newly failing: `SID/` 14 → 155, `general` 319 → 321 alongside the VIC-II work). Before that: 902 → 913 (+11, none newly failing: `VICII/` 215 → 226,
projected from runs of `VICII/`, `interrupts/` and Lorenz 264 → 264 on 2026-10-31;
no full run yet). Before that: 874 → 902 (+28, none newly failing: all of
`CIA/shiftregister/cia-sdr-icr`; last full run 898, the last 4 confirmed with `CIA/`,
`interrupts/` and Lorenz 264 → 264 only). Before that: 850 → 874 (+24, none newly failing: all of
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
| general (incl. Lorenz 2.15: 314/314) | 321 | 3 | 0 | 1 | 99.1 % |
| CPU | 115 | 0 | 0 | 5 | 100 % |
| CIA | 188 | 0 | 0 | 3 | 100 % |
| interrupts | 39 | 0 | 0 | 4 | 100 % |
| VICII | 226 | 47 | 0 | 88 | 82.8 % |
| SID | 155 | 14 | 0 | 22 | 91.7 % |
| C64 | 12 | 10 | 48 | 25 | 17.1 % |
| drive | 6 | 32 | 13 | 4 | 11.8 % |
| testbench (self test) | 8 | 0 | 0 | 0 | 100 % |
| vdrive | 2 | 0 | 0 | 1 | 100 % |
| memory-expansions | 1 | 0 | 0 | 5 | 100 % |
| REU | 0 | 0 | 0 | 115 | – |
| GEO-RAM | 0 | 0 | 0 | 5 | – |
| mouse, propmouse, keypad, testjoy | 0 | 0 | 0 | 15 | – |
| others (audio-io, c64-cpm, crtemulation, digimax, plus60k, plus256k, sfx_soundsampler, userportrtc) | 0 | 0 | 0 | 10 | – |
| **total** | **1073** | **106** | **61** | **303** | **86.5 %** |

Largest open blocks: `VICII/videomode` (27, of which 14 NTSC entries with unusable references, see
below), `drive/viavarious` (19); the 48 errors under
`C64` are cartridges of types not supported yet (36 Retro Replay, 1 Action
Replay, 9 Nordic Power, 53). Skipped: tests that need hardware
or options the runner does not set up (REU and GEO-RAM tests, mice,
interactive and analyser programs).

Details per program: `target/tmp/vice-testprogs.txt`, Lorenz:
`target/tmp/lorenz.txt`.

## Changes of 2026-10-31, interrupts during DMA

`interrupts/irqdma` 2 → 19 of 19. The machine did not call the CPU while the
VIC-II held AEC low (from the fourth BA cycle), so no interrupt was recognised
during sprite DMA. `Cpu6502::stall_off_bus` now does that polling; the C64 and
C128 call it in their `!aec()` branch. Measured by comparing the 16 384 value
pairs of `record*.prg` with the real C64's reference embedded in `test*.prg`.
The rules below are derived from those data, not from a chip description.

While RDY holds a cycle, the CPU keeps recognising interrupts as it would at
the end of that cycle, with these exceptions:

- no polling while the opcode fetch is held (polling there breaks test 3);
- none during BRK or an interrupt sequence;
- none in the last cycle of a taken branch (test 5);
- SEI and CLI held in their last cycle already count with the new I flag,
  PLP does not (`saved.p` for opcode $28).

`stall_p` holds the flags used for this and is saved (CPU savestate version 5;
older states take the current P). The speed of the C64 is unchanged (6000
frames 37.4 s before, 37.5 s after, three runs each).

## Changes of 2026-10-31, SID

`chips-cbm/src/sid.rs`, digital part rebuilt (filter and mixer unchanged),
14 → 155 of 169:

- envelope: the measured rate periods, the 15-bit rate counter with its wrap
  (ADSR delay bug), the exponential divider latched at its level marks, level 0
  frozen until the next attack; delays found by scanning against `env_test`
  (attack step 3 cycles after a rate match, decay/release 4; gate on 1, gate off
  2, 4 with a step under way; one rate compare with the decay rate after gate
  on; a step under way that went down stays down);
- noise: output bits from register bits 20, 18, 14, 11, 9, 5, 2, 0; two-phase
  clock; combined noise writes back in every cycle except the latching one, not
  only while shifting (the readme of `noisewriteback` says otherwise, the
  reference sequences of `wb_testsuite` need this); on releasing the test bit
  the selector reaches the latch only through tri/saw used by both settings
  (6581) or by each of them (8580); running into the test bit with tri/saw
  clears the output bits (`noiselfsrinit` against `wb_testsuite`);
- oscillator: power-up value $555555, shift register bit 0 low; the 6581's
  adder MSB pulled by combined waveforms with sawtooth; the 8580's output
  register one cycle behind for triangle and sawtooth; ring modulation as in the
  readme's TriXOR expression; no waveform holds the output, then it leaks;
- reads of read-only registers refresh the bus value, which leaks too.

Still failing (14): `env_test_adra_1`, `adra_2`, `ar_2`, `resid-test/envdelay`
(one cycle: it asks for 3 cycles to a release step, the env_test family fits 4,
no setting satisfies both), `envsample`, `wf12nsr` (both chips), and 7
`wb_testsuite` cases (6581 D→C, D→E, E→C, F→C, which the source marks "seems
unstable", 8580 C→9, C→E, C→F). Combined waveforms stay the AND of their parts;
no test here requires more (`oscsample` judges the pure waveforms only), but
they sound and read wrong on real software. The hold times of the bus, the
floating output and the 8580's shift register fade are not measured.

## Changes of 2026-10-31, VIC-II

`chips-cbm/src/vic2.rs`, 215 → 226 of 273:

- a sprite row being shifted out when the display turns off in cycle 58 runs
  to its end (`border/hvborder1`, `hvborder2`, `sb_sprite_fetch/sbsprf24-163`);
- without DMA a sprite's data latches take the byte of a CPU access to a VIC-II
  register in the φ2 of their slot (`sbsprf24-164`, readme);
- the vertical border flip-flop stops the sequencer's loads as it was at the
  left edge: set later, the line is still shown (`border/vborder2-36`);
- a sprite shows nothing in the pixel before its p-access (`spritegap2`,
  `spritegap3`, logs of real chips; the hole also applies to a running row,
  which those logs do not separate);
- the g-accesses of cycles 16 and 17 stay at $3FFF on a fresh bad line, later
  ones use $38FF/$3807 (`colorfetchbug/main`; the limit fits this test and
  `vsp-tester`, it is not a measured signal);
- the raster compare in cycle 1 uses the compare value from the start of the
  previous cycle: a $D012 write in the last cycle of a line is too late
  (`rasterirq/rasterirq_hold`);
- XSCROLL is compared at the pixel a byte is due, not fixed at the g-access
  (`border/border-bm-ysh2`, fewer differences in `vicii_timing`);
- mode changes reach the output late, in the cycle of the write: ECM and BMM
  four pixels on the 8565/8562, two when switched off on the 6569/6567; MCM
  four black pixels in a multicolour text cell when switched on, three pixels
  of high-resolution pairs in a multicolour cell when switched off. The next
  g-access still uses the old ECM/BMM (on the older types only when switching
  off). `videomode-z` passes on the 8565; the other PAL `videomode` tests are
  left with 4-83 pixels, `vicii_reg_timing` with about 150-190. These rules
  are read off references the readmes call hand-made and partly dependent on
  chip and temperature, not from a measured signal. Holding MCM for four pixels
  with ECM or BMM on made things worse and was dropped.

The 14 failing NTSC `videomode` entries (`videomode1`, `2`, `-v` … `-z`
`_ntsc.prg`, 6567 and 8562) cannot pass: their references do not belong to the
programs in the suite. The programs set $D022/$D023 to 7/5 and colour RAM to
$0E on PAL and NTSC alike, the references show 1/2 and, in `videomode1` and
`videomode2`, colour 6 for the text. The PAL references were matched against
real machines in December 2025 (readme), no NTSC reference is listed there, and
there is no `-8562` picture, so the 8562 entries compare against the same
picture as the 6567. Patching a scratch copy of the programs to those colours
leaves 4-21 pixels on raster lines 116 and 121 (`videomode1_ntsc` passes on the
6567), `videomode2_ntsc` about 970, in whole lines of other colours. The
emulation was not changed for these tests: fitting it to pictures of another
program version would prove nothing.

## Changes of 2026-10-30, CIA

`chips-cbm/src/cia.rs`, 160 → 188 of 188, checked at the chip for every timer
A latch 0-127 on both models against the reference of
`cia-sdr-icr-v3.asm` and `cia-sdr-icr-test2.asm` (`generate.c` differs from the
`.asm` in phase 1):

- a byte waiting in the SDR is dropped when SP changes direction;
- an underflow in the two cycles after an SDR write finds the shifter idle
  (this replaces the rule that restarted the end of a byte at latch 2);
- the internal signal also runs out during a byte's end, and the last
  half-bit's underflow clears it after two cycles;
- switching to input in the middle of a byte sets the SP flag, from four cycles
  after the first half-bit on.

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
byte never completes). The rest is under "Changes of 2026-10-30, CIA".

Still failing and looked at: `VICII/videomode` (mode-change pixels per
combination, references marked unsafe), `fldscroll-20`, the 8565 variants
of the multicolour splits in `VICII/spritesplit`,
`spritex/demusinterruptus`, `spritevssprite`, `spritefetchbug`,
`vicii_reg_timing`, `modesplit`, `fetchsplit`, `spritescan`.

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
