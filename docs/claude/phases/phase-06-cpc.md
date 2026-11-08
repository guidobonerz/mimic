# Phase 6: Amstrad CPC

Part of the project's instructions for Claude Code; the core is `CLAUDE.md` at the repository root, the index of these files is its section "Further documentation".

| # | Phase | Est. | Gate |
| --- | --- | --- | --- |
| 6 | CPC 464, 664, 6128 (models of one machine): reuse Z80, CRTC, Gate Array, AY, FDC, DSK/CDT | 6 wk | BASIC + disk work |

## Result

Phase 6 done 2026-10-21: CPC 464/664/6128 boot, tape and disc work with real ROMs; Arnold acid tests 90/98 (91.8 %, CRTC type 1, 2026-11-02; 66/70 before `crtctest` and `disc/fdc` counted) in the 10 programs that report results; port B reads &5E on the 664/6128 and &7E on the 464 (values the author recorded in `ppi.asm`); still failing PPI mode 1 port C output latching (two tests the author's type 0/1 CPCs fail too), "port b read (vsync on)" (the test expects a constant port while the 16-line VSYNC comes and goes; its source marks this "todo: fix, vsync gets in the way"), and in `crtctest` (runs on the 664, $FF to port &4000 would bank the 6128's RAM over it) "R8 - count lines" (2 values of 32 one line short: the loop samples 25-33 µs after VSYNC starts; moving the half line to 32 or 31 µs only moves which 2 values fail, so the half line alone does not explain it) plus 4 tests that fail on a real type 1 too: I/O decode (the test reloads BC before recording the port), both "vsync r8" and "htot/2" (their comments give type 1's values, which we match). `crtctest` on the other types (2026-11-02): type 0 18/22 (324 000 fields), type 2 18/23. Built from the test's tables: type 0 interlaces with R8 = 3 only (rows end at R9, the next starts on the other parity, no half line) and counts one row in the vertical adjust; type 2 starts no VSYNC with HSYNC on in a line's last character. Left on both: I/O decode (the test bug above), both "vsync r8" (one table for all types, the source does not say from which chip; r8=3 on type 0 fails on more lines since its interlace is in, the test assumes full row heights) and "htot/2" (only type 1's result is noted, we match that); on type 2 "R8 - count lines" compares with type 3's table, the notes say type 2 hangs there. Type 2 interlace is not modelled. Type 1 compares `VLC >= R9` and `VCC >= R4` (inferred from the demo From Scratch by CNGSOFT, whose logo plasma needs them: with equality VCC ran on to R7 = 61, a second VSYNC halved the field and the music doubled; suite unchanged at 90/98; other register-lowering software unchecked). Compared with WinAPE 2.0 Beta 2 on 2026-11-02 (`crtctest` patched to its vsync r8, htot/2 and R8 tests, plain 6128, turbo off, which changes Z80 timing): htot/2 is identical on all three types (280 values); on type 2 all five tests are identical; WinAPE models no interlace on any type (R8 table: 99 at R8 = 1 and 3), so where it differs on types 0 and 1 mimic is the one matching the test's real-chip table or the note "type 1 fails all of these with 0s". The comparison gives no fix. In that run the type 1 R8 error moved to another entry (91 got 90 instead of 81 got 80, later 89 got 88): it depends on the program's timing phase.

## Device, freezer and CRT work in this phase

- Device work by phase (part of each gate): **Phase 5/6:** Atari joystick/paddles/SIO drives; CPC dual joystick, AMX mouse, FDC drives.
- CRT work by phase: **Phase 5/6:** Atari NTSC artifact colors (gate includes an artifacting test image); CPC CTM644 and GT65 profiles.

## Backlog detail

- **CPC 464 and 664** alongside the 6128 in Phase 6 (see the phase table).
- **Amstrad CPC Plus** (Phase 19; 464+/6128+/GX4000): ASIC (hardware sprites, DMA sound, split screen, 4096-colour palette, soft scroll), CRTC types 3/4, CPR cartridges; as models of `machine-cpc` after Phase 6.
