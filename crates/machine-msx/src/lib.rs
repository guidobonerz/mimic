//! MSX (MSX1; NTSC and PAL).
//!
//! Z80A at 3.579545 MHz (a third of the 10.738635 MHz crystal, on PAL
//! machines too), the TMS9918A video display processor (PAL: TMS9929A) with
//! 16 KB VRAM (`chips-ti`), the AY-3-8910 sound generator and the 8255 PPI
//! (`chips-amstrad`), 32 KB BIOS and BASIC ROM, up to 64 KB RAM, two
//! cartridge slots and two general-purpose (joystick) ports.
//!
//! **Slots.** The 64 KB address space is four pages of 16 KB; each page
//! shows one of four primary slots, chosen by the PPI's port A (bits 1–0
//! page 0 … bits 7–6 page 3). While port A is an input (after reset) every
//! page shows slot 0.
//!
//! | Slot | Contents                                                        |
//! | ---- | --------------------------------------------------------------- |
//! | 0    | $0000–$7FFF BIOS and BASIC; with C-BIOS its logo ROM at $8000–$BFFF |
//! | 1    | cartridge 1                                                     |
//! | 2    | cartridge 2                                                     |
//! | 3    | RAM, the top `ram` KB of the slot (64 KB: all of it)            |
//!
//! Everything else reads $FF. Without the disk interface no slot is
//! expanded. With it slot 3 is: its secondary slot register is at $FFFF
//! while page 3 shows slot 3 (bits 1–0 page 0 … bits 7–6 page 3; it reads
//! back inverted), slot 3-2 is the RAM, slot 3-1 the disk ROM at
//! $4000–$7FFF and again at $8000–$BFFF with the interface's registers
//! ([`disk`]), 3-0 and 3-3 are empty. (The RAM is in 3-2 because disk
//! software of the time often starts with `POKE &HFFFF,&HAA` — all pages
//! on secondary slot 2, where widespread machines have their RAM; with
//! the RAM anywhere else that POKE takes it away.)
//!
//! **I/O** (the low address byte):
//!
//! | Port    | Function                                                     |
//! | ------- | ------------------------------------------------------------ |
//! | $98     | VDP data                                                     |
//! | $99     | VDP control (write), status (read)                           |
//! | $A0     | PSG register number (write)                                  |
//! | $A1     | PSG data (write)                                             |
//! | $A2     | PSG data (read)                                              |
//! | $A8     | PPI port A: the slot of each page                            |
//! | $A9     | PPI port B (input): the selected keyboard row, a held key 0  |
//! | $AA     | PPI port C: bits 3–0 keyboard row, 4 cassette motor (0 on), 5 cassette out, 6 CAPS lamp (0 lit), 7 key click |
//! | $AB     | PPI control                                                  |
//!
//! Other ports read $FF (the printer at $90 so reports "not ready").
//!
//! **PSG ports.** Register 14 (port A, input): bits 0–3 up, down, left,
//! right and bits 4–5 triggers A and B of the joystick port that register
//! 15 bit 6 selects (0 port 1, 1 port 2), all low when held; bit 6 high
//! (the keyboard layout input of Japanese machines), bit 7 the cassette
//! input (the tape's level; low while no tape runs). Register 15 (port B, output): bits 0–1
//! drive pins 6 and 7 of port 1, bits 2–3 those of port 2 (a 0 holds the
//! pin low, so the trigger reads as held), bits 4–5 pin 8 of each port, bit
//! 7 the KANA lamp. While port B is an input (register 7 bit 7 clear) its
//! pins float high.
//!
//! **Keyboard.** Machine key `row * 8 + bit` of the 11 × 8 matrix,
//! international layout (bits 7 → 0):
//!
//! | Row | Keys                                   |
//! | --- | -------------------------------------- |
//! | 0   | 7 6 5 4 3 2 1 0                        |
//! | 1   | ; ] [ \ = - 9 8                        |
//! | 2   | B A (dead key) / . , ` '               |
//! | 3   | J I H G F E D C                        |
//! | 4   | R Q P O N M L K                        |
//! | 5   | Z Y X W V U T S                        |
//! | 6   | F3 F2 F1 CODE CAPS GRAPH CTRL SHIFT    |
//! | 7   | RETURN SELECT BS STOP TAB ESC F5 F4    |
//! | 8   | → ↓ ↑ ← DEL INS HOME SPACE             |
//! | 9   | keypad 4 3 2 1 0 / + *                 |
//! | 10  | keypad . , - 9 8 7 6 5                 |
//!
//! **Interrupts.** The VDP's /INT drives the Z80's /INT (level: held until
//! the status register is read).
//!
//! **Timing.** The Z80 runs whole instructions with [`Z80::step_timed`];
//! the MSX's wait circuit adds one wait state to every M1 cycle (opcode
//! fetches and the interrupt acknowledge). Before each bus access the chips
//! are brought up to that T-state: the VDP runs three pixel clocks per two
//! T-states, the PSG one input clock per two T-states (1.79 MHz). A field:
//! 262 (PAL 313) lines of 228 T-states.
//!
//! **Sound.** The PSG's three channels are mixed to mono, plus the PPI's
//! key click bit and the sound chip of a Konami SCC cartridge (clocked with
//! the PSG, two bus clocks at a time).
//!
//! Video formats (`--videoformat`): "ntsc" and "pal"; without one the
//! BIOS decides (bit 7 of its byte $002B: 1 is a 50 Hz machine). No models
//! yet (MSX2 will be one).
//! Options:
//! - `port1`, `port2`: "joystick" (default) or "none".
//! - `ram`: "8", "16", "32" or "64" (default) KB.
//! - `mapper1`, `mapper2`: the bank switching scheme of the cartridge in
//!   that slot: "auto" (default, guessed from the image), "plain",
//!   "konami", "konamiscc", "ascii8", "ascii16", or with battery RAM
//!   "ascii8sram", "ascii16sram" or "gamemaster2" (never guessed).
//! - `disk`: "on" (default with the disk ROM) or "off".
//! - `roms`: "stub" runs a small built-in BIOS ([`stub`]): a cartridge
//!   with an "AB" header is started directly.
//!
//! Media slots "cartridge 1" and "cartridge 2": a ROM image
//! (`devices_cartridge::msx`). Inserting or removing one powers the
//! machine up again. Media slot "battery RAM": the battery RAM of
//! cartridge 1 as a `.srm` file ([`slot`]); it is written at the end of
//! every field in which the program changed the RAM.
//!
//! ROMs: role "msx-bios" (32 KB BIOS and BASIC), user-supplied, never in
//! the repository; C-BIOS (free, `tools/fetch_cbios.py`) is accepted in its
//! place, with its 16 KB logo ROM as the optional role "msx-logo", used
//! only when the BIOS is C-BIOS.
//!
//! **Cassette** (media slot "cassette", CAS images;
//! [`drives_other::msx_tape`]): PPI port C bit 4 is the motor relay, bit 5
//! the output the deck records, PSG register 14 bit 7 the input. The deck
//! follows the machine's time whenever the program reads the input or
//! writes port C, and at the end of every field. Inserting a tape presses
//! PLAY: `CLOAD` loads, for `CSAVE` press RECORD first (the slot's
//! controls: play, record, stop, rewind, forward). The tape's sound is not
//! mixed into the machine's.
//!
//! **Disks** (media slots "drive A" and "drive B", sector dumps of 160
//! to 720 KB): the Spectravideo SVI-738's interface ([`disk`]), there when
//! its disk ROM (role "msx-disk-svi738", user-supplied) is and the option
//! `disk` is not "off". The machine then starts Disk BASIC, or boots the
//! disk in drive A.
//!
//! Not built yet: other disk interfaces, the printer port, key ghosting,
//! MSX2.
//!
//! Sources: MSX Technical Data Book (ASCII/Sony, 1984: slots, I/O map,
//! keyboard matrix, PSG ports, cartridge header); MSX Red Book (BIOS byte
//! $002B); C-BIOS 0.29a documentation and its machine descriptions (its
//! ROM layout); Texas Instruments TMS9918A, General Instrument AY-3-8910
//! and Intel 8255A data (see `chips-ti`, `chips-amstrad`). The
//! implementation is this project's own.

#![forbid(unsafe_code)]

pub mod disk;
pub mod slot;
pub mod stub;

use chips_amstrad::ay::Ay;
use chips_amstrad::ppi::Ppi;
use chips_ti::{vdp, Tms9918, VdpModel};
use cpu_z80::{Access, TimedBus, IO_BASE, Z80};
use devices_cartridge::msx::{Mapper, MsxCart};
use devices_input::joystick::pin;
use disk::DiskInterface;
use drives_other::msx_tape::MsxTape;
use emu_core::audio::Decimator;
use emu_core::frame::{FieldParity, FrameRef, OutputType, PixelData, SignalInfo, VideoStandard};
use emu_core::gfx::{GfxOptions, GfxView, GfxViewInfo};
use emu_core::input::{InputEvent, InputQueue, InputTarget, MachineKey, TimedInput};
use emu_core::machine::ChipReg;
use emu_core::media::{MediaKind, MediaSlot, MediaSlotInfo, SlotId};
use emu_core::overlay::{Overlay, OverlayInfo};
use emu_core::port::{PortDevice, PortError, PortId, PortInfo, PortKind};
use emu_core::registry::{ConfigError, MachineConfig, MachineEntry};
use emu_core::{
    BeamPos, Bus, ChipInfo, Cpu, Cycle, DebugHooks, DebugTarget, DeviceId, Instr, Machine,
    MemSpace, MemSpaceId, RegisterSet, ResetKind, Savestate, Scheduler, Size, StateError,
    StateReader, StateWriter, StopReason,
};
use slot::CartPort;

pub const CPU: DeviceId = DeviceId("cpu");
/// The Z80's view: memory at 0–$FFFF, I/O ports at `IO_BASE + port`.
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);
/// The VDP's 16 KB of video RAM.
pub const VRAM_SPACE: MemSpaceId = MemSpaceId(1);
pub const SLOT_CART1: SlotId = SlotId(0);
pub const SLOT_CART2: SlotId = SlotId(1);
/// The battery RAM of cartridge 1 as a `.srm` file (`slot.rs`).
pub const SLOT_BATTERY: SlotId = SlotId(2);
pub const SLOT_TAPE: SlotId = SlotId(3);
pub const SLOT_DRIVE_A: SlotId = SlotId(4);
pub const SLOT_DRIVE_B: SlotId = SlotId(5);
pub const PORT1: PortId = PortId(0);
pub const PORT2: PortId = PortId(1);

pub const CPU_HZ: u32 = 3_579_545;
/// T-states per raster line (342 pixel clocks × 2/3).
pub const LINE_T: u64 = 228;
/// Keyboard rows.
pub const KEY_ROWS: usize = 11;

/// Machine keys: `row * 8 + bit` (see the crate doc).
pub mod keys {
    pub const fn key(row: u16, bit: u16) -> u16 {
        row * 8 + bit
    }
    pub const SHIFT: u16 = key(6, 0);
    pub const CTRL: u16 = key(6, 1);
    pub const RETURN: u16 = key(7, 7);
    pub const SPACE: u16 = key(8, 0);
}

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "msx",
        name: "MSX",
        models: &[],
        video_formats: emu_core::registry::PAL_NTSC,
        create: |config| Ok(Box::new(Msx::new(config)?)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    Ntsc,
    Pal,
}

/// The device a port option names.
fn port_option(
    config: &MachineConfig,
    key: &str,
) -> Result<Option<Box<dyn PortDevice>>, ConfigError> {
    match config.options.get(key).map(String::as_str) {
        None | Some("joystick") => Ok(Some(Box::new(devices_input::Joystick::new(
            PortKind::MsxJoy,
        )))),
        Some("none") => Ok(None),
        Some(other) => Err(ConfigError::Invalid(format!(
            "{key} option '{other}' (joystick or none)"
        ))),
    }
}

/// The scheme a mapper option names; `None` for "auto".
fn mapper_option(config: &MachineConfig, key: &str) -> Result<Option<Mapper>, ConfigError> {
    match config.options.get(key).map(String::as_str) {
        None | Some("auto") => Ok(None),
        Some(name) => Mapper::parse(name).map(Some).ok_or_else(|| {
            ConfigError::Invalid(format!("{key} option '{name}' (auto, {})", Mapper::NAMES))
        }),
    }
}

/// Everything on the bus except the CPU and the port devices.
#[derive(Clone, Debug)]
struct Board {
    bios: Vec<u8>,
    /// C-BIOS's logo ROM in slot 0 at $8000.
    logo: Option<Vec<u8>>,
    /// The RAM, at the top of slot 3.
    ram: Vec<u8>,
    /// The disk ROM in slot 3-1; with it slot 3 is expanded.
    disk_rom: Option<Vec<u8>>,
    /// Slot 3's secondary slot register.
    sub: u8,
    /// The working copies of the inserted cartridges.
    carts: [Option<MsxCart>; 2],
    /// A cartridge has a sound chip.
    cart_sound: bool,
    vdp: Tms9918,
    psg: Ay,
    ppi: Ppi,
    /// Held keys per keyboard row (bit set: held).
    keys: [u8; KEY_ROWS],
    /// The mixed sound level, kept between PSG clocks.
    level: i32,
    /// T-states the chips have run.
    chips_t: u64,
    /// A field ended since the last check.
    field_done: bool,
    audio: Decimator,
    /// DC blocker: last input and output (16.16).
    dc: [i32; 2],
    samples: Vec<i16>,
}

impl Board {
    /// The PSG's channels and the key click as one level.
    fn mix(&mut self) {
        let [a, b, c] = self.psg.output();
        let click = if self.ppi.c_pins() & 0x80 != 0 {
            3000
        } else {
            0
        };
        self.level = (i32::from(a) + i32::from(b) + i32::from(c)) * 2 / 3 + click;
        if self.cart_sound {
            // The SCC's 11 bits: a channel at full volume about as loud
            // as one of the PSG's.
            for cart in self.carts.iter().flatten() {
                self.level += 12 * i32::from(cart.audio());
            }
        }
    }

    /// One T-state of the chips: the PSG a clock every other T-state, the
    /// VDP one or two pixel clocks (three per two T-states).
    fn tick(&mut self) {
        if self.chips_t & 1 == 0 {
            self.psg.tick();
            if self.cart_sound {
                for cart in self.carts.iter_mut().flatten() {
                    cart.clock(2);
                }
            }
            self.mix();
        }
        if let Some(s) = self.audio.push(self.level) {
            // A first-order high-pass (the output's AC coupling).
            let x = i32::from(s) << 16;
            let y = x - self.dc[0] + ((i64::from(self.dc[1]) * 65_208) >> 16) as i32;
            self.dc = [x, y];
            self.samples
                .push((y >> 16).clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16);
        }
        let pixels = 1 + (self.chips_t & 1);
        for _ in 0..pixels {
            self.field_done |= self.vdp.tick();
        }
        self.chips_t += 1;
    }

    /// Run the chips up to (not including) T-state `to`.
    fn run_to(&mut self, to: u64) {
        while self.chips_t < to {
            self.tick();
        }
    }

    /// The slot register: all pages on slot 0 while port A is an input.
    fn slots(&self) -> u8 {
        if self.ppi.a_is_input() {
            0
        } else {
            self.ppi.a
        }
    }

    /// The primary slot the page of `addr` shows.
    fn slot_of(&self, addr: u16) -> u8 {
        self.slots() >> (2 * (addr >> 14)) & 3
    }

    /// The secondary slot of slot 3 the page of `addr` shows (0 while the
    /// slot is not expanded).
    fn sub_of(&self, addr: u16) -> u8 {
        if self.disk_rom.is_some() {
            self.sub >> (2 * (addr >> 14)) & 3
        } else {
            0
        }
    }

    /// The RAM cell at `addr`, if its page shows the RAM (slot 3, or 3-2
    /// when the slot is expanded).
    fn ram_index(&self, addr: u16) -> Option<usize> {
        let ram_sub = if self.disk_rom.is_some() { 2 } else { 0 };
        if self.slot_of(addr) != 3 || self.sub_of(addr) != ram_sub {
            return None;
        }
        (usize::from(addr) + self.ram.len()).checked_sub(0x1_0000)
    }

    /// `addr` is one of the disk interface's registers and its page shows
    /// the disk ROM.
    fn disk_selected(&self, addr: u16) -> bool {
        matches!(addr & 0xfff8, disk::BASE | disk::MIRROR)
            && self.slot_of(addr) == 3
            && self.sub_of(addr) == 1
    }

    /// Slot 3 as the page of `addr` shows it.
    fn slot3_peek(&self, addr: u16) -> u8 {
        let Some(rom) = &self.disk_rom else {
            return self.ram_index(addr).map_or(0xff, |i| self.ram[i]);
        };
        if addr == 0xffff {
            return !self.sub;
        }
        match self.sub_of(addr) {
            2 => self.ram_index(addr).map_or(0xff, |i| self.ram[i]),
            1 if matches!(addr >> 14, 1 | 2) => rom[usize::from(addr & 0x3fff)],
            _ => 0xff,
        }
    }

    fn mem_peek(&self, addr: u16) -> u8 {
        match self.slot_of(addr) {
            0 => match addr {
                0x0000..=0x7fff => self.bios[usize::from(addr)],
                0x8000..=0xbfff => self
                    .logo
                    .as_ref()
                    .map_or(0xff, |l| l[usize::from(addr & 0x3fff)]),
                _ => 0xff,
            },
            3 => self.slot3_peek(addr),
            s => self.carts[usize::from(s) - 1]
                .as_ref()
                .map_or(0xff, |c| c.peek(addr)),
        }
    }

    fn mem_write(&mut self, addr: u16, v: u8) {
        match self.slot_of(addr) {
            0 => {}
            3 => {
                if addr == 0xffff && self.disk_rom.is_some() {
                    self.sub = v;
                } else if let Some(i) = self.ram_index(addr) {
                    self.ram[i] = v;
                }
            }
            s => {
                if let Some(c) = &mut self.carts[usize::from(s) - 1] {
                    c.write(addr, v);
                }
            }
        }
    }

    /// The PSG's port B pins: its register 15 while an output, else high.
    fn psg_port_b(&self) -> u8 {
        if self.psg.regs[7] & 0x80 != 0 {
            self.psg.regs[15]
        } else {
            0xff
        }
    }

    /// The keyboard row PPI port C selects, a held key low.
    fn key_row(&self) -> u8 {
        let row = usize::from(self.ppi.c_pins() & 0x0f);
        self.keys.get(row).map_or(0xff, |k| !k)
    }
}

/// The PSG's port A pins (see the crate doc) with port B's pins `b`.
fn psg_port_a(ports: &[Option<Box<dyn PortDevice>>; 2], b: u8, now: Cycle) -> u8 {
    let n = usize::from(b >> 6 & 1);
    let low = ports[n]
        .as_ref()
        .map_or(0, |d| d.read_lines(now).pulled_low);
    let mut v = 0xff;
    for (p, bit) in [
        (pin::UP, 0x01),
        (pin::DOWN, 0x02),
        (pin::LEFT, 0x04),
        (pin::RIGHT, 0x08),
        (pin::FIRE, 0x10),
        (pin::CPC_FIRE1, 0x20),
    ] {
        if low & p != 0 {
            v &= !bit;
        }
    }
    // Pins 6 and 7 are also driven by port B (open collector).
    let out = if n == 0 { b } else { b >> 2 };
    if out & 1 == 0 {
        v &= !0x10;
    }
    if out & 2 == 0 {
        v &= !0x20;
    }
    v
}

/// The PPI's cassette lines as they are at `now`: the motor relay (port C
/// bit 4, low: on) and the output (bit 5).
fn cassette_lines(ppi: &Ppi, tape: &mut MsxTape, now: u64) {
    let c = ppi.c_pins();
    tape.set_motor(c & 0x10 == 0, now);
    tape.write(c & 0x20 != 0, now);
}

/// The Z80's bus for one instruction.
struct MsxBus<'a> {
    b: &'a mut Board,
    ports: &'a mut [Option<Box<dyn PortDevice>>; 2],
    tape: &'a mut MsxTape,
    disk: Option<&'a mut DiskInterface>,
    /// T-state the instruction started on.
    start: u64,
    dbg: Option<&'a mut DebugHooks>,
    beam: BeamPos,
}

impl MsxBus<'_> {
    fn io_read(&mut self, port: u8) -> u8 {
        match port {
            0x98 => self.b.vdp.read_data(),
            0x99 => self.b.vdp.read_status(),
            0xa2 => {
                let mut a = psg_port_a(self.ports, self.b.psg_port_b(), Cycle(self.b.chips_t));
                self.tape.sync(self.b.chips_t);
                if !self.tape.level() {
                    a &= 0x7f;
                }
                self.b.psg.read(a)
            }
            0xa8..=0xab => self.b.ppi.read(port, 0, self.b.key_row()),
            _ => 0xff,
        }
    }

    fn io_write(&mut self, port: u8, v: u8) {
        match port {
            0x98 => self.b.vdp.write_data(v),
            0x99 => self.b.vdp.write_control(v),
            0xa0 => self.b.psg.bus_write(3, v),
            0xa1 => {
                self.b.psg.bus_write(2, v);
                self.b.mix();
            }
            0xa8..=0xab => {
                self.b.ppi.write(port, v);
                self.b.mix();
                cassette_lines(&self.b.ppi, self.tape, self.b.chips_t);
            }
            _ => {}
        }
    }
}

impl Bus for MsxBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        let v = if addr >= IO_BASE {
            u32::from(self.io_read(addr as u8))
        } else if self.b.disk_selected(addr as u16) {
            let now = self.b.chips_t;
            self.disk
                .as_deref_mut()
                .map_or(0xff, |d| u32::from(d.read(addr as u16, now)))
        } else {
            u32::from(self.b.mem_peek(addr as u16))
        };
        match self.dbg.as_deref_mut() {
            Some(dbg) if dbg.read_armed(CPU_SPACE) => {
                let seen = dbg.on_read(CPU_SPACE, addr, size, v);
                dbg.trace_read(CPU_SPACE, addr, size, seen, self.beam);
                seen
            }
            _ => v,
        }
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(peek(self.b, addr))
    }

    fn write(&mut self, addr: u32, size: Size, v: u32) {
        if let Some(dbg) = self.dbg.as_deref_mut() {
            if dbg.write_armed(CPU_SPACE) {
                dbg.trace_write(CPU_SPACE, addr, size, v & 0xff, self.beam);
                if !dbg.on_write(CPU_SPACE, addr, size, v & 0xff) {
                    return;
                }
            }
        }
        if addr >= IO_BASE {
            self.io_write(addr as u8, v as u8);
        } else if self.b.disk_selected(addr as u16) {
            let now = self.b.chips_t;
            if let Some(d) = self.disk.as_deref_mut() {
                d.write(addr as u16, v as u8, now);
            }
        } else {
            self.b.mem_write(addr as u16, v as u8);
        }
    }

    fn addr_bits(&self) -> u8 {
        17
    }
}

impl TimedBus for MsxBus<'_> {
    /// One wait state on every M1 cycle.
    fn wait_states(&mut self, access: Access, t: u32) -> u32 {
        let waits = u32::from(matches!(access, Access::Fetch | Access::IntAck));
        self.b.run_to(self.start + u64::from(t + waits));
        waits
    }
}

/// Side-effect-free CPU view (I/O: VDP buffer/status, the PPI's ports,
/// the PSG's latched register without the joystick; $FF elsewhere).
fn peek(b: &Board, addr: u32) -> u8 {
    if addr >= IO_BASE {
        let port = addr as u8;
        return match port {
            0x98 => b.vdp.peek().0,
            0x99 => b.vdp.peek().1,
            0xa2 => b.psg.read(0xff),
            0xa8..=0xab => b.ppi.read(port, 0, b.key_row()),
            _ => 0xff,
        };
    }
    b.mem_peek(addr as u16)
}

struct PeekBus<'a>(&'a Board);

impl Bus for PeekBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(peek(self.0, addr))
    }

    fn write(&mut self, _addr: u32, _size: Size, _v: u32) {}

    fn addr_bits(&self) -> u8 {
        17
    }
}

pub struct Msx {
    model: Model,
    cpu: Z80,
    board: Board,
    ports: [Option<Box<dyn PortDevice>>; 2],
    carts: [CartPort; 2],
    /// T-states since power-on.
    sched: Scheduler,
    input: InputQueue,
    field_done_pending: bool,
    /// Debug overlays switched on (kept over a power cycle) and whether
    /// the beam is at the start of a field, where `overlay` shows the last
    /// complete one. Not machine state.
    overlay_ids: Vec<&'static str>,
    overlay_fresh: bool,
    tape: MsxTape,
    disk: Option<DiskInterface>,
}

impl Msx {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        if let Some(other) = config.model.as_deref() {
            return Err(ConfigError::UnknownModel(other.into()));
        }
        let (bios, logo) = match config.options.get("roms").map(String::as_str) {
            Some("stub") => (stub::rom(), None),
            Some(other) => return Err(ConfigError::Invalid(format!("roms option '{other}'"))),
            None => {
                let r = config
                    .roms
                    .get("msx-bios")
                    .ok_or_else(|| ConfigError::MissingRom("msx-bios".into()))?;
                if r.len() != 0x8000 {
                    return Err(ConfigError::Invalid(format!(
                        "msx-bios must be 32768 bytes, got {}",
                        r.len()
                    )));
                }
                // The logo ROM belongs to C-BIOS only.
                let cbios = r.windows(6).any(|w| w == b"C-BIOS");
                let logo = config
                    .roms
                    .get("msx-logo")
                    .filter(|l| cbios && l.len() == 0x4000)
                    .cloned();
                (r.clone(), logo)
            }
        };
        let model = match config.video_format {
            Some(VideoStandard::Ntsc) => Model::Ntsc,
            Some(VideoStandard::Pal) => Model::Pal,
            None if bios[0x2b] & 0x80 != 0 => Model::Pal,
            None => Model::Ntsc,
        };
        let ram_kb = match config.options.get("ram").map(String::as_str) {
            None | Some("64") => 64,
            Some("32") => 32,
            Some("16") => 16,
            Some("8") => 8,
            Some(other) => {
                return Err(ConfigError::Invalid(format!(
                    "ram option '{other}' (8, 16, 32 or 64)"
                )))
            }
        };
        let disk_rom = match config.options.get("disk").map(String::as_str) {
            None | Some("on") => config
                .roms
                .get("msx-disk-svi738")
                .filter(|r| r.len() == 0x4000)
                .cloned(),
            Some("off") => None,
            Some(other) => {
                return Err(ConfigError::Invalid(format!(
                    "disk option '{other}' (on or off)"
                )))
            }
        };
        if disk_rom.is_none() && config.options.get("disk").is_some_and(|d| d == "on") {
            return Err(ConfigError::MissingRom("msx-disk-svi738".into()));
        }
        let ports = [port_option(config, "port1")?, port_option(config, "port2")?];
        let mappers = [
            mapper_option(config, "mapper1")?,
            mapper_option(config, "mapper2")?,
        ];
        let vdp_model = match model {
            Model::Ntsc => VdpModel::Ntsc,
            Model::Pal => VdpModel::Pal,
        };
        let mut m = Self {
            model,
            cpu: Z80::new(),
            board: Board {
                bios,
                logo,
                ram: vec![0; ram_kb * 1024],
                disk_rom: disk_rom.clone(),
                sub: 0,
                carts: [None, None],
                cart_sound: false,
                vdp: Tms9918::new(vdp_model),
                psg: Ay::new(),
                ppi: Ppi::new(),
                keys: [0; KEY_ROWS],
                level: 0,
                chips_t: 0,
                field_done: false,
                audio: Decimator::new(CPU_HZ, emu_core::SAMPLE_RATE)
                    .ok_or_else(|| ConfigError::Invalid("audio rate".into()))?,
                dc: [0; 2],
                samples: Vec::new(),
            },
            ports,
            carts: mappers.map(CartPort::new),
            sched: Scheduler::new(),
            input: InputQueue::new(),
            field_done_pending: false,
            overlay_ids: Vec::new(),
            overlay_fresh: false,
            tape: MsxTape::new(CPU_HZ),
            disk: disk_rom.is_some().then(|| DiskInterface::new(CPU_HZ)),
        };
        m.cpu.reset();
        Ok(m)
    }

    pub fn model(&self) -> Model {
        self.model
    }

    fn beam_pos(&self) -> BeamPos {
        let v = &self.board.vdp;
        BeamPos {
            frame: v.fields,
            line: v.line,
            cycle: v.dot * 2 / 3,
            // The frame column: pixel clock 0 is the left border's first
            // column; the blanking after the right border clamps to the
            // last column.
            hpos: v.dot.min(vdp::WIDTH as u16 - 1),
        }
    }

    /// One instruction (or interrupt response). Returns `Some` if
    /// emulation must stop.
    fn instruction(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        let changed = self
            .carts
            .iter_mut()
            .fold(false, |c, slot| slot.inner.take_changed() | c);
        if changed {
            self.reset(ResetKind::Hard);
        }
        for (port, cart) in self.carts.iter_mut().zip(&mut self.board.carts) {
            // A battery RAM file went in: the cartridge's RAM is its contents.
            if std::mem::take(&mut port.battery.loaded) {
                if let (Some(c), Some(file)) = (cart, &port.battery.contents) {
                    if c.sram().len() == file.len() {
                        c.sram_mut().copy_from_slice(file);
                    }
                }
            }
        }
        while let Some(ev) = self.input.pop_due(self.sched.now()) {
            self.apply_input(ev);
        }
        let start = self.sched.now().0;
        let beam = self.beam_pos();
        let t = {
            let mut bus = MsxBus {
                b: &mut self.board,
                ports: &mut self.ports,
                tape: &mut self.tape,
                disk: self.disk.as_mut(),
                start,
                dbg: dbg.as_deref_mut(),
                beam,
            };
            self.cpu.step_timed(&mut bus)
        };
        let end = start + u64::from(t);
        self.board.run_to(end);
        self.sched.advance(u64::from(t));
        self.cpu.set_int(self.board.vdp.int());
        let entered = self.cpu.take_interrupt();
        let field = std::mem::take(&mut self.board.field_done);
        if field && self.board.vdp.overlays_on() {
            self.board.vdp.overlay_end_frame();
            self.overlay_fresh = true;
        }
        if field {
            self.tape.sync(end);
            if let Some(d) = &mut self.disk {
                d.tick(end);
            }
            for (port, cart) in self.carts.iter_mut().zip(&self.board.carts) {
                if let Some(c) = cart.as_ref().filter(|c| !c.sram().is_empty()) {
                    port.battery.update(c.sram());
                }
            }
        }
        if let Some(dbg) = dbg {
            if let Some(kind) = entered {
                if dbg.interrupt_armed(kind) {
                    dbg.check_interrupt(kind);
                }
            }
            let next_pc = u32::from(self.cpu.pc);
            if dbg.trace_instr_armed() {
                dbg.trace_instr(CPU, next_pc, self.beam_pos());
            }
            if dbg.instr_step_armed() {
                dbg.check_instr_step();
            }
            if dbg.exec_armed(CPU_SPACE) {
                dbg.check_exec(CPU_SPACE, next_pc);
            }
            if dbg.raster_armed() {
                let b = self.beam_pos();
                dbg.check_raster(b.line, b.cycle);
            }
            if let Some(stop) = dbg.take_stop() {
                self.field_done_pending |= field;
                return Some(stop);
            }
        }
        field.then_some(StopReason::FrameDone)
    }

    fn apply_input(&mut self, ev: TimedInput) {
        match ev.target {
            InputTarget::Port(p) => {
                if let Some(Some(dev)) = self.ports.get_mut(usize::from(p.0)) {
                    dev.input(&ev.event);
                }
            }
            InputTarget::Keyboard => {
                if let InputEvent::Key { machine_key, down } = ev.event {
                    self.key(machine_key, down);
                }
            }
            InputTarget::Panel => {
                if let InputEvent::Control { id, value: 1 } = ev.event {
                    match id.0 {
                        0 => self.reset(ResetKind::Hard),
                        1 => self.reset(ResetKind::Soft),
                        _ => {}
                    }
                }
            }
        }
    }

    /// A keyboard key: its switch in the matrix.
    fn key(&mut self, key: MachineKey, down: bool) {
        let Some(row) = self.board.keys.get_mut(usize::from(key.0 / 8)) else {
            return;
        };
        let bit = 1 << (key.0 % 8);
        if down {
            *row |= bit;
        } else {
            *row &= !bit;
        }
    }

    fn signal(&self) -> SignalInfo {
        let standard = match self.model {
            Model::Ntsc => VideoStandard::Ntsc,
            Model::Pal => VideoStandard::Pal,
        };
        SignalInfo {
            pixel_clock_hz: CPU_HZ * 3 / 2,
            active_pixels: vdp::WIDTH,
            total_pixels: u32::from(vdp::LINE),
            lines_per_field: u32::from(self.board.vdp.model.lines()),
            standard,
            output: OutputType::Composite,
            field: FieldParity::Progressive,
        }
    }
}

/// Read a byte as the CPU sees it now (tests and tools).
pub fn peek_cpu(m: &Msx, addr: u16) -> u8 {
    m.board.mem_peek(addr)
}

/// The VDP (tests and tools).
pub fn vdp(m: &Msx) -> &Tms9918 {
    &m.board.vdp
}

impl Machine for Msx {
    fn run_frame(&mut self, dbg: &mut DebugHooks) -> StopReason {
        if std::mem::take(&mut self.field_done_pending) {
            return StopReason::FrameDone;
        }
        self.overlay_fresh = false;
        if self.board.vdp.line == 0 && self.board.vdp.dot < 3 {
            for f in dbg.per_frame_freezes() {
                if f.space == CPU_SPACE && f.addr < IO_BASE {
                    for i in 0..f.size.bytes() {
                        self.board
                            .mem_write((f.addr + i) as u16, (f.value >> (8 * i)) as u8);
                    }
                }
            }
        }
        loop {
            if let Some(stop) = self.instruction(Some(dbg)) {
                return stop;
            }
        }
    }

    fn step_cycle(&mut self) -> StopReason {
        self.overlay_fresh = false;
        self.instruction(None);
        StopReason::StepDone
    }

    /// Hard: power cycle (RAM and VRAM cleared, the cartridges taken from
    /// their slots anew). Soft: the RESET button (the Z80, the PPI, the PSG,
    /// the VDP's registers and the cartridges' bank registers).
    fn reset(&mut self, kind: ResetKind) {
        self.cpu = Z80::new();
        self.cpu.reset();
        self.board.vdp.reset();
        self.board.ppi.reset();
        self.board.psg.reset();
        self.board.sub = 0;
        if let Some(d) = &mut self.disk {
            d.reset();
        }
        if kind == ResetKind::Hard {
            self.board.ram.fill(0);
            self.board.vdp = Tms9918::new(self.board.vdp.model);
            self.board.vdp.set_overlays(&self.overlay_ids);
            self.board.keys = [0; KEY_ROWS];
            for i in 0..2 {
                self.board.carts[i] = self.carts[i].working_copy();
                // The battery keeps the RAM through the power cycle.
                let bat = &mut self.carts[i].battery;
                bat.loaded = false;
                if let (Some(c), Some(kept)) = (&mut self.board.carts[i], &bat.contents) {
                    if kept.len() == c.sram().len() {
                        c.sram_mut().copy_from_slice(kept);
                    }
                }
            }
        }
        self.board.cart_sound = self.board.carts.iter().flatten().any(|c| c.scc.is_some());
        for c in self.board.carts.iter_mut().flatten() {
            c.reset();
        }
        self.board.mix();
        cassette_lines(&self.board.ppi, &mut self.tape, self.board.chips_t);
        self.input.clear();
        self.field_done_pending = false;
    }

    fn framebuffer(&self) -> FrameRef<'_> {
        FrameRef {
            width: vdp::WIDTH,
            height: self.board.vdp.model.height(),
            pixels: PixelData::Indexed {
                pixels: self.board.vdp.frame(),
                palette: &vdp::PALETTE,
            },
            luma_chroma: None,
            signal: self.signal(),
        }
    }

    /// The TMS9918A/TMS9929A's views (`tiles`, `sprites`, `screen`,
    /// `palette`) from VRAM; option `addr` is a VRAM address.
    fn gfx_views(&self) -> Vec<GfxViewInfo> {
        vdp::gfx::views(self.board.vdp.model.chip_name())
    }

    fn gfx_view(&self, id: &str, opts: &GfxOptions) -> Option<GfxView> {
        self.board.vdp.gfx_view(id, VRAM_SPACE, opts)
    }

    /// `sprites` (a box per sprite where the VDP drew it, per line) and
    /// `limit` (bands on lines with a fifth sprite).
    fn overlays(&self) -> Vec<OverlayInfo> {
        vdp::overlay::infos(self.board.vdp.model.chip_name())
    }

    fn set_overlays(&mut self, ids: &[&str]) {
        self.overlay_ids = self
            .overlays()
            .into_iter()
            .map(|o| o.id)
            .filter(|id| ids.contains(id))
            .collect();
        self.board.vdp.set_overlays(ids);
    }

    fn overlay(&self, id: &str) -> Option<Overlay> {
        self.board.vdp.overlay(id, self.overlay_fresh)
    }

    /// Line 0 is the first active line; the borders and the top border
    /// of the next field are rows too.
    fn frame_row(&self, line: u16) -> Option<u32> {
        self.board.vdp.line_row(line)
    }

    fn audio(&mut self, out: &mut Vec<i16>) -> emu_core::AudioFormat {
        out.append(&mut self.board.samples);
        emu_core::AudioFormat {
            sample_rate: emu_core::SAMPLE_RATE,
            channels: 1,
        }
    }

    fn beam(&self) -> Option<BeamPos> {
        Some(self.beam_pos())
    }

    fn memory_spaces(&self) -> Vec<MemSpace> {
        vec![
            MemSpace {
                id: CPU_SPACE,
                name: "cpu",
                device: CPU,
                addr_bits: 17,
                big_endian: false,
            },
            MemSpace {
                id: VRAM_SPACE,
                name: "vram",
                device: CPU,
                addr_bits: 14,
                big_endian: false,
            },
        ]
    }

    fn chips(&self) -> Vec<ChipInfo> {
        let b = &self.board;
        let v = &b.vdp;
        let reg = |name, value: u32, bits| ChipReg {
            name,
            addr: None,
            value,
            bits,
        };
        const R: [&str; 16] = [
            "R0", "R1", "R2", "R3", "R4", "R5", "R6", "R7", "R8", "R9", "R10", "R11", "R12", "R13",
            "R14", "R15",
        ];
        let mut vdp_regs: Vec<ChipReg> = R
            .iter()
            .zip(v.regs)
            .map(|(n, val)| reg(*n, u32::from(val), 8))
            .collect();
        vdp_regs.extend([
            reg("STATUS", u32::from(v.peek().1), 8),
            reg("ADDR", u32::from(v.addr), 14),
            reg("LINE", u32::from(v.line), 9),
            reg("DOT", u32::from(v.dot), 9),
        ]);
        let mut psg_regs: Vec<ChipReg> = R
            .iter()
            .zip(b.psg.regs)
            .map(|(n, val)| reg(*n, u32::from(val), 8))
            .collect();
        psg_regs.push(reg("LATCH", u32::from(b.psg.addr), 8));
        let slots = b.slots();
        let mut slot_regs = vec![
            reg("PAGE0", u32::from(slots & 3), 2),
            reg("PAGE1", u32::from(slots >> 2 & 3), 2),
            reg("PAGE2", u32::from(slots >> 4 & 3), 2),
            reg("PAGE3", u32::from(slots >> 6), 2),
            reg("SUB3", u32::from(b.sub), 8),
        ];
        const BANKS: [[&str; 4]; 2] = [
            ["CART1_4000", "CART1_6000", "CART1_8000", "CART1_A000"],
            ["CART2_4000", "CART2_6000", "CART2_8000", "CART2_A000"],
        ];
        for (cart, names) in b.carts.iter().zip(BANKS) {
            let banks = cart.as_ref().map_or([0; 4], |c| c.banks);
            slot_regs.extend(
                names
                    .iter()
                    .zip(banks)
                    .map(|(n, bank)| reg(*n, u32::from(bank), 8)),
            );
        }
        vec![
            ChipInfo {
                name: "vdp",
                regs: vdp_regs,
            },
            ChipInfo {
                name: "psg",
                regs: psg_regs,
            },
            ChipInfo {
                name: "ppi",
                regs: vec![
                    reg("A", u32::from(b.ppi.a), 8),
                    reg("B", u32::from(b.key_row()), 8),
                    reg("C", u32::from(b.ppi.c_pins()), 8),
                    reg("CONTROL", u32::from(b.ppi.control), 8),
                ],
            },
            ChipInfo {
                name: "slots",
                regs: slot_regs,
            },
        ]
    }

    fn devices(&self) -> Vec<DeviceId> {
        vec![CPU]
    }

    fn debug_target(&mut self, dev: DeviceId) -> Option<&mut dyn DebugTarget> {
        (dev == CPU).then_some(self as &mut dyn DebugTarget)
    }

    fn input(&mut self, ev: TimedInput) {
        self.input.push(ev);
    }

    fn now(&self) -> Cycle {
        self.sched.now()
    }

    fn controls(&self) -> Vec<emu_core::ControlInfo> {
        vec![
            emu_core::ControlInfo::button(0, "POWER", false),
            emu_core::ControlInfo::button(1, "RESET", false),
        ]
    }

    fn ports(&self) -> Vec<PortInfo> {
        [(PORT1, "joystick 1"), (PORT2, "joystick 2")]
            .into_iter()
            .map(|(id, name)| PortInfo {
                id,
                kind: PortKind::MsxJoy,
                name,
                connected: self.ports[usize::from(id.0)].is_some(),
            })
            .collect()
    }

    fn connect(&mut self, port: PortId, dev: Box<dyn PortDevice>) -> Result<(), PortError> {
        let Some(slot) = self.ports.get_mut(usize::from(port.0)) else {
            return Err(PortError::NoSuchPort(port));
        };
        if dev.kind() != PortKind::MsxJoy {
            return Err(PortError::Incompatible {
                port: PortKind::MsxJoy,
                device: dev.kind(),
            });
        }
        if slot.is_some() {
            return Err(PortError::Occupied(port));
        }
        *slot = Some(dev);
        Ok(())
    }

    fn disconnect(&mut self, port: PortId) -> Option<Box<dyn PortDevice>> {
        self.ports.get_mut(usize::from(port.0))?.take()
    }

    fn media_slots(&self) -> Vec<MediaSlotInfo> {
        let mut slots: Vec<MediaSlotInfo> =
            [(SLOT_CART1, "cartridge 1"), (SLOT_CART2, "cartridge 2")]
                .into_iter()
                .map(|(id, name)| MediaSlotInfo {
                    id,
                    name,
                    kind: MediaKind::Cartridge,
                    inserted: self.carts[usize::from(id.0)].inner.is_inserted(),
                })
                .collect();
        slots.push(MediaSlotInfo {
            id: SLOT_BATTERY,
            name: "battery RAM",
            kind: MediaKind::BatteryRam,
            inserted: self.carts[0].battery.is_inserted(),
        });
        slots.push(MediaSlotInfo {
            id: SLOT_TAPE,
            name: "cassette",
            kind: MediaKind::Tape,
            inserted: self.tape.has_tape(),
        });
        if let Some(d) = &self.disk {
            for (i, (id, name)) in [(SLOT_DRIVE_A, "drive A"), (SLOT_DRIVE_B, "drive B")]
                .into_iter()
                .enumerate()
            {
                slots.push(MediaSlotInfo {
                    id,
                    name,
                    kind: MediaKind::Disk,
                    inserted: d.drives[i].image.is_some(),
                });
            }
        }
        slots
    }

    fn media_slot(&mut self, slot: SlotId) -> Option<&mut dyn MediaSlot> {
        if slot == SLOT_BATTERY {
            return Some(&mut self.carts[0].battery as &mut dyn MediaSlot);
        }
        if slot == SLOT_TAPE {
            return Some(&mut self.tape as &mut dyn MediaSlot);
        }
        if slot == SLOT_DRIVE_A || slot == SLOT_DRIVE_B {
            let n = usize::from(slot.0 - SLOT_DRIVE_A.0);
            return self
                .disk
                .as_mut()
                .map(|d| &mut d.drives[n] as &mut dyn MediaSlot);
        }
        self.carts
            .get_mut(usize::from(slot.0))
            .map(|c| c as &mut dyn MediaSlot)
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        let b = &self.board;
        w.chunk(*b"MSX1", 1, |w| {
            w.u8(self.model as u8);
            w.bool(self.field_done_pending);
            w.bytes(&b.ram);
            w.raw(&b.keys);
            w.u8(b.sub);
            w.bool(self.disk.is_some());
            w.u64(b.chips_t);
            w.bool(b.field_done);
            b.audio.save(w);
            w.u32(b.dc[0] as u32);
            w.u32(b.dc[1] as u32);
            for c in &b.carts {
                w.bool(c.is_some());
            }
        });
        self.cpu.save(&mut w);
        b.vdp.save(&mut w);
        b.psg.save(&mut w);
        b.ppi.save(&mut w);
        for c in b.carts.iter().flatten() {
            c.save(&mut w);
        }
        self.sched.save(&mut w);
        self.input.save(&mut w);
        for (i, tag) in [*b"MSP1", *b"MSP2"].into_iter().enumerate() {
            w.chunk(tag, 1, |w| {
                w.bytes(
                    &self.ports[i]
                        .as_ref()
                        .map(|d| d.save_state())
                        .unwrap_or_default(),
                );
            });
        }
        self.tape.save(&mut w);
        if let Some(d) = &self.disk {
            d.save(&mut w);
        }
        w.into_bytes()
    }

    /// Atomic: everything loads into copies first.
    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"MSX1", 1)?;
        if c.u8()? != self.model as u8 {
            return Err(StateError::Invalid("model differs"));
        }
        let field_done_pending = c.bool()?;
        let mut board = self.board.clone();
        let ram = c.bytes()?;
        if ram.len() != board.ram.len() {
            return Err(StateError::Invalid("RAM size"));
        }
        board.ram = ram.to_vec();
        c.raw_into(&mut board.keys)?;
        board.sub = c.u8()?;
        if c.bool()? != self.disk.is_some() {
            return Err(StateError::Invalid("disk interface differs"));
        }
        board.chips_t = c.u64()?;
        board.field_done = c.bool()?;
        board.audio.load(&mut c)?;
        board.dc = [c.u32()? as i32, c.u32()? as i32];
        for cart in &board.carts {
            if c.bool()? != cart.is_some() {
                return Err(StateError::Invalid("cartridge differs"));
            }
        }
        board.samples.clear();
        let mut cpu = self.cpu.clone();
        let mut sched = self.sched.clone();
        let mut input = self.input.clone();
        cpu.load(&mut r)?;
        board.vdp.load(&mut r)?;
        board.psg.load(&mut r)?;
        board.ppi.load(&mut r)?;
        for cart in board.carts.iter_mut().flatten() {
            cart.load(&mut r)?;
        }
        sched.load(&mut r)?;
        input.load(&mut r)?;
        let mut port_states = Vec::new();
        for tag in [*b"MSP1", *b"MSP2"] {
            let (_, mut c) = r.chunk(tag, 1)?;
            port_states.push(c.bytes()?.to_vec());
        }
        // The deck holds media: check its chunk now, load it last.
        let tape_at = s.len() - r.remaining();
        MsxTape::new(CPU_HZ).load(&mut r)?;
        // The same for the disk interface.
        let disk_at = s.len() - r.remaining();
        if self.disk.is_some() {
            DiskInterface::new(CPU_HZ).load(&mut r)?;
        }
        for (dev, st) in self.ports.iter().zip(&port_states) {
            if dev.is_none() != st.is_empty() {
                return Err(StateError::Invalid("joysticks differ"));
            }
        }
        for (dev, st) in self.ports.iter_mut().zip(&port_states) {
            if let Some(d) = dev {
                d.load_state(st)?;
            }
        }
        board.mix();
        self.tape.load(&mut StateReader::new(&s[tape_at..]))?;
        if let Some(d) = &mut self.disk {
            d.load(&mut StateReader::new(&s[disk_at..]))?;
        }
        self.cpu = cpu;
        self.board = board;
        self.sched = sched;
        self.input = input;
        self.field_done_pending = field_done_pending;
        Ok(())
    }
}

impl DebugTarget for Msx {
    fn regs(&self) -> RegisterSet {
        self.cpu.regs()
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        self.cpu.set_reg(name, v);
    }

    fn peek(&self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
        let byte = |a: u32| -> Option<u32> {
            match space {
                CPU_SPACE if a < IO_BASE && self.board.disk_selected(a as u16) => self
                    .disk
                    .as_ref()
                    .map(|d| u32::from(d.peek(a as u16, self.board.chips_t))),
                CPU_SPACE => Some(u32::from(peek(&self.board, a))),
                VRAM_SPACE => Some(u32::from(self.board.vdp.vram[(a & 0x3fff) as usize])),
                _ => None,
            }
        };
        (0..size.bytes()).try_fold(0, |v, i| Some(v | byte(addr.wrapping_add(i))? << (8 * i)))
    }

    /// CPU space: RAM where a page shows it; ROM and I/O are not poked.
    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        for i in 0..size.bytes() {
            let a = addr.wrapping_add(i);
            let byte = (v >> (8 * i)) as u8;
            match space {
                CPU_SPACE if a < IO_BASE => match self.board.ram_index(a as u16) {
                    Some(i) => self.board.ram[i] = byte,
                    None => return false,
                },
                VRAM_SPACE => self.board.vdp.vram[(a & 0x3fff) as usize] = byte,
                _ => return false,
            }
        }
        true
    }

    fn disassemble(&self, addr: u32) -> Instr {
        self.cpu.disassemble(&PeekBus(&self.board), addr)
    }
}

#[cfg(test)]
mod tests;
