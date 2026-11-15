# Phase 25: Robotron KC85/3 and KC85/5

Part of the project's instructions for Claude Code; the core is `CLAUDE.md` at the repository root, the index of these files is its section "Further documentation".

| # | Phase | Est. | Gate |
| --- | --- | --- | --- |
| 25 | Robotron KC85/3 and KC85/5 (`machine-kc85`, added 2026-11-01 on the user's request; numbered last, not by effort): U880 (Z80 clone, reuse `cpu-z80`), IRM video RAM with pixels and colour attributes, CTC and two PIOs (Z80 family peripherals, new), keyboard over its serial remote-control protocol, cassette (KCC/TAP images), expansion modules (M-series slots), memory banking; clarify the exact clocks, RAM sizes and the floppy unit D004 when the phase starts; CAOS and BASIC ROMs user-supplied | 6 wk | both models boot to the CAOS menu with real ROMs; a cassette program loads |

## Status

Begun 2026-11-15 on the user's request ("the ROMs are there": `~/.mimic/roms/kc85/`, KCemu's ROM set: CAOS 2.2 (HC900), 3.1, 4.2, 4.4, BASIC, the D004's floppy ROMs, module ROMs M006/M012/M026/M027/M033 and more). No questions asked at the start; these choices are Claude's, not the user's: all four models (KC85/2, /3, /4, /5; the phase named /3 and /5) since the ROMs are there and the /4 is the /5's base; default model "4"; the D004 floppy unit left out; cassette playback first, saving later.

**The gate is met (2026-11-15):** `tests/tests/kc85_boot.rs` (ignored, real ROMs): KC85/2, /3, /4 and /5 boot to the CAOS menu; a BASIC program typed through the keyboard's pulses runs on the /3, /4 and /5; LOAD reads a three-block KCC program from the cassette on all four models and starts it.

Built:
- `chips-zilog` (new crate): Z80 CTC (timer and counter mode, prescaler, trigger start, a running channel takes a new constant at zero), Z80 PIO (modes 0 to 3, strobe and bit-mode interrupts), the interrupt daisy chain (`Chain`: request, in service, RETI). The machine sees the RETI by looking at the bytes at PC before the instruction (`ED 4D`); `cpu-z80` is unchanged.
- `machine-kc85`: memory switching by PIO A/B and ports `$84`/`$86` (RAM0, RAM4, RAM8 blocks, IRM planes, CAOS C and E, BASIC, the /5's USER ROM segments), the IRM picture line by line (the /2's and /3's spread addresses with a colour byte per 8 × 4 pixels, the /4's column layout with a colour byte per 8 × 1 and the four-colour mode), blink through CTC 2, two sound channels through CTC 0 and 1 with PIO B's volume, the keyboard's pulse protocol (timed by CAOS with CTC 3), cassette playback (KC-TAP and KCC; a strobe at PIO A per zero crossing), two module slots (`slot8`, `slotc`: M022, M011, M006, M012, M026, M027, M033), a stub ROM, savestates, `typed_keys` (paste and `type`), a host keymap.
- Read from the user's ROMs (CAOS 4.2 and 3.1 disassembled with the test file's `disassemble` tool, nothing taken from another emulator): the keyboard's pauses and key numbers (interrupt routines `$E339`/`$E345`, key table KTAB), the tape's half-wave lengths and the lead the load routine needs (`$E56B`, `$E58E`), the inverted volume bits (`$F92E`).
- Tried by hand (headless): M026 Forth on the /4 (`SWITCH 8 C1`, FORTH appears in the menu); M006 on the /2 (`JUMP C` starts the module's own HC901 CAOS with BASIC).

Findings:
- CAOS 3.1 takes six seconds to its menu (it clears memory and prints the screen clear), CAOS 4.x two. The KC85/3's menu shows an empty `%` line behind REBASIC: the BASIC ROM holds the bytes `7F 7F 00` at `$D9C4` (constants), which the menu takes for a word.
- The PIO's output register must be loaded before the port becomes an output: the lines switch the memory the moment the mode is set.
- CAOS 3.1's keyboard routine has no limits on the pauses: a key that starts less than 20.6 ms after the last word is read as part of it. The emitter here keeps 25 ms of silence between keys (whether the real U807 does is not checked).
- Typing is blind (see the debugger notes): BASIC's start takes longer than the pause behind ENTER, so text sent right behind `BASIC` loses its first line.

Not checked against hardware or documents, taken from memory of the system manuals: the colour values, the picture's timing inside a line (the picture is drawn at each line's end; nothing is delayed against the CPU), the PIO's lines while it is an input after RESET (all high), the priority of modules against each other, the KC85/5's RAM8 block numbers, the M011's block rotation and the M033's segment bit. The frame is 312 lines of 112 (KC85/2, /3) or 113.5 clocks.

Open:
- Saving to tape (CAOS writes with CTC 1; the recorder only plays).
- The D004 floppy unit (its own Z80, FDC and CP/M; ROMs at hand), the M003 V.24, M001 digital I/O, joystick modules, further ROM modules (M900/M901 WordPro and the others in the folder).
- Real software from the user's tapes; a picture test against a real machine or screenshots; the tape's sound on the loudspeaker; the BASIC debugger for HC-BASIC (Phase 35).
- The desktop and web frontends run the machine through the registry but were not tried by hand.

User documentation (2026-11-15, on the user's request): `docs/usage.md` (ROM files, models, options, media, keys), the user guide (`tools/guide_de.py`, `guide_en.py`: a "Robotron KC85" section, the machine and ROM tables; PDFs rebuilt) and the README gallery: `docs/screenshots/kc85-4.png` (the KC85/4's CAOS 4.2 menu, 200 fields) and `kc85-basic.png` (HC-BASIC on the KC85/4, a typed program printing the 15 foreground colours on black and black on the 8 background colours: `--type 'BASIC\n\n\n\n\n10 CLS:FOR V=1 TO 15\n20 COLOR V,0:PRINT " ROBOTRON KC85/4 ";:COLOR 0,V AND 7:PRINT " HC-BASIC "\n30 NEXT:COLOR 7,1:PRINT\nRUN\n' --type-at 150 --frames 2000`); both `--crt raw` (640 × 512, the machine's picture without a border).
