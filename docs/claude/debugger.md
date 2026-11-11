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
- Machine-specific extensions are negotiated via `capabilities`; generic clients must work without them.
- Every protocol change needs a contract test and a version bump in the handshake.


## Live preview

- `GET /snapshot.png?machine=<id>[&screen=vic|vdc]` — single frame on demand.
- WebSocket `/preview` — configurable fps (1–50), scale, format (QOI/PNG, delta frames).
- `GET /controls` — the console's buttons and switches as JSON; the viewer shows them as a panel and sends `{"type": "control", "id", "value"}` back.
- WebSocket `/stream` — raw stream for local viewers (idea from the Ultimate 64 video stream): palette indices at 4/8 bpp or RGB, only changed lines, the pixel aspect ratio for the viewer, no image encoding; optional audio (`audio=1`, 16-bit PCM). Layout in `emu-preview/src/raw.rs`; the bundled viewer uses it for `filter=raw`.
- On `breakpoint_hit`, attach the current (partial) frame; partial frames mark current raster line/cycle.
- Encoding runs on its own thread from a frame copy; the emulator never waits on the encoder — drop frames under backpressure.
- Optional overlays: raster grid, bad lines, sprite boxes, Atari display-list modes, CPC CRTC changes, Amiga DMA slot map.
