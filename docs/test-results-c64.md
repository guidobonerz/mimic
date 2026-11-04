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
| VICE test programs (`c64-testlist.in`) | CIA, CPU, VIC-II, SID, interrupts, drives, expansions | 1173 / 1240 projected (62 failed, 5 errors, 303 skipped; full run 1163 on 2026-11-04, then `C64/` and `general/` runs) | 94.6 % | 48.8 min |

Trend: VICE test programs 1163 → 1173 projected (runs of `C64/` and `general/` on 2026-11-04 with the power-on RAM pattern, the port write and the autostart changes below: `C64` 60 → 68, `general` 321 → 323, none newly failing). Before that: 1141 → 1163 (full run 2026-11-04, confirming the projection from the runs of `drive/` and `VICII/` the same day: `drive` 26 → 45 with the VIA rules, the 1541 address decoding and the byte-ready delay below, `VICII` 226 → 229 with the graphics sequencer rules, none newly failing; every other area unchanged by the VIA users' tests). Before that: 1073 → 1141 (full run 2026-11-01: `C64` 12 → 60, `drive` 6 → 26, every other area unchanged; the per-test trend shows +0 because filtered runs had already moved its base), Lorenz 264 → 264. Before that: 1056 → 1073 (full run 2026-10-31, none newly failing: `interrupts/` 22 → 39, all of `irqdma`), Lorenz 264 → 264, Acid800 54 → 54, NES 116 → 116, Harte 6502/2A03/65C02 complete green after the CPU change. Before that: 913 → 1056 (full run 2026-10-31, none newly failing: `SID/` 14 → 155, `general` 319 → 321 alongside the VIC-II work). Before that: 902 → 913 (+11, none newly failing: `VICII/` 215 → 226,
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
| general (incl. Lorenz 2.15: 314/314) | 323 | 1 | 0 | 1 | 99.7 % |
| CPU | 115 | 0 | 0 | 5 | 100 % |
| CIA | 188 | 0 | 0 | 3 | 100 % |
| interrupts | 39 | 0 | 0 | 4 | 100 % |
| VICII | 229 | 44 | 0 | 88 | 83.9 % |
| SID | 155 | 14 | 0 | 22 | 91.7 % |
| C64 | 68 | 2 | 0 | 25 | 97.1 % |
| drive | 45 | 1 | 5 | 4 | 88.2 % |
| testbench (self test) | 8 | 0 | 0 | 0 | 100 % |
| vdrive | 2 | 0 | 0 | 1 | 100 % |
| memory-expansions | 1 | 0 | 0 | 5 | 100 % |
| REU | 0 | 0 | 0 | 115 | – |
| GEO-RAM | 0 | 0 | 0 | 5 | – |
| mouse, propmouse, keypad, testjoy | 0 | 0 | 0 | 15 | – |
| others (audio-io, c64-cpm, crtemulation, digimax, plus60k, plus256k, sfx_soundsampler, userportrtc) | 0 | 0 | 0 | 10 | – |
| **total** | **1173** | **62** | **5** | **303** | **94.6 %** |

Largest open blocks: `VICII/videomode` (27, of which 14 NTSC entries with unusable references, see
below); `drive/viavarious` is complete. The 5 errors are `drive` entries that need
P64 images or disks missing from the test data; the one `drive` failure left
is `drive-emu-check`. Skipped: tests that need hardware
or options the runner does not set up (REU and GEO-RAM tests, mice,
interactive and analyser programs).

Details per program: `target/tmp/vice-testprogs.txt`, Lorenz:
`target/tmp/lorenz.txt`.

## Changes of 2026-11-04, C64 power-on RAM, the port's RAM and autostart

C64 60 → 68, general 321 → 323.

- RAM comes up as a C64C's DRAMs leave it: four `$00`, four `$FF`,
  repeating (`machine-c64/src/memory.rs`, `power_on_ram`; the readme of
  `C64/raminitpattern` lists `00 00 FF FF FF FF 00 00` for C64C boards with
  HM50464P-15 chips, the phase differs between boards). It was all zeros,
  which Cyberload, Platoon's Freeload and Darkstar BBS take for a frozen
  cartridge. The phase puts `$00` at offsets 1 and 2 of every page: Lorenz's
  trap16/trap17 wait for the raster with the VIC banked out and read those
  bytes of RAM instead, and "Typical" needs a harmless byte at `$3FFF`.
- A CPU write to `$00`/`$01` puts the data bus's byte, the VIC-II's last
  fetch, into the RAM cell under the port, not the written value
  (`general/ram0001`, `fuxxortest/ef2-inst4a`, both reading the cells back
  through sprite collisions).
- The runner's program load also sets EAL (`$AE`) to the end of the
  program, as the KERNAL's LOAD does (`C64/autostart/basic`), and entries
  asking to be loaded as "TEST" are loaded from a disk by typing
  `LOAD"TEST",8` and `RUN` (the program goes on a generated D64 when the
  entry mounts none): their reference memory dump holds the typed file
  name (`C64/autostart/defaults`).

Left: `fuxxortest/test-fuxxored` (the combined emu-fuxxor v2 program
prints "use the real thing" although each of its tests passes on its
own; which one fails in the combination is not known), `ef-eapi`
(EasyFlash EAPI, times out), `ultimax-vic-rr` (reference fits no
documented table).

## Changes of 2026-11-04, VIC-II graphics sequencer

VICII 226 → 229/273 (`videomode1`, `-w`, `-x` on the 8565 pass, projected
from the group's run); `vicii_timing/vicii_reg_timing` goes from 147 to 30
differing pixels (8565) and 179 to 30 (6569), the other `videomode` PAL
entries from 4–17 to 1–4 pixels on the 8565, `modesplit` from 683 to 455
(8565) but 761 to 833 (6569); the interrupts group and Lorenz are
unchanged. Read off the reference pictures
line by line (`vice_diff.py`, `vice_rows.py` in the session's scratchpad:
colour indices of ours and the reference side by side) and the test's
register writes per cycle:

- A fetched byte's load point follows XSCROLL while it lies ahead; one
  that a decrease has already passed stays where it was, so the byte is
  never lost (Lines 0–3: the "/" cells keep their shear; Line 15: the last
  cell's tail with the border open). Before, such a byte was dropped.
- A write of XSCROLL moves the load point from the write cycle's pixel 4
  on (`XSCROLL_DELAY`; Lines 18/19: the last cell loads at its old point
  when the register rises in the cycle after its g-access; 3 fits as well,
  5 breaks Lines 0–3).
- The four settling pixels after MCM is switched on: the first shows
  background colour 0, the next three the multicolour reading of the bit
  pair at pixels 2 and 3 of the write cycle (`videomode1`, `-w`, `-x`: pair
  10 gives background 2; Line 17: pair 00 gives background 0); black when
  the new mode is the invalid ECM + MCM (`videomode-z`). Why the chip
  shows that pair is not known; the rule is the reading that fits all
  five pictures.
- For the g-access address on the older types (6569, 6567) only BMM's
  switch-off reaches the access one cycle late; ECM changes the next access
  either way (Lines 17–19: BMM alone, ECM alone, both). The HMOS-II types
  keep the whole old mode for the access after the write.

Tried and dropped: a black pixel where an invalid mode ends (fits
`videomode2` and `-v` on the 8565 and the a5/ff variants of
`vicii_reg_timing`, but `videomode-w`, `-x` and `-z` show the data bit
there); the 6569's repeated bit pair after MCM is switched off at pixels 2
and 3 instead of 0 and 1 (fits `videomode1` and `-z` on the 6569 coming out
of the invalid mode, costs every other 6569 entry). Left in the 8565
`videomode` entries: one pixel each in `videomode2` and `-v`, the four
settling pixels of BMM switched off under MCM in `-y`; the 6569 entries
are 2–82 pixels off, mostly at the same transitions.

Left in `vicii_reg_timing`: single pixels at the last cell of Line 17 and
the BC0 read-modify-write lines 20/21, where two identical lines of the
reference (same writes, same data) show different pixels; and in the a5/ff
variants the first pixel after the invalid ECM + BMM mode ends, which the
reference shows black. A rule for that pixel helps those variants but costs
the plain one, `videomode` and the 6569, so it is not in. Note: the
`videomode` readme (SVN of December 2025) says its PAL references were
matched against a breadbox 6569 and a C64C 8565; they are no longer
"unsafe".

## Changes of 2026-11-04, VIA 6522, the 1541's address decoding and byte ready

drive 26 → 45: all of `drive/viavarious` (via1–via21),
`drive/interrupts/timera`, both `drive/openbus` programs and
`drive/hls-protection`.

Byte ready comes one cycle after the eighth bit enters the shift register
(`BYTE_READY_DELAY` in `drives-cbm/src/c1541.rs`). The HLS protection check
reads a byte, polls SYNC about 19 cycles after byte ready and expects to
catch every data-block sync (its byte before the sync is `$AF`, four sync
bits already in the latch, the tenth due 6 bit cells = 19.2 µs on the
test's dense track later) while never seeing a header sync (two bits in the
latch, the tenth 25.6 µs later). With byte ready at the shift itself the
poll landed a cycle early once in sixteen rows (the bit stream is not locked
to the CPU clock in the emulation); 1 or 2 cycles satisfy both bounds on
tracks 17 and 19, the smaller is kept. Found through `dump_hls_test` in
`tests/tests/via_dump.rs`, which prints the drive's 16 result rows next to
the expected ones.

The 1541 decodes A15, A12 and A11 only (`drives-cbm/src/c1541.rs`,
`region`): RAM where A12 = A11 = 0, the VIAs where both are 1 (A10 picks
one), nothing where they differ, so `$0800–$17FF` is open bus and RAM and
VIAs repeat every `$2000`. The open bus returns the last byte that was on the
data bus, which the test reads back through an indexing dummy cycle and the
high byte of an absolute or indirect address (`openbus`, `openbus2`; the
author's 1541 and OC118, not a 1570/1581). Before, RAM was mirrored up to
`$17FF` and `$2000–$7FFF` was open. The rules were read off the reference data, which
the test author recorded on a 1541C (and checked on a 1541-II), by replaying
each sub test at the chip and comparing byte for byte
(`tests/tests/via_dump.rs` prints the drive's measurements next to
`viaNref.bin`; a temporary chip-level probe gave the same runs):

- T1 reloads its latch after a timeout in one-shot mode as well, so with
  latch 0 or 1 the counter reads 0 or 1/0 for ever instead of counting down
  freely (via1 tests A/B, via2); only the flag and PB7 stay quiet after the
  first timeout.
- ACR bit 5 (T2 counts PB6 pulses) reaches the counter a cycle late in both
  directions: after the switch to the clock the first decrement comes two
  cycles after the write, after the switch back one more cycle counts (via1
  test G: the first read 6 cycles after the write shows −5; via9).
- The T1 output flip-flop (PB7 with ACR bit 7) rests high after reset, goes
  low on a write of the counter's high byte only while ACR bit 7 is set,
  toggles at a timeout while ACR bit 7 is set and is left high by a timeout
  with it clear; with ACR bit 7 set a read of port B shows the flip-flop in
  bit 7 whatever DDRB says (via10–via14: a timer started before PB7 is
  switched on reads 1 until it times out, started after it reads 0 first).
  The "rests high after a timeout with PB7 off" part is the reading that
  fits all five programs (the drive's reset code and `setdefaults` run T1
  with PB7 off between sub tests); a write of ACR clearing bit 7 would fit
  them as well.
- With the shift register clocked by T2 (ACR modes 1, 4, 5) T2 counts in
  8-bit mode: the low byte reloads from its latch when it underflows, the
  high byte counts those reloads, and its own underflow ($0000 → $FFFF)
  sets the T2 flag (via20/via21: T2 = $0114 flags after two low-byte
  periods, $0101 within 6 cycles).

Checked unchanged: PET, VIC-20, C128 (1571/1581), Mac and Vectrex unit and
boot tests (the Vectrex draws through PB7/T1, Mine Storm plays).

## Changes of 2026-11-01, cartridges and drive

C64 12 → 60 and drive 6 → 26; all 48 cartridge errors are gone.

Cartridges (`devices-cartridge`): Action Replay (1), Atomic/Nordic Power (9),
Retro Replay and Nordic Replay (36, by subtype) with `$DE00`/`$DE01`, and
Pagefox (53). The rules come from the test programs and their readmes:

- Bits 1, 2 and 6 of `$DE01` can be written once until reset.
- Mode `$22` differs by cartridge: on the Action Replay cartridge RAM sits
  beside C64 RAM and a read gives the OR of the two; on the Nordic cards ROM
  reads at `$8000`, cartridge RAM at `$A000`, and writes there miss C64 RAM.
- The Pagefox decodes `$8000–$BFFF` itself, also with ROM banked out.
- In Ultimax mode the VIC-II sees RAM at `$1000`; the Retro Replay leaves the
  VIC-II its normal map (`ultimax/readme.txt`, first RR table).

Simplified: the freeze button pulls NMI and switches to Ultimax directly, flash
writes and the flash jumper are missing. The C128 has none of the new bus hooks
(`write_under`, `blocks_ram_write`, `drives_over_ram`), so these cartridges work
there only as far as plain banking goes. Final Cartridge III and Super Snapshot
stay open, and no frontend or debug command calls the freeze button yet.

Drive (`drives-cbm/src/c1541.rs`, savestate version 3, version 2 still loads):

- The head reads bit by bit. A revolution takes 200 ms whatever a track holds,
  so a long or short G64 track simply runs denser or thinner.
- Sync is ten 1 bits. The write shift register takes port A at byte ready, and
  leaving write mode cuts off the byte in it (`drive/rpm`).
- G64 images with half tracks insert and write back.
- The head keeps its angle over a track change. D64 tracks are rotated by
  `TRACK_SKEW`, 777 bits, which is this project's choice and not a measurement.
- IEC lines from the C64 reach the drive 1 µs late, which fixed
  `drive/selftest`, the scanners and many of `viavarious`.

Still open in `drive`: `viavarious` 15, `openbus` 2, `interrupts/timera`,
`inertia/drive-emu-check`, `hls-protection/hlstest.g64` (not examined).
Errors: `rpm1`–`rpm3` need P64, which is not implemented: no specification
turned up, and deriving the format from VICE would break golden rule 7.
`1541-testsuite` needs `disks/pc64_1.d64` and `pc64_2.d64`, which are not in
the test data and must be built with tools that are missing there.

Still open in `C64/carts`: `ef-eapi/test-eapi.crt` (timeout, red before too) and
`ultimax/ultimax-vic-rr.crt` (about 3500 pixels). Its reference fits neither RR
table of the readme exactly and differs from the 8K reference in rows the
mode should not touch; the readme says more real cartridges need testing.

Speed: 6000 C64 frames take 54.9 s before and 54.3 s after this block.
`cargo bench -p cpu-6502` against 8cedd61 (before the `stall_off_bus` change):
before 9.83 and 10.22 ms, after 10.31 and 10.26 ms per million cycles. The same
code moved 4 % between its two runs, so no change can be told from that noise.

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
older states take the current P). Speed: see the cartridges and drive section;
an earlier figure here (37.4 s against 37.5 s) came from an outdated binary and
was wrong.

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
