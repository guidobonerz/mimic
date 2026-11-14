# Phase 35: BASIC interpreter debugger

Part of the project's instructions for Claude Code; the core is `CLAUDE.md` at the repository root, the index of these files is its section "Further documentation".

| # | Phase | Est. | Gate |
| --- | --- | --- | --- |
| 35 | BASIC interpreter debugger (added and begun 2026-11-14 on the user's request): for every machine with a BASIC in ROM the debugger lists the stored program, shows the line and statement being executed, breaks on lines and statements, steps by statement and line, over and out of GOSUBs, reads and writes variables and shows the GOSUB/FOR stack — through the native protocol and `polydbg`, without the debugger knowing a machine or a dialect | 8 wk | per machine a ROM test (`#[ignore]`) that, through the native protocol, types in a small program, stops at a line, steps by line and statement, steps over and out of a GOSUB, reads and sets a variable and checks the stack |

## Decisions (user, 2026-11-13/14)

- **Scope "full":** listing (detokenised), current line and statement, breakpoints on lines, steps by line and statement, step over a GOSUB and step out, variables read and written, the GOSUB/FOR stack. Not chosen: watchpoints on variables, conditions on BASIC breakpoints, a line trace.
- **Machines:** all with a BASIC in ROM, in milestones by dialect family, Commodore first. BASICs loaded from disk or tape (AmigaBASIC, Microsoft BASIC on the Mac, the IMSAI's 8K BASIC and MBASIC) stay out.
- **Frontends:** the native protocol and `polydbg` only. No panel in the desktop or web frontend (the team's own IDE speaks the native protocol).
- **Core trait:** `Machine::basic()` as a new method with a default (`None`), as `gfx_views` was added. Approved.

Own choices (not asked): the types and the trait `BasicDebug` in `emu-core::basic`; stop points as ordinary breakpoints the machine names and the session arms; the toy BASIC machine in `emu-core::testing::basic` for tests without ROMs; BASIC breakpoints numbered apart from `break_add`'s; a line breakpoint stops when the program enters the line (not when a loop returns into it); the first step from a machine that sits before a statement's dispatch stops at that statement; a shared crate `basic-dialects` for the decoders from M1 on.

## Design

- **`emu-core::basic`:** `BasicDebug` with `info` (dialect, granularity, memory space), `state` (mode inactive/direct/running, line and statement, text pointer, GOSUB depth), `program` (lines with number, address, raw bytes, text in Unicode and the offsets of the statements in it), `variables`, `array`, `stack`, `set_variable` and `stop_points`. All reads are peeks (golden rule 2).
- **Stop points:** the machine names the places where its interpreter begins a statement or a line as `BreakpointKind`s — an exec address in the dispatch loop, or a watchpoint on the "current line" variable — each with its granularity (`statement` or `line`). Contract: when one stops the machine, `state()` already reports the statement about to run.
- **Session (`emu-debug/src/session/basic.rs`):** while a BASIC breakpoint is enabled or a BASIC step runs, the session keeps hidden breakpoints at the stop points (asked for again before every run, since ROM banking may move them); at each hit it reads `state()` and resumes silently unless a breakpoint or the step is satisfied. With nothing active nothing is armed (golden rule 5); no `tick` path of any machine changed. A breakpoint of the user's at a stop point's address still stops.
- **Steps:** `statement` — the next stop point; `line` — the next one in another line or at a line's first statement (into subroutines); `over` — the same with at most as many GOSUBs open as at the start; `out` — the next one with fewer GOSUBs open. All asynchronous, ended by `stopped` with the reason kind `basic_step` (`basic_break` for breakpoints), the line and the statement.
- **ROM versions (from M1):** system variables are stable over ROM revisions; dispatch addresses are found by a byte pattern in the ROM, not hard-wired, so patched ROMs (JiffyDOS) work. Without a match the machine offers line granularity only.
- **Protocol 11:** `basic_info`, `basic_state`, `basic_list {from, to}`, `basic_vars`, `basic_array {name, first, count}`, `basic_set_var {name, index, value}`, `basic_stack`, `basic_break_add {line, statement, enabled}`, `basic_break_remove`, `basic_break_enable`, `basic_break_list`, `basic_step {kind}`, `basic_run_to {line, statement}`; `"basic": true|false` in `capabilities`. `polydbg basic …`.

## Milestones

- M0 Core types and trait, session logic, protocol 11 with contract tests, `polydbg`, docs; the toy BASIC machine.
- M1 Microsoft 6502 BASIC as the reference, crate `basic-dialects`: C64, VIC-20, PET (BASIC 1, 2, 4).
- M2 Plus/4 (3.5), C128 (7.0: program in bank 0, variables in bank 1), MEGA65.
- M3 Apple II: Applesoft and Integer BASIC.
- M4 Microsoft Z80 BASIC: TRS-80 Level II and Model III, MSX1/MSX2; TRS-80 Level I.
- M5 Sinclair: ZX81, Spectrum 48K/128K/+2 (the statement number is a system variable there).
- M6 Atari BASIC; Locomotive BASIC 1.0/1.1 on the CPC.
- M7 BBC BASIC II/IV (Model B, B+, Master), then BBC BASIC V on the Archimedes.
- M8 TI BASIC on the TI-99/4A (runs in the GPL interpreter, its data in VDP RAM).

## Status

2026-11-14: **M0 done.** `emu-core::basic` (types, `BasicDebug`, `Machine::basic()`), the toy machine `emu-core::testing::basic::ToyBasic` (an interpreter in Rust that keeps its state in the machine's RAM, one statement every 8 cycles, one stop point; LET, add, GOTO, GOSUB/RETURN, FOR/NEXT, END; integer variables, an array, a string, a float; savestates; an option for line granularity; 7 tests), the session's BASIC debugger (9 tests, one more for a machine without a BASIC), protocol 11 with the contract test `debug_protocol::basic_debugger`, `polydbg basic` with its unit test and the end-to-end test `python_client::command_line_client_debugs_basic`. No real machine offers a BASIC yet: `capabilities` says `"basic": false` for all of them until M1.

2026-11-14, later: **M1 done.** New crate `basic-dialects` (trait `Dialect`, macro `impl_basic_debug!`; the decoders read the machine through the main CPU's `DebugTarget`, so a machine adds a field, three lines in `Machine::basic()` and the macro) with `cbm::CbmBasic` for Commodore BASIC (9 tests against a hand-built memory image). `machine-c64`, `machine-vic20` and `machine-pet` offer it. Nothing in it is tied to a ROM revision: the statement loop (`JSR …: JSR GONE3: JMP NEWSTT`) and the keyword table are searched in the CPU's view of $8000–$FFFF, the zero-page layout follows from the TXTPTR operand NEWSTT uses, the keywords in the listing are the ROM's own. The stop point is `JMP ($0308)` on the VIC-20 and C64 (before the IGONE vector, so wedges are followed) and the loop's first JSR on the PET. The gate test `tests/tests/basic_debug.rs` (`--ignored`, real ROMs) passes on the **C64, VIC-20, PET 2001 (BASIC 1.0), PET 3032 (BASIC 2) and PET 4032 (BASIC 4.0)**: it types a program in, lists it, stops at a line breakpoint after RUN, steps by statement through a FOR loop (frame with variable, limit and step), writes a float, an integer and a string variable the program then uses, steps by line into a GOSUB, out of it and over it, runs to a statement and finds the stack while halted inside statements.

Found on the way: after a cold start CURLIN is not $FFxx on BASIC 2 and later, so "running" also needs TXTPTR inside the line CURLIN names; BASIC 2 and later leave the statement loop's return address on the stack above each GOSUB frame (BASIC 1.0 does not); BASIC 4.0's loop calls a routine of its own instead of CHRGET (the first pattern expected `JSR $0070`).

Limits of the Commodore decoder: a string variable can only be overwritten in place with a string of the same length, and not a literal inside the program text (BASIC 4.0's heap has back pointers; changing lengths would need the interpreter's own allocation); statements behind THEN are part of the IF statement (the interpreter does not pass its loop for them); functions (DEF FN) are not listed; characters without a plain counterpart show as `{$xx}` (upper case/graphics set assumed); while INPUT reads its buffer the mode shows as direct; with the BASIC ROM banked out (C64 $01) the mode is inactive. Not checked: a cartridge or wedge that replaces the statement loop altogether, the 8032/8296 and SuperPET models (same BASIC 4.0 ROM as the 4032).

## Risks and open items

- **BBC BASIC** has no "current line" variable: the line is found from the text pointer, the stop point only by a ROM pattern.
- **TI BASIC** is not machine code: a stop point there needs a breakpoint on a GROM address, which `DebugHooks` may not offer yet; at worst line granularity.
- **Archimedes:** BBC BASIC V is a RISC OS module with its workspace behind a register; the least certain part, hence last in M7.
- **Atari BASIC** tokenises at entry and keeps a table of variable names: a decoder of its own.
- **MEGA65:** ROM builds move addresses often; only the pattern search carries there.
- **Watchpoint stop points** fire at the first byte written; a machine that uses one on a two-byte line variable has to watch the byte written last (contract above). Not exercised by the toy machine, which uses an exec stop point.
- **Sources:** token tables and system variables come from documentation only (Mapping the C64, the Atari BASIC Source Book, the Spectrum ROM disassembly …), never from another emulator's source; each goes into `docs/sources.md` with its milestone.
- Not in the phase: panels in the desktop and web frontends, loaded BASICs, watchpoints on variables, conditions on BASIC breakpoints, a line trace, entering or changing program lines through the debugger. Machines still to come (Colour Genie, QL, KC85) join `basic-dialects` in their own phases.
