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
10. **Machines never see host devices.** No `gilrs`, `winit`, file-system or USB code inside `machine-*`, `chips-*` or `devices-*`. Host input and host storage enter only through the port/media abstractions in `emu-core` (see `docs/claude/devices.md`).

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
  cpu-upd7725/     NEC µPD77C25 (SNES DSP-1 to DSP-4 cartridge processor)
  cpu-6809/        Motorola 6809 / 68A09 (Vectrex)
  cpu-arm/         Acorn ARM2 (26-bit, Archimedes)
  cpu-8080/        Intel 8080A (IMSAI 8080), machine cycles reported with their status word
  cpu-ada/         Ada, the 16-bit RISC CPU of the fictional Logophor 16 (Phase 31)
  asm-ada/         assembler + linker for Ada (tests, Foundry ROM build, host tool)
  chips-cbm/       VIC, VIC-II(e), TED, VDC 8563, SID, CIA, VIA, PIA, MMU 8722, TIA 6523
  chips-amiga/     Agnus (DMA, Copper, Blitter), Denise, Paula, Gary, CIA
  chips-atari/     ANTIC, GTIA, POKEY, PIA
  chips-amstrad/   CRTC 6845, Gate Array, AY-3-8912, 8255 PPI, FDC 765
  chips-acorn/     MEMC, IOC, VIDC, PCF8583, WD1772 (Archimedes)
  chips-ti/        TMS9918A/TMS9929A VDP, SN76489 (ColecoVision)
  chips-sinclair/  ZX81 ULA; ZX Spectrum ULA (display, contention, floating bus, port $FE)
  chips-logophor/ Logophor 16: Babbage (bus arbiter, Jacquard, Lithograph, Stereoscope 3D), Lumière (video, sprites), Calliope (sound), Phonograph (MP3/AAC), Telegraph (I/O, network)
  chips-nintendo/  NES: PPU 2C02/2C07 with its palette, APU of the 2A03/2A07; SNES: S-PPU, S-DSP
  drives-cbm/      IEC bus, 1541, 1571, 1581 (each with its own CPU), TCBM bus with the 1551
  drives-other/    Amiga floppy (DF0–DF3, MFM), Atari SIO drives (810/1050), CPC FDC drives, Apple Disk II, Mac IWM + Sony 400K/800K drives, datasettes, Apple cassette, Spectrum cassette, ZX Microdrives
  devices-input/   Emulated input peripherals: joysticks, paddles, mice (1351, Amiga, AMX, Kempston), light pens, keypads
  devices-cartridge/ Expansion-port cartridges: C64 CRT types (banking, EasyFlash), VIC-20 and C16/Plus/4 ROM carts
  devices-net/     Devices that reach outside through emu-core::host: Ultimate 64 command interface (UCI), WiC64
  devices-printer/ Printers: Epson FX-80, NEC P6+ and ImageWriter models with page pictures, Commodore bus end, output through emu-core::host
  media/           Image formats: PRG, D64/D71/D81, T64/TAP, CRT, ADF(→MFM), ATR, XEX, CAS, DSK, CDT/TZX and Spectrum TAP, Apple DSK/DO/PO(→GCR)/WOZ, Mac DSK/DiskCopy 4.2(→GCR), WAV, MDR, punched-card decks (.deck/.crd/.chx)
  media-backends/  Where media bytes come from: image file, copy-on-write overlay, host directory, real hardware
  emu-input/       Host side: gamepads (gilrs), keyboard-as-joystick, mouse capture, mapping profiles
  emu-profile/     Start profiles (profiles.yaml: settings + named machine setups) for the frontends
  emu-cheats/      Cheat files with patches, POKE/VICE/code imports, CRC-32 database, memory search
  video-crt/       CRT/monitor emulation: composite/RGB signal path, WGSL shaders (wgpu), CPU fallback, monitor profiles
  machine-pet/  machine-vic20/  machine-c64/  machine-ted/  machine-c128/
  machine-amiga/  machine-atari8/  machine-atari2600/  machine-cpc/  machine-coleco/  machine-nes/  machine-snes/  machine-vectrex/
  machine-apple2/  Apple II, II+, IIe (enhanced and unenhanced) as models; Mockingboard in slot 4
  machine-mac/     Macintosh 128K, 512K, Plus, SE as models: RTC, keyboard, ADB, SCC 8530, SCSI 5380 with hard disks
  machine-archimedes/ Acorn Archimedes A305, A310, A440, A3000 as models
  machine-imsai/   IMSAI 8080: the picture is the front panel (clickable paddles, lamps by bus duty), serial terminal as second screen; card reader
  machine-keypunch/ IBM 029 card punch: the user types card decks (picture: the card), read back by the IMSAI's card reader
  machine-zx81/    Sinclair ZX81 / TS1000
  machine-trs80/   Tandy TRS-80 Model I (Level I/II, cassette, Expansion Interface with an FD1771) and Model III (FD1793, WAIT, NMI); floppy controller at track level on DMK/JV1/JV3
  machine-logophor16/ Logophor 16, the project's own fictional steampunk computer (spec: docs/logophor16/SPEC.md); ROM built from rom/ sources, shipped as a built-in profile
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
    fn ports(&self) -> Vec<PortInfo>;                   // physical connectors (see `docs/claude/devices.md`)
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

## Further documentation

The detail lives in `docs/claude/`. Read the file for the area you touch before changing it, and keep it up to date there instead of growing this file.

- [Machine notes](docs/claude/machines.md) — clocks and gotchas per machine; model variants are configuration, PAL/NTSC is `--videoformat`.
- [External devices and ports](docs/claude/devices.md) — `PortDevice`, `InputEvent`, `MediaSlot`/`MediaBackend`, drive modes.
- [Display and CRT emulation](docs/claude/crt.md) — `FrameRef`, the `video-crt` pipeline, monitor profiles.
- [Memory freezer, cheats and remote device control](docs/claude/freezer.md) — `Freeze`, cheat search, cheat files, freezer cartridges, device commands.
- [Debug server and live preview](docs/claude/debugger.md) — protocol, adapters, graphics data views, preview endpoints.
- [Commands](docs/claude/commands.md) — every build, run and test command with its options.
- [Testing](docs/claude/testing.md) — test suites per machine, results, regression and trend rules.
- [Roadmap and current phase](docs/claude/roadmap.md) — phase table, machine backlog; one file per phase in `docs/claude/phases/` (gate, decisions, status, open items).

Essentials:

```bash
cargo build --workspace
cargo test --workspace                       # unit + integration (ROM tests skipped)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
python tools/check-deps.py                   # dependency direction (golden rules 1 and 10)
```

**Current phase: 8** (open); Phases 10, 14–17, 24, 26, 31 and 32 are in progress as well, Phases 1–7, 6b and 9 are done. See `docs/claude/roadmap.md`; when a phase moves, update its file in `docs/claude/phases/` and the summary paragraph in `roadmap.md`.

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
