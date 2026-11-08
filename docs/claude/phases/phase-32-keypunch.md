# Phase 32: IBM 029 card punch

Part of the project's instructions for Claude Code; the core is `CLAUDE.md` at the repository root, the index of these files is its section "Further documentation".

| # | Phase | Est. | Gate |
| --- | --- | --- | --- |
| 32 | IBM 029 card punch (`machine-keypunch`, begun 2026-11-08 on the user's request, from the backlog item "card punch for the user"): a machine of its own the user types on; the card drawn as the picture; decks as files in three formats; a card reader on the IMSAI reads them | 2 wk | a deck typed on the punch is read by CP/M on the IMSAI (`PIP FILE=RDR:`) |

## Decisions (user, 2026-11-08)

- Model: IBM 029 (EBCDIC card code, 63 characters with the blank).
- Scope: the basic functions — feed, punching by typing, interpret printing along the top edge, backspace (the holes stay), release/eject with AUTO FEED, SKIP, CLEAR; save and load decks; the card drawn to scale with its holes. No program drum, duplication or multi-punch key.
- Programming languages: the PROG switch chooses a card format (FREE, FORTRAN, COBOL, ASSEMBLER) whose field starts are Tab stops and drawn as lines; SEQ punches sequence numbers (73–80, COBOL 1–6, from 10 by 10) into an empty sequence field of a released card. [S]: helpers, not 029 parts.
- Deck files (`media::cards`): binary `.deck` (160 bytes a card, 80 big-endian left-justified 12-bit columns), text `.crd` (a line a card) and, added by the user, hex `.chx` (a line a card, 80 columns of three hex digits, `#` comments) for hole patterns no key punches.
- Core changes the user approved: `MediaKind::Cards` and `MediaBackend::set_len` (defaulted to "cannot change size"; memory, file and copy-on-write backends grow and shrink). `media_backends::image::open_cards` creates a missing deck file, so the stacker writes into a new file.
- IMSAI reading: both, chosen by option `cardreader`: `text` (default) plays the deck into the reader line as lines with CR LF and a closing ^Z; `raw` is a parallel interface at ports $20–$22 that hands out every column's 12 rows ([S]: this project's interface).

## Status

Built 2026-11-08: `media::cards` (029 code table, three formats, tests), `machine-keypunch` (keys as on the IMSAI's terminal: Enter = REL or FEED, Tab = SKIP, Backspace, → or space = space; controls MAIN LINE, FEED, REL, BACKSPACE, SKIP, CLEAR, AUTO FEED, PRINT, PROG, SEQ; slots "hopper" — cards from a deck first, then blank — and "stacker"; savestate; debug target "cpu" (the first device by convention; the 029 has no processor) with memory space "card" in the binary layout), the keymap in `emu-input`, `machine-imsai::cardreader` (slot "cards"). The gate is met by `tests/tests/imsai.rs` `pip_reads_a_card_deck_from_rdr`: z80pack's CP/M reads a three-card FORTRAN deck with `STAT RDR:=UR2:` and `PIP HELLO.FOR=RDR:`, `TYPE` shows it. Findings: z80pack's BIOS reads RDR: from the console (TTY:), returns ^Z for PTR:/UR1: and reads SIO-2 channel B (ports 4/5) for UR2:; its boot sets up only channel A's 8251, so channel B needs its mode and command words first (the DDT program in `docs/claude/machines.md`). Reading through the console (`reader=console`) does not work with PIP: PIP takes the waiting characters as a key press and aborts.

Open: the picture is not clickable (the controls are in the panel strip); no sound of the punch; the web frontend's media list knows the "cards" kind but has not been tried with the punch; the user still has to try the desktop.
