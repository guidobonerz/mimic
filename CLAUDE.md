# CLAUDE.md — Multi-Platform Retro Emulator

This file guides Claude Code when working in this repository. Read it fully before making changes.

## Project in one paragraph

A cycle-accurate, multi-machine emulator for classic home computers — the whole Commodore family (PET, VIC-20, C16/Plus/4, C64, C128, Amiga), Atari 800XL and Amstrad CPC 6128 — built on one shared core. Its distinguishing feature: **every machine automatically gets the same remote debugger and a live preview stream** (frames of the running machine, also headless and while halted at a breakpoint). Debugger and preview depend only on core traits, never on a concrete machine.

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
  media/           Image formats: PRG, D64/D71/D81, T64/TAP, CRT, ADF(→MFM), ATR, XEX, CAS, DSK, CDT
  media-backends/  Where media bytes come from: image file, copy-on-write overlay, host directory, real hardware
  emu-input/       Host side: gamepads (gilrs), keyboard-as-joystick, mouse capture, mapping profiles
  video-crt/       CRT/monitor emulation: composite/RGB signal path, WGSL shaders (wgpu), CPU fallback, monitor profiles
  machine-pet/  machine-vic20/  machine-c64/  machine-ted/  machine-c128/
  machine-amiga/  machine-atari8/  machine-cpc/
  machine-sim6502/ Reference/test machine: 6502 + 64 KB RAM + minimal raster chip (not real hardware)
  emu-machines/    Fills the machine registry; the only crate that links every machine-*
  emu-debug/       Debug server: native protocol + adapters (VICE binary monitor, GDB RSP, DAP)
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
    fn beam(&self) -> Option<BeamPos>;                  // frame, raster line, cycle in line, hpos; None without a beam
    fn memory_spaces(&self) -> Vec<MemSpace>;           // CPU, chip RAM, fast RAM, ROM, drive, VDC RAM…
    fn chips(&self) -> Vec<ChipInfo>;                   // register dumps for debugger views
    fn devices(&self) -> Vec<DeviceId>;                 // debug targets, main CPU first: "cpu", 1541, C128 Z80, Copper, Blitter
    fn debug_target(&mut self, dev: DeviceId) -> Option<&mut dyn DebugTarget>; // regs, set_reg, peek/poke per MemSpace, disassemble
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
| Amiga 500 | 7.09379 MHz | Agnus DMA slot arbitration per line; Copper/Blitter; CPU only gets free chip-RAM slots; ADF converted to MFM tracks |
| Atari 800XL | CPU 1.773447 MHz | ANTIC DMA steals cycles; display lists; DLIs |
| CPC 6128 | Z80 4 MHz | Wait states align memory access to 1 µs; CRTC register tricks |

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

- the **full visible area including overscan/border** (not cropped), as indexed pixels + palette (8-bit machines) or RGB (Amiga);
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
- **Adapters** translate to: VICE binary monitor (port 6502, all Commodore 8-bit machines), GDB Remote Serial Protocol (esp. m68k), Debug Adapter Protocol (VS Code).
- Machine-specific extensions are negotiated via `capabilities`; generic clients must work without them.
- Every protocol change needs a contract test and a version bump in the handshake.

## Live preview

- `GET /snapshot.png?machine=<id>[&screen=vic|vdc]` — single frame on demand.
- WebSocket `/preview` — configurable fps (1–50), scale, format (QOI/PNG, delta frames).
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
bash tools/fetch-testdata.sh                 # Harte + Dormann CPU test data into testdata/ (~850 MB)
EMU_TESTDATA_DIR=$PWD/testdata cargo test -p cpu-6502 --release -- --ignored   # every 10th Harte test
EMU_HARTE_FULL=1 EMU_TESTDATA_DIR=$PWD/testdata cargo test -p cpu-6502 --release -- --ignored
python tools/check-deps.py                   # dependency direction (golden rules 1 and 10)
cargo bench -p cpu-6502                      # before/after when touching hot paths
cargo build -p emu-web --target wasm32-unknown-unknown
cargo run -p emu-headless -- --machine c64 --prg demo.prg --frames 300 --snapshot out.png
cargo run -p emu-headless -- --machine sim6502 --debug          # JSON-lines debug protocol on 127.0.0.1:6510
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

Work in phase order; each phase ends at a verifiable gate. **Current phase: 2.** (Phase 1 done 2026-10-20: 6502 Harte + Dormann green, CI green.)

| # | Phase | Est. | Gate |
| --- | --- | --- | --- |
| 1 | Foundation: workspace, CI, scheduler, 6502 core, bus, savestates, headless runner, minimal debug server (pause/step/regs) | 6 wk | 6502 tests green |
| 2 | C64 boots: VIC-II, CIA, SID, keyboard, PRG/D64/T64, simplified 1541 | 10 wk | Lorenz suite ≥ 90 % |
| 3 | Debugger + Preview v1: native protocol + Python client, VICE adapter, snapshot + stream | 6 wk | VS64 debugs the C64 |
| 4 | Commodore family: VIC-20, PET, C16/Plus/4 (TED), C128 (8502+Z80, VDC, MMU), 1571/1581, IEC bus | 12 wk | All boot to BASIC |
| 5 | Atari 800XL: ANTIC, GTIA, POKEY, PIA, ATR/XEX/CAS — debugger unchanged | 8 wk | Acid800 mostly green |
| 6 | CPC 6128: reuse Z80, CRTC, Gate Array, AY, FDC, DSK/CDT | 6 wk | BASIC + disk work |
| 7 | Amiga 500 (OCS): 68000, Agnus DMA slots, Copper, Blitter, Denise, Paula, ADF via MFM, Copper debugger | 20 wk | Workbench 1.3 boots |
| 8 | Expansion: GDB stub, DAP, overlays, rewind, partial frames, A1200 (AGA), web frontend | 10 wk | Release 1.0 |

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
- **Phase 4:** C64/C128 freezer cartridges (Action Replay, Retro Replay, Final Cartridge III), cheat-file import.
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

When finishing a phase, update "Current phase" above.

## Coding conventions

- `#![forbid(unsafe_code)]` in all crates except where a measured hot path justifies it; document every `unsafe` block.
- No panics in emulation paths for guest-controlled input; illegal states become `StopReason::Fault`.
- Chip registers documented with the address, bit layout and a source reference (datasheet / hardware reference manual) in doc comments.
- Name things after the hardware (`vic2`, `agnus`, `cia1`), not after host concepts.
- Small, focused commits; message format: `<crate>: <change>` (e.g. `cpu-6502: fix decimal-mode flags`).
- Prefer adding a failing test from a known test suite before fixing an accuracy bug.

## When unsure

- Accuracy questions: consult hardware docs (C64 Programmer's Reference, "VIC-II article" by Christian Bauer, Amiga Hardware Reference Manual, Altirra Hardware Reference, CPC CRTC docs) — not other emulators' source.
- Architecture questions: prefer extending `emu-core` traits over special-casing in debugger/preview. Ask before changing a core trait signature.
