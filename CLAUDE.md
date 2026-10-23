# CLAUDE.md — Multi-Platform Retro Emulator

This file guides Claude Code when working in this repository. Read it fully before making changes.

## Project in one paragraph

A cycle-accurate, multi-machine emulator for classic home computers — the whole Commodore family (PET, VIC-20, C16/Plus/4, C64, C128, Amiga), Atari 400/800/XL/XE and Amstrad CPC 6128 — built on one shared core. Its distinguishing feature: **every machine automatically gets the same remote debugger and a live preview stream** (frames of the running machine, also headless and while halted at a breakpoint). Debugger and preview depend only on core traits, never on a concrete machine.

## Golden rules

1. **Debugger and preview code must never import a `machine-*` crate.** They talk to `emu-core` traits only. If a feature seems to need machine knowledge, extend the trait (`capabilities`, `chips()`, `devices()`, `memory_spaces()`) instead.
2. **`peek` vs `read`:** `Bus::read` may have side effects (e.g. reading CIA ICR clears interrupts). The debugger, preview and any inspection code use `peek` exclusively. Every chip must implement a side-effect-free `peek`.
3. **Determinism:** same inputs + same savestate ⇒ identical frames and cycle counts. No wall-clock time, no `HashMap` iteration order, no randomness without an explicit seeded RNG inside machine state. Tests rely on this.
4. **Cycle granularity:** emulation advances per CPU cycle (and per DMA slot on the Amiga), not per instruction.
5. **Hot path stays cheap:** breakpoint checks use per-address-space bitmaps; with no active breakpoints the debug overhead must be ~0. Benchmark before/after when touching `tick` paths.
6. **No ROMs in the repo.** Kernal, BASIC, Kickstart, Atari OS, AMSDOS are user-supplied. Tests that need ROMs are `#[ignore]` unless `EMU_ROM_DIR` is set. Free replacements (AROS Kickstart, Altirra OS) may be supported as optional downloads, never committed.
7. **Licensing:** do not copy code from VICE, WinUAE, FS-UAE, vAmiga, Atari800, Caprice32 or Arnold (GPL/other). Use their documentation and observable behavior only.
8. **Savestates for everything:** every chip/device struct implements state serialization from day one. A component without savestate support is not done.
9. **CRT emulation is post-processing.** Machines output raw frames plus signal metadata; CRT/monitor effects run afterwards in `video-crt`. Regression tests and the debugger always compare/see raw frames. Exception: effects that change *what the guest can produce* (NTSC artifact colors, PAL delay-line color blending) are part of the optional composite signal path, still outside the machine crates.
10. **Machines never see host devices.** No `gilrs`, `winit`, file-system or USB code inside `machine-*`, `chips-*` or `devices-*`. Host input and host storage enter only through the port/media abstractions in `emu-core` (see "External devices and ports").

## Tech stack

- **Rust** (stable, edition 2021), Cargo workspace.
- Desktop frontend: `winit` + `wgpu`, UI with `egui`. Audio: `cpal`.
- Async server: `tokio`; WebSocket via `tokio-tungstenite`.
- Preview encoding: QOI / PNG (`qoi`, `png` crates); H.264/WebRTC is a later, optional feature.
- Targets: Windows, macOS, Linux, `wasm32-unknown-unknown` (web frontend runs the same core).
- CI: GitHub Actions on all three OS + a WASM build.

## Workspace layout

```
crates/
  emu-core/        Traits (Cpu, Bus, Machine), scheduler, savestates, DebugHooks, FrameRef
  cpu-6502/        6502 family: 6502, 6510, 8502, 7501/8501, 6502C (SALLY) via variant flags
  cpu-z80/         Z80 (C128, CPC)
  cpu-m68k/        68000 first, 68020 later (A1200)
  chips-cbm/       VIC, VIC-II(e), TED, VDC 8563, SID, CIA, VIA, PIA, MMU 8722
  chips-amiga/     Agnus (DMA, Copper, Blitter), Denise, Paula, Gary, CIA
  chips-atari/     ANTIC, GTIA, POKEY, PIA
  chips-amstrad/   CRTC 6845, Gate Array, AY-3-8912, 8255 PPI, FDC 765
  drives-cbm/      IEC bus, 1541, 1571, 1581 (each with its own CPU)
  drives-other/    Amiga floppy (DF0–DF3, MFM), Atari SIO drives (810/1050), CPC FDC drives, datasettes
  devices-input/   Emulated input peripherals: joysticks, paddles, mice (1351, Amiga, AMX), light pens, keypads
  devices-cartridge/ Expansion-port cartridges: C64 CRT types (banking, EasyFlash), VIC-20 and C16/Plus/4 ROM carts
  media/           Image formats: PRG, D64/D71/D81, T64/TAP, CRT, ADF(→MFM), ATR, XEX, CAS, DSK, CDT
  media-backends/  Where media bytes come from: image file, copy-on-write overlay, host directory, real hardware
  emu-input/       Host side: gamepads (gilrs), keyboard-as-joystick, mouse capture, mapping profiles
  video-crt/       CRT/monitor emulation: composite/RGB signal path, WGSL shaders (wgpu), CPU fallback, monitor profiles
  machine-pet/  machine-vic20/  machine-c64/  machine-ted/  machine-c128/
  machine-amiga/  machine-atari8/  machine-cpc/
  machine-sim6502/ Reference/test machine: 6502 + 64 KB RAM + minimal raster chip (not real hardware)
  emu-machines/    Fills the machine registry; the only crate that links every machine-*
  emu-debug/       Debug server: native protocol + adapter (VICE binary monitor)
  emu-preview/     Frame encoders, snapshot endpoint, WebSocket stream, overlays
  emu-desktop/  emu-web/  emu-headless/   Frontends
tests/             Integration tests, reference frames, test-suite runners
tools/             Python debug client, scripts
```

Keep dependencies pointing downward: `machine-*` → `chips-*`/`cpu-*` → `emu-core`. `emu-debug`/`emu-preview` depend on `emu-core` only (`emu-preview` may also use `video-crt`). Frontends pick machines via a registry in `emu-core`; the only machine-aware crate they may depend on is `emu-machines`, which registers all machines. `tools/check-deps.py` enforces these rules in CI — update its `RULES` table when adding a crate.

## Core traits (contract — change deliberately)

```rust
pub enum Size { Byte, Word, Long }

pub trait Bus {
    fn read(&mut self, addr: u32, size: Size) -> u32;   // may have side effects
    fn peek(&self, addr: u32, size: Size) -> u32;       // MUST be side-effect free
    fn write(&mut self, addr: u32, size: Size, v: u32);
    fn addr_bits(&self) -> u8;                          // 16 for 8-bit machines, 24 for Amiga
}

pub trait Cpu {
    fn tick(&mut self, bus: &mut dyn Bus) -> CycleResult;
    fn regs(&self) -> RegisterSet;                      // generic name -> value
    fn set_reg(&mut self, name: &str, v: u32);
    fn disassemble(&self, bus: &dyn Bus, addr: u32) -> Instr;
}

pub trait Machine: Send {
    fn run_frame(&mut self, dbg: &mut DebugHooks) -> StopReason;
    fn step_cycle(&mut self) -> StopReason;
    fn framebuffer(&self) -> FrameRef;                  // Indexed+palette or RGB (Amiga HAM); partial while halted mid-frame
    fn audio(&mut self, out: &mut Vec<i16>) -> AudioFormat; // samples since last call, fixed rate (48 kHz)
    fn beam(&self) -> Option<BeamPos>;                  // frame, raster line, cycle in line, hpos; None without a beam
    fn memory_spaces(&self) -> Vec<MemSpace>;           // CPU, chip RAM, fast RAM, ROM, drive, VDC RAM…
    fn chips(&self) -> Vec<ChipInfo>;                   // register dumps for debugger views
    fn devices(&self) -> Vec<DeviceId>;                 // debug targets, main CPU first: "cpu", 1541, C128 Z80, Copper, Blitter
    fn debug_target(&mut self, dev: DeviceId) -> Option<&mut dyn DebugTarget>; // regs, set_reg, peek/poke per MemSpace, disassemble
    fn input(&mut self, ev: TimedInput);                // queued, applied exactly at ev.at (part of savestate)
    fn now(&self) -> Cycle;                             // master cycles since power-on, time base of TimedInput
    fn controls(&self) -> Vec<ControlInfo> { vec![] }    // console buttons/switches (power, reset, 2600 switches …) for panels
    fn ports(&self) -> Vec<PortInfo>;                   // physical connectors (see "External devices and ports")
    fn connect(&mut self, port: PortId, dev: Box<dyn PortDevice>) -> Result<(), PortError>;
    fn disconnect(&mut self, port: PortId) -> Option<Box<dyn PortDevice>>;
    fn media_slots(&self) -> Vec<MediaSlotInfo>;        // drive 8, DF0:, D1:, tape, cartridge port …
    fn media_slot(&mut self, slot: SlotId) -> Option<&mut dyn MediaSlot>;     // None for an unknown slot
    fn save_state(&self) -> Vec<u8>;
    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError>; // corrupt data never panics
}
```

Multi-CPU machines (C64 + 1541, C128 8502 + Z80, Amiga Copper/Blitter) expose each processor as a separate debug target via `devices()`, each with its own address space. The debugger reaches registers, memory and disassembly only through `debug_target()` → `DebugTarget`, never through a concrete CPU type.

## Machine notes

| Machine | Master clock (PAL) | Gotchas |
| --- | --- | --- |
| C64 | CPU 985 248 Hz | Bad lines steal CPU cycles; sprite DMA; `$01` port banking |
| VIC-20 | CPU 1.108 MHz | VIC 6560 (NTSC) / 6561 (PAL); memory expansion configs |
| C16/Plus/4 | TED-driven, single/double clock | TED is video+sound+timers; CPU speed changes per line |
| PET | 1 MHz | Models with/without CRTC (2001 vs 4032/8032) |
| C128 | 8502 1/2 MHz, Z80 | MMU switches CPUs; VDC with its own RAM and 2nd screen |
| Amiga 500/1000/2000 | 7.09379 MHz | Agnus DMA slot arbitration per line; Copper/Blitter; CPU only gets free chip-RAM slots; ADF (DD/HD, extended, DMS) converted to MFM tracks; A1000 WCS, Zorro II autoconfig fast RAM |
| Atari 2600 | colour clock 3.546894 MHz, CPU ÷3 (NTSC 3.579545) | No frame buffer: the CPU races the beam; WSYNC halts it via RDY; fields follow VSYNC; bank switching by cartridge hotspots |
| Atari 400/800/800XL/65XE/130XE | CPU 1.773447 MHz | ANTIC DMA steals cycles; display lists; DLIs; XL/XE bank ROMs by PORTB, the 130XE banks 64 KB extended RAM for CPU and ANTIC separately; 400/800: OS-B, four joystick ports, RESET key is an NMI |
| CPC 464/664/6128 | Z80 4 MHz | Wait states align memory access to 1 µs; CRTC register tricks; 464 tape only, 664/6128 FDC, 6128 128 KB banking |
| ZX81 / TS1000 | Z80 3.25 MHz | No frame buffer: the CPU executes the display file and the ULA turns the fetches into pixels (NOP substitution, I register, R bit 6 as /INT, NMI generator, /WAIT while halted); 207 T-states per line; LOAD/SAVE by ROM traps on .P files |

Model variants (PAL/NTSC, Kernal revisions, RAM sizes) are **configuration**, not separate machine crates.

## External devices and ports

Peripherals are modeled the way the hardware is: a machine has **ports** (physical connectors), **devices** plug into ports, and **drives** hold **media** in **slots**. Host hardware is mapped onto these from the outside. Three layers, never mixed:

```
Host layer (emu-input, media-backends)   gamepad, keyboard, mouse, files, folders, USB floppy adapters
        │  host-neutral InputEvent / MediaBackend
Device layer (devices-input, drives-*)   1351 mouse, Amiga mouse, joystick, paddles, 1541, DF0:, 810 …
        │  PortDevice (electrical lines) / MediaSlot
Machine layer (machine-*)                control port 1/2, gameport 0/1, IEC, SIO, user port, cartridge
```

### Ports

```rust
pub enum PortKind {
    CbmControl,      // C64/C128/VIC-20/Plus4*: 9-pin, digital + POTX/POTY analog (*Plus/4 uses mini-DIN)
    AmigaGameport,   // 9-pin, digital + quadrature mouse + POT lines
    AtariJoy,        // 9-pin, digital + paddles via POKEY POT
    CpcJoy,          // 9-pin, two joysticks via daisy-chain, AMX mouse
    CbmUserPort, Iec, Sio, Cartridge, Datasette, AmigaFloppy, CentronicsPrinter,
}

pub trait PortDevice: Send {
    fn kind(&self) -> PortKind;
    fn read_lines(&self, now: Cycle) -> PortLines;      // what the device drives onto the pins
    fn write_lines(&mut self, lines: PortLines, now: Cycle); // what the machine drives
    fn input(&mut self, ev: &InputEvent);                // host-neutral events from the host layer
    fn save_state(&self) -> Vec<u8>;
    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError>;
}
```

- Devices work at **pin/line level** with cycle timestamps, so analog/timing devices are accurate: 1351 proportional mouse via SID POT sampling, Amiga quadrature counting into JOYxDAT, paddles via POKEY/SID pot timers, light pens latching VIC/ANTIC registers.
- A port accepts only devices with a matching `PortKind`; `connect` returns `PortError::Incompatible` otherwise.
- Default devices per machine come from config (e.g. C64: joystick in port 2; Amiga: mouse in port 0, joystick in port 1).

### Input events and host mapping

```rust
pub enum InputEvent {
    Digital { dirs: Dirs, buttons: u8 },     // joystick
    Analog  { axis: u8, value: u16 },        // paddles / pots
    MouseRel { dx: i16, dy: i16, buttons: u8 },
    LightPen { x: u16, y: u16, trigger: bool },
    Key { machine_key: MachineKey, down: bool }, // keyboard matrix position, not host keycode
    Control { id: ControlId, value: u8 },        // console panel (InputTarget::Panel): button 0/1, switch position
}
```

- `emu-input` translates host input (gamepads via `gilrs`, keyboard-as-joystick, captured mouse) into `InputEvent`s using **mapping profiles** (TOML, per machine + per game override). Host keycodes → `MachineKey` via per-machine keymaps (positional and symbolic modes).
- Events go through a **timestamped input queue** applied at defined cycle positions (start of frame by default). This keeps emulation deterministic and enables input **record/replay** files used for regression tests.
- Hot-plugging a host gamepad never touches machine state; only the mapping changes.

### Drives and media

```rust
pub trait MediaSlot {
    fn insert(&mut self, media: Media) -> Result<(), MediaError>;
    fn eject(&mut self) -> Option<Media>;
    fn write_protect(&mut self, on: bool);
    fn flush(&mut self) -> Result<(), MediaError>;      // write back through the backend
}

pub trait MediaBackend: Send {                       // where bytes live
    fn len(&self) -> u64;
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError>;
    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), MediaError>;
    fn commit(&mut self) -> Result<(), MediaError>;   // e.g. apply copy-on-write overlay to the original
}
```

- **Backends:** image file; **copy-on-write overlay** (default — original image never modified until the user commits); **host directory** (a folder appears as a drive); **real hardware** behind a cargo feature (OpenCBM/ZoomFloppy for real 1541s, Greaseweazle-style flux readers for importing). Real hardware support is optional and must never be required to build.
- **Two drive modes per Commodore drive slot:** *true drive emulation* (1541 CPU + GCR, cycle-exact, needed for fastloaders/copy protection) and *virtual drive* (IEC protocol-level device serving files from a backend; fast, compatible with KERNAL load/save only). The mode is a slot setting.
- Amiga floppies are MFM track-based; ADF is converted on insert, written back on flush. Atari SIO and CPC FDC drives follow the same `MediaSlot` contract.
- Multi-disk sets: a slot can hold a disk list (swap with hotkey / debugger command).

## Display and CRT emulation

### What machines output (`FrameRef`)

Every machine delivers, per field:

- the **visible area including overscan/border** as a monitor shows it, centred with equal borders left and right (VIC-II: VICE's normal border, 32 pixels each side; PAL 384×272), as indexed pixels + palette (8-bit machines) or RGB (Amiga);
- **signal metadata**: pixel clock, active/total pixels per line, lines per field, field parity (interlace), video standard (PAL/NTSC), output type (composite, S-video/luma-chroma, RGB, monochrome);
- optionally **per-line luma/chroma values** instead of palette indices, so the composite path can reproduce colors palettes cannot express.

Cropping, aspect ratio and pixel-aspect correction (e.g. C64 PAL 0.9365, CPC mode 2 half-width pixels) are applied downstream from this metadata, never inside the machine.

### Pipeline in `video-crt`

```
raw field ─► [signal stage] ─► [deinterlace/field stage] ─► [CRT stage] ─► host output
             composite encode/decode   weave / bob /           scanlines, mask,
             (optional, per standard)  phosphor-blend          bloom, curvature, persistence
```

1. **Signal stage** (optional, per monitor profile): models the connection the user picked.
   - Composite NTSC: chroma/luma crosstalk and **artifact colors** — required for Atari 8-bit games that rely on artifacting, and for C64/VIC-20 NTSC looks.
   - Composite PAL: **delay-line averaging** of chroma between lines — produces the blended colors C64 PAL demos rely on (e.g. "PAL color mixing").
   - S-video / luma-chroma (C64, C128 40-col, Atari 800XL): reduced crosstalk.
   - RGB (Amiga, CPC, C128 VDC): clean, no signal stage.
   - Monochrome (CPC GT65 green, PET built-in white/green): luma only, phosphor tint.
2. **Field stage:** interlace handling for Amiga (weave, bob, or phosphor-persistence blend) and 50/60 Hz field pacing.
3. **CRT stage:** scanlines (beam width tied to brightness), shadow mask / aperture grille / slot mask, bloom/halation, barrel curvature, vignette, **phosphor decay** across frames, configurable gamma.

### Monitor profiles

TOML presets, each choosing signal type + CRT parameters: Commodore 1702, 1084S (RGB and composite inputs), Amstrad CTM644 (color) and GT65 (green), Atari via TV RF/composite, PET built-in, plus "generic PAL TV", "generic NTSC TV" and "raw (sharp pixels)". Users can override any parameter; per-machine default profiles live in machine config.

### Implementation notes

- GPU path: WGSL shaders via `wgpu` in desktop and web frontends; multi-pass, render targets sized from signal metadata (not the window).
- CPU path: a simplified, deterministic implementation for headless use and the preview server (snapshots with `filter=crt`), so CI and remote viewers can see CRT output without a GPU.
- Effects must work at integer and non-integer scale; scanline count always follows the emulated line count, not the window height.
- Phosphor persistence keeps its own frame history in `video-crt`, never in machine state (savestates stay CRT-independent).
- Light pens and anything beam-position-dependent use raster timing from the core, not the CRT stage's geometry.
- Performance budget: full CRT pipeline at 50/60 Hz on integrated GPUs at 1080p; provide a "lite" preset for weak hardware and WASM.

### Preview and debugger integration

- Preview parameters: `filter=raw|crt`, `profile=<name>`; default for the stream is `raw`, default for the desktop window is the machine's monitor profile.
- Debug overlays (raster grid, DMA map, beam position) are always drawn on the raw frame, optionally side by side with the CRT view.

## Memory freezer and cheats

Two related features: a **software memory freezer** in the emulator core, and emulation of **freezer cartridges** (Action Replay & co.) as real hardware.

### Software memory freezer (`emu-core::freeze`)

```rust
pub struct Freeze {
    pub space: MemSpaceId,      // CPU view, chip RAM, drive RAM, VDC RAM …
    pub addr: u32,
    pub size: Size,             // Byte | Word | Long
    pub value: u32,
    pub mode: FreezeMode,       // BlockWrites | ReapplyPerFrame | ReapplyOnRead
    pub enabled: bool,
    pub label: Option<String>,  // "Lives", "Energy"
}
```

- **BlockWrites** (default): guest writes to the address are suppressed at bus level; reads return the frozen value. Implemented with the same per-space bitmap mechanism as watchpoints, so zero cost when no freeze is active.
- **ReapplyPerFrame**: value written at the start of each frame (closest to classic trainers / POKE cheats; tolerates code that checks its own writes).
- **ReapplyOnRead**: value forced only when read (for I/O-mapped or bank-switched addresses).
- Freezes respect banking: a freeze is bound to a memory space + physical address, not just a CPU address. Freezes on I/O registers are allowed but flagged in the UI.
- Freezes are **part of savestates and input replays** so runs stay reproducible.

### Cheat search (memory finder)

Classic narrowing search over a memory space:

1. `search_start(space, size)` snapshots all candidate addresses.
2. Repeat `search_next(op)` with `op` = equal/not-equal to a value, increased, decreased, unchanged, changed, increased-by-N.
3. Remaining candidates can be watched live (values update with each frame) and turned into freezes in one step.

Search runs on `peek` snapshots taken at frame boundaries — it never halts or slows emulation beyond the copy.

### Cheat files

- Per-game cheat sets in TOML (`cheats/<machine>/<game-hash>.toml`), keyed by media/image hash so they apply automatically.
- Import of common formats where documented: POKE lists, VICE-style monitor commands, Amiga Action Replay codes, Atari/CPC POKE lists.
- Cheats can include **code patches** (byte sequences at addresses) in addition to value freezes.

### Freezer cartridges (hardware)

Emulated as `Cartridge` port devices with a **freeze button** (NMI + bank switch), so original cartridges and their menus work:

- C64/C128: Action Replay, Retro Replay, Final Cartridge III, Super Snapshot.
- Amiga: Action Replay (A500 side-port).
- Freeze button is triggerable from UI, hotkey and the debug server (`press_cartridge_button`).

### Debugger and preview integration

- Commands: `freeze_add`, `freeze_remove`, `freeze_list`, `freeze_toggle`, `search_start`, `search_next`, `search_results`, `cheats_load`, `cheats_save`.
- Event `freeze_blocked_write` (optional, rate-limited) tells the debugger which code tried to change a frozen address — useful for finding the routine to patch.
- Memory views highlight frozen addresses.

### Remote control of devices

The debug server exposes devices too, so tools, tests and the web preview can drive a machine:

- `list_ports`, `connect_device`, `disconnect_device`
- `list_media_slots`, `insert_media`, `eject_media`, `set_drive_mode`, `flush_media`
- `input` (inject `InputEvent`s, timestamped), `record_input`, `replay_input`
- `controls`, `set_control` (console panel: the machine's buttons and switches; protocol v8)

The web frontend uses these over WebSocket to forward browser keyboard/gamepad/mouse input, making the live preview interactive.

## Debug server

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
  - Devices: see "Remote control of devices"
- **Adapters** translate to: VICE binary monitor (port 6502, all Commodore 8-bit machines). A GDB remote stub was built and dropped on 2026-10-22 (user decision): stock GDB has no 6502 and crashes stepping a Z80, so it would not work for every machine. DAP is deferred too (user decision 2026-10-22: the team has its own IDE on the native protocol).
- **Graphics data views** (Phase 8; needs a core trait extension, ask before changing the trait): where a machine has them, the debugger also delivers the graphics the video chip works from, decoded and raw, without the debugger knowing the chip:
  - `Machine::gfx_views() -> Vec<GfxViewInfo>` (default: none) and `Machine::gfx_view(id, opts) -> Option<GfxView>`; a view is a kind (`sprites`, `charset`, `bitmap`, `screen`, `tiles`), a name ("VIC-II sprites", "ANTIC charset $E000", "GTIA players", "Amiga bitplanes"), the source memory space and address range, its parameters (size, colours, multicolour/hires, current bank/pointer) and a picture as indexed pixels + palette (same type as `FrameRef`), plus the raw bytes.
  - Examples: C64 VIC-II — 8 sprites (pointers, positions, colours, multicolour, expansion) and the current charset/bitmap/screen RAM by bank; C128 VDC charset; TED/VIC charsets; Atari — players/missiles, the character set at CHBASE, the display list's playfield memory; Amiga — sprites, bitplanes, Copper-set palettes; CPC — screen memory in the current mode.
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

## Commands

```bash
cargo build --workspace
cargo test --workspace                       # unit + integration (ROM tests skipped)
EMU_ROM_DIR=~/roms cargo test --workspace -- --include-ignored
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
bash tools/fetch-testdata.sh                 # Harte, Dormann, Lorenz test data into testdata/ (~850 MB)
EMU_TESTDATA_DIR=$PWD/testdata cargo test -p cpu-6502 --release -- --ignored   # every 10th Harte test
EMU_HARTE_FULL=1 EMU_TESTDATA_DIR=$PWD/testdata cargo test -p cpu-6502 --release -- --ignored
EMU_TESTDATA_DIR=$PWD/testdata cargo test -p cpu-z80 --release --test harte -- --ignored   # every 10th Z80 test (EMU_HARTE_ONLY=ed: only "ed …" files)
EMU_TESTDATA_DIR=$PWD/testdata cargo test -p cpu-m68k --release --test harte -- --ignored --nocapture   # every 10th 68000 test (EMU_HARTE_FULL=1 all, EMU_HARTE_ONLY=MOVE,DIVU file prefixes)
EMU_TESTDATA_DIR=$PWD/testdata cargo test -p cpu-z80 --release --test zex -- --ignored --nocapture   # ZEXDOC + ZEXALL, minutes
EMU_TESTDATA_DIR=$PWD/testdata cargo test -p emu-tests --release --test lorenz lorenz_suite_stub -- --ignored   # Lorenz, no ROMs (CI gate)
EMU_ROM_DIR=$PWD/roms EMU_TESTDATA_DIR=$PWD/testdata cargo test -p emu-tests --release --test lorenz lorenz_suite -- --ignored --exact
EMU_ROM_DIR=$PWD/roms EMU_TESTDATA_DIR=$PWD/testdata cargo test -p emu-tests --release --test acid800 -- --ignored --nocapture   # Acid800 on the 800XL (EMU_ACID_MODEL=800xl-ntsc|130xe, EMU_ACID_FILTER=antic)
EMU_ROM_DIR=$PWD/roms EMU_TESTDATA_DIR=$PWD/testdata cargo test -p emu-tests --release --test cpc_acid -- --ignored --nocapture   # Arnold Acid tests on the CPC 6128 (~10 min; EMU_CPC_FILTER=ppi, EMU_CPC_CRTC=0, EMU_CPC_FRAMES=n, EMU_CPC_TAIL=n, EMU_CPC_BREAK=hex)
python tools/check-deps.py                   # dependency direction (golden rules 1 and 10)
cargo bench -p cpu-6502                      # before/after when touching hot paths
cargo build -p emu-web --target wasm32-unknown-unknown
cargo run -p emu-headless -- --machine c64 --prg demo.prg --frames 300 --snapshot out.png
cargo run -p emu-headless -- --machine vic20 --model ntsc --option ram=8k --rom-dir roms --frames 200 --snapshot vic.png   # ROMs in roms/<machine>/, roms/drives/
cargo run -p emu-headless -- --machine pet --model 8032 --option hz=50 --rom-dir roms --frames 200 --snapshot pet.png   # 2001, 3032, 4032, 8032
cargo run -p emu-headless -- --machine plus4 --rom-dir roms --frames 200 --snapshot plus4.png   # also --machine c16, --model ntsc
cargo run -p emu-headless -- --machine c128 --option display=80 --rom-dir roms --frames 600 --screen vdc --snapshot c128.png   # --screen vic|vdc
cargo run -p emu-headless -- --machine atari8 --rom-dir roms --frames 350 --snapshot atari.png   # ROMs in roms/atari/; --model 800xl|65xe|130xe|800|400 (+ -ntsc); --option basic=off (XL/XE: self-test; 400/800: no BASIC cartridge), ram=8k|16k|32k|48k (400/800), port3/port4 (400/800)
EMU_ROM_DIR=$PWD/roms cargo test -p emu-tests --release --test atari_boot other_models_boot -- --ignored --nocapture   # 130XE, 800 (OS-B + BASIC cartridge / memo pad) and 400 boot
cargo run -p emu-headless -- --machine atari8 --rom-dir roms --disk game.atr --frames 600 --snapshot a.png   # also --prg game.xex, --tape game.cas, --cart game.car; --option drives=0..4, drive=810|1050|xf551
cargo run -p emu-headless -- --machine atari2600 --cart game.a26 --frames 120 --snapshot vcs.png   # no system ROM; --model pal; --option roms=stub (built-in test cartridge)
cargo run -p emu-headless -- --machine amiga --rom-dir roms --frames 400 --snapshot amiga.png   # ROMs in roms/amiga/ ("Kickstart 1.3.rom", kick13.rom …); --model 500|1000|2000 (+ -ntsc); --option kickstart=1.2|1.3|2.0|3.1, chip=256k|512k|1m, slow=512k|0, fast=0|512k|…|8m, drives=1..4, drivetype=dd|hd, serial=none|loopback, wcs=rom|disk (A1000), roms=stub
EMU_ROM_DIR=$PWD/roms cargo test -p emu-tests --release --test amiga_boot expansions_and_models_boot -- --ignored --nocapture   # 1 MB chip + fast RAM via autoconfig, A2000, A1000 (WCS) reach insert-disk
EMU_DMS_DIR=/path/with/dms cargo test -p media --release --test dms_files -- --ignored --nocapture   # real DMS archives unpack with good checksums (freely distributed ones from Aminet)
EMU_ROM_DIR=$PWD/roms EMU_VAMIGATS_DIR=/path/to/vAmigaTS/Denise/Sprites EMU_CLXDAT=1 cargo test -p emu-tests --release --test vamigats -- --ignored --nocapture   # vAmigaTS programs (GPL test data, not in the repo): snapshots in target/tmp/vamigats/ to compare with the suite's A500 photographs; EMU_FILTER=name, EMU_FRAMES=n
EMU_ROM_DIR=$PWD/roms cargo test -p emu-tests --release --test amiga_boot -- --ignored --nocapture   # Kickstarts reach "insert disk" and boot a generated boot block; trace_kickstart: EMU_KICK=1.3, EMU_FRAMES=n, EMU_DISK=a.adf, EMU_DISK1=b.adf, EMU_OPTION=drives=2; workbench_13_boots (Phase 7 gate) with EMU_WB_ADF=workbench1.3.adf
python tools/amiga_bustest.py bustest.adf [tests/data/amiga_bustest.bootblock]   # chip-bus timing test disk (boot it in WinUAE/on hardware; the test chip_bus_arbitration_matches_the_bus_test compares us with WinUAE's numbers)
python tools/amiga_bustest2.py bustest2.adf [tests/data/amiga_bustest2.bootblock]   # Blitter vs 68000 (BLTPRI off/on, 5 planes + Copper, D-only); blitter_bus_arbitration_matches_the_bus_test compares with WinUAE's numbers
python tools/amiga_bustest3.py bustest3.adf [tests/data/amiga_bustest3.bootblock]   # Copper jumping into a sub-list and back on every line (5/30 MOVEs, with/without planes); copper_jump_bus_timing_matches_the_bus_test compares with WinUAE's numbers
python tools/amiga_bustest4.py bustest4.adf [tests/data/amiga_bustest4.bin]   # the Interference cube's blit loop (without/with its 1x30 blits) next to Copper lists like the demo's (per-line BPLxPTL sub-lists, 30 COLOR MOVEs every second/every line) and 5 planes; the program is loaded by the boot block (trackdisk); cpu_blitter_copper_bus_timing_matches_the_bus_test compares with WinUAE's numbers (EMU_BUSTEST4_REF)
cargo run --release -p emu-desktop -- --machine amiga --disk demo-a.adf --disk demo-b.adf   # disks fill DF0:, DF1: … (--option drives=N), extra ones swap into DF0: with Ctrl+D (or drop an image on the window); ADF/HD ADF/extended ADF/DMS; stereo Paula audio; mouse in port 0 (click to capture), joystick port 1, Amiga keys on the Windows keys, HELP on Insert, Ctrl+Win+Win resets; --deinterlace weave|bob (Ctrl+I)
cargo run -p emu-headless -- --machine zx81 --rom-dir roms --frames 200 --snapshot zx81.png   # ROMs in roms/zx81/; --model ts1000; --option ram=1k|2k|16k, traps=off, roms=stub; --tape program.p
EMU_ROM_DIR=$PWD/roms cargo test -p emu-tests --release --test zx81_boot -- --ignored --nocapture   # Phase 10 gate (ZX81): boots, PRINT, a .P file saved and loaded through the traps
cargo run -p emu-headless -- --machine cpc --rom-dir roms --frames 150 --snapshot cpc.png   # ROMs in roms/cpc/; --model 6128|664|464, --option crtc=0|1|2, dos=amsdos|parados|none
cargo run --release -p emu-desktop -- --machine cpc --option joy1=amx   # AMX mouse (click to capture); joy2=joystick daisy-chains a second stick; --crt gt65 green monitor
cargo run --release -p emu-desktop -- --machine cpc --disk game.dsk   # CPC 664/6128: DSK/EDSK in drive A (RUN"DISC or CAT)
cargo run --release -p emu-desktop -- --machine cpc --model 464 --tape game.cdt   # CPC cassette (CDT/TZX); on the 664/6128 type |TAPE first
cargo run -p emu-headless -- --machine sim6502 --debug          # JSON-lines debug protocol on 127.0.0.1:6510
cargo run -p emu-headless -- --machine c64 --vice --preview --run   # + VICE binary monitor (6502), live preview http://127.0.0.1:6580/
python tools/polydbg.py regs                 # Python debug client (stdlib only): --help, or `shell` for a prompt; `chips`, Atari `dlist`
python -m unittest discover -s tools         # its unit tests
cargo run --release -p emu-desktop -- --machine c64 --disk game.d64 --autostart   # window, sound, keyboard, gamepad
cargo run --release -p emu-desktop -- --machine c64 --option drive8=virtual --disk ./myprgs --autostart   # virtual drive; a folder of PRGs as disk
cargo run --release -p emu-desktop -- --machine vic20 --tape game.tap   # datasette (PLAY pressed; a .prg is recorded as a tape); F9 play, Shift+F9 record, F10 stop, F11 rewind
cargo run --release -p emu-desktop -- --machine c64 --cart game.crt   # cartridge: CRT (C64/C128), raw VIC-20 (load address + ROM) or C16/Plus/4 (16/32 KB) dump; also emu-headless
# Linux build deps for emu-desktop: libasound2-dev libudev-dev
```

## Testing

- CPU cores: Tom Harte SingleStepTests (6502, Z80, 68000), Klaus Dormann 6502 functional test, ZEXALL (Z80), WinUAE `cputester` data (68000).
- C64 / Commodore 8-bit: Lorenz test suite, VICE test programs.
- Atari: Acid800, Altirra test programs. CPC: CRTC/Z80 timing test programs. Amiga: vAmigaTS.
- Regression: headless run → N frames → compare against reference images in `tests/reference/` (exact match; update only with an explanation in the commit).
- Test-suite data and ROMs are fetched by `tools/fetch-testdata.sh`, never committed if license forbids.
- A new chip or machine feature is not done without tests and savestate round-trip coverage.
- Freezer: tests for each `FreezeMode` (blocked write, frame reapply, banked addresses), cheat-search narrowing on a scripted test program, and freezer-cartridge freeze/restart round trips.
- CRT: CPU-path output is compared against reference images per profile (tolerance-based, not exact); signal-stage tests use known test patterns (color bars, PAL color-mix pairs, Atari artifact stripes). GPU shaders are checked against the CPU path within a tolerance.
- Devices: each `PortDevice` has unit tests at pin level (e.g. 1351 POT values for a given mouse delta); input replay files under `tests/replays/` drive end-to-end tests (load game, replay input, compare frame).

## Roadmap and current phase

Work in phase order; each phase ends at a verifiable gate. **Current phase: 8 (open; Phase 9 done 2026-10-22 on the user's request before Phase 8's blocks: 400/800 with OS-B boot to BASIC (cartridge) or the memo pad, 65XE and 130XE boot to BASIC, Acid800 on the 130XE 52/57 like the 800XL, the 130XE's CPU/ANTIC bank switching covered by machine tests).** (Phase 1 done 2026-10-20: 6502 Harte + Dormann green, CI green. Phase 2 done 2026-10-20: Lorenz 264/264 with ROMs, PAL + NTSC C64. Phase 3 done 2026-10-20: VS64 debugs the C64 through the VICE binary monitor adapter. Phase 4 done 2026-10-21: VIC-20, PET, C16/Plus/4 and C128 boot to BASIC with real ROMs; the items under "Deferred from Phase 4" are held back. Phase 5 done 2026-10-21: Acid800 52/57 on PAL and NTSC; still failing `antic_virtdma`, `gtia_phantomdma` (need the CPU's bus value in ANTIC/GTIA), `antic_hscrolbug`, `antic_hiresbug`, `gtia_psuedomodee`. Phase 6 done 2026-10-21: CPC 464/664/6128 boot, tape and disc work with real ROMs; Arnold acid tests 66/70 (94.3 %, CRTC type 1) in the 8 programs that report results; still failing PPI mode 1 port C output latching (two tests the author's type 0/1 CPCs fail too), "port b read (vsync on)" (depends on where in the frame it starts) and "ppi i/o decode (f5xx)" (port B /EXP level unverified); `crtctest` and `disc/fdc` (needs its own test disc) do not finish. Phase 6b done 2026-10-21: Atari 2600 NTSC/PAL with TIA, RIOT, all planned bank-switching schemes and controllers; 24 games of a ROM collection run with correct scheme detection; DPC (Pitfall II), Supercharger and SECAM later. Phase 7 done 2026-10-22: Amiga 500 OCS — 68000 passes all Tom Harte 68000 tests (three contradictory cases skipped); Kickstart 1.2, 1.3, 2.0 and 3.1 reach their insert-disk screens; Workbench 1.3 boots from ADF and works with mouse and keyboard; demos with their own trackloaders run (Desert Dream); Copper and Blitter are debug targets; the items under "Deferred from Phase 7" are held back.)

| # | Phase | Est. | Gate |
| --- | --- | --- | --- |
| 1 | Foundation: workspace, CI, scheduler, 6502 core, bus, savestates, headless runner, minimal debug server (pause/step/regs) | 6 wk | 6502 tests green |
| 2 | C64 boots: VIC-II, CIA, SID, keyboard, PRG/D64/T64, simplified 1541 | 10 wk | Lorenz suite ≥ 90 % |
| 3 | Debugger + Preview v1: native protocol + Python client, VICE adapter, snapshot + stream | 6 wk | VS64 debugs the C64 |
| 4 | Commodore family: VIC-20, PET, C16/Plus/4 (TED), C128 (8502+Z80, VDC, MMU), 1571/1581, IEC bus | 12 wk | All boot to BASIC |
| 5 | Atari 800XL: ANTIC, GTIA, POKEY, PIA, ATR/XEX/CAS — debugger unchanged | 8 wk | Acid800 mostly green |
| 6 | CPC 464, 664, 6128 (models of one machine): reuse Z80, CRTC, Gate Array, AY, FDC, DSK/CDT | 6 wk | BASIC + disk work |
| 6b | Atari 2600 (NTSC, PAL): 6507, TIA (video, sound, inputs), RIOT 6532, cartridge bank switching (2K/4K, F8/F6/F4 + Superchip, FE, E0, E7, 3F, FA), joystick, paddles, keypad, driving controller, console switches | 6 wk | own TIA/RIOT test programs green; user-supplied games run correctly |
| 7 | Amiga 500 (OCS): 68000, Agnus DMA slots, Copper, Blitter, Denise, Paula, ADF via MFM, Copper debugger | 20 wk | Workbench 1.3 boots |
| 8 | Expansion: graphics data views (sprites/chars/bitmaps), overlays, rewind, partial frames, A1200 (AGA), web frontend | 10 wk | Release 1.0 |
| 9 | Atari 8-bit models as configuration of `machine-atari8`: 400/800 (OS-A/B, four joystick ports, no PORTB banking, RESET as NMI), 65XE, 130XE (128 KB, PORTB banking with separate CPU/ANTIC access) | 2 wk | all boot their OS; Acid800 on the 130XE as on the 800XL; a 130XE bank-switching test program |
| 10 | Z80 home computers: Sinclair ZX81 (`machine-zx81`: CPU-generated display, ULA line counting, 1/16 KB RAM, tape via ear/mic, .P files) and Tandy TRS-80 Model I/III (`machine-trs80`: text screen with block graphics, keyboard matrix, cassette, Expansion Interface with FDC 1771/1793 and DMK/JV1/JV3 images) | 4 wk | both boot to BASIC with real ROMs; a ZX81 .P program and a TRS-80 disk load |
| 11 | ColecoVision (`machine-coleco`): Z80, TMS9918A (`chips-ti`, 16 KB VRAM, sprites, all modes), SN76489, keypad controllers with joystick, cartridge ROMs; console panel POWER/RESET | 4 wk | BIOS boots; user-supplied cartridges run; TMS9918A mode and sprite tests against reference pictures |
| 12 | Nintendo NES/Famicom (`machine-nes`): 2A03 as a cpu-6502 variant (no decimal mode, APU on chip), PPU 2C02/2C07 with VRAM/OAM (`chips-nintendo`), mappers NROM, MMC1, UxROM, CNROM, MMC3 (`devices-cartridge::nes`, iNES/NES 2.0), controllers; console panel POWER/RESET | 8 wk | nestest and blargg's CPU/PPU/APU test ROMs green; user-supplied games run |
| 13 | Mattel Intellivision (`machine-intv`): new `cpu-cp1610` (16-bit), STIC (MOBs, GRAM/GROM), AY-3-8914 PSG, hand controllers; Executive ROM and GROM user-supplied | 6 wk | Executive ROM title screen; user-supplied cartridges run; CP1610 instruction tests |
| 14 | Vectrex (`machine-vectrex`): new `cpu-6809`, VIA 6522 (reuse `chips-cbm`), AY-3-8912 (reuse `chips-amstrad`), vector display with a vector frame type next to `FrameRef` (beam segments with intensity; phosphor decay in `video-crt`; debugger/preview carry the segments and a rasterised picture — core trait extension, ask first), cartridges, controllers | 6 wk | built-in Mine Storm plays; 6809 instruction tests green |
| 15 | Sinclair ZX Spectrum 48K and 128K (`machine-spectrum`, added 2026-10-22 on the user's request): Z80 at 3.5 MHz with the ULA's memory contention (`chips-sinclair`), 48K and 128K/+2 paging with AY sound, border and attribute display with the floating bus, TAP/TZX tape, SNA/Z80 snapshots, Kempston and Sinclair joysticks; ROMs user-supplied | 6 wk | 48K and 128K boot to BASIC with real ROMs; a TAP game loads; contention and border timing checked against known test programs |
| 16 | Apple II, II+ and IIe (`machine-apple2`, added 2026-10-22 on the user's request): 6502 (II/II+) and 65C02 (enhanced IIe) as cpu-6502 variants, soft-switch I/O page, text/lo-res/hi-res with NTSC artifact colours (composite signal path), language card, IIe auxiliary memory and 80 columns, Disk II controller at nibble level (`drives-other`, DSK/DO/PO converted to GCR 6-and-2 tracks, WOZ), speaker, paddles/joystick, cassette; ROMs user-supplied | 8 wk | II+ and IIe boot to Applesoft with real ROMs; DOS 3.3 and ProDOS disks boot; artifact colours checked against reference pictures |
| 17 | Apple Macintosh 128K/512K/Plus/SE (`machine-mac`, added 2026-10-22 on the user's request): 68000 (reuse `cpu-m68k`), 512 × 342 monochrome video from RAM, VIA 6522 (reuse `chips-cbm`), SCC 8530, IWM floppy with 400K/800K GCR disks (DSK/DiskCopy 4.2 images), sound from the PWM buffer, ADB (SE) / the original keyboard and mouse protocol, SCSI 5380 (Plus/SE) with hard-disk images, PRAM/RTC; ROMs user-supplied. Mac II (68020, colour, NuBus) later, after the 68020 from Phase 8 | 10 wk | Plus and SE boot System 6 from a floppy and a SCSI image to the Finder; mouse and keyboard work |

Device work by phase (part of each gate):

- **Phase 1:** `PortDevice`, `MediaSlot`, `MediaBackend`, `InputEvent` and the input queue defined in `emu-core` (even if only stubbed) — they shape the `Machine` trait.
- **Phase 2:** C64 control ports with digital joystick, keyboard matrix + keymaps, `emu-input` with gamepad and keyboard-as-joystick, 1541 slot with image + copy-on-write backend.
- **Phase 3:** remote device commands, input record/replay, interactive web preview.
- **Phase 4:** 1351 mouse, paddles, virtual drive mode, host-directory backend, 1571/1581, datasette.
- **Phase 5/6:** Atari joystick/paddles/SIO drives; CPC dual joystick, AMX mouse, FDC drives.
- **Phase 7:** Amiga mouse (quadrature), gameports, DF0–DF3 with MFM write-back.
- **Phase 8:** light pens, real-hardware backends (feature-gated), mapping-profile UI.

Memory freezer by phase:

- **Phase 1:** freeze bitmaps share the watchpoint mechanism in `DebugHooks`; `Freeze` in savestate format.
- **Phase 3:** freeze + cheat-search commands in the debug server, frozen-address highlighting.
- **Phase 4 (deferred, see "Deferred from Phase 4"):** C64/C128 freezer cartridges (Action Replay, Retro Replay, Final Cartridge III), cheat-file import.
- **Phase 7:** Amiga Action Replay, word/long freezes in chip/fast RAM.
- **Phase 8:** cheat UI in desktop/web frontends, per-game cheat database.

CRT work by phase:

- **Phase 1:** `FrameRef` with full overscan + signal metadata (pixel clock, line timing, standard, output type, field parity).
- **Phase 2:** `video-crt` skeleton: aspect correction, integer scaling, simple scanline shader; C64 1702/1084S composite profiles without signal stage.
- **Phase 3:** CPU path for preview snapshots (`filter=crt`).
- **Phase 4:** PAL delay-line + NTSC composite signal stage (C64/VIC-20/Plus/4), C128 VDC on RGB profile, PET monochrome.
- **Phase 5/6:** Atari NTSC artifact colors (gate includes an artifacting test image); CPC CTM644 and GT65 profiles.
- **Phase 7:** Amiga RGB, interlace field stage (weave/bob/persistence), 1084S RGB profile.
- **Phase 8:** full CRT stage (mask types, bloom, curvature, phosphor decay), profile editor, "lite" preset for WASM.

### Machine backlog

Added 2026-10-21. On 2026-10-22 the user scheduled the remaining machines as Phases 9–14 (see the phase table; order by effort, the Vectrex vector frame type is decided in its own phase, not prepared in Phase 8). What stays here is detail for those phases and the items without a phase:

- **Atari 8-bit models** (Phase 9; configuration of `machine-atari8`, Phase 5 ships the 800XL only): **400** (and 800: OS-A/B, four joystick ports, no PORTB banking, RESET as NMI), **65XE** (800XL hardware in XE case), **130XE** (128 KB, PORTB banking with separate CPU/ANTIC access).
- **Atari 2600 (VCS)**: now Phase 6b (see the phase table): `machine-atari2600` with the 6507 (cpu-6502 variant), TIA and RIOT 6532 in `chips-atari`, bank-switching schemes in `devices-cartridge::atari2600`. Later: DPC (Pitfall II), Starpath Supercharger, SECAM.
- **CPC 464 and 664** alongside the 6128 in Phase 6 (see the phase table).
- **Amstrad CPC Plus** (464+/6128+/GX4000): ASIC (hardware sprites, DMA sound, split screen, 4096-colour palette, soft scroll), CRTC types 3/4, CPR cartridges; as models of `machine-cpc` after Phase 6.
- **Mattel Intellivision** (Phase 13): new CPU core `cpu-cp1610` (GI CP1610, 16-bit), STIC video (MOBs, GRAM/GROM), PSG AY-3-8914; Executive ROM and GROM user-supplied.
- **Sinclair ZX81** (Phase 10): reuses `cpu-z80`; the CPU itself generates the display (NMI/HALT-driven, ULA counts lines), 1 KB/16 KB RAM, tape via ear/mic; ROM user-supplied.
- **Nintendo NES (Famicom)** (Phase 12): new CPU variant 2A03 (6502 without decimal mode, APU on chip), PPU 2C02/2C07 with its own VRAM and OAM, cartridge mappers (NROM, MMC1, UxROM, CNROM, MMC3 …), controllers; console panel: POWER, RESET.
- **ColecoVision** (Phase 11): reuses `cpu-z80`; TMS9918A video (16 KB VRAM, sprites), SN76489 sound, keypad controllers with joystick; BIOS user-supplied; console panel: POWER, RESET.
- **Vectrex** (Phase 14): 6809 CPU (new `cpu-6809`), VIA 6522 (reuse from `chips-cbm`), AY-3-8912 (reuse from `chips-amstrad`), and a vector display: needs a vector frame type next to `FrameRef` (beam lines with intensity, phosphor decay in `video-crt`) — the debugger/preview contract then carries vectors or a rasterized picture; overlays; console panel: POWER, RESET.
- **Tandy TRS-80 Model I/III** (Phase 10): reuses `cpu-z80`; memory-mapped text screen with block graphics, keyboard matrix in the address space, cassette, Expansion Interface / FDC 1771/1793 floppies (DMK/JV1/JV3 images); ROMs user-supplied.

### Deferred from Phase 7

Held back on 2026-10-22; not part of the Phase 7 gate. Done on 2026-10-22 (user request "implement the rest of Phase 7"): Paula attach modes and UART (loopback option), 1 MB chip RAM, Zorro II fast RAM via autoconfig, A1000 (WCS, bootstrap ROM) and A2000 models, HD drives/disks, extended ADF, DMS, bob deinterlacing in the desktop (`--deinterlace`, Ctrl+I), woven fields in the preview, LED filter as a second-order Butterworth. Still open:

- **Action Replay (A500 side-port freezer):** needs its ROM (user-supplied) and the side-port hardware (freeze NMI, ROM/RAM overlay). User decision 2026-10-22: left out (no ROM).
- **Paula:** disk write precompensation has no effect in the bit-cell drive model (documented); the fixed output filter stays first-order. The serial port has no host device yet (only `serial=loopback`).
- **A1000 bootstrap path** (`wcs=disk`) is tested with the stub only — no bootstrap ROM or Kickstart disk at hand.
- **ECS** (Agnus 8372A/8375 registers, Denise 8373, 2 MB chip RAM): with the A1200/AGA block in Phase 8 (user decision 2026-10-22). The Agnus id in VPOSR stays 0 (OCS) with 1 MB chip RAM.
- **Chip-bus residual (CPU next to a long Copper sub-list):** `tools/amiga_bustest4.py` rows 2, 3, 6, 7 (30 COLOR MOVEs on every second or every line, their tail competing with the CPU for the free slots of the bitplane fetch) run 2.6–3.4 % faster than WinUAE; the other rows and the demo's own frame work match within 2 %. WinUAE's own result for one configuration varies by up to 6 % with the loop's start phase, ours does not.
- **Chip-bus residual:** the bus test (`tools/amiga_bustest.py`, reference from a cycle-exact WinUAE A500 in `tests/tests/amiga_boot.rs`) matches within 0.2 % except the 4-plane + per-line-Copper row, where we are 0.7 % fast after modelling the Copper's bus-holding wake-up after WAIT (a second wake-up slot changes nothing). The measurement itself moves up to 1 % with the loop's start phase.
- **Timing details:** Blitter idle cycles reach the CPU (unverified against hardware), the D channel's one-word pipeline; the interrupt latencies (VERTB two colour clocks after the strobe, IPL four after a request, the 68000 sampling at the start of its last bus cycle) follow community measurements, not a datasheet; CPU accesses to the CIAs and the autovector use a simplified E-clock rule.
- **Sprite DMA:** VSTOP wins over VSTART (a sprite parked with VSTART = VSTOP fetches control words, not data; found with Interference on 2026-10-23, where the data fetches of eight parked sprites cost the CPU 16 slots a line and its frame work ran past the field). Not verified beyond the demo and the unit test.
- **Sprites, checked against vAmigaTS photographs on 2026-10-22** (`tests/tests/vamigats.rs`): all 18 `sprcoll` collision tests, the `clip`/`diwclip` window clipping (sprites appear only after the line's first BPL1DAT, or on lines without planes) and `sprbpu` match. `sprites1` looks right at a glance (priorities not checked pixel by pixel). Open: the `timing` family — in `sprtimpos1` the second sprite column (SPRxPOS rewritten by the Copper near the sprite start) comes out white instead of in the sprites' colours; `sprtimctl1`, `sprtimdat*`, `sprdmadat*`, `sprxpos` not compared.
- **AGA** (A1200) in Phase 8.

### Deferred from Phase 4

Held back on 2026-10-21 so Phase 5 can start; pick them up in a later phase. They are not part of the Phase 4 gate.

- **Freezer cartridges (C64/C128):** Action Replay, Retro Replay, Final Cartridge III (later Super Snapshot) as `devices-cartridge` types with a freeze button (NMI + bank switch), triggerable from desktop UI, hotkey and the debug command `press_cartridge_button`; tests for freeze/restart round trips.
- **Cheat-file import:** POKE lists and VICE monitor commands into `cheats/<machine>/<game-hash>.toml`.
- **True-emulated 2031** (IEEE-488 drive with its own CPU, `dos2031` ROM); the PET has the virtual IEEE drive only.
- **Input replay files** in `tests/replays/` for end-to-end game tests (need game images, user-supplied).
- **Preview:** marking the current raster line in partial frames.
- **VICE adapter:** D64 autostart through the VICE monitor protocol.
- **Signal stage on the GPU:** composite/S-Video decoding runs on the CPU (`video_crt::signal`) in every frontend; a WGSL version is optional.
- **CI:** move `actions/checkout`/`actions/cache` from v4 (Node 20) to v5.
- **Known issues:** `tests/tests/preview.rs::stream_sends_key_frame_then_deltas_and_the_paused_state` fails in `--release` builds (passes in debug); in the `polydbg` shell, the `stopped` event from `pause` is printed again after the next command.

When finishing a phase, update "Current phase" above.

## Coding conventions

- `#![forbid(unsafe_code)]` in all crates except where a measured hot path justifies it; document every `unsafe` block.
- No panics in emulation paths for guest-controlled input; illegal states become `StopReason::Fault`.
- Chip registers documented with the address, bit layout and a source reference (datasheet / hardware reference manual) in doc comments.
- Every outside source (document, test suite, data download, consulted web page) goes into `docs/sources.md` as well as into the module's doc comment.
- Name things after the hardware (`vic2`, `agnus`, `cia1`), not after host concepts.
- Small, focused commits; message format: `<crate>: <change>` (e.g. `cpu-6502: fix decimal-mode flags`).
- Prefer adding a failing test from a known test suite before fixing an accuracy bug.

## When unsure

- Accuracy questions: consult hardware docs (C64 Programmer's Reference, "VIC-II article" by Christian Bauer, Amiga Hardware Reference Manual, Altirra Hardware Reference, CPC CRTC docs) — not other emulators' source.
- Architecture questions: prefer extending `emu-core` traits over special-casing in debugger/preview. Ask before changing a core trait signature.
