# Debug server and live preview

Part of the project's instructions for Claude Code; the core is `CLAUDE.md` at the repository root, the index of these files is its section "Further documentation".

- Listens on `127.0.0.1` only by default; remote binding requires `--debug-bind` **and** a token.
- Emulation runs on its own thread; the server communicates via a command queue + event channel. Never lock the emulation thread from a network task.
- **Native protocol** (binary over TCP, JSON over WebSocket, same message set; Phase 1 ships JSON lines over TCP, documented in `emu-debug/src/protocol.rs`, contract test in `tests/tests/debug_protocol.rs`):
  - Session: `list_machines`, `attach`, `capabilities`
  - Execution: `pause`, `resume`, `step_instr`, `step_over`, `step_out`, `step_cycle`, `step_line`, `run_to`, `step_frame`, `reset`
  - Raster: `beam` position, `step_line` (follow a frame being drawn line by line, with the partial frame), raster trace (instructions and traced reads/writes stamped with frame/line/cycle; ring buffer in `DebugHooks`, zero cost when off)
  - Breakpoints: exec, read/write watchpoints, chip-register I/O, raster line/cycle, IRQ/NMI; conditions + hit counts
  - Inspect/modify: registers, memory (peek, bank-aware), disassembly, chip registers, stack
  - State: `save_state`, `load_state`, rewind ring buffer
  - Symbols: VICE labels, cc65 `.dbg`, KickAss `.sym`, sjasmplus `.sld`, m68k ELF/hunk symbols
  - Push events: `stopped`, `breakpoint_hit`, `frame_ready`, trace stream, `media_changed`, `device_connected`
  - Devices: see `docs/claude/freezer.md`
- **Adapters** translate to: VICE binary monitor (port 6502, all Commodore 8-bit machines). A GDB remote stub was built and dropped on 2026-10-22 (user decision): stock GDB has no 6502 and crashes stepping a Z80, so it would not work for every machine. DAP is deferred too (user decision 2026-10-22: the team has its own IDE on the native protocol).
- **Graphics data views** (Phase 8; trait extension approved and built 2026-10-25, see `docs/claude/phases/phase-08-expansion.md`): where a machine has them, the debugger also delivers the graphics the video chip works from, decoded and raw, without the debugger knowing the chip:
  - `Machine::gfx_views() -> Vec<GfxViewInfo>` (default: none) and `Machine::gfx_view(id, opts) -> Option<GfxView>`; a view is a kind (`sprites`, `charset`, `bitmap`, `screen`, `tiles`), a name ("VIC-II sprites", "ANTIC charset $E000", "GTIA players", "Amiga bitplanes"), the source memory space and address range, its parameters (size, colours, multicolour/hires, current bank/pointer) and a picture as indexed pixels + palette (same type as `FrameRef`), plus the raw bytes.
  - Examples: C64 VIC-II — 8 sprites (pointers, positions, colours, multicolour, expansion) and the current charset/bitmap/screen RAM by bank; C128 VDC charset; TED/VIC charsets; Atari — players/missiles, the character set at CHBASE, the display list's playfield memory; Amiga — sprites, bitplanes, Copper-set palettes; CPC — screen memory in the current mode; BBC Micro — the 6845/Video ULA screen (teletext through the SAA5050), the palette and the SAA5050 character set; MEGA65 — the VIC-IV display area, the character set, the sprites and a palette bank.
  - Options select what a view shows (e.g. a different charset address, colours, sprite number), so a debugger can browse memory as graphics.
  - Native protocol: `gfx_list`, `gfx_get {view, format=png|qoi|indexed, scale, options}`; the preview server serves the same as `/gfx/<view>.png`; `polydbg gfx`. Contract tests for each.
- **Vector displays** (protocol 10, 2026-11-11): `vectors` returns the last complete frame of a vector display (`Machine::vectors`, the Vectrex): size in coordinate units, `cycle_hz`, `frame_cycles`, `frame`, `phosphor`, `style` and every segment as `[x0, y0, x1, y1, intensity, r, g, b, cycles, at]` in drawing order (`at`: the cycle in the frame it began); machines without one answer with an error, `capabilities` has `"vectors": true|false`. The preview serves the same as `GET /vectors.json`, and its stream with `filter=crt` draws vector frames through the phosphor stage (the frame copies carry the segments, `OwnedFrame::vectors`). `polydbg vectors [--all] [--svg FILE]`. JSON form in `emu_preview::vectors_json`; contract test `vector_display_segments`.
- **BASIC debugger** (protocol 11, Phase 35, begun 2026-11-14; see `docs/claude/phases/phase-35-basic-debugger.md`): for machines whose BASIC interpreter the machine describes (`Machine::basic() -> Option<&mut dyn BasicDebug>`, default none; types in `emu-core::basic`), the debugger works on the BASIC program without knowing the dialect:
  - `basic_info` (dialect, `granularity` = `statement` | `line`, memory space), `basic_state` (`mode` = `inactive` | `direct` | `running`, `line`, `statement`, `addr` = text pointer, `depth` = open GOSUBs), `basic_list {from, to}` (lines with `line`, `addr`, `text` in Unicode, `statements` = offsets of the statements in the text, `raw` as base64; `current` = where the program is), `basic_vars` (name, type `int` | `float` | `string`, address, value; arrays with `dims`), `basic_array {name, first, count}`, `basic_set_var {name, index, value}`, `basic_stack` (frames innermost first: `gosub` with the line and statement of the GOSUB, `for` with variable, limit, step and the FOR's place).
  - Breakpoints: `basic_break_add {line, statement, enabled}` → `{bp}`, `basic_break_remove`, `basic_break_enable`, `basic_break_list`. Their numbers are separate from `break_add`'s. Without `statement` a breakpoint stops when the program enters the line, not when a loop returns into it.
  - Steps, asynchronous like `step_over`: `basic_step {kind}` with `line` (default; into subroutines), `statement`, `over` (the next line at this GOSUB depth) or `out` (until the subroutine has returned), and `basic_run_to {line, statement}`. A run they or a breakpoint end reports `stopped` with the reason `{"kind": "basic_break", "bp", "hits"}` or `{"kind": "basic_step", "step"}` plus `mode`, `line` and `statement`.
  - How it works: the machine names stop points (`BasicDebug::stop_points`: an exec address in the interpreter's dispatch loop or a watchpoint on its "current line" variable). The session arms hidden breakpoints there only while a BASIC breakpoint is enabled or a BASIC step runs, asks `state()` at each hit and resumes silently if nobody waits for it; with nothing active the cost is zero. They never show in `break_list`.
  - Machines so far (2026-11-14): C64, VIC-20, PET (BASIC 1.0, 2, 4.0) through `basic_dialects::cbm`; statement granularity; the gate test is `tests/tests/basic_debug.rs`.
  - `capabilities` has `"basic": true|false`; machines without one answer the `basic_*` commands with an error. `polydbg basic [info | list [FROM [TO]] | vars | array NAME [FIRST [COUNT]] | set NAME|NAME(I) VALUE | stack | break LINE[:STATEMENT] | breaks | unbreak BP | step [statement|line|over|out] | until LINE[:STATEMENT]]`. Contract test `basic_debugger` against the toy BASIC machine `emu_core::testing::basic::ToyBasic` (no ROM needed).
- Machine-specific extensions are negotiated via `capabilities`; generic clients must work without them.
- Every protocol change needs a contract test and a version bump in the handshake.


## Live preview

- `GET /snapshot.png?machine=<id>[&screen=vic|vdc]` — single frame on demand.
- WebSocket `/preview` — configurable fps (1–50), scale, format (QOI/PNG, delta frames).
- `GET /vectors.json` — a vector display's beam segments of the last frame (as the debugger's `vectors`); 404 for raster machines.
- `GET /controls` — the console's buttons and switches as JSON; the viewer shows them as a panel and sends `{"type": "control", "id", "value"}` back.
- WebSocket `/stream` — raw stream for local viewers (idea from the Ultimate 64 video stream): palette indices at 4/8 bpp or RGB, only changed lines, the pixel aspect ratio for the viewer, no image encoding; optional audio (`audio=1`, 16-bit PCM). Layout in `emu-preview/src/raw.rs`; the bundled viewer uses it for `filter=raw`.
- On `breakpoint_hit`, attach the current (partial) frame; partial frames mark current raster line/cycle.
- Encoding runs on its own thread from a frame copy; the emulator never waits on the encoder — drop frames under backpressure.
- Optional overlays: raster grid, bad lines, sprite boxes, Atari display-list modes, CPC CRTC changes, Amiga DMA slot map.
