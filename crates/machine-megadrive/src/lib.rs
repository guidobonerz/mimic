//! Sega Mega Drive / Genesis (machine `megadrive`).
//!
//! **Clocks.** The master clock (NTSC 53.693175 MHz, PAL 53.203424 MHz) is
//! the time base: the 68000 runs at ÷7, the Z80 and the PSG at ÷15, the
//! YM2612 at ÷7 (a sample every 144 of its clocks), the VDP's line is 3420
//! master clocks.
//!
//! **68000 memory map.** $000000–$3FFFFF the cartridge (ROM, with the SSF2
//! mapper eight 512 KB banks at $A130F3–$A130FF; backup RAM where the
//! header puts it, switched in by $A130F1 bit 0 when the ROM reaches it;
//! the X24C01 EEPROM at $200001), $A00000–$A0FFFF the Z80's space while
//! the 68000 has its bus (byte-wide: a word write stores the high byte, a
//! word read repeats the byte), $A10000–$A1001F the I/O chip (version,
//! three ports' data and control, the serial registers), $A11100 the Z80
//! bus request (bit 0 / bit 8; reads 0 when the 68000 has the bus),
//! $A11200 the Z80 reset (0 holds it, with the YM2612), $A14000 the TMSS
//! "SEGA" register, $A14101 the TMSS ROM switch, $C00000–$DFFFFF the VDP
//! (`110n n000 nnnn nnnn 000m mmmm`; byte writes repeat the byte, the PSG
//! at the odd bytes $11–$17), $E00000–$FFFFFF the 64 KB work RAM,
//! repeated.
//!
//! **Z80 memory map.** $0000–$3FFF its 8 KB RAM (twice), $4000–$5FFF the
//! YM2612 (four ports, repeated), $6000 the bank register (each write
//! shifts bit 0 in as the bank's next address bit 15–23), $7F00–$7F1F the
//! VDP (the PSG, the HV counter), $8000–$FFFF the 32 KB bank of the
//! 68000's space. The VDP's /INT on line $E0 is the Z80's /INT (mode 1).
//!
//! **I/O.** The version register: bit 7 export, bit 6 PAL, bit 5 no
//! expansion unit, bits 3–0 the TMSS version. A data register's bits set
//! as outputs by its control register read back what was written, the
//! inputs read the port's lines (bit 0 UP … bit 3 RIGHT, bit 4 TR, bit 5
//! TL, bit 6 TH), bit 7 a latch. TH, written as an output, selects the
//! pad's halves ([`devices_input::MdPad`]).
//!
//! **TMSS** (option `tmss=on`; version 1): the VDP ignores the 68000 until
//! "SEGA" is written to $A14000. With a TMSS ROM (role `md-tmss`, 2 KB)
//! it starts first, mapped at $000000 until $A14101 bit 0 is set.
//!
//! **Interrupts.** The VDP's levels 6 (vertical) and 4 (horizontal), taken
//! by autovector; the acknowledge clears the VDP's pending flag.
//!
//! Sources: Charles MacDonald, *Sega Genesis hardware notes* v0.8 (memory
//! maps, the Z80's bus, banking and reset, I/O registers, pads, EEPROM,
//! TMSS), *Sega Genesis VDP documentation* v1.5f; Bart Trzynadlowski,
//! *SSFII Genesis technical information* (2000; the mapper); Sega,
//! *Genesis Software Manual*. The implementation is this project's own.

#![forbid(unsafe_code)]

pub mod demo;
pub mod eeprom;

use chips_sega::mdvdp::{self, MdVdp};
use chips_ti::Sn76489;
use chips_yamaha::Ym2612;
use cpu_m68k::{Access as Access68, Step, TimedBus68, M68000};
use cpu_z80::{Access as AccessZ80, TimedBus, IO_BASE, Z80};
use devices_cartridge::megadrive::MdCart;
use devices_cartridge::slot::BatterySlot;
use devices_cartridge::CartSlot;
use devices_input::megadrive::pin;
use devices_input::MdPad;
use eeprom::X24c01;
use mimic_core::audio::Decimator;
use mimic_core::frame::{FieldParity, FrameRef, OutputType, PixelData, SignalInfo, VideoStandard};
use mimic_core::gfx::{GfxOptions, GfxView, GfxViewInfo};
use mimic_core::input::{InputEvent, InputQueue, InputTarget, TimedInput};
use mimic_core::machine::ChipReg;
use mimic_core::media::{Media, MediaError, MediaKind, MediaSlot, MediaSlotInfo, SlotId};
use mimic_core::overlay::{Overlay, OverlayInfo};
use mimic_core::port::{PortDevice, PortError, PortId, PortInfo, PortKind, PortLines};
use mimic_core::registry::{ConfigError, MachineConfig, MachineEntry};
use mimic_core::{
    BeamPos, Bus, ChipInfo, Cpu, Cycle, DebugHooks, DebugTarget, DeviceId, Instr, Machine,
    MemSpace, MemSpaceId, RegisterSet, ResetKind, Savestate, Scheduler, Size, StateError,
    StateReader, StateWriter, StopReason,
};

pub const CPU: DeviceId = DeviceId("cpu");
pub const Z80_DEV: DeviceId = DeviceId("z80");
/// The 68000's 24-bit space.
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);
pub const VRAM_SPACE: MemSpaceId = MemSpaceId(1);
pub const CRAM_SPACE: MemSpaceId = MemSpaceId(2);
pub const VSRAM_SPACE: MemSpaceId = MemSpaceId(3);
/// The Z80's view (memory at 0–$FFFF).
pub const Z80_SPACE: MemSpaceId = MemSpaceId(4);
pub const SLOT_CARTRIDGE: SlotId = SlotId(0);
/// The cartridge's backup RAM or EEPROM (`.srm`).
pub const SLOT_BATTERY: SlotId = SlotId(1);
pub const PORT1: PortId = PortId(0);
pub const PORT2: PortId = PortId(1);

pub const MCLK_NTSC: u32 = 53_693_175;
pub const MCLK_PAL: u32 = 53_203_424;

const CONTROL_POWER: u16 = 0;
const CONTROL_RESET: u16 = 1;

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "megadrive",
        name: "Sega Mega Drive / Genesis",
        models: &["megadrive"],
        video_formats: mimic_core::registry::PAL_NTSC,
        create: |config| Ok(Box::new(MegaDrive::new(config)?)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    Japan,
    Usa,
    Europe,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Save {
    None,
    Sram,
    Eeprom,
}

/// What backs up the game: the option, else the header's backup RAM.
fn save_kind(option: Option<Save>, cart: Option<&MdCart>) -> Save {
    match (option, cart) {
        (Some(s), _) => s,
        (None, Some(c)) if c.backup.is_some() => Save::Sram,
        _ => Save::None,
    }
}

/// The backup memory's size in bytes.
fn backup_len(save: Save, cart: Option<&MdCart>) -> usize {
    match save {
        Save::Sram => cart
            .and_then(|c| c.backup)
            .map_or(0x10000, |r| r.len().max(1)),
        Save::Eeprom => eeprom::SIZE,
        Save::None => 0,
    }
}

/// The cartridge port's two media: the cartridge and its backup memory
/// (`SLOT_BATTERY`, a `.srm` file of exactly the backup's size; frontends
/// attach it with `media_backends::image::attach_battery_ram`). A new
/// cartridge tells the battery slot its size at once.
struct MdSlot {
    inner: CartSlot<MdCart>,
    battery: BatterySlot,
    save_option: Option<Save>,
}

impl MediaSlot for MdSlot {
    fn insert(&mut self, media: Media) -> Result<(), MediaError> {
        self.inner.insert(media)?;
        let cart = self.inner.cart();
        let len = backup_len(save_kind(self.save_option, cart), cart);
        self.battery.cart_changed((len > 0).then_some(len));
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        self.battery.cart_changed(None);
        self.inner.eject()
    }

    fn write_protect(&mut self, on: bool) {
        self.inner.write_protect(on);
    }

    fn flush(&mut self) -> Result<(), MediaError> {
        self.inner.flush()
    }
}

/// One I/O port's registers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct IoPort {
    data: u8,
    ctrl: u8,
    tx: u8,
    serial: u8,
}

/// Everything except the 68000 and the port devices.
#[derive(Clone, Debug)]
struct Board {
    pal: bool,
    region: Region,
    tmss: bool,
    tmss_unlocked: bool,
    /// The bytes written to $A14000–$A14003.
    tmss_key: [u8; 4],
    tmss_rom: Option<Vec<u8>>,
    tmss_mapped: bool,
    ram: Vec<u8>,
    cart: Option<MdCart>,
    /// SSF2 banks (512 KB each) of the eight regions.
    banks: [u8; 8],
    save: Save,
    sram: Vec<u8>,
    /// $A130F1 bit 0 (backup RAM over ROM), bit 1 write protect.
    sram_ctl: u8,
    sram_dirty: bool,
    eeprom: X24c01,
    vdp: MdVdp,
    psg: Sn76489,
    ym: Ym2612,
    z80: Z80,
    z80_ram: Vec<u8>,
    z80_bank: u16,
    /// /RESET held (the Z80 and the YM2612) and /BUSREQ asserted.
    z80_reset: bool,
    z80_busreq: bool,
    /// Master clocks the Z80, the YM2612 and the PSG have reached.
    z80_t: u64,
    ym_t: u64,
    psg_t: u64,
    ym_clocks: u8,
    io: [IoPort; 3],
    /// The ports' input lines as last sampled (data register bits).
    io_inputs: [u8; 3],
    /// 68000 clocks the next bus cycle waits (a DMA froze it).
    pending_wait: u32,
    /// The 68000's last program fetch: what unmapped addresses read (the
    /// next instruction word's high byte, the low byte 0).
    prefetch: u16,
    field_done: bool,
    audio: [Decimator; 2],
    dc: [[i32; 2]; 2],
    samples: Vec<i16>,
}

impl Board {
    fn z80_running(&self) -> bool {
        !self.z80_reset && !self.z80_busreq
    }

    /// The 68000 may use the Z80's bus.
    fn z80_bus_granted(&self) -> bool {
        self.z80_busreq && !self.z80_reset
    }

    fn version(&self) -> u8 {
        u8::from(self.region != Region::Japan) << 7
            | u8::from(self.pal) << 6
            | 0x20
            | u8::from(self.tmss)
    }

    fn vdp_locked(&self) -> bool {
        self.tmss && !self.tmss_unlocked
    }

    // ---- Time ----------------------------------------------------------

    /// Advance everything but the 68000 to master clock `t`.
    fn run_to(&mut self, t: u64) {
        loop {
            if !self.z80_running() {
                self.z80_t = self.z80_t.max(t);
                break;
            }
            if self.z80_t >= t {
                break;
            }
            self.z80_instruction();
        }
        self.chips_to(t);
    }

    fn z80_instruction(&mut self) {
        let mut z80 = std::mem::take(&mut self.z80);
        z80.set_int(self.vdp.z80_int());
        let start = self.z80_t;
        let t = {
            let mut bus = Z80Bus { b: self, start };
            z80.step_timed(&mut bus)
        };
        self.z80 = z80;
        self.z80_t = start + u64::from(t) * 15;
    }

    /// The VDP, YM2612 and PSG to `t`.
    fn chips_to(&mut self, t: u64) {
        if t > self.vdp.now {
            self.field_done |= self.vdp.advance((t - self.vdp.now) as u32);
        }
        while self.psg_t + 15 <= t {
            self.psg_t += 15;
            self.psg.tick();
        }
        while self.ym_t + 7 <= t {
            self.ym_t += 7;
            if !self.z80_reset {
                self.ym.tick();
            }
            self.ym_clocks += 1;
            if self.ym_clocks == chips_yamaha::ym2612::CLOCKS_PER_SAMPLE {
                self.ym_clocks = 0;
                self.mix();
            }
        }
    }

    /// One output sample at the YM2612's rate.
    fn mix(&mut self) {
        let (l, r) = self.ym.output();
        let psg = self.psg.level() - 16_382;
        for (c, v) in [(0, l), (1, r)] {
            let level = v / 2 + psg / 2;
            if let Some(s) = self.audio[c].push(level) {
                let x = i32::from(s) << 16;
                let y = x - self.dc[c][0] + ((i64::from(self.dc[c][1]) * 65_208) >> 16) as i32;
                self.dc[c] = [x, y];
                self.samples
                    .push((y >> 16).clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16);
            }
        }
    }

    // ---- The cartridge area --------------------------------------------

    fn backup_offset(&self, addr: u32) -> Option<usize> {
        if self.save != Save::Sram {
            return None;
        }
        let cart = self.cart.as_ref()?;
        let b = cart.backup?;
        // Over ROM only while $A130F1 bit 0 says so.
        if cart.rom().len() > b.start as usize && self.sram_ctl & 1 == 0 {
            return None;
        }
        b.offset(addr).filter(|&o| o < self.sram.len())
    }

    fn cart_byte(&self, addr: u32) -> u8 {
        if self.tmss_mapped {
            if let Some(rom) = &self.tmss_rom {
                return rom[addr as usize % rom.len()];
            }
        }
        if self.save == Save::Eeprom && addr == 0x20_0001 {
            return u8::from(self.eeprom.sda());
        }
        if let Some(o) = self.backup_offset(addr) {
            return self.sram[o];
        }
        let Some(cart) = &self.cart else {
            return 0xff;
        };
        let offset = if cart.ssf2 {
            usize::from(self.banks[(addr >> 19) as usize & 7]) * 0x8_0000
                + (addr as usize & 0x7_ffff)
        } else {
            addr as usize
        };
        cart.byte(offset)
    }

    fn cart_write(&mut self, addr: u32, v: u8) {
        if self.save == Save::Eeprom && (addr | 1) == 0x20_0001 {
            self.eeprom.write(v & 2 != 0, v & 1 != 0);
            return;
        }
        if self.sram_ctl & 2 != 0 {
            return;
        }
        if let Some(o) = self.backup_offset(addr) {
            if self.sram[o] != v {
                self.sram[o] = v;
                self.sram_dirty = true;
            }
        }
    }

    // ---- The Z80's space -----------------------------------------------

    fn z80_peek(&self, a: u16) -> u8 {
        match a {
            0x0000..=0x3fff => self.z80_ram[usize::from(a & 0x1fff)],
            0x4000..=0x5fff => self.ym.read_status(),
            0x7f00..=0x7f1f => self.vdp_peek_byte(u32::from(a & 0x1f)).unwrap_or(0xff),
            0x8000..=0xffff => {
                let addr = u32::from(self.z80_bank) << 15 | u32::from(a & 0x7fff);
                self.peek68_byte(addr)
            }
            _ => 0xff,
        }
    }

    fn z80_read(&mut self, a: u16) -> u8 {
        match a {
            0x7f00..=0x7f1f => self.vdp_read_byte(u32::from(a & 0x1f)).unwrap_or(0xff),
            _ => self.z80_peek(a),
        }
    }

    fn z80_write(&mut self, a: u16, v: u8) {
        match a {
            0x0000..=0x3fff => self.z80_ram[usize::from(a & 0x1fff)] = v,
            0x4000..=0x5fff => {
                if !self.z80_reset {
                    self.ym.write((a & 3) as u8, v);
                }
            }
            0x6000..=0x60ff => self.z80_bank = (self.z80_bank >> 1) | u16::from(v & 1) << 8,
            0x7f00..=0x7f1f => self.vdp_write_byte(u32::from(a & 0x1f), v),
            0x8000..=0xffff => {
                let addr = u32::from(self.z80_bank) << 15 | u32::from(a & 0x7fff);
                if addr >= 0xe0_0000 {
                    self.ram[(addr & 0xffff) as usize] = v;
                } else if addr < 0x40_0000 {
                    self.cart_write(addr, v);
                }
            }
            _ => {}
        }
    }

    // ---- The VDP -------------------------------------------------------

    /// A VDP port word without side effects: the status's top six bits and
    /// the unused ports $10-$1E come from the open bus.
    fn vdp_peek_word(&self, reg: u32) -> u16 {
        match reg & 0x1e {
            0x00 | 0x02 => self.vdp.peek().0,
            0x04 | 0x06 => (self.prefetch & 0xfc00) | (self.vdp.peek().1 & 0x03ff),
            0x08..=0x0e => self.vdp.hv_counter(),
            _ => self.prefetch,
        }
    }

    fn vdp_peek_byte(&self, reg: u32) -> Option<u8> {
        if reg & 0x1e >= 0x10 {
            return None;
        }
        let w = self.vdp_peek_word(reg);
        Some(if reg & 1 == 0 {
            (w >> 8) as u8
        } else {
            w as u8
        })
    }

    fn vdp_read_word(&mut self, reg: u32) -> u16 {
        if self.vdp_locked() {
            return 0;
        }
        match reg & 0x1e {
            0x00 | 0x02 => self.vdp.read_data(),
            0x04 | 0x06 => (self.prefetch & 0xfc00) | (self.vdp.read_status() & 0x03ff),
            0x08..=0x0e => self.vdp.hv_counter(),
            _ => self.prefetch,
        }
    }

    fn vdp_read_byte(&mut self, reg: u32) -> Option<u8> {
        if reg & 0x1e >= 0x10 {
            return None;
        }
        let w = self.vdp_read_word(reg);
        Some(if reg & 1 == 0 {
            (w >> 8) as u8
        } else {
            w as u8
        })
    }

    fn vdp_write_word(&mut self, reg: u32, v: u16) -> Option<mdvdp::DmaRequest> {
        if self.vdp_locked() && reg & 0x18 != 0x10 {
            return None;
        }
        match reg & 0x1e {
            0x00 | 0x02 => {
                self.vdp.write_data(v);
                None
            }
            0x04 | 0x06 => self.vdp.write_control(v),
            0x10..=0x16 => {
                self.psg.write(v as u8);
                None
            }
            _ => None,
        }
    }

    fn vdp_write_byte(&mut self, reg: u32, v: u8) {
        match reg & 0x1f {
            0x11 | 0x13 | 0x15 | 0x17 => self.psg.write(v),
            0x10..=0x1f => {}
            r => {
                let req = self.vdp_write_word(r, u16::from(v) << 8 | u16::from(v));
                if let Some(req) = req {
                    self.dma(req);
                }
            }
        }
    }

    /// A 68000 → VDP transfer: the words now, the 68000 frozen for their
    /// time.
    fn dma(&mut self, req: mdvdp::DmaRequest) {
        let mut src = req.source;
        let mut n = 0;
        for _ in 0..req.words {
            let w = u16::from(self.peek68_byte(src)) << 8 | u16::from(self.peek68_byte(src | 1));
            if !self.vdp.dma_word(w) {
                break;
            }
            n += 1;
            // The source wraps within its 128 KB window.
            src = (src & 0xfe_0000) | (src.wrapping_add(2) & 0x1_ffff);
        }
        let frozen = self.vdp.dma_cost(n);
        self.vdp.dma_done(req.words, frozen);
        self.pending_wait += (frozen / 7) as u32;
    }

    // ---- The 68000's space ---------------------------------------------

    /// A byte as the 68000 would read it, without side effects.
    /// The open bus at `addr`: the next instruction word's high byte, the
    /// low byte 0 (MacDonald, hardware notes section 1, notes 1 and 4).
    fn open_byte(&self, addr: u32) -> u8 {
        if addr & 1 == 0 {
            (self.prefetch >> 8) as u8
        } else {
            0
        }
    }

    fn peek68_byte(&self, addr: u32) -> u8 {
        let addr = addr & 0xff_ffff;
        match addr {
            0x00_0000..=0x3f_ffff => self.cart_byte(addr),
            0xa0_0000..=0xa0_ffff if self.z80_bus_granted() => self.z80_peek(addr as u16),
            0xa1_0000..=0xa1_001f => self.io_peek(addr),
            0xa1_1100 => self.open_byte(addr) | u8::from(!self.z80_bus_granted()),
            0xc0_0000..=0xdf_ffff if addr & 0xe7_00e0 == 0xc0_0000 => {
                let w = self.vdp_peek_word(addr & 0x1e);
                if addr & 1 == 0 {
                    (w >> 8) as u8
                } else {
                    w as u8
                }
            }
            0xe0_0000..=0xff_ffff => self.ram[(addr & 0xffff) as usize],
            _ => self.open_byte(addr),
        }
    }

    fn io_peek(&self, addr: u32) -> u8 {
        let r = (addr & 0x1f) >> 1;
        match r {
            0 => self.version(),
            1..=3 => {
                let p = &self.io[r as usize - 1];
                p.data & 0x80
                    | (p.data & p.ctrl & 0x7f)
                    | (self.io_inputs[r as usize - 1] & !p.ctrl & 0x7f)
            }
            4..=6 => self.io[r as usize - 4].ctrl,
            7 | 10 | 13 => self.io[((r - 7) / 3) as usize].tx,
            9 | 12 | 15 => self.io[((r - 9) / 3) as usize].serial,
            _ => 0,
        }
    }
}

// The port inputs are sampled by the bus into the board before a read.
impl Board {
    #[allow(clippy::needless_range_loop)]
    fn sample_ports(&mut self, ports: &[Option<Box<dyn PortDevice>>; 2], now: Cycle) {
        for i in 0..3 {
            let lines = ports
                .get(i)
                .and_then(|p| p.as_ref())
                .map_or(0, |d| d.read_lines(now).pulled_low);
            let bit = |p: u32, b: u8| if lines & p != 0 { 0 } else { b };
            self.io_inputs[i] = bit(pin::UP, 0x01)
                | bit(pin::DOWN, 0x02)
                | bit(pin::LEFT, 0x04)
                | bit(pin::RIGHT, 0x08)
                | bit(pin::TR, 0x10)
                | bit(pin::TL, 0x20)
                | 0x40;
        }
    }
}

/// The Z80's bus.
struct Z80Bus<'a> {
    b: &'a mut Board,
    start: u64,
}

impl Bus for Z80Bus<'_> {
    fn read(&mut self, addr: u32, _size: Size) -> u32 {
        if addr >= IO_BASE {
            return 0xff;
        }
        u32::from(self.b.z80_read(addr as u16))
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        if addr >= IO_BASE {
            return 0xff;
        }
        u32::from(self.b.z80_peek(addr as u16))
    }

    fn write(&mut self, addr: u32, _size: Size, v: u32) {
        if addr < IO_BASE {
            self.b.z80_write(addr as u16, v as u8);
        }
    }

    fn addr_bits(&self) -> u8 {
        17
    }
}

impl TimedBus for Z80Bus<'_> {
    fn wait_states(&mut self, _access: AccessZ80, _t: u32) -> u32 {
        0
    }

    fn wait_states_at(&mut self, access: AccessZ80, t: u32, addr: u16) -> u32 {
        // The chips the Z80 can see catch up first.
        if access == AccessZ80::IntAck || (0x4000..0x8000).contains(&addr) {
            self.b.chips_to(self.start + u64::from(t) * 15);
        }
        0
    }
}

/// The 68000's bus for one instruction.
struct MdBus<'a> {
    b: &'a mut Board,
    ports: &'a mut [Option<Box<dyn PortDevice>>; 2],
    /// Master clock the instruction began on.
    start: u64,
    dbg: Option<&'a mut DebugHooks>,
    beam: BeamPos,
    /// The coming read is a program fetch.
    fetch: bool,
}

impl MdBus<'_> {
    fn now(&self) -> u64 {
        self.b.vdp.now.max(self.start)
    }

    fn read_byte(&mut self, addr: u32) -> u8 {
        let addr = addr & 0xff_ffff;
        match addr {
            0xa0_0000..=0xa0_ffff if self.b.z80_bus_granted() => self.b.z80_read(addr as u16),
            0xa1_0000..=0xa1_001f => {
                let now = Cycle(self.now());
                self.b.sample_ports(self.ports, now);
                self.b.io_peek(addr)
            }
            0xc0_0000..=0xdf_ffff if addr & 0xe7_00e0 == 0xc0_0000 => {
                let w = self.b.vdp_read_word(addr & 0x1e);
                if addr & 1 == 0 {
                    (w >> 8) as u8
                } else {
                    w as u8
                }
            }
            _ => self.b.peek68_byte(addr),
        }
    }

    fn read_word(&mut self, addr: u32) -> u16 {
        let addr = addr & 0xff_fffe;
        match addr {
            0xc0_0000..=0xdf_ffff if addr & 0xe7_00e0 == 0xc0_0000 => {
                self.b.vdp_read_word(addr & 0x1f)
            }
            0xa0_0000..=0xa0_ffff if self.b.z80_bus_granted() => {
                let v = u16::from(self.read_byte(addr));
                v << 8 | v
            }
            0xa1_1100 => (self.b.prefetch & 0xff00) | u16::from(!self.b.z80_bus_granted()) << 8,
            0x00_0000..=0x3f_ffff | 0xe0_0000..=0xff_ffff => {
                u16::from(self.b.peek68_byte(addr)) << 8 | u16::from(self.b.peek68_byte(addr | 1))
            }
            _ => u16::from(self.read_byte(addr)) << 8 | u16::from(self.read_byte(addr | 1)),
        }
    }

    fn io_write(&mut self, addr: u32, v: u8) {
        let r = (addr & 0x1f) >> 1;
        match r {
            1..=3 => {
                self.b.io[r as usize - 1].data = v;
                self.drive_th(r as usize - 1);
            }
            4..=6 => {
                self.b.io[r as usize - 4].ctrl = v;
                self.drive_th(r as usize - 4);
            }
            7 | 10 | 13 => self.b.io[((r - 7) / 3) as usize].tx = v,
            9 | 12 | 15 => self.b.io[((r - 9) / 3) as usize].serial = v & 0xf8,
            _ => {}
        }
    }

    /// TH to the port's device: low when it is an output written 0.
    fn drive_th(&mut self, port: usize) {
        let p = self.b.io[port];
        let low = p.ctrl & 0x40 != 0 && p.data & 0x40 == 0;
        let now = Cycle(self.now());
        if let Some(Some(dev)) = self.ports.get_mut(port) {
            dev.write_lines(
                PortLines {
                    pulled_low: if low { pin::TH } else { 0 },
                    analog: [0; 2],
                },
                now,
            );
        }
    }

    fn write_byte(&mut self, addr: u32, v: u8) {
        let addr = addr & 0xff_ffff;
        match addr {
            0x00_0000..=0x3f_ffff => self.b.cart_write(addr, v),
            0xa0_0000..=0xa0_ffff => {
                if self.b.z80_bus_granted() {
                    self.b.z80_write(addr as u16, v);
                }
            }
            0xa1_0000..=0xa1_001f => self.io_write(addr, v),
            0xa1_1100 => self.b.z80_busreq = v & 1 != 0,
            0xa1_1200 => self.z80_reset_line(v & 1 == 0),
            0xa1_30f1 => self.b.sram_ctl = v & 3,
            0xa1_30f3..=0xa1_30ff if addr & 1 == 1 => {
                self.b.banks[((addr - 0xa1_30f1) / 2) as usize] = v & 0x3f;
            }
            0xa1_4000..=0xa1_4003 => self.tmss_write(addr, v),
            0xa1_4101 => self.b.tmss_mapped = self.b.tmss_rom.is_some() && v & 1 == 0,
            0xc0_0000..=0xdf_ffff if addr & 0xe7_00e0 == 0xc0_0000 => {
                self.b.vdp_write_byte(addr & 0x1f, v)
            }
            0xe0_0000..=0xff_ffff => self.b.ram[(addr & 0xffff) as usize] = v,
            _ => {}
        }
    }

    fn tmss_write(&mut self, addr: u32, v: u8) {
        let i = (addr & 3) as usize;
        let mut key = self.b.tmss_key;
        key[i] = v;
        self.b.tmss_key = key;
        if &key == b"SEGA" {
            self.b.tmss_unlocked = true;
        }
    }

    fn z80_reset_line(&mut self, held: bool) {
        if held && !self.b.z80_reset {
            self.b.z80.reset();
            self.b.ym = Ym2612::new();
        }
        self.b.z80_reset = held;
    }

    fn write_word(&mut self, addr: u32, v: u16) {
        let addr = addr & 0xff_fffe;
        match addr {
            0xc0_0000..=0xdf_ffff if addr & 0xe7_00e0 == 0xc0_0000 => {
                if let Some(req) = self.b.vdp_write_word(addr & 0x1f, v) {
                    self.b.dma(req);
                }
            }
            0xa0_0000..=0xa0_ffff => self.write_byte(addr, (v >> 8) as u8),
            0xa1_1100 | 0xa1_1200 => self.write_byte(addr, (v >> 8) as u8),
            0xe0_0000..=0xff_ffff => {
                let a = (addr & 0xffff) as usize;
                self.b.ram[a] = (v >> 8) as u8;
                self.b.ram[a + 1] = v as u8;
            }
            _ => {
                self.write_byte(addr, (v >> 8) as u8);
                self.write_byte(addr | 1, v as u8);
            }
        }
    }
}

impl Bus for MdBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        let v = match size {
            Size::Byte => u32::from(self.read_byte(addr)),
            Size::Word => u32::from(self.read_word(addr)),
            Size::Long => {
                u32::from(self.read_word(addr)) << 16
                    | u32::from(self.read_word(addr.wrapping_add(2)))
            }
        };
        if std::mem::take(&mut self.fetch) {
            self.b.prefetch = v as u16;
        }
        match self.dbg.as_deref_mut() {
            Some(dbg) if dbg.read_armed(CPU_SPACE) => {
                let seen = dbg.on_read(CPU_SPACE, addr & 0xff_ffff, size, v);
                dbg.trace_read(CPU_SPACE, addr & 0xff_ffff, size, seen, self.beam);
                seen
            }
            _ => v,
        }
    }

    fn peek(&self, addr: u32, size: Size) -> u32 {
        peek68(self.b, addr, size)
    }

    fn write(&mut self, addr: u32, size: Size, v: u32) {
        if let Some(dbg) = self.dbg.as_deref_mut() {
            if dbg.write_armed(CPU_SPACE) {
                dbg.trace_write(CPU_SPACE, addr & 0xff_ffff, size, v, self.beam);
                if !dbg.on_write(CPU_SPACE, addr & 0xff_ffff, size, v) {
                    return;
                }
            }
        }
        match size {
            Size::Byte => self.write_byte(addr, v as u8),
            Size::Word => self.write_word(addr, v as u16),
            Size::Long => {
                self.write_word(addr, (v >> 16) as u16);
                self.write_word(addr.wrapping_add(2), v as u16);
            }
        }
    }

    fn addr_bits(&self) -> u8 {
        24
    }
}

impl TimedBus68 for MdBus<'_> {
    fn bus_cycle(&mut self, access: Access68, fc: u8, addr: u32, _size: Size, t: u32) -> u32 {
        self.fetch = access == Access68::Read && fc & 3 == 2;
        let at = self.start + u64::from(t) * 7;
        let addr = addr & 0xff_ffff;
        if access == Access68::IntAck {
            self.b.run_to(at);
            self.b.vdp.acknowledge((addr >> 1 & 7) as u8);
            return 0;
        }
        if (0xa0_0000..0xe0_0000).contains(&addr) {
            self.b.run_to(at);
        }
        let w = std::mem::take(&mut self.b.pending_wait);
        if w > 0 {
            self.b.run_to(at + u64::from(w) * 7);
        }
        w
    }
}

fn peek68(b: &Board, addr: u32, size: Size) -> u32 {
    (0..size.bytes()).fold(0, |v, i| {
        v << 8 | u32::from(b.peek68_byte(addr.wrapping_add(i)))
    })
}

struct PeekBus<'a>(&'a Board);

impl Bus for PeekBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }

    fn peek(&self, addr: u32, size: Size) -> u32 {
        peek68(self.0, addr, size)
    }

    fn write(&mut self, _: u32, _: Size, _: u32) {}

    fn addr_bits(&self) -> u8 {
        24
    }
}

struct Z80PeekBus<'a>(&'a Board);

impl Bus for Z80PeekBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        if addr >= IO_BASE {
            0xff
        } else {
            u32::from(self.0.z80_peek(addr as u16))
        }
    }

    fn write(&mut self, _: u32, _: Size, _: u32) {}

    fn addr_bits(&self) -> u8 {
        17
    }
}

pub struct MegaDrive {
    standard: VideoStandard,
    cpu: M68000,
    board: Board,
    ports: [Option<Box<dyn PortDevice>>; 2],
    cart: MdSlot,
    sched: Scheduler,
    input: InputQueue,
    field_done_pending: bool,
    overlay_ids: Vec<&'static str>,
    overlay_fresh: bool,
    debug_dev: DeviceId,
}

impl MegaDrive {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        let opt = |k: &str| config.options.get(k).map(String::as_str);
        if let Some(m) = config.model.as_deref() {
            if m != "megadrive" {
                return Err(ConfigError::UnknownModel(m.into()));
            }
        }
        let region = match opt("region") {
            None => {
                if config.video_format == Some(VideoStandard::Pal) {
                    Region::Europe
                } else {
                    Region::Usa
                }
            }
            Some("japan") => Region::Japan,
            Some("usa") => Region::Usa,
            Some("europe") => Region::Europe,
            Some(o) => {
                return Err(ConfigError::Invalid(format!(
                    "region option '{o}' (japan, usa or europe)"
                )))
            }
        };
        let standard = config.video_format.unwrap_or(if region == Region::Europe {
            VideoStandard::Pal
        } else {
            VideoStandard::Ntsc
        });
        let pal = standard == VideoStandard::Pal;
        let tmss = match opt("tmss") {
            None | Some("off") => false,
            Some("on") => true,
            Some(o) => {
                return Err(ConfigError::Invalid(format!(
                    "tmss option '{o}' (on or off)"
                )))
            }
        };
        let save_option = match opt("save") {
            None | Some("auto") => None,
            Some("sram") => Some(Save::Sram),
            Some("eeprom") => Some(Save::Eeprom),
            Some("none") => Some(Save::None),
            Some(o) => {
                return Err(ConfigError::Invalid(format!(
                    "save option '{o}' (auto, sram, eeprom or none)"
                )))
            }
        };
        let port = |key: &str, default: &str| -> Result<Option<Box<dyn PortDevice>>, ConfigError> {
            match opt(key).unwrap_or(default) {
                "pad3" => Ok(Some(Box::new(MdPad::three_button()))),
                "pad6" => Ok(Some(Box::new(MdPad::six_button()))),
                "none" => Ok(None),
                o => Err(ConfigError::Invalid(format!(
                    "{key} option '{o}' (pad3, pad6 or none)"
                ))),
            }
        };
        let ports = [port("port1", "pad3")?, port("port2", "pad3")?];
        let mclk = if pal { MCLK_PAL } else { MCLK_NTSC };
        let rate = mclk / 7 / u32::from(chips_yamaha::ym2612::CLOCKS_PER_SAMPLE);
        let decimator = || {
            Decimator::new(rate, mimic_core::SAMPLE_RATE)
                .ok_or_else(|| ConfigError::Invalid("audio rate".into()))
        };
        let tmss_rom = config
            .roms
            .get("md-tmss")
            .cloned()
            .filter(|r| !r.is_empty());
        let mut m = Self {
            standard,
            cpu: M68000::new(),
            board: Board {
                pal,
                region,
                tmss,
                tmss_unlocked: false,
                tmss_key: [0; 4],
                tmss_rom: if tmss { tmss_rom } else { None },
                tmss_mapped: false,
                ram: vec![0; 0x10000],
                cart: None,
                banks: [0, 1, 2, 3, 4, 5, 6, 7],
                save: Save::None,
                sram: Vec::new(),
                sram_ctl: 0,
                sram_dirty: false,
                eeprom: X24c01::new(),
                vdp: MdVdp::new(pal),
                psg: Sn76489::sega(),
                ym: Ym2612::new(),
                z80: Z80::new(),
                z80_ram: vec![0; 0x2000],
                z80_bank: 0,
                z80_reset: true,
                z80_busreq: false,
                z80_t: 0,
                ym_t: 0,
                psg_t: 0,
                ym_clocks: 0,
                io: [IoPort {
                    data: 0x7f,
                    ctrl: 0,
                    tx: 0xff,
                    serial: 0,
                }; 3],
                io_inputs: [0x7f; 3],
                pending_wait: 0,
                prefetch: 0,
                field_done: false,
                audio: [decimator()?, decimator()?],
                dc: [[0; 2]; 2],
                samples: Vec::new(),
            },
            ports,
            cart: MdSlot {
                inner: devices_cartridge::megadrive::slot(),
                battery: BatterySlot::default(),
                save_option,
            },
            sched: Scheduler::new(),
            input: InputQueue::new(),
            field_done_pending: false,
            overlay_ids: Vec::new(),
            overlay_fresh: false,
            debug_dev: CPU,
        };
        match opt("cart") {
            None => {}
            Some("demo") => {
                let media = Media {
                    name: "demo".into(),
                    kind: MediaKind::Cartridge,
                    backend: Box::new(MemoryImage(demo::rom())),
                };
                m.cart
                    .insert(media)
                    .map_err(|e| ConfigError::Invalid(format!("demo cartridge: {e}")))?;
            }
            Some(o) => return Err(ConfigError::Invalid(format!("cart option '{o}' (demo)"))),
        }
        m.cart.inner.take_changed();
        m.power_up();
        Ok(m)
    }

    /// Power on: the cartridge (re)read, every chip fresh, the 68000 reset.
    fn power_up(&mut self) {
        let pal = self.standard == VideoStandard::Pal;
        let b = &mut self.board;
        b.cart = self.cart.inner.cart().cloned();
        b.save = save_kind(self.cart.save_option, b.cart.as_ref());
        let len = backup_len(b.save, b.cart.as_ref());
        // The backup memory: what the battery keeps, else erased.
        b.sram = vec![0xff; if b.save == Save::Sram { len } else { 0 }];
        b.eeprom = X24c01::new();
        self.cart.battery.loaded = false;
        if let Some(c) = &self.cart.battery.contents {
            match b.save {
                Save::Sram if c.len() == b.sram.len() => b.sram.copy_from_slice(c),
                Save::Eeprom if c.len() == eeprom::SIZE => b.eeprom.mem.copy_from_slice(c),
                _ => {}
            }
        }
        b.ram.fill(0);
        b.z80_ram.fill(0);
        b.banks = [0, 1, 2, 3, 4, 5, 6, 7];
        b.sram_ctl = 0;
        b.vdp = MdVdp::new(pal);
        b.vdp.set_overlays(&self.overlay_ids);
        b.psg = Sn76489::sega();
        b.ym = Ym2612::new();
        b.z80 = Z80::new();
        b.z80.reset();
        b.z80_bank = 0;
        b.z80_reset = true;
        b.z80_busreq = false;
        let now = self.sched.now().0;
        b.vdp.now = now;
        (b.z80_t, b.ym_t, b.psg_t) = (now, now, now);
        b.ym_clocks = 0;
        b.tmss_unlocked = false;
        b.tmss_key = [0; 4];
        b.tmss_mapped = b.tmss_rom.is_some();
        b.pending_wait = 0;
        b.io.fill(IoPort {
            data: 0x7f,
            ctrl: 0,
            tx: 0xff,
            serial: 0,
        });
        self.cpu = M68000::new();
        let start = now;
        let beam = self.beam_pos();
        let mut bus = MdBus {
            b: &mut self.board,
            ports: &mut self.ports,
            start,
            dbg: None,
            beam,
            fetch: false,
        };
        self.cpu.reset_timed(&mut bus);
    }

    fn beam_pos(&self) -> BeamPos {
        let v = &self.board.vdp;
        BeamPos {
            frame: v.fields,
            line: v.line,
            cycle: (v.pos / 7) as u16,
            hpos: (v.pos / if v.h40() { 8 } else { 10 }) as u16,
        }
    }

    /// The battery file in, or the game's writes out.
    fn sync_battery(&mut self) {
        let battery = &mut self.cart.battery;
        if std::mem::take(&mut battery.loaded) {
            if let Some(file) = &battery.contents {
                let b = &mut self.board;
                match b.save {
                    Save::Eeprom if file.len() == eeprom::SIZE => {
                        b.eeprom.mem.copy_from_slice(file)
                    }
                    Save::Sram if file.len() == b.sram.len() => b.sram.copy_from_slice(file),
                    _ => {}
                }
            }
        }
        let b = &mut self.board;
        if std::mem::take(&mut b.sram_dirty) {
            battery.update(&b.sram);
        }
        if std::mem::take(&mut b.eeprom.dirty) {
            battery.update(&b.eeprom.mem);
        }
    }

    fn instruction(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        if self.cart.inner.take_changed() {
            self.reset(ResetKind::Hard);
        }
        while let Some(ev) = self.input.pop_due(self.sched.now()) {
            self.apply_input(ev);
        }
        let start = self.sched.now().0;
        let beam = self.beam_pos();
        let step = {
            let mut bus = MdBus {
                b: &mut self.board,
                ports: &mut self.ports,
                start,
                dbg: dbg.as_deref_mut(),
                beam,
                fetch: false,
            };
            self.cpu.step_timed(&mut bus)
        };
        // A halted 68000 (a double bus fault: no cartridge reads $FF
        // vectors) stays halted while the rest of the machine runs on.
        let clocks = match step {
            Step::Done { clocks } | Step::Stopped { clocks } => clocks.max(1),
            Step::Halted => 4,
        };
        let mut end = start + u64::from(clocks) * 7;
        // A DMA started by the instruction's last write freezes the 68000.
        let w = std::mem::take(&mut self.board.pending_wait);
        end += u64::from(w) * 7;
        self.board.run_to(end);
        self.sched.advance(end - start);
        self.cpu.set_ipl(self.board.vdp.ipl());
        let entered = self.cpu.take_interrupt();
        let field = std::mem::take(&mut self.board.field_done);
        if field {
            if self.board.vdp.overlays_on() {
                self.overlay_fresh = true;
            }
            self.sync_battery();
        }
        if let Some(dbg) = dbg {
            if let Some(kind) = entered {
                if dbg.interrupt_armed(kind) {
                    dbg.check_interrupt(kind);
                }
            }
            let next_pc = self.cpu.pc();
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
            InputTarget::Keyboard => {}
            InputTarget::Panel => {
                if let InputEvent::Control { id, value } = ev.event {
                    match id.0 {
                        CONTROL_POWER if value == 1 => self.reset(ResetKind::Hard),
                        CONTROL_RESET if value == 1 => self.reset(ResetKind::Soft),
                        _ => {}
                    }
                }
            }
        }
    }

    fn signal(&self) -> SignalInfo {
        let (w, _, _) = self.board.vdp.frame();
        let mclk = if self.board.pal { MCLK_PAL } else { MCLK_NTSC };
        let h40 = w == mdvdp::H40_WIDTH;
        SignalInfo {
            pixel_clock_hz: mclk / if h40 { 8 } else { 10 },
            active_pixels: w,
            total_pixels: mdvdp::LINE_MCLK / if h40 { 8 } else { 10 },
            lines_per_field: u32::from(self.board.vdp.lines()),
            standard: self.standard,
            output: OutputType::Composite,
            field: FieldParity::Progressive,
        }
    }
}

/// The 68000's view of a byte (tests and tools).
pub fn peek_cpu(m: &MegaDrive, addr: u32) -> u8 {
    m.board.peek68_byte(addr)
}

/// The Z80's view of a byte.
pub fn peek_z80(m: &MegaDrive, addr: u16) -> u8 {
    m.board.z80_peek(addr)
}

impl Machine for MegaDrive {
    fn run_frame(&mut self, dbg: &mut DebugHooks) -> StopReason {
        if std::mem::take(&mut self.field_done_pending) {
            return StopReason::FrameDone;
        }
        self.overlay_fresh = false;
        if self.board.vdp.line == 0 {
            for f in dbg.per_frame_freezes() {
                if f.space == CPU_SPACE {
                    for i in 0..f.size.bytes() {
                        let a = (f.addr + i) & 0xff_ffff;
                        if a >= 0xe0_0000 {
                            self.board.ram[(a & 0xffff) as usize] =
                                (f.value >> (8 * (f.size.bytes() - 1 - i))) as u8;
                        }
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
        match self.instruction(None) {
            Some(f @ StopReason::Fault { .. }) => f,
            _ => StopReason::StepDone,
        }
    }

    /// Hard: power cycle. Soft: the RESET button — the 68000 and the VDP
    /// restart, the RAMs and the Z80 stay.
    fn reset(&mut self, kind: ResetKind) {
        if kind == ResetKind::Hard {
            self.power_up();
        } else {
            self.board.vdp.reset();
            self.board.pending_wait = 0;
            self.cpu = M68000::new();
            let start = self.sched.now().0;
            let beam = self.beam_pos();
            let mut bus = MdBus {
                b: &mut self.board,
                ports: &mut self.ports,
                start,
                dbg: None,
                beam,
                fetch: false,
            };
            self.cpu.reset_timed(&mut bus);
        }
        self.input.clear();
        self.field_done_pending = false;
    }

    fn framebuffer(&self) -> FrameRef<'_> {
        let (width, height, px) = self.board.vdp.frame();
        FrameRef {
            width,
            height,
            pixels: PixelData::Rgb(px),
            luma_chroma: None,
            signal: self.signal(),
        }
    }

    fn gfx_views(&self) -> Vec<GfxViewInfo> {
        mdvdp::gfx::views()
    }

    fn gfx_view(&self, id: &str, opts: &GfxOptions) -> Option<GfxView> {
        self.board.vdp.gfx_view(id, opts, VRAM_SPACE, CRAM_SPACE)
    }

    fn overlays(&self) -> Vec<OverlayInfo> {
        mdvdp::overlay_infos()
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

    fn frame_row(&self, line: u16) -> Option<u32> {
        self.board.vdp.line_row(line)
    }

    fn audio(&mut self, out: &mut Vec<i16>) -> mimic_core::AudioFormat {
        out.append(&mut self.board.samples);
        mimic_core::AudioFormat {
            sample_rate: mimic_core::SAMPLE_RATE,
            channels: 2,
        }
    }

    fn beam(&self) -> Option<BeamPos> {
        Some(self.beam_pos())
    }

    fn memory_spaces(&self) -> Vec<MemSpace> {
        let s = |id, name, device, addr_bits, big_endian| MemSpace {
            id,
            name,
            device,
            addr_bits,
            big_endian,
        };
        vec![
            s(CPU_SPACE, "cpu", CPU, 24, true),
            s(VRAM_SPACE, "vram", CPU, 16, true),
            s(CRAM_SPACE, "cram", CPU, 7, true),
            s(VSRAM_SPACE, "vsram", CPU, 7, true),
            s(Z80_SPACE, "z80", Z80_DEV, 16, false),
        ]
    }

    fn chips(&self) -> Vec<ChipInfo> {
        let v = &self.board.vdp;
        let b = &self.board;
        let reg = |name, value: u32, bits| ChipReg {
            name,
            addr: None,
            value,
            bits,
        };
        const R: [&str; 24] = [
            "R0", "R1", "R2", "R3", "R4", "R5", "R6", "R7", "R8", "R9", "R10", "R11", "R12", "R13",
            "R14", "R15", "R16", "R17", "R18", "R19", "R20", "R21", "R22", "R23",
        ];
        let mut vdp_regs: Vec<ChipReg> = R
            .iter()
            .zip(v.regs)
            .map(|(n, val)| reg(*n, u32::from(val), 8))
            .collect();
        vdp_regs.extend([
            reg("STATUS", u32::from(v.status()), 16),
            reg("ADDR", u32::from(v.addr), 16),
            reg("CODE", u32::from(v.code), 6),
            reg("LINE", u32::from(v.line), 9),
            reg("HV", u32::from(v.hv_counter()), 16),
        ]);
        let p = &b.psg;
        let psg = vec![
            reg("TONE0", u32::from(p.period[0]), 10),
            reg("TONE1", u32::from(p.period[1]), 10),
            reg("TONE2", u32::from(p.period[2]), 10),
            reg("NOISE", u32::from(p.noise), 3),
            reg("ATT0", u32::from(p.atten[0]), 4),
            reg("ATT1", u32::from(p.atten[1]), 4),
            reg("ATT2", u32::from(p.atten[2]), 4),
            reg("ATT3", u32::from(p.atten[3]), 4),
        ];
        let y = &b.ym;
        let mut ym = vec![
            reg("LFO", u32::from(y.regs[0x22]), 8),
            reg("TIMERS", u32::from(y.regs[0x27]), 8),
            reg("DAC", u32::from(y.regs[0x2a]), 8),
            reg("DACEN", u32::from(y.regs[0x2b]), 8),
            reg("STATUS", u32::from(y.read_status()), 8),
        ];
        const CH: [&str; 6] = ["FREQ1", "FREQ2", "FREQ3", "FREQ4", "FREQ5", "FREQ6"];
        for (i, name) in CH.iter().enumerate() {
            let part = (i / 3) * 0x100;
            let c = i % 3;
            let f = u32::from(y.regs[part + 0xa4 + c]) << 8 | u32::from(y.regs[part + 0xa0 + c]);
            ym.push(reg(name, f, 14));
        }
        let sys = vec![
            reg("VERSION", u32::from(b.version()), 8),
            reg("Z80BUSREQ", u32::from(b.z80_busreq), 1),
            reg("Z80RESET", u32::from(b.z80_reset), 1),
            reg("Z80BANK", u32::from(b.z80_bank) << 15, 24),
            reg("PAD1", u32::from(b.io[0].data), 8),
            reg("CTRL1", u32::from(b.io[0].ctrl), 8),
            reg("PAD2", u32::from(b.io[1].data), 8),
            reg("CTRL2", u32::from(b.io[1].ctrl), 8),
            reg("SRAMCTL", u32::from(b.sram_ctl), 2),
        ];
        vec![
            ChipInfo {
                name: "vdp",
                regs: vdp_regs,
            },
            ChipInfo {
                name: "ym2612",
                regs: ym,
            },
            ChipInfo {
                name: "psg",
                regs: psg,
            },
            ChipInfo {
                name: "system",
                regs: sys,
            },
        ]
    }

    fn devices(&self) -> Vec<DeviceId> {
        vec![CPU, Z80_DEV]
    }

    fn debug_target(&mut self, dev: DeviceId) -> Option<&mut dyn DebugTarget> {
        if dev == CPU || dev == Z80_DEV {
            self.debug_dev = dev;
            return Some(self as &mut dyn DebugTarget);
        }
        None
    }

    fn input(&mut self, ev: TimedInput) {
        self.input.push(ev);
    }

    fn now(&self) -> Cycle {
        self.sched.now()
    }

    fn controls(&self) -> Vec<mimic_core::ControlInfo> {
        vec![
            mimic_core::ControlInfo::button(CONTROL_POWER, "POWER", false),
            mimic_core::ControlInfo::button(CONTROL_RESET, "RESET", false),
        ]
    }

    fn ports(&self) -> Vec<PortInfo> {
        [(PORT1, "control 1"), (PORT2, "control 2")]
            .into_iter()
            .map(|(id, name)| PortInfo {
                id,
                kind: PortKind::MegaDrivePad,
                name,
                connected: self.ports[usize::from(id.0)].is_some(),
            })
            .collect()
    }

    fn connect(&mut self, port: PortId, dev: Box<dyn PortDevice>) -> Result<(), PortError> {
        let Some(slot) = self.ports.get_mut(usize::from(port.0)) else {
            return Err(PortError::NoSuchPort(port));
        };
        if dev.kind() != PortKind::MegaDrivePad {
            return Err(PortError::Incompatible {
                port: PortKind::MegaDrivePad,
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
        vec![
            MediaSlotInfo {
                id: SLOT_CARTRIDGE,
                name: "cartridge",
                kind: MediaKind::Cartridge,
                inserted: self.cart.inner.is_inserted(),
            },
            MediaSlotInfo {
                id: SLOT_BATTERY,
                name: "backup RAM",
                kind: MediaKind::BatteryRam,
                inserted: self.cart.battery.is_inserted(),
            },
        ]
    }

    fn media_slot(&mut self, slot: SlotId) -> Option<&mut dyn MediaSlot> {
        match slot {
            SLOT_CARTRIDGE => Some(&mut self.cart as &mut dyn MediaSlot),
            SLOT_BATTERY => Some(&mut self.cart.battery as &mut dyn MediaSlot),
            _ => None,
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        let b = &self.board;
        w.chunk(*b"MDBD", 1, |w| {
            w.bool(self.field_done_pending);
            w.bool(b.tmss_unlocked);
            w.raw(&b.tmss_key);
            w.bool(b.tmss_mapped);
            w.bytes(&b.ram);
            w.bool(b.cart.is_some());
            w.raw(&b.banks);
            w.bytes(&b.sram);
            w.u8(b.sram_ctl);
            w.bytes(&b.z80_ram);
            w.u16(b.z80_bank);
            w.bool(b.z80_reset);
            w.bool(b.z80_busreq);
            w.u64(b.z80_t);
            w.u64(b.ym_t);
            w.u64(b.psg_t);
            w.u8(b.ym_clocks);
            for p in &b.io {
                w.raw(&[p.data, p.ctrl, p.tx, p.serial]);
            }
            w.raw(&b.io_inputs);
            w.u32(b.pending_wait);
            w.u16(b.prefetch);
            w.bool(b.field_done);
            for a in &b.audio {
                a.save(w);
            }
            for d in b.dc {
                w.u32(d[0] as u32);
                w.u32(d[1] as u32);
            }
        });
        self.cpu.save(&mut w);
        b.z80.save(&mut w);
        b.vdp.save(&mut w);
        b.psg.save(&mut w);
        b.ym.save(&mut w);
        b.eeprom.save(&mut w);
        self.sched.save(&mut w);
        self.input.save(&mut w);
        for (i, tag) in [*b"MDP1", *b"MDP2"].into_iter().enumerate() {
            w.chunk(tag, 1, |w| {
                w.bytes(
                    &self.ports[i]
                        .as_ref()
                        .map(|d| d.save_state())
                        .unwrap_or_default(),
                );
            });
        }
        w.into_bytes()
    }

    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"MDBD", 1)?;
        let field_done_pending = c.bool()?;
        let mut b = self.board.clone();
        b.tmss_unlocked = c.bool()?;
        c.raw_into(&mut b.tmss_key)?;
        b.tmss_mapped = c.bool()? && b.tmss_rom.is_some();
        let ram = c.bytes()?;
        if ram.len() != b.ram.len() {
            return Err(StateError::Invalid("RAM size"));
        }
        b.ram.copy_from_slice(ram);
        if c.bool()? != b.cart.is_some() {
            return Err(StateError::Invalid("cartridge differs"));
        }
        c.raw_into(&mut b.banks)?;
        for x in &mut b.banks {
            *x &= 0x3f;
        }
        let sram = c.bytes()?;
        if sram.len() != b.sram.len() {
            return Err(StateError::Invalid("backup RAM size"));
        }
        b.sram.copy_from_slice(sram);
        b.sram_ctl = c.u8()? & 3;
        let zr = c.bytes()?;
        if zr.len() != b.z80_ram.len() {
            return Err(StateError::Invalid("Z80 RAM size"));
        }
        b.z80_ram.copy_from_slice(zr);
        b.z80_bank = c.u16()? & 0x1ff;
        b.z80_reset = c.bool()?;
        b.z80_busreq = c.bool()?;
        b.z80_t = c.u64()?;
        b.ym_t = c.u64()?;
        b.psg_t = c.u64()?;
        b.ym_clocks = c.u8()? % chips_yamaha::ym2612::CLOCKS_PER_SAMPLE;
        for p in &mut b.io {
            let mut x = [0u8; 4];
            c.raw_into(&mut x)?;
            *p = IoPort {
                data: x[0],
                ctrl: x[1],
                tx: x[2],
                serial: x[3],
            };
        }
        c.raw_into(&mut b.io_inputs)?;
        b.pending_wait = c.u32()?;
        b.prefetch = c.u16()?;
        b.field_done = c.bool()?;
        for a in &mut b.audio {
            a.load(&mut c)?;
        }
        for d in &mut b.dc {
            *d = [c.u32()? as i32, c.u32()? as i32];
        }
        b.samples.clear();
        let mut cpu = self.cpu.clone();
        let mut sched = self.sched.clone();
        let mut input = self.input.clone();
        cpu.load(&mut r)?;
        b.z80.load(&mut r)?;
        b.vdp.load(&mut r)?;
        b.psg.load(&mut r)?;
        b.ym.load(&mut r)?;
        b.eeprom.load(&mut r)?;
        sched.load(&mut r)?;
        input.load(&mut r)?;
        let mut port_states = Vec::new();
        for tag in [*b"MDP1", *b"MDP2"] {
            let (_, mut c) = r.chunk(tag, 1)?;
            port_states.push(c.bytes()?.to_vec());
        }
        for (dev, st) in self.ports.iter().zip(&port_states) {
            if dev.is_none() != st.is_empty() {
                return Err(StateError::Invalid("pads differ"));
            }
        }
        for (dev, st) in self.ports.iter_mut().zip(&port_states) {
            if let Some(d) = dev {
                d.load_state(st)?;
            }
        }
        self.cpu = cpu;
        self.board = b;
        self.sched = sched;
        self.input = input;
        self.field_done_pending = field_done_pending;
        Ok(())
    }
}

impl DebugTarget for MegaDrive {
    fn regs(&self) -> RegisterSet {
        if self.debug_dev == Z80_DEV {
            self.board.z80.regs()
        } else {
            self.cpu.regs()
        }
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        if self.debug_dev == Z80_DEV {
            self.board.z80.set_reg(name, v);
        } else {
            self.cpu.set_reg(name, v);
            if name.eq_ignore_ascii_case("pc") {
                self.cpu.refill_queue(&PeekBus(&self.board));
            }
        }
    }

    fn peek(&self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
        let v = &self.board.vdp;
        let byte = |a: u32| -> Option<u8> {
            Some(match space {
                CPU_SPACE => self.board.peek68_byte(a),
                VRAM_SPACE => v.vram[(a & 0xffff) as usize],
                CRAM_SPACE => {
                    let w = v.cram[(a >> 1 & 63) as usize];
                    if a & 1 == 0 {
                        (w >> 8) as u8
                    } else {
                        w as u8
                    }
                }
                VSRAM_SPACE => {
                    let w = v.vsram.get((a >> 1 & 63) as usize).copied().unwrap_or(0);
                    if a & 1 == 0 {
                        (w >> 8) as u8
                    } else {
                        w as u8
                    }
                }
                Z80_SPACE => self.board.z80_peek(a as u16),
                _ => return None,
            })
        };
        let n = size.bytes();
        if space == Z80_SPACE {
            return (0..n).try_fold(0, |acc, i| {
                Some(acc | u32::from(byte(addr.wrapping_add(i))?) << (8 * i))
            });
        }
        (0..n).try_fold(0, |acc, i| {
            Some(acc << 8 | u32::from(byte(addr.wrapping_add(i))?))
        })
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, value: u32) -> bool {
        let n = size.bytes();
        for i in 0..n {
            let a = addr.wrapping_add(i);
            let byte = if space == Z80_SPACE {
                (value >> (8 * i)) as u8
            } else {
                (value >> (8 * (n - 1 - i))) as u8
            };
            let b = &mut self.board;
            match space {
                CPU_SPACE if (a & 0xff_ffff) >= 0xe0_0000 => b.ram[(a & 0xffff) as usize] = byte,
                VRAM_SPACE => b.vdp.vram[(a & 0xffff) as usize] = byte,
                Z80_SPACE if (a & 0xffff) < 0x4000 => b.z80_ram[(a & 0x1fff) as usize] = byte,
                _ => return false,
            }
        }
        true
    }

    fn disassemble(&self, addr: u32) -> Instr {
        if self.debug_dev == Z80_DEV {
            self.board.z80.disassemble(&Z80PeekBus(&self.board), addr)
        } else {
            self.cpu.disassemble(&PeekBus(&self.board), addr)
        }
    }
}

/// A ROM image in memory (the demo cartridge).
struct MemoryImage(Vec<u8>);

impl mimic_core::media::MediaBackend for MemoryImage {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }

    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), mimic_core::media::MediaError> {
        let o = offset as usize;
        buf.copy_from_slice(&self.0[o..o + buf.len()]);
        Ok(())
    }

    fn write(&mut self, _offset: u64, _data: &[u8]) -> Result<(), mimic_core::media::MediaError> {
        Err(mimic_core::media::MediaError::WriteProtected)
    }

    fn commit(&mut self) -> Result<(), mimic_core::media::MediaError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
