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
  cpu-6502/        6502 family: 6502, 6510, 8502, 7501/8501, 6502C (SALLY), 2A03 (NES), CMOS 65C02 (Apple IIe) via variant flags
  cpu-z80/         Z80 (C128, CPC, ZX81, Spectrum; internal T-states reported with their bus address for the Spectrum's contention)
  cpu-m68k/        68000 (Amiga, Macintosh) and 68020 (A1200)
  cpu-65816/       WDC 65C816 (SNES 5A22 core)
  cpu-spc700/      Sony SPC700 (SNES sound CPU)
  chips-cbm/       VIC, VIC-II(e), TED, VDC 8563, SID, CIA, VIA, PIA, MMU 8722
  chips-amiga/     Agnus (DMA, Copper, Blitter), Denise, Paula, Gary, CIA
  chips-atari/     ANTIC, GTIA, POKEY, PIA
  chips-amstrad/   CRTC 6845, Gate Array, AY-3-8912, 8255 PPI, FDC 765
  chips-ti/        TMS9918A/TMS9929A VDP, SN76489 (ColecoVision)
  chips-sinclair/  ZX81 ULA; ZX Spectrum ULA (display, contention, floating bus, port $FE)
  chips-nintendo/  NES: PPU 2C02/2C07 with its palette, APU of the 2A03/2A07; SNES: S-PPU, S-DSP
  drives-cbm/      IEC bus, 1541, 1571, 1581 (each with its own CPU)
  drives-other/    Amiga floppy (DF0–DF3, MFM), Atari SIO drives (810/1050), CPC FDC drives, Apple Disk II, Mac IWM + Sony 400K/800K drives, datasettes, Apple cassette, Spectrum cassette, ZX Microdrives
  devices-input/   Emulated input peripherals: joysticks, paddles, mice (1351, Amiga, AMX, Kempston), light pens, keypads
  devices-cartridge/ Expansion-port cartridges: C64 CRT types (banking, EasyFlash), VIC-20 and C16/Plus/4 ROM carts
  devices-net/     Devices that reach outside through emu-core::host: Ultimate 64 command interface (UCI), WiC64
  media/           Image formats: PRG, D64/D71/D81, T64/TAP, CRT, ADF(→MFM), ATR, XEX, CAS, DSK, CDT/TZX and Spectrum TAP, Apple DSK/DO/PO(→GCR)/WOZ, Mac DSK/DiskCopy 4.2(→GCR), WAV, MDR
  media-backends/  Where media bytes come from: image file, copy-on-write overlay, host directory, real hardware
  emu-input/       Host side: gamepads (gilrs), keyboard-as-joystick, mouse capture, mapping profiles
  emu-profile/     Start profiles (profiles.yaml: settings + named machine setups) for the frontends
  video-crt/       CRT/monitor emulation: composite/RGB signal path, WGSL shaders (wgpu), CPU fallback, monitor profiles
  machine-pet/  machine-vic20/  machine-c64/  machine-ted/  machine-c128/
  machine-amiga/  machine-atari8/  machine-cpc/  machine-coleco/  machine-nes/  machine-snes/
  machine-apple2/  Apple II, II+, IIe (enhanced and unenhanced) as models; Mockingboard in slot 4
  machine-mac/     Macintosh 128K, 512K, Plus, SE as models: RTC, keyboard, ADB, SCC 8530, SCSI 5380 with hard disks
  machine-zx81/    Sinclair ZX81 / TS1000
  machine-spectrum/ ZX Spectrum 48K, 128K, +2 as models: AY, tape traps, SNA/Z80, joysticks, Multiface 128, Interface 1 with Microdrives
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
    fn attach_host(&mut self, host: HostServices) {}  // network + sandboxed folder (emu-core::host, in a Journal) for UCI/WiC64; PortDevice::attach_host likewise
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
| NES / Famicom | 2A03 1.789773 MHz (PAL 2A07 1.662607), PPU 3 (PAL 3.2) dots a CPU cycle | 2A03 = 6502 without decimal mode; PPU dots of a cycle run 2 before / 1 after the CPU's bus access, /NMI and /IRQ sampled late in the cycle (`Cpu6502::sample_interrupts`); OAM and DMC DMA stop the CPU on read cycles (get/put alignment); mappers watch PPU A12 (MMC3); no frame buffer in VRAM: the PPU renders per dot |
| SNES / Super Famicom | master 21.477 MHz (PAL 21.281), 65C816 bus cycles of 6/8/12 master clocks by address; SPC700 1.024 MHz | One instruction per CPU step with a callback per bus cycle (the machine advances its clock by the access speed and runs line events: HDMA, refresh, IRQs, auto joypad); the S-PPU draws a line at a time; the sound module runs behind and catches up at port accesses; 512 × 478 RGB picture |
| ColecoVision | Z80A 3.58 MHz (PAL 3.55) | TMS9918A/9929A: 342 pixel clocks per line = 228 T-states; VDP /INT drives /NMI (no further NMI until the status is read); 1 KB RAM mirrored over $6000–$7FFF; controller segments (joystick/keypad) selected by port writes; spinner steps raise /INT |
| Apple II / II+ / IIe | 6502 (enhanced IIe: 65C02) 1.0205 MHz | The video scanner reads RAM every cycle (65 cycles × 262 lines): its byte is the floating bus; text/lo-res/hi-res addresses from Sather's adder; colour only from the composite signal (14.318 MHz dots, the frame at 28.6 MHz with the picture 1.5 dots off the colour reference; colour burst off in text); language card banks; IIe aux memory switches and INTCXROM/SLOTC3ROM/INTC8ROM; Disk II LSS from the P6 PROM (address lines scrambled) or a functional model |
| Macintosh 128K/512K/Plus/SE | 68000 7.8336 MHz | Video takes every other 4-clock RAM slot while a line shows (SE: one in four); 704 × 370 pixel periods at 15.6672 MHz, 512 × 342 shown; ROM overlay at reset (VIA PA4; SE: off at the first ROM access); VIA on the E clock (CPU ÷ 10) with the keyboard/ADB on its external-clock shift register; mouse X1/Y1 on the SCC's DCD; the 400K drive's speed from the sound buffer's low bytes (a shift register the ROM steps), the 800K drive regulates itself; Mac ROMs start with their checksum (the first SSP) |
| ZX Spectrum 48K / 128K / +2 | Z80 3.5 MHz (128K, +2: 3.5469 MHz) | The ULA holds the CPU's clock on contended addresses (`$4000–$7FFF`, the 128K's odd banks) while it fetches the display: 6,5,4,3,2,1,0,0 from T-state 14335 (128K 14361), also on internal T-states with such an address on the bus (`Z80::idle_cycles`) and on I/O by the port's high byte and bit 0; the floating bus 3 T-states after the first contended one; 224 × 312 (228 × 311) T-states; /INT for 32 (36) T-states at the frame start; a read of $7FFD on the 128K latches the floating byte into the paging port; picture drawn lazily up to each screen write / border change |
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
- **Graphics data views** (Phase 8; trait extension approved and built 2026-10-25, see "Phase 8 status"): where a machine has them, the debugger also delivers the graphics the video chip works from, decoded and raw, without the debugger knowing the chip:
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
EMU_TESTDATA_DIR=$PWD/testdata cargo test -p cpu-6502 --release -- --ignored   # every 10th Harte test (6502 and the NES 2A03's nes6502 set)
EMU_HARTE_FULL=1 EMU_TESTDATA_DIR=$PWD/testdata cargo test -p cpu-6502 --release -- --ignored   # also the 65C02 (synertek65c02 set)
EMU_TESTDATA_DIR=$PWD/testdata cargo test -p cpu-z80 --release --test harte -- --ignored   # every 10th Z80 test (EMU_HARTE_ONLY=ed: only "ed …" files)
EMU_TESTDATA_DIR=$PWD/testdata cargo test -p cpu-m68k --release --test harte -- --ignored --nocapture   # every 10th 68000 test (EMU_HARTE_FULL=1 all, EMU_HARTE_ONLY=MOVE,DIVU file prefixes)
python tools/m68020_random.py build m020.adf [--seed N] [--bin tests/data/m68020_random.bin]   # 68020 random instruction tests as an A1200 boot disk; boot it in WinUAE (A1200, 68EC020, no FPU/JIT, 2 MB chip, KS 3.1, disk writable) until the screen turns green, then `extract m020.adf tests/data/m68020_random.ref`
cargo test -p emu-tests --release --test m68020_random -- --ignored --nocapture   # compares the bare 68020 with the reference (report target/tmp/m68020-random.txt, trend per case); random_68020_disk_runs_on_the_a1200 with EMU_ROM_DIR + EMU_M68020_ADF checks the disk on mimic's A1200 first
EMU_TESTDATA_DIR=$PWD/testdata cargo test -p cpu-z80 --release --test zex -- --ignored --nocapture   # ZEXDOC + ZEXALL, minutes
EMU_TESTDATA_DIR=$PWD/testdata cargo test -p emu-tests --release --test lorenz lorenz_suite_stub -- --ignored   # Lorenz, no ROMs (CI gate)
EMU_ROM_DIR=$PWD/roms EMU_TESTDATA_DIR=$PWD/testdata cargo test -p emu-tests --release --test lorenz lorenz_suite -- --ignored --exact
python tools/fetch_vice_testprogs.py && EMU_ROM_DIR=$PWD/roms EMU_TESTDATA_DIR=$PWD/testdata cargo test -p emu-tests --release --test vice_testprogs -- --ignored --nocapture   # VICE test programs on the C64 (c64-testlist.in, debugcart $D7FF + reference screenshots; EMU_VICE_FILTER=CIA/; report target/tmp/vice-testprogs.txt, EMU_VICE_DUMP=1 saves failed pictures)
EMU_ROM_DIR=$PWD/roms EMU_TESTDATA_DIR=$PWD/testdata cargo test -p emu-tests --release --test acid800 -- --ignored --nocapture   # Acid800 on the 800XL (EMU_ACID_MODEL=800xl-ntsc|130xe, EMU_ACID_FILTER=antic)
EMU_ROM_DIR=$PWD/roms EMU_TESTDATA_DIR=$PWD/testdata cargo test -p emu-tests --release --test cpc_acid -- --ignored --nocapture   # Arnold Acid tests on the CPC 6128 (~10 min; EMU_CPC_FILTER=ppi, EMU_CPC_CRTC=0, EMU_CPC_FRAMES=n, EMU_CPC_TAIL=n, EMU_CPC_BREAK=hex)
python tools/check-deps.py                   # dependency direction (golden rules 1 and 10)
cargo run --release -p emu-desktop -- --profile mac-se-755   # start profile from ./profiles.yaml or %APPDATA%\mimic\profiles.yaml (example: docs/profiles.example.yaml); without --machine/--profile the window lists the profiles; --list-profiles, --save-profile NAME, --profiles FILE; also mimic-headless
cargo bench -p cpu-6502                      # before/after when touching hot paths
cargo build -p emu-web --target wasm32-unknown-unknown
bash tools/build-web.sh && python -m http.server -b 127.0.0.1 -d target/web 8080   # web frontend (all machines as WebAssembly; ROMs/images uploaded in the browser), http://127.0.0.1:8080/?machine=c64&options=roms=stub&start=1
node tools/web-smoke.mjs                     # every machine runs in the wasm module (stub ROMs, no browser; CI)
cargo run -p emu-headless -- --machine c64 --prg demo.prg --frames 300 --snapshot out.png
cargo run -p emu-headless -- --machine vic20 --model ntsc --option ram=8k --rom-dir roms --frames 200 --snapshot vic.png   # ROMs in roms/<machine>/, roms/drives/
cargo run -p emu-headless -- --machine pet --model 8032 --option hz=50 --rom-dir roms --frames 200 --snapshot pet.png   # 2001, 3032, 4032, 8032
cargo run -p emu-headless -- --machine plus4 --rom-dir roms --frames 200 --snapshot plus4.png   # also --machine c16, --model ntsc
cargo run -p emu-headless -- --machine c128 --option display=80 --rom-dir roms --frames 600 --screen vdc --snapshot c128.png   # --screen vic|vdc
cargo run -p emu-headless -- --machine atari8 --rom-dir roms --frames 350 --snapshot atari.png   # ROMs in roms/atari/; --model 800xl|65xe|130xe|800|400 (+ -ntsc); --option basic=off (XL/XE: self-test; 400/800: no BASIC cartridge), ram=8k|16k|32k|48k (400/800), port3/port4 (400/800)
EMU_ROM_DIR=$PWD/roms cargo test -p emu-tests --release --test atari_boot other_models_boot -- --ignored --nocapture   # 130XE, 800 (OS-B + BASIC cartridge / memo pad) and 400 boot
cargo run -p emu-headless -- --machine atari8 --rom-dir roms --disk game.atr --frames 600 --snapshot a.png   # also --prg game.xex, --tape game.cas, --cart game.car; --option drives=0..4, drive=810|1050|xf551
cargo run -p emu-headless -- --machine atari2600 --cart game.a26 --frames 120 --snapshot vcs.png   # no system ROM; --model auto (default: NTSC/PAL from the program's field length)|ntsc|pal; --option bank=f8|…|dpc; --option roms=stub (built-in test cartridge)
cargo run -p emu-headless -- --machine amiga --rom-dir roms --frames 400 --snapshot amiga.png   # ROMs in roms/amiga/ ("Kickstart 1.3.rom", kick13.rom …); --model 500|1000|2000|500plus|1200 (+ -ntsc; 500plus = ECS, Kickstart 2.0; 1200 = 68EC020 + AGA, Kickstart 3.1); --option kickstart=1.2|1.3|2.0|3.0|3.1, chip=256k|512k|1m|2m, chipset=ocs|ecs|aga, agnus=ocs|ecs, denise=ocs|ecs, slow=512k|0, fast=0|512k|…|8m, drives=1..4, drivetype=dd|hd, serial=none|loopback, wcs=rom|disk (A1000), roms=stub
EMU_ROM_DIR=$PWD/roms cargo test -p emu-tests --release --test amiga_boot expansions_and_models_boot -- --ignored --nocapture   # 1 MB chip + fast RAM via autoconfig, A2000, A1000 (WCS), A500+ (ECS, 2 MB chip, Kickstart 2.0/3.1) and the A1200 (Kickstart 3.1) reach insert-disk
EMU_DMS_DIR=/path/with/dms cargo test -p media --release --test dms_files -- --ignored --nocapture   # real DMS archives unpack with good checksums (freely distributed ones from Aminet)
EMU_ROM_DIR=$PWD/roms EMU_VAMIGATS_DIR=/path/to/vAmigaTS/Denise/Sprites EMU_CLXDAT=1 cargo test -p emu-tests --release --test vamigats -- --ignored --nocapture   # vAmigaTS programs (GPL test data, not in the repo): snapshots in target/tmp/vamigats/ to compare with the suite's A500 photographs; EMU_FILTER=name, EMU_FRAMES=n
EMU_ROM_DIR=$PWD/roms cargo test -p emu-tests --release --test amiga_boot -- --ignored --nocapture   # Kickstarts reach "insert disk" and boot a generated boot block; trace_kickstart: EMU_KICK=1.3, EMU_FRAMES=n, EMU_DISK=a.adf, EMU_DISK1=b.adf, EMU_OPTION=drives=2; workbench_13_boots (Phase 7 gate) with EMU_WB_ADF=workbench1.3.adf
python tools/amiga_bustest.py bustest.adf [tests/data/amiga_bustest.bootblock]   # chip-bus timing test disk (boot it in WinUAE/on hardware; the test chip_bus_arbitration_matches_the_bus_test compares us with WinUAE's numbers)
python tools/amiga_bustest2.py bustest2.adf [tests/data/amiga_bustest2.bootblock]   # Blitter vs 68000 (BLTPRI off/on, 5 planes + Copper, D-only); blitter_bus_arbitration_matches_the_bus_test compares with WinUAE's numbers
python tools/amiga_bustest3.py bustest3.adf [tests/data/amiga_bustest3.bootblock]   # Copper jumping into a sub-list and back on every line (5/30 MOVEs, with/without planes); copper_jump_bus_timing_matches_the_bus_test compares with WinUAE's numbers
python tools/amiga_agatest.py agatest.adf [FMODE]   # AGA test disk (A1200): 13 bands with DDFSTRT $48…$18, 8 high-res planes with FMODE $F, all 8 sprites at 64 pixels; compare WinUAE's picture with mimic's (which sprites survive, where the fetched data starts)
python tools/amiga_bustest4.py bustest4.adf [tests/data/amiga_bustest4.bin]   # the Interference cube's blit loop (without/with its 1x30 blits) next to Copper lists like the demo's (per-line BPLxPTL sub-lists, 30 COLOR MOVEs every second/every line) and 5 planes; the program is loaded by the boot block (trackdisk); cpu_blitter_copper_bus_timing_matches_the_bus_test compares with WinUAE's numbers (EMU_BUSTEST4_REF)
cargo run --release -p emu-desktop -- --machine amiga --disk demo-a.adf --disk demo-b.adf   # disks fill DF0:, DF1: … (--option drives=N), extra ones swap into DF0: with Ctrl+D (or drop an image on the window); ADF/HD ADF/extended ADF/DMS; stereo Paula audio; mouse in port 0 (click to capture), joystick port 1, Amiga keys on the Windows keys, HELP on Insert, Ctrl+Win+Win resets; --deinterlace weave|bob (Ctrl+I)
cargo run -p emu-headless -- --machine apple2 --rom-dir roms --disk game.dsk --frames 600 --snapshot a2.png   # ROMs in roms/apple2/ (Apple part numbers); --model iie|iie-unenhanced|iiplus|ii; --option ram=4k..48k, lc=on|off (II/II+), aux=64k|1k|none (IIe), slot4=mockingboard|none, slot6=disk2|none, port=joystick|paddles|none, roms=stub; DSK/DO/PO/NIB/WOZ, --tape program.wav; --crt tv for colour
EMU_ROM_DIR=$PWD/roms cargo test -p emu-tests --release --test apple2 -- --include-ignored --nocapture   # Phase 16: all models boot, a built disk boots through P5/P6, cassette W/R through the monitor; EMU_APPLE2_DISK=game.dsk (EMU_APPLE2_MODEL, EMU_FRAMES) boots a user disk
cargo run -p emu-headless -- --machine mac --rom-dir roms --disk system.dsk --frames 1200 --snapshot mac.png   # ROMs in roms/mac/ (mac128k.rom, mac512k.rom, macplus.rom, macse.rom or the H/L chips); --model plus|128k|512k|se; --option ram=512k|1m|2m|4m (Plus/SE), serial=loopback, roms=stub; floppies DSK (400K/800K) or DiskCopy 4.2, then hard disk images (whole 512-byte blocks) go to SCSI 0…6: each --disk takes the next drive that accepts it
EMU_ROM_DIR=$PWD/roms cargo test -p emu-tests --release --test mac -- --include-ignored --nocapture   # Phase 17: all models reach the disk icon, a built MFS disk boots on each (400K/800K); EMU_MAC_DISK=a.dsk;hd.img (EMU_MAC_MODEL, EMU_FRAMES) boots user disks
EMU_ROM_DIR=$PWD/roms EMU_MAC_MODEL=se cargo test -p machine-mac --release trace_rom -- --ignored --nocapture   # development aid: where a real ROM spends its time (EMU_DISK, EMU_FRAMES, EMU_BREAK=pc, EMU_DIS=addr,n, EMU_RING_VECTOR=n, EMU_TRACE_AT=clock)
cargo run -p emu-headless -- --machine spectrum --rom-dir roms --tape game.tzx --autostart --frames 1500 --snapshot zx.png   # ROMs in roms/spectrum/ (Spec48.rom, spec128.rom, specp2.rom; mf128.rom, specif1.rom); --model 48k|128k|plus2; --prg game.z80/.sna; --option traps=off, joystick=kempston|sinclair1|sinclair2|cursor|none, mouse=kempston, issue=2, multiface=on (panel MULTIFACE), if1=on, microdrives=1..8 (--disk cart.mdr), roms=stub
EMU_ROM_DIR=$PWD/roms EMU_TESTDATA_DIR=$PWD/testdata cargo test -p emu-tests --release --test spectrum_boot -- --ignored --nocapture   # Phase 15: all models boot and PRINT, a built TAP loads with and without the trap, the Fuse Test passes on 48K/128K/+2, Multiface menu and return, Interface 1 FORMAT/SAVE/CAT/LOAD on a Microdrive; user_tap: EMU_SPECTRUM_TAP=game.tzx (EMU_SPECTRUM_MODEL, EMU_FRAMES, EMU_OPTION)
cargo run -p emu-headless -- --machine zx81 --rom-dir roms --frames 200 --snapshot zx81.png   # ROMs in roms/zx81/; --model ts1000; --option ram=1k|2k|16k, traps=off, roms=stub; --tape program.p
EMU_ROM_DIR=$PWD/roms cargo test -p emu-tests --release --test zx81_boot -- --ignored --nocapture   # Phase 10 gate (ZX81): boots, PRINT, a .P file saved and loaded through the traps
cargo run -p emu-headless -- --machine coleco --rom-dir roms --cart game.col --frames 300 --snapshot cv.png   # ROMs in roms/coleco/ (coleco.rom, 8 KB); --model pal; --option port1/port2=controller|superaction|driving|roller|none, roms=stub (own mini BIOS that starts $55AA/$AA55 cartridges)
EMU_ROM_DIR=$PWD/roms EMU_COLECO_CARTS=/path/to/carts cargo test -p emu-tests --release --test coleco_boot -- --ignored --nocapture   # Phase 11 gate: BIOS screen NTSC/PAL, every cartridge in the folder runs (pictures in target/tmp)
cargo run -p emu-headless -- --machine nes --cart game.nes --frames 300 --snapshot nes.png   # no system ROM; --model pal; --option port1/port2=controller|zapper|arkanoid|none, fourscore=on (ports 3 and 4)
cargo run -p emu-headless -- --machine snes --cart game.sfc --frames 400 --snapshot snes.png   # --model pal; --option port1/port2=pad|mouse|superscope|none, multitap=on, roms=stub (own sound boot loader); optional ROM roms/snes/spc700.rom (64 bytes)
EMU_TESTDATA_DIR=$PWD/testdata cargo test -p cpu-65816 --release --test harte -- --ignored --nocapture   # every 10th 65816 test (EMU_HARTE_FULL=1 all 5 080 400)
EMU_TESTDATA_DIR=$PWD/testdata cargo test -p cpu-spc700 --release --test harte -- --ignored --nocapture   # all 256 000 SPC700 tests
EMU_TESTDATA_DIR=$PWD/testdata cargo test -p emu-tests --release --test nes_testroms -- --ignored --nocapture   # Phase 12: nestest, blargg's CPU/PPU/APU/DMA/MMC3 suites from nes-test-roms (EMU_NES_FILTER=text; ~5 min)
cargo run -p emu-headless -- --machine cpc --rom-dir roms --frames 150 --snapshot cpc.png   # ROMs in roms/cpc/; --model 6128|664|464, --option crtc=0|1|2, dos=amsdos|parados|none
cargo run --release -p emu-desktop -- --machine cpc --option joy1=amx   # AMX mouse (click to capture); joy2=joystick daisy-chains a second stick; --crt gt65 green monitor
cargo run --release -p emu-desktop -- --machine cpc --disk game.dsk   # CPC 664/6128: DSK/EDSK in drive A (RUN"DISC or CAT)
cargo run --release -p emu-desktop -- --machine cpc --model 464 --tape game.cdt   # CPC cassette (CDT/TZX); on the 664/6128 type |TAPE first
cargo run -p emu-headless -- --machine sim6502 --debug          # JSON-lines debug protocol on 127.0.0.1:6510
cargo run -p emu-headless -- --machine c64 --vice --preview --run   # + VICE binary monitor (6502), live preview http://127.0.0.1:6580/
python tools/user_guide.py                   # user guide PDF -> docs/mimic-user-guide.pdf (needs reportlab); keep its text in step with the frontends
python tools/polydbg.py regs                 # Python debug client (stdlib only): --help, or `shell` for a prompt; `chips`, Atari `dlist`; `gfx [VIEW opts]`, `overlay ids`, `picture --overlays beam`, `rewind [N] --on 1`, `back N`
cargo run -p emu-headless -- --machine c64 --disk game.d64 --cheats cheats --cheat lives.pokes   # cheat database cheats/<machine>/<crc32>.toml applies by image CRC; --cheat FILE: .toml, POKE list (.pokes/.bas), VICE monitor (.mon), address/value codes (.txt)
cargo build --release -p emu-headless --features real-hardware   # real-hardware backends (also `greaseweazle`/`opencbm` alone); never needed for the normal build
cargo test -p media-backends --all-features   # flux PLL, GCR/MFM decoding, fake Greaseweazle transport, fake OpenCBM drive
mimic-headless --import-flux game.d64 --gw-port COM5 [--gw-format 1541|1571|1581|amiga|amiga-hd|ibm…] [--gw-drive a] [--gw-tracks N] [--gw-step auto|1|2] [--gw-revs 2]   # Greaseweazle: real disk → D64/G64/D71/D81/ADF/IMG
mimic-headless --import-drive copy.d64 [--cbm-device 8] [--cbm-type 1541]   # ZoomFloppy (OpenCBM, opencbm.dll in PATH or MIMIC_OPENCBM): real drive → image; --export-drive game.d64 writes one back
mimic-headless --machine c64 --rom-dir roms --real-drive 8 [--real-drive-write] --frames 600 --snapshot c64.png   # a real 1541 as the emulated drive's disk
python -m unittest discover -s tools         # its unit tests
cargo run --release -p emu-desktop -- --machine c64 --disk game.d64 --autostart   # window, sound, keyboard, gamepad
cargo run --release -p emu-desktop -- --machine c64 --option drive8=virtual --disk ./myprgs --autostart   # virtual drive; a folder of PRGs as disk
cargo run --release -p emu-desktop -- --machine vic20 --tape game.tap   # datasette (PLAY pressed; a .prg is recorded as a tape); F9 play, Shift+F9 record, F10 stop, F11 rewind
cargo run --release -p emu-desktop -- --machine c64 --cart game.crt   # cartridge: CRT (C64/C128), raw VIC-20 (load address + ROM) or C16/Plus/4 (16/32 KB) dump; also emu-headless
cargo run --release -p emu-desktop -- --machine c64 --rom-dir roms --option uci=on --uci-dir ./ultimate   # Ultimate 64 command interface at $DF1C-$DF1F: DOS targets 1/2 on the folder (sandboxed), network target 3 (TCP/UDP), control 4 identify; --net off = no network; also emu-headless
cargo run --release -p emu-desktop -- --machine c64 --rom-dir roms --option userport=wic64   # WiC64 (firmware 2.x, legacy + protocol 2: HTTP GET/POST, TCP, IP/MAC/time) on the user port; host network through emu-core::host (media-backends), journalled so record_stop/replay_input replay exactly; --option net=off or --net off
# Linux build deps for emu-desktop: libasound2-dev libudev-dev
```

## Testing

- CPU cores: Tom Harte SingleStepTests (6502, Z80, 68000), Klaus Dormann 6502 functional test, ZEXALL (Z80), WinUAE `cputester` data (68000).
- C64 / Commodore 8-bit: Lorenz test suite, VICE test programs.
- Atari: Acid800, Altirra test programs. CPC: CRTC/Z80 timing test programs. Amiga: vAmigaTS. NES: nestest and blargg's suites (nes-test-roms collection).
- Regression: headless run → N frames → compare against reference images in `tests/reference/` (exact match; update only with an explanation in the commit).
- Test-suite data and ROMs are fetched by `tools/fetch-testdata.sh`, never committed if license forbids.
- Suite runners (Lorenz, Acid800, CPC Acid, VICE test programs, 68020 random tests) record a trend against their previous run with `emu_tests::trend` (`target/tmp/trend/<suite>.txt` + `-history.tsv`): passed before → now, newly passing, newly failing. Report it with every result.
- A new chip or machine feature is not done without tests and savestate round-trip coverage.
- Freezer: tests for each `FreezeMode` (blocked write, frame reapply, banked addresses), cheat-search narrowing on a scripted test program, and freezer-cartridge freeze/restart round trips.
- CRT: CPU-path output is compared against reference images per profile (tolerance-based, not exact); signal-stage tests use known test patterns (color bars, PAL color-mix pairs, Atari artifact stripes). GPU shaders are checked against the CPU path within a tolerance.
- Devices: each `PortDevice` has unit tests at pin level (e.g. 1351 POT values for a given mouse delta); input replay files under `tests/replays/` drive end-to-end tests (load game, replay input, compare frame).

## Roadmap and current phase

Work in phase order; each phase ends at a verifiable gate. **Current phase: 8 (open, see "Phase 8 status"; Phase 15 ZX Spectrum begun 2026-10-26 on the user's request, see "Phase 15 status"; Phase 16 Apple II begun 2026-10-24 on the user's request, see "Phase 16 status"; Phase 17 Macintosh begun 2026-10-24 on the user's request, see "Phase 17 status"; Phase 9 done 2026-10-22 on the user's request before Phase 8's blocks: 400/800 with OS-B boot to BASIC (cartridge) or the memo pad, 65XE and 130XE boot to BASIC, Acid800 on the 130XE 52/57 like the 800XL, the 130XE's CPU/ANTIC bank switching covered by machine tests).** (Phase 1 done 2026-10-20: 6502 Harte + Dormann green, CI green. Phase 2 done 2026-10-20: Lorenz 264/264 with ROMs, PAL + NTSC C64. Phase 3 done 2026-10-20: VS64 debugs the C64 through the VICE binary monitor adapter. Phase 4 done 2026-10-21: VIC-20, PET, C16/Plus/4 and C128 boot to BASIC with real ROMs; the items under "Deferred from Phase 4" are held back. Phase 5 done 2026-10-21: Acid800 52/57 on PAL and NTSC; 54/57 since 2026-10-25 (`antic_hiresbug`: players collide in vertical blank after a hires last line; `antic_hscrolbug`: playfield DMA follows ANTIC's DMA clock, Altirra HRM 4.12); still failing `antic_virtdma`, `gtia_phantomdma` (need the CPU's bus value in ANTIC/GTIA), `gtia_psuedomodee`. Phase 6 done 2026-10-21: CPC 464/664/6128 boot, tape and disc work with real ROMs; Arnold acid tests 66/70 (94.3 %, CRTC type 1) in the 8 programs that report results; still failing PPI mode 1 port C output latching (two tests the author's type 0/1 CPCs fail too), "port b read (vsync on)" (depends on where in the frame it starts) and "ppi i/o decode (f5xx)" (port B /EXP level unverified); `crtctest` and `disc/fdc` (needs its own test disc) do not finish. Phase 6b done 2026-10-21: Atari 2600 NTSC/PAL with TIA, RIOT, all planned bank-switching schemes and controllers; 24 games of a ROM collection run with correct scheme detection; DPC (Pitfall II) added 2026-10-25 (picture and three-voice music; `EMU_PITFALL2=path` test), Supercharger and SECAM later. Phase 7 done 2026-10-22: Amiga 500 OCS — 68000 passes all Tom Harte 68000 tests (three contradictory cases skipped); Kickstart 1.2, 1.3, 2.0 and 3.1 reach their insert-disk screens; Workbench 1.3 boots from ADF and works with mouse and keyboard; demos with their own trackloaders run (Desert Dream); Copper and Blitter are debug targets; the items under "Deferred from Phase 7" are held back.)

| # | Phase | Est. | Gate |
| --- | --- | --- | --- |
| 1 | Foundation: workspace, CI, scheduler, 6502 core, bus, savestates, headless runner, minimal debug server (pause/step/regs) | 6 wk | 6502 tests green |
| 2 | C64 boots: VIC-II, CIA, SID, keyboard, PRG/D64/T64, simplified 1541 | 10 wk | Lorenz suite ≥ 90 % |
| 3 | Debugger + Preview v1: native protocol + Python client, VICE adapter, snapshot + stream | 6 wk | VS64 debugs the C64 |
| 4 | Commodore family: VIC-20, PET, C16/Plus/4 (TED), C128 (8502+Z80, VDC, MMU), 1571/1581, IEC bus | 12 wk | All boot to BASIC |
| 5 | Atari 800XL: ANTIC, GTIA, POKEY, PIA, ATR/XEX/CAS — debugger unchanged | 8 wk | Acid800 mostly green |
| 6 | CPC 464, 664, 6128 (models of one machine): reuse Z80, CRTC, Gate Array, AY, FDC, DSK/CDT | 6 wk | BASIC + disk work |
| 6b | Atari 2600 (NTSC, PAL): 6507, TIA (video, sound, inputs), RIOT 6532, cartridge bank switching (2K/4K, F8/F6/F4 + Superchip, FE, E0, E7, 3F, FA), joystick, paddles, keypad, driving controller, console switches | 6 wk | own TIA/RIOT test programs green; user-supplied games run correctly |
| 7 | Amiga 500 (OCS, ECS added 2026-10-23): 68000, Agnus DMA slots, Copper, Blitter, Denise, Paula, ADF via MFM, Copper debugger | 20 wk | Workbench 1.3 boots |
| 8 | Expansion: graphics data views (sprites/chars/bitmaps), overlays, rewind, partial frames, A1200 (AGA; begun 2026-10-23), web frontend | 10 wk | Release 1.0 |
| 9 | Atari 8-bit models as configuration of `machine-atari8`: 400/800 (OS-A/B, four joystick ports, no PORTB banking, RESET as NMI), 65XE, 130XE (128 KB, PORTB banking with separate CPU/ANTIC access) | 2 wk | all boot their OS; Acid800 on the 130XE as on the 800XL; a 130XE bank-switching test program |
| 10 | Z80 home computers: Sinclair ZX81 (`machine-zx81`: CPU-generated display, ULA line counting, 1/16 KB RAM, tape via ear/mic, .P files) and Tandy TRS-80 Model I/III (`machine-trs80`: text screen with block graphics, keyboard matrix, cassette, Expansion Interface with FDC 1771/1793 and DMK/JV1/JV3 images) | 4 wk | both boot to BASIC with real ROMs; a ZX81 .P program and a TRS-80 disk load |
| 11 | ColecoVision (`machine-coleco`; begun 2026-10-23): Z80, TMS9918A (`chips-ti`, 16 KB VRAM, sprites, all modes), SN76489, keypad controllers with joystick, cartridge ROMs; console panel POWER/RESET | 4 wk | BIOS boots; user-supplied cartridges run; TMS9918A mode and sprite tests against reference pictures |
| 12 | Nintendo NES/Famicom (`machine-nes`; begun 2026-10-23): 2A03 as a cpu-6502 variant (no decimal mode, APU on chip), PPU 2C02/2C07 with VRAM/OAM (`chips-nintendo`), mappers NROM, MMC1, UxROM, CNROM, MMC3 (`devices-cartridge::nes`, iNES/NES 2.0), controllers; console panel POWER/RESET | 8 wk | nestest and blargg's CPU/PPU/APU test ROMs green; user-supplied games run |
| 12b | Super Nintendo / Super Famicom (`machine-snes`, added 2026-10-23 on the user's request, after the NES; begun 2026-10-23): new `cpu-65816` (WDC 65C816, 8/16-bit modes, emulation mode) as the 5A22 with its DMA/HDMA, multiplier/divider and auto-joypad read; PPU1/PPU2 (S-PPU: modes 0–7, Mode 7 rotation/scaling, sprites, windows, colour math, mosaic, hi-res and interlace; `chips-nintendo`); S-SMP sound CPU (new `cpu-spc700`) with the S-DSP (8 voices, BRR samples, echo) and the IPL ROM; cartridges LoROM/HiROM/ExHiROM with SRAM (`devices-cartridge::snes`, SFC/SMC with or without copier header), coprocessors later (DSP-1 first, then Super FX, SA-1); controllers, mouse; NTSC/PAL; console panel POWER/RESET | 16 wk | 65C816 and SPC700 instruction tests (Tom Harte) green; user-supplied LoROM/HiROM games run with sound; Mode 7 and colour-math pictures checked against known test ROMs |
| 13 | Dropped 2026-10-25 (user decision): the Mattel Intellivision is off the list | – | – |
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
- **Atari 8-bit expansions** (added 2026-10-25 on the user's request, no phase yet): **VBXE** (VideoBoard XE: FPGA video with its own VRAM, 640-pixel modes, 256-colour palettes, overlay over ANTIC/GTIA, attribute maps, blitter; FX core), **VBXL** (the user's term; clarify which board and its specs when the item starts — presumably the VBXE for the XL series), **Ultimate1MB** (1 MB RAM expansion with flash-ROM OS/BASIC/SpartaDOS X slots, PBI BIOS, configuration registers); as options of `machine-atari8`; firmware/cores user-supplied.
- **Atari 2600 (VCS)**: now Phase 6b (see the phase table): `machine-atari2600` with the 6507 (cpu-6502 variant), TIA and RIOT 6532 in `chips-atari`, bank-switching schemes in `devices-cartridge::atari2600`. DPC (Pitfall II) done 2026-10-25 (`devices-cartridge::atari2600::dpc`; music oscillator taken as the documented ~20 kHz); the `auto` model (default) picks NTSC or PAL from the program's field lengths. Later: Starpath Supercharger, SECAM.
- **CPC 464 and 664** alongside the 6128 in Phase 6 (see the phase table).
- **Amstrad CPC Plus** (464+/6128+/GX4000): ASIC (hardware sprites, DMA sound, split screen, 4096-colour palette, soft scroll), CRTC types 3/4, CPR cartridges; as models of `machine-cpc` after Phase 6.
- **Sinclair ZX81** (Phase 10): reuses `cpu-z80`; the CPU itself generates the display (NMI/HALT-driven, ULA counts lines), 1 KB/16 KB RAM, tape via ear/mic; ROM user-supplied.
- **Nintendo NES (Famicom)** (Phase 12): new CPU variant 2A03 (6502 without decimal mode, APU on chip), PPU 2C02/2C07 with its own VRAM and OAM, cartridge mappers (NROM, MMC1, UxROM, CNROM, MMC3 …), controllers; console panel: POWER, RESET.
- **Super Nintendo (SNES/Super Famicom)** (Phase 12b, added 2026-10-23): three processors — the 5A22 (65C816 core with DMA/HDMA), the S-SMP (SPC700) with the S-DSP and its own 64 KB audio RAM — and two PPUs sharing `chips-nintendo` with the NES; cartridge coprocessors (DSP-n, Super FX, SA-1, S-DD1, Cx4) as `devices-cartridge::snes` types, only DSP-1 planned in the phase; ROMs (games) user-supplied, the SPC700 IPL ROM is 64 bytes inside the chip (user-supplied too).
- **ColecoVision** (Phase 11): reuses `cpu-z80`; TMS9918A video (16 KB VRAM, sprites), SN76489 sound, keypad controllers with joystick; BIOS user-supplied; console panel: POWER, RESET.
- **Vectrex** (Phase 14): 6809 CPU (new `cpu-6809`), VIA 6522 (reuse from `chips-cbm`), AY-3-8912 (reuse from `chips-amstrad`), and a vector display: needs a vector frame type next to `FrameRef` (beam lines with intensity, phosphor decay in `video-crt`) — the debugger/preview contract then carries vectors or a rasterized picture; overlays; console panel: POWER, RESET.
- **Atari ST/TT series** (added 2026-10-24 on the user's request, TT and Falcon added 2026-10-27, no phase yet): 520ST/1040ST, STF/STFM, Mega ST, STE (and Mega STE later) as models of one machine; 68000 (reuse `cpu-m68k`), GLUE/MMU/Shifter video (low/medium/high resolution, STE hardware scrolling and 4096-colour palette), MFP 68901, YM2149 (reuse `chips-amstrad` AY), STE DMA sound, ACIA 6850 with the IKBD (HD6301 keyboard/mouse/joystick controller), WD1772 FDC with ST/MSA/STX images, ACSI hard disks, MIDI; TOS ROMs user-supplied (EmuTOS as a free optional download). Later the **TT030** (68030 at 32 MHz — new `cpu-m68k` model with MMU and data cache — TT RAM, TT video modes with 256 colours and the 1280 × 960 monochrome mode, second MFP, SCSI 5380 (reuse the Mac's), SCC 8530 (reuse the Mac's), VME bus) and the **Falcon030** (68030 at 16 MHz, VIDEL video with true colour, DSP 56001, SDMA/CODEC sound, IDE, SCSI) as further models; TOS 3.x/4.x user-supplied.
- **Acorn Archimedes family** (added 2026-10-24 on the user's request, widened to the whole family 2026-10-27, no phase yet): A305/A310/A440 first, then A3000, A410/1, A540, A5000, A3010/A3020/A4000 (ARM250: ARM2 core with MEMC, VIDC and IOC on one chip) and the A4 laptop; new `cpu-arm` (ARM2, later ARM3 with cache, ARM250), MEMC (memory controller, logical-to-physical page mapping; MEMC1a with 4 MB per MEMC, two MEMCs on the A440/A540), VIDC (video and 8-channel sound; VIDC enhancer for VGA modes), IOC (timers, interrupts, keyboard serial link), the keyboard's own controller with mouse, WD1772 FDC with ADF (ADFS) images (the later models' 82C710/82C711 with 1.6 MB high-density disks), ST506 and IDE hard disks, podules later; RISC OS / Arthur ROMs user-supplied (RISC OS 3.x free for non-commercial use from RISC OS Open as an optional download).
- **Acorn BBC Micro** (added 2026-10-27 on the user's request, no phase yet): Model B, B+ and Master 128 (Electron later) as models of one machine; 6502 at 2 MHz (reuse `cpu-6502`; the Master's 65SC12 as a CMOS variant), Video ULA and 6845 CRTC (reuse `chips-amstrad`'s CRTC) for modes 0–7 including the SAA5050 teletext mode 7, 6522 VIAs (reuse `chips-cbm`) for the keyboard, sound, speech, user port and printer, SN76489 sound (reuse `chips-ti`), 6850 ACIA with cassette and RS-423, 8271 (Model B) and 1770/1772 FDCs with SSD/DSD/ADF images, Tube later, sideways ROMs/RAM and the Master's MOS 3.20 banking; MOS, BASIC and DFS/ADFS ROMs user-supplied.
- **Sinclair QL** (added 2026-10-27 on the user's request, no phase yet): 68008 (8-bit bus variant of `cpu-m68k`, 4-clock bus cycles on a byte bus), ZX8301 (video with modes 4 and 8, 512 × 256 and 256 × 256, display DMA from RAM, master clock) and ZX8302 (RTC, serial, Microdrive and network interface, IPC link), the 8049 IPC (keyboard, sound, joysticks; its program as a user-supplied ROM or a functional model), two internal Microdrives (reuse `drives-other::microdrive` with the QL's sector format and MDV images), RS-232, Trump Card/Gold Card and floppy expansions later; QDOS / Minerva ROMs user-supplied (Minerva is free).
- **Texas Instruments TI-99/4A** (added 2026-10-27 on the user's request, no phase yet): TMS9900 16-bit CPU at 3 MHz (new `cpu-tms9900`: workspace registers in RAM, context switching, CRU bit-serial I/O), TMS9918A/9929A video (reuse `chips-ti`; the video RAM is the only large RAM, the CPU sees 256 bytes of scratchpad plus the 16-bit console ROM), TMS9919/SN94624 sound (reuse the SN76489 of `chips-ti`), TMS9901 (interrupts, keyboard, joysticks, cassette), GROM (the Graphics ROMs with their auto-incrementing address and the GPL interpreter), cartridges (GROM + ROM, the 8 KB banking of later carts), 32 KB expansion RAM with its wait states, cassette, Peripheral Expansion Box with the TI disk controller (DSK images) and the Speech Synthesizer later; console ROM, GROMs and cartridge images user-supplied.
- **EACA Colour Genie EG2000** (added 2026-10-27 on the user's request, no phase yet): Z80 at 2.2 MHz (reuse `cpu-z80`), 6845 CRTC (reuse `chips-amstrad`'s CRTC) with 40 × 24 (and 40 × 25) text from a character generator plus a programmable 128-character set in RAM and 160 × 96 (… × 102) low-resolution graphics, 16 colours, AY-3-8910 sound (reuse `chips-amstrad`'s AY), 16/32 KB RAM, memory-mapped keyboard matrix, cassette (CAS images), light pen, joysticks, the floppy controller with the EG2000's DOS (CGD images) and the RS-232/printer cards later; BASIC ROM and the character generator user-supplied.
- **Görlitz printer interface for the C64** (added 2026-10-26 on the user's request, no phase yet): the German Görlitz interface that connects a Centronics (parallel) printer to the C64; clarify the model, how it is connected (serial IEC bus as a printer device 4/5, or user port) and its command set/firmware when the item starts. Needs an emulated printer behind it (`PortKind::CentronicsPrinter`) with output to a host file through the media/host layer.
- **Device mapper tool** (added 2026-10-25 on the user's request, no phase yet): a tool (window/UI) to map host keyboard keys, gamepads/joysticks, mouse and other host input to each machine's keys, ports and devices, stored as `emu-input` mapping profiles (per machine, per game), and linked from the start profiles (`emu-profile`).
- **Tandy TRS-80 Model I/III** (Phase 10): reuses `cpu-z80`; memory-mapped text screen with block graphics, keyboard matrix in the address space, cassette, Expansion Interface / FDC 1771/1793 floppies (DMK/JV1/JV3 images); ROMs user-supplied.

### Phase 8 status (open; full scope chosen 2026-10-25)

User choices 2026-10-25: everything of Phase 8; graphics data views for all machines (trait extension approved); the web frontend in full; overlays through a core trait extension (approved); rewind as a ring of savestates plus `step_back`; the basic mapping-profile UI only (the device mapper tool stays in the backlog); real-hardware backends for the user's ZoomFloppy and Greaseweazle; the A1200 gets the 68020 cache and timing while ROM and ADF are missing. Intellivision (Phase 13) dropped.
Done: `emu_core::gfx` (`Machine::gfx_views`/`gfx_view`, `Canvas` helpers) with views for every raster machine (VIC-II, VIC, TED, PET, VDC, ANTIC/GTIA, TIA objects, Amiga sprites/bitplanes/palette, CPC screen, TMS9918, NES PPU, S-PPU, Apple II text/lo-res/hi-res, Mac screen, ZX81); `emu_core::overlay` (`overlays`/`set_overlays`/`overlay`/`frame_row`, recording only while switched on, not machine state) with machine overlays (C64/C128 badlines+sprites, TED badlines, Atari dlist/players/dli, 2600 objects, Amiga dma/sprites/copper, CPC crtc/vsync, Coleco sprites/limit, NES sprites/sprite0/irq, SNES sprites/hdma/irq, Apple II modes) and generic `beam`/`grid`; `BeamPos::hpos` is the frame column on every machine (fixed for VIC-II, TED, PET, NES, SNES, Coleco, Apple II); `emu_core::rewind` (reverse-delta ring) with protocol `rewind_config`/`rewind_info`/`rewind`/`step_back` and desktop Ctrl+R (`--rewind-mb`); protocol v9 (`gfx_list`/`gfx_get`, `overlay_list`/`overlay_set`/`overlay_get`, `frame`, events `stopped_frame`, `cheats_loaded`, `cheats_load` formats); preview `/gfx`, `/gfx/<view>.png`, `/overlays`, `overlay=` on snapshot/stream, viewer toggles and a graphics panel; polydbg `gfx`, `overlay`, `picture`, `rewind`, `back`; `emu-cheats` (cheat files with patches, POKE/VICE/codes imports, CRC-32 database, search) in the debugger, headless and web; `emu-web` web frontend (raw wasm exports, page in `crates/emu-web/www`, files kept in IndexedDB, cheats, savestates, profiles; all 18 machines run as wasm; Sonic 4 (SNES) and a C64 with real ROMs ran in headless Chrome). Full CRT stage (`video-crt`): shadow/aperture/slot masks, bloom, curvature, vignette, phosphor persistence (`Crt` keeps the history; desktop ping-pong targets), `lite` preset, the same model on CPU and GPU (GPU vs CPU test with an adapter), reference images per preset in `tests/reference/crt/` (`EMU_UPDATE_REFERENCE=1` rewrites them), profile editor at the preview's `/crt` (sliders, live picture, TOML download) and live reload of a `--crt FILE.toml` in the desktop.
Light pens (`devices-input::lightpen`) on the C64/C128 (VIC-II LP latch, IRQ), Atari 8-bit (PENH/PENV), Amiga (VPOSR/VHPOSR latch with LPEN) and CPC (expansion-port strobe into CRTC R16/R17), aimed by the host pointer (`HostEvent::Pointer`, `[[pointer]]` maps, `Profile::for_options`) in the desktop and the web. Real-hardware backends behind `media-backends` features (`flux`, `greaseweazle` over `serialport`, `opencbm` loaded at run time with `libloading`, the crate's only `unsafe` in `opencbm/ffi.rs`): flux PLL, 1541 GCR → D64/G64 (`media::g64` writer), 1571/1581, Amiga ADF, IBM MFM; OpenCBM sector access through DOS commands (`U1`/`U2`/`B-P`), a real drive as media (`--real-drive`); verified only against simulated devices. Desktop: cheats as OFF/ON panel switches (`--cheats`, `--cheat`), PAD FIRE and SAVE MAPPING (`<user folder>/input/<machine>.toml`, `--input FILE`) as the basic mapping-profile UI.
68020 instruction cache (64 long words, CACR E/F/C/CE, CAAR, FC2 tag) and cycle timing from the MC68020 User's Manual tables 8.2.1–8.2.18 (cache case; bus cycles of 3 clocks plus the machine's wait states; Harte 68000 unchanged, A500 picture/audio byte-identical).
Open: the gate (Release 1.0): the web frontend confirmed by the user in a browser; the real-hardware backends tested by the user on the ZoomFloppy and the Greaseweazle; A1200 Workbench 3.x boot (needs a WB 3.x ADF from the user; the A1200 Kickstart 3.1 40.068 is at hand since 2026-10-27, Amiga Forever's `amiga-os-310-a1200.rom`); DAP stays deferred.

### Phase 11 status (ColecoVision, begun 2026-10-23)

User choices: build with an own stub BIOS first (the user adds the BIOS to `roms/coleco/` later), NTSC and PAL, carts up to 32 KB plus MegaCart, all four controller kinds (standard, Super Action, Driving Module, Roller Controller). Done: `chips-ti` (TMS9918A/TMS9929A: all four modes, sprites with the four-per-line limit, 5S and coincidence, status/IRQ, read-ahead buffer; SN76489: tones, noise with the 15-bit register, attenuation), `devices-input::coleco` (`PortKind::ColecoControl`; segments, keypad codes, quadrature axes played out one step per 1/400 s), `devices-cartridge::coleco` (plain, MegaCart), `machine-coleco` (memory/I/O map, VDP → NMI, spinner → INT, panel POWER/RESET, keypads on the host keyboard, debugger with a VRAM space, savestates), stub BIOS, `tests/tests/coleco_boot.rs`. Open: the gate with the real BIOS and user cartridges (no BIOS at hand yet); TMS9918A reference pictures from test programs; unverified details (keypad code table, the spinner interrupt wiring and which bits carry the phases, the Driving Module's pedal line, the MegaCart scheme, the PAL CPU clock); VRAM access timing (the minimum time between CPU accesses is not enforced); the 4/16K bit; the VDP draws a line from the registers as they are at its start.

### Phase 12 status (NES, begun 2026-10-23)

User choices: NTSC and PAL, the mappers of the phase (NROM, MMC1, UxROM, CNROM, MMC3), the standard controller, Zapper, Four Score and Arkanoid controller, the Famicom Disk System later; test ROMs from the nes-test-roms collection (fetched, never committed). Done: `cpu-6502` `Variant::Rp2A03` (no decimal mode; Harte `nes6502` green; late interrupt sampling and the vector chosen as P is pushed, so an NMI in BRK's last cycles follows the handler's first instruction), `chips-nintendo` (PPU with dot-level background pipeline, sprite evaluation timed for the overflow flag, A12 one dot ahead of pattern reads, odd-frame skip decided at dot 338, I/O latch decay, the power-up palette blargg measured; palette computed from the composite signal levels; APU with frame counter, all five channels, load/reload DMC fetches, non-linear mixer), `devices-cartridge::nes` (iNES/NES 2.0, trainer, four-screen, mappers 0–4, MMC3A as submapper 4), `devices-input::nes` (`PortKind::NesControl`; controller, Zapper with the light decided by the console from the picture, Arkanoid), `machine-nes` (DMA unit, Four Score, panel POWER/RESET, debugger spaces cpu/ppu/oam and chips ppu/apu/mapper, savestates), `tests/tests/nes_testroms.rs`: 116 ROMs pass (nestest; instr_test-v5, instr_misc, instr_timing, cpu_interrupts_v2, cpu_dummy_writes, cpu_exec_space, cpu_reset, ppu_vbl_nmi, vbl_nmi_timing, ppu_open_bus, ppu_read_buffer, oam_read, oam_stress, sprite_hit, sprite_overflow, blargg_ppu_tests, apu_test, apu_reset, blargg_apu_2005 1–9, sprdma_and_dmc_dma, mmc3_test_2 1–5, branch_timing). Known failures: `6-MMC3_alt` (tests the MMC3A, which its iNES 1.0 header cannot select), blargg_apu_2005 `10.len_halt_timing` and `11.len_reload_timing` (a halt or reload write in the cycle of the length clock; apu_test's versions pass). Open: user-supplied games (none at hand yet); battery-backed PRG RAM is not written back to a file; screen-only test ROMs (dmc_dma_during_read4, cpu_dummy_reads, read_joy3, blargg_nes_cpu_test5, cpu_timing_test6) not judged; the Zapper's light model (bright point up to 20 lines above the beam) is this project's approximation; the LXA constant $FF follows blargg's instr_test, while Harte's nes6502 data uses $EE.

### Phase 12b status (SNES, begun 2026-10-23)

User choices: NTSC and PAL; the SPC700's 64-byte boot ROM user-supplied (role "spc700") with this project's own loader for the documented upload protocol when it is missing; of the coprocessors only the DSP-1; joypad, mouse, Multiplayer 5 and Super Scope. Done: `cpu-65816` (all 5 080 400 Harte tests, every bus cycle including the internal ones' addresses), `cpu-spc700` (all 256 000 Harte tests with dummy reads), `chips-nintendo::sdsp` (BRR, Gaussian interpolation with the documented table, ADSR/GAIN, noise, pitch modulation, echo with FIR), `chips-nintendo::sppu` (modes 0–7, 16×16 tiles, offset-per-tile, Mode 7 matrix and screen-over, sprites with the 32/34 limits and flags, windows, colour math, mosaic, direct colour, hires and pseudo-hires, interlace; a line at a time), `devices-cartridge::snes` (LoROM/HiROM/ExHiROM, SRAM, copier header, header scoring), `devices-input::snes` (`PortKind::SnesControl`), `machine-snes` (bus speeds, refresh, DMA/HDMA, NMI/IRQ/H/V timers, multiplier/divider, auto joypad, Multiplayer 5, Super Scope latch, S-SMP with timers and ports, 32 → 48 kHz resampling, debugger targets cpu and spc700 with spaces cpu/vram/cgram/oam/aram, savestates). A user-supplied game (Sonic the Hedgehog 4 (Unl), LoROM) runs from the title screen into the level with sound. Open: the DSP-1 (needs a µPD77C25 core and its user-supplied program ROM); other coprocessors (Super FX, SA-1 …) refuse to load; mid-line PPU register changes; the multiplier/divider delays; battery SRAM is not written back to a file; SNES test ROMs.

### Phase 15 status (ZX Spectrum, begun 2026-10-26)

User choices: models 48K, 128K and +2 (+2A/+3, 16K and Timex later); the user's ROMs copied to `roms/spectrum/` (not committed) plus a stub ROM; Kempston, Sinclair (Interface 2) and cursor joysticks, the Kempston mouse, the Multiface 128 and the Interface 1 with Microdrives and RS-232; freely distributed timing test programs. Done: `cpu-z80` reports every M-cycle's address (`TimedBus::wait_states_at`) and, with `Z80::idle_cycles`, every internal T-state with the address on the bus (`TimedBus::idle`; all Harte tests check those addresses); `chips-sinclair::spectrum` (ULA timing for 48K and 128K, contention and I/O contention, floating bus, border in 4-T-state steps, flash, lazy picture, savestate); `machine-spectrum` (memory and $7FFD paging, AY at half the CPU clock, beeper/tape/AY mix at 48 kHz, keyboard, joysticks on ports and keys, Kempston mouse, LD-BYTES trap and PLAY on LOAD, `autostart`, SNA/Z80 snapshots in a Program slot, Multiface 128, Interface 1 with shadow ROM paging, Microdrive select register and status, panel RESET/MULTIFACE, debugger spaces cpu/ram/rom and chips ula/paging/ay, gfx views screen/bitmap/charset); `drives-other::spectrum_tape` (TAP/TZX, speed scaled for 3.5469 MHz, TZX stop blocks), `drives-other::microdrive` (byte-level tape loop: gap, preamble, data; write after a header; FORMAT's filler past a record), `media::mdr`, `media::cdt` TAP and block list (the TZX pause now begins with an edge — fixes the last bit of every block for CPC tapes too), `devices-input::KempstonMouse`, `PortKind::SpectrumJoy`/`KempstonMouse`, keymap. All three models boot with real ROMs and PRINT; a TAP built by the test loads with the trap and in real time (48K and 128K Tape Loader); the Fuse Test (frame length, contended memory/I/O, floating bus, LDIR, high-port contention, $7FFD/$3FFD reads) passes completely on 48K, 128K and +2; the Multiface 128 menu appears and returns on 48K and 128K; the Interface 1 formats a cartridge, saves, CATs and loads it back. Open (gate): a user-supplied TAP game (none at hand); border timing beyond the Fuse Test (no border test program at hand: the 4-T-state step and its phase are this project's choice); SAVE to tape (no recording deck); the RS-232/network port has no host device; Microdrive timing is byte-level (programs that time the tape themselves are not covered); the +2's own quirks beyond the 128K's; snow (I in contended memory) is not drawn; Multiface port read values other than bit 7 unverified.

### Phase 16 status (Apple II, begun 2026-10-24)

User choices: II, II+ and IIe (unenhanced and enhanced) as models of `machine-apple2`; the original ROMs from the user (`roms/apple2/`, Apple part numbers: II 341-0001…0004, II+ 341-0011…0015/0020, IIe 342-0134/0135, enhanced 342-0303/0304, character ROMs 341-0036, 342-0133, 342-0265, Disk II P5 341-0027 and P6 341-0028); DSK/DO/PO + WOZ, paddles/joystick, cassette, Mockingboard. Done: `cpu-6502` `Variant::Cmos65C02` (all 256 000 Harte `synertek65c02` tests; the undefined opcodes and decimal-mode dummy reads follow those traces; benchmark unchanged), `machine-apple2` (video scanner with floating bus, text/lo-res/hi-res/80-column/double modes, character ROMs or own font, language card, IIe memory switches and status reads, keyboard encoder with IIe auto-repeat, speaker, paddles/buttons, Mockingboard, stub ROM, savestates, debugger spaces cpu/main/aux), `drives-other::disk2` (P6 sequencer with its line order found by reading nibbles through the user's PROM; functional model without it; stepper with quarter tracks; DSK/NIB/WOZ write-back), `drives-other::apple_tape` (WAV play/record; the tape moves while the program listens), `media::a2disk` (6-and-2 GCR, WOZ 1/2), `media::wav`, `devices-input::apple` (`PortKind::AppleGame`), keymap (Alt = Apple keys, F5 = RESET), default monitor "tv". All four models boot with real ROMs, Zork I (DSK) boots on the II+ and IIe, a disk built by the test boots through P5 (with P6 and functional), cassette W/R works, hi-res colours checked through the composite stage. Open (gate): DOS 3.3 and ProDOS disks (none at hand), artifact colours against reference pictures (only the four hues checked), Mockingboard with real software, WOZ copy protections; the II+ keyboard layout (Shift-2 ") is not reproduced; no PAL (Europlus) models, no IIc/IIgs; RAM shows a simple power-on pattern; double lo-res aux colours unverified.

### Phase 17 status (Macintosh, begun 2026-10-24)

User choices: 128K, 512K, Plus and SE as models of `machine-mac`; the original ROMs from the user (`roms/mac/`: 128K 28BA61CE, 512K 28BA4E50, Plus v3 4D1F8172, SE B2E362A8); SCSI hard disks, DSK + DiskCopy 4.2, sound, SCC; System 6 disks later from the user. Done: `machine-mac` (memory map with overlay, video with RAM contention, VIA wiring, RTC with 20/256-byte PRAM, M0110/M0110A keyboard on the VIA's external-clock shift register, SE ADB transceiver with keyboard and mouse, SCC 8530 with DCD interrupts and the modified vector, NCR 5380 with SCSI-1 disk targets and pseudo-DMA, sound from the buffer at the E clock, interrupt switch on the panel, stub ROM, savestates, debugger spaces cpu/ram/rom), `drives-other::iwm` (IWM latch mode and write path, Sony 400K/800K drives with sense lines, stepping, tachometer, the 400K drive's PWM speed), `media::macdisk` (GCR 6-and-2 sectors with the three-checksum data field, zones, 2:1 interleave, raw and DiskCopy 4.2 images with write-back), `chips-cbm::via` external-clock shift modes 3/7 and CA2 input, `PortKind::MacMouse` (the Amiga mouse device with its own kind), keymap (Alt = Command, Windows = Option), monitor profile `mac`. All four models reach the disk icon with real ROMs (the startup sound plays); a disk the test builds (MFS volume, boot blocks with bbVersion $44xx) boots on all four through the IWM (400K on the 128K/512K, 800K on the Plus/SE), which checks the GCR checksums. System 6.0.8 (archive.org, 800K raw images) boots from floppy to the Finder on the Plus and SE; System 7.5.5 from SCSI images (archive.org BlueSCSI set, a user image) starts on the Plus and SE (4 MB) up to the extensions' alerts. Fixes found on the way: the Sony STEP line must clear within the driver's ~5 ms, SCSI disks report no unit attention, the Plus tells SCSI is present by $440000 not repeating the ROM, AppleTalk needs the SCC's Tx underrun interrupt and RR0 hunt bit, the Plus's Y2 is inverted against the device and the ROM counts one pixel per two phases, the SE's ADB transceiver auto-polls the last Talk in idle. Open (gate): 7.5.5 to the Finder (dialogs need a click, then check further); mouse directions and keyboard under a running system; the ADB handshake (when /INT falls, auto-polling) and the SE's SCSI only checked as far as its ROM's scan; the 400K drive's speed curve is a linear stand-in; IWM bit-cell timing (every bit reaches the latch whatever the speed); copy-protected disks; the 2.5 MB Plus/SE RAM layout; the SE's second internal drive select (VIA PA4) unverified; serial ports have no host device.

### Deferred from Phase 7

Held back on 2026-10-22; not part of the Phase 7 gate. Done on 2026-10-22 (user request "implement the rest of Phase 7"): Paula attach modes and UART (loopback option), 1 MB chip RAM, Zorro II fast RAM via autoconfig, A1000 (WCS, bootstrap ROM) and A2000 models, HD drives/disks, extended ADF, DMS, bob deinterlacing in the desktop (`--deinterlace`, Ctrl+I), woven fields in the preview, LED filter as a second-order Butterworth. Still open:

- **Action Replay (A500 side-port freezer):** needs its ROM (user-supplied) and the side-port hardware (freeze NMI, ROM/RAM overlay). User decision 2026-10-22: left out (no ROM).
- **Paula:** disk write precompensation has no effect in the bit-cell drive model (documented); the fixed output filter stays first-order. The serial port has no host device yet (only `serial=loopback`).
- **A1000 bootstrap path** (`wcs=disk`) is tested with the stub only — no bootstrap ROM or Kickstart disk at hand.
- **ECS** (done 2026-10-23, user request "mache weiter amiga ecs"): `--model 500plus` (Agnus 8375, Denise 8373, 1 MB chip, Kickstart 2.0 default) and the options `chipset=ocs|ecs`, `agnus=ocs|ecs`, `denise=ocs|ecs`, `chip=2m`. Agnus: id $20/$30 in VPOSR with LOL, BEAMCON0 (PAL/NTSC switch resizes the picture, LOLDIS, VARBEAMEN with HTOTAL/VTOTAL), the other beam registers stored for the debugger only, DIWHIGH (vertical), BLTSIZV/BLTSIZH, Copper CDANG reaching every register. Denise: DENISEID $FC, ECSENA-gated BPLCON3 BRDRBLNK and BRDSPRT, DIWHIGH H8 bits, super-hires (two planes, two bits per gun, every second 35 ns pixel dropped since the picture keeps 70 ns columns). Kickstart 2.0 and 3.1 reach insert-disk on the 500plus (`expansions_and_models_boot`). Open: the sprite/bitplane horizontal registers of the programmable beam (SPRHSTRT … BPLHSTOP, VBSTRT/VBSTOP with VARVBEN, HBSTRT/HBSTOP blanking) have no effect; the super-hires fetch order, the colour-bit reduction and the SHRES DDFSTRT granularity are this project's reading of the HRM, not verified; ECS sprite/HAM details not checked against vAmigaTS (no ECS photographs on hand); Workbench 2.x/3.x not booted (no ADF).
- **Chip-bus residual (CPU next to a long Copper sub-list):** `tools/amiga_bustest4.py` rows 2, 3, 6, 7 (30 COLOR MOVEs on every second or every line, their tail competing with the CPU for the free slots of the bitplane fetch) run 2.6–3.4 % faster than WinUAE; the other rows and the demo's own frame work match within 2 %. WinUAE's own result for one configuration varies by up to 6 % with the loop's start phase, ours does not.
- **Chip-bus residual:** the bus test (`tools/amiga_bustest.py`, reference from a cycle-exact WinUAE A500 in `tests/tests/amiga_boot.rs`) matches within 0.2 % except the 4-plane + per-line-Copper row, where we are 0.7 % fast after modelling the Copper's bus-holding wake-up after WAIT (a second wake-up slot changes nothing). The measurement itself moves up to 1 % with the loop's start phase.
- **Timing details:** Blitter idle cycles reach the CPU (unverified against hardware), the D channel's one-word pipeline; the interrupt latencies (VERTB two colour clocks after the strobe, IPL four after a request, the 68000 sampling at the start of its last bus cycle) follow community measurements, not a datasheet; CPU accesses to the CIAs and the autovector use a simplified E-clock rule.
- **Denise against Agnus:** Denise's pixel counter runs two colour clocks behind Agnus' hpos (register writes show from pixel 2h − 4) and a sprite starts one pixel right of its HSTART; both from one WinUAE screenshot of Interference's tunnel frame (2026-10-23), not from hardware or a test program.
- **Sprite DMA:** VSTOP wins over VSTART (a sprite parked with VSTART = VSTOP fetches control words, not data; found with Interference on 2026-10-23, where the data fetches of eight parked sprites cost the CPU 16 slots a line and its frame work ran past the field). Not verified beyond the demo and the unit test.
- **Sprites, checked against vAmigaTS photographs on 2026-10-22** (`tests/tests/vamigats.rs`): all 18 `sprcoll` collision tests, the `clip`/`diwclip` window clipping (sprites appear only after the line's first BPL1DAT; a line without plane data — BPU = 0 or bitplane DMA off — shows none, which the `sprbpu1` photographs (OCS and ECS) and WinUAE with `tools/amiga_sprtest.py` agree on, 2026-10-23) and `sprbpu` match. `sprites1` looks right at a glance (priorities not checked pixel by pixel). Open: the `timing` family — in `sprtimpos1` the second sprite column (SPRxPOS rewritten by the Copper near the sprite start) comes out white instead of in the sprites' colours; `sprtimctl1`, `sprtimdat*`, `sprdmadat*`, `sprxpos` not compared.
- **AGA / A1200** (begun 2026-10-23, user request "baue erstmal den a1200/aga teil"): `--model 1200` = 68EC020 (`cpu-m68k` `Model::Mc68020`: full-format extension words, the 68020 instructions, VBR/CACR/MSP, format 0/1/2 frames, misaligned accesses, 3-clock bus cycles, instruction cache and manual-table timing since 2026-10-25; no test suite beyond unit tests), `Chipset::AGA` (Alice: ids $22/$32, BPL7/8PT, FMODE fetch widths and fetch units; Lisa: id $F8, 256 colours of 24 bits with BPLCON3 BANK/LOCT, 8 planes, HAM8, BPLCON4 BPLAM/sprite banks, PF2OF, KILLEHB, extended BPLCON1 scroll, wide/hires sprites, CLXCON2), 2 MB chip RAM, Gayle id register only (no IDE, no PCMCIA, no RTC; the empty IDE port reads $FF, the floating bus — with 0 the ROM's scsi.device waited for a drive: 40.068 never reached insert-disk, 40.063 only after about 850 frames; fixed 2026-10-27, both now at about 150 frames), Kickstart 3.1 by default: the A1200's own 40.068 (role `kick31a1200`, e.g. Amiga Forever's `amiga-os-310-a1200.rom`, decrypted with its `rom.key` by `media_backends::roms`) when present, else the 40.063 image (A500/A600/A2000), which boots the model too. The "3.0" image at hand is the A4000's 39.106 (CRC 9E6AC152), which scans for 32-bit RAM at $08000000 and, the 68EC020 folding that onto chip RAM, crashes — an A1200 3.0 goes in as `kick30a1200`. The wide-fetch window rounds an unaligned DDFSTOP up (the unit in progress completes, one more follows; from BigTime's hires FMODE 3 picture, DDFSTRT $68/DDFSTOP $90 = 4 units, 2026-10-26). Fixed with BigTime on 2026-10-26: a wider fetch unit has room for more planes (hires takes 8 planes from FMODE 1, super-hires from FMODE 3; before, hires HAM8 lost planes 7 and 8), and the 68020 samples IPL at the end of the instruction with Paula's delay plus one colour clock (`IPL_DELAY_020`; with the 68000's rule `MOVE.W #x,INTREQ` / `RTE` took the interrupt twice), and the picture's field parity is the drawn field's LOF (`Custom::frame_long`; LOF had already turned, so the desktop wove interlaced fields in the wrong order — every Amiga model). Speed (profile of BigTime disk 2): Denise skips colour clocks without plane data or sprites and checks sprites once a colour clock, the fetch window is cached, `denise_h` has no division — disk 2 runs 19 % faster, pictures byte-identical. BigTime disk 2's "…PRESENTS" scroller moves unevenly (−4, −4, −4, −20, …, +12 pixels a field: odd-byte BPLxPT with FMODE 0, word-aligned fetch) — the user saw it uneven in WinUAE too (2026-10-26), so it stays. 35 ns output (user request 2026-10-26, "nur bei Bedarf"): a field turns 1440 columns wide (`custom::WIDE`, pixel clock doubled in the signal metadata) as soon as a line needs 35 ns — super-hires (ECS and AGA), BPLCON1 PF1H0/PF2H0, a sprite in use with SPRES 35 ns or the SPRxCTL 35 ns position bit — and returns to 720 after 50 fields without; A500 pictures and speed unchanged. AGA wide fetches against WinUAE's A1200 with `tools/amiga_agatest.py` (2026-10-26): the fetch runs in blocks aligned to the unit (DDFSTRT & ~(unit − 1); the unit count still from DDFSTRT), the planes take a long unit's last eight clocks with plane 1 last, a sprite whose first slot ($17 + 4n) is at or after DDFSTRT gets no DMA on bitplane lines, and a sprite that keeps DMA wins its slot against a plane (WinUAE shows garbage for DDFSTRT ≡ $C mod 16, not reproduced); fixes the stripes in BigTime's "Real AGA Plasma" sprite logo (sprites 0-3, DDFSTRT $24). Open: BigTime's music sometimes sounds broken or too fast on disks 1 and 2 (the user, 2026-10-26; no desktop underruns, so in the emulation — CPU delay loops or the replay interrupt; not compared with WinUAE yet, deferred by the user); BigTime's AXIS picture shows a COLOR00 band on the left from its 64-pixel BPLCON1 scroll ($8800), not checked against hardware; Workbench 3.x boot needs an ADF; BPLCON0 BYPASS and the genlock bits ignored.

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
