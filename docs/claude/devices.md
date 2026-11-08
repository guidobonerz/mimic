# External devices and ports

Part of the project's instructions for Claude Code; the core is `CLAUDE.md` at the repository root, the index of these files is its section "Further documentation".

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
