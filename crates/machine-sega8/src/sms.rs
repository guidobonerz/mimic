//! The Mark III, Master System, Master System 2 and Game Gear (models
//! "mark3", "sms", "sms2", "gg" of `sega8`).
//!
//! Z80A at 3.579545 MHz (PAL SMS: 3.546893 MHz), the VDP with its SN76489
//! ([`chips_sega::SmsVdp`], [`chips_ti::Sn76489::sega`]), 8 KB work RAM, the
//! cartridge through a mapper, two joystick ports; the optional BIOS.
//!
//! **Memory.** $0000–$BFFF the slot area: the BIOS or the cartridge, as
//! the memory control register (port $3E) enables them; $C000–$FFFF the
//! work RAM, repeated (bit 4 of $3E switches it off). The **Sega mapper**
//! ($FFFC–$FFFF, also written into RAM): $FFFD, $FFFE, $FFFF the 16 KB
//! banks at $0000 (the first 1 KB fixed), $4000, $8000; $FFFC bit 3 puts
//! the cartridge RAM at $8000 (bit 2 its second 16 KB). The **Codemasters
//! mapper** (option `mapper=codemasters`): writes to $0000, $4000, $8000
//! select the three banks. Without a BIOS the machine starts as the BIOS
//! leaves it: $3E = $AB, also at $C000.
//!
//! **I/O** (A7, A6, A0): $00–$3F writes go to the memory control (even)
//! and the I/O control (odd; TH/TR directions and levels), reads give $FF
//! (on the Game Gear $00–$06 are its own registers: START and the region
//! at $00, the link registers $01–$05, the stereo mask $06); $40–$7F
//! writes the PSG, reads the V counter (even) and the H counter (odd);
//! $80–$BF the VDP's data and control; $C0–$FF reads the pads: $DC port A
//! (pad 1 up, down, left, right, TL, TR, pad 2 up, down), $DD port B (pad
//! 2 left, right, TL, TR, the RESET button, 1, the TH lines). With the I/O
//! chip switched off ($3E bit 2) they read $FF. An export machine reads a
//! TH line set as an output back at its level, a Japanese one as 0
//! (option `region`).
//!
//! **FM sound** (option `fm=on|off`; built into the Japanese SMS — model
//! "sms" with `region=japan`, on by default there —, the FM Sound Unit on
//! the Mark III): a YM2413 ([`chips_yamaha::Ym2413`]) at ports $F0
//! (address) and $F1 (data); port $F2 the audio control: bit 0 FM on,
//! bit 1 with bit 0 the PSG (Japanese SMS: $00 PSG, $01 FM, $02 none, $03
//! both; the Mark III unit forces bit 1 to 0, PSG and FM exclude each
//! other). $F2 reads back its two bits while the I/O chip is off ($3E bit
//! 2) — the games' detection (SMS Power, "Audio control port").
//!
//! **Interrupts.** The VDP's /INT drives the Z80's /INT (mode 1); PAUSE the
//! /NMI. The Game Gear has START instead (port $00 bit 7).
//!
//! Sources: Charles MacDonald, *SMS/GG hardware notes* and *Sega Master
//! System VDP documentation*; SMS Power, "Mappers", "SN76489",
//! "RegionDetection" (the Game Gear's port $00). The implementation is
//! this project's own.

use chips_sega::vdp::{self, SmsVdp, VdpKind};
use chips_ti::Sn76489;
use chips_yamaha::Ym2413;
use cpu_z80::{Access, TimedBus, IO_BASE, Z80};
use devices_cartridge::sega::SmsCart;
use devices_cartridge::CartSlot;
use devices_input::Joystick;
use mimic_core::audio::Decimator;
use mimic_core::frame::{FieldParity, FrameRef, OutputType, PixelData, SignalInfo, VideoStandard};
use mimic_core::input::{InputEvent, InputQueue, InputTarget, TimedInput};
use mimic_core::machine::ChipReg;
use mimic_core::media::{Media, MediaKind, MediaSlot, MediaSlotInfo, SlotId};
use mimic_core::overlay::{Overlay, OverlayInfo};
use mimic_core::port::{PortDevice, PortError, PortId, PortInfo, PortKind};
use mimic_core::registry::{ConfigError, MachineConfig};
use mimic_core::{
    BeamPos, Bus, ChipInfo, Cpu, Cycle, DebugHooks, DebugTarget, Instr, Machine, MemSpace,
    MemSpaceId, RegisterSet, ResetKind, Savestate, Scheduler, Size, StateError, StateReader,
    StateWriter, StopReason,
};

use crate::{demo, pad_row, MemoryImage, CPU, CPU_SPACE, PORT1, PORT2, SLOT_CARTRIDGE, VRAM_SPACE};

pub const CPU_HZ_PAL: u32 = 3_546_893;

/// Panel controls.
const CONTROL_POWER: u16 = 0;
/// PAUSE (the /NMI) or, on the Game Gear, START.
const CONTROL_PAUSE: u16 = 1;
/// The SMS's RESET button (port $DD bit 4).
const CONTROL_RESET: u16 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SmsModel {
    Mark3,
    Sms,
    Sms2,
    GameGear,
}

impl SmsModel {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "mark3" => SmsModel::Mark3,
            "sms" => SmsModel::Sms,
            "sms2" => SmsModel::Sms2,
            "gg" => SmsModel::GameGear,
            _ => return None,
        })
    }

    fn vdp(self) -> VdpKind {
        match self {
            SmsModel::Mark3 | SmsModel::Sms => VdpKind::Sms1,
            SmsModel::Sms2 => VdpKind::Sms2,
            SmsModel::GameGear => VdpKind::GameGear,
        }
    }

    fn rom_role(self) -> &'static str {
        if self == SmsModel::GameGear {
            "gg-bios"
        } else {
            "sms-bios"
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapperKind {
    Sega,
    Codemasters,
}

/// The cartridge's mapper registers: Sega: $FFFC–$FFFF; Codemasters: the
/// banks of the three slots in 1–3.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Mapper {
    kind: MapperKind,
    regs: [u8; 4],
}

impl Mapper {
    fn new(kind: MapperKind) -> Self {
        let regs = match kind {
            MapperKind::Sega => [0, 0, 1, 2],
            MapperKind::Codemasters => [0, 0, 1, 0],
        };
        Mapper { kind, regs }
    }

    /// The ROM offset of slot-area address `addr`.
    fn rom_offset(&self, addr: u16) -> usize {
        let a = usize::from(addr);
        let bank = |r: u8| usize::from(r) * 0x4000 + (a & 0x3fff);
        match (self.kind, addr) {
            (MapperKind::Sega, 0x0000..=0x03ff) => a,
            (_, 0x0000..=0x3fff) => bank(self.regs[1]),
            (_, 0x4000..=0x7fff) => bank(self.regs[2]),
            _ => bank(self.regs[3]),
        }
    }

    /// The cartridge RAM's offset for $8000–$BFFF, when it is mapped there.
    fn ram_offset(&self, addr: u16) -> Option<usize> {
        (self.kind == MapperKind::Sega
            && self.regs[0] & 0x08 != 0
            && (0x8000..0xc000).contains(&addr))
        .then(|| usize::from(self.regs[0] >> 2 & 1) * 0x4000 + usize::from(addr - 0x8000))
    }

    fn write(&mut self, addr: u16, v: u8) {
        match (self.kind, addr) {
            (MapperKind::Sega, 0xfffc..=0xffff) => self.regs[usize::from(addr - 0xfffc)] = v,
            (MapperKind::Codemasters, 0x0000) => self.regs[1] = v,
            (MapperKind::Codemasters, 0x4000) => self.regs[2] = v,
            (MapperKind::Codemasters, 0x8000) => self.regs[3] = v,
            _ => {}
        }
    }
}

/// Everything on the bus except the CPU and the port devices.
#[derive(Clone, Debug)]
struct Board {
    model: SmsModel,
    ram: Vec<u8>,
    bios: Option<Vec<u8>>,
    cart: Option<SmsCart>,
    /// 32 KB cartridge RAM (Sega mapper).
    cart_ram: Vec<u8>,
    mapper: Mapper,
    vdp: SmsVdp,
    psg: Sn76489,
    /// The YM2413 when there is one.
    fm: Option<Ym2413>,
    /// Port $F2's two bits.
    fm_ctl: u8,
    /// The Japanese SMS's own YM2413 (else the Mark III's unit).
    fm_builtin: bool,
    /// Ports $3E and $3F.
    memctl: u8,
    ioctl: u8,
    /// The Game Gear's ports $00–$06.
    gg: [u8; 7],
    export: bool,
    /// The pads as the joystick ports give them (12 bits, active high).
    pads: u16,
    reset_button: bool,
    start_button: bool,
    chips_t: u64,
    field_done: bool,
    audio: [Decimator; 2],
    dc: [[i32; 2]; 2],
    samples: Vec<i16>,
}

impl Board {
    fn stereo(&self) -> bool {
        self.model == SmsModel::GameGear
    }

    fn tick(&mut self) {
        self.psg.tick();
        let mut levels = self.psg.levels();
        let mut fm = 0;
        if let Some(chip) = &mut self.fm {
            chip.tick();
            let (b0, b1) = (
                self.fm_ctl & 1 != 0,
                self.fm_ctl & 2 != 0 && self.fm_builtin,
            );
            if b0 {
                fm = chip.output() / 2;
            }
            if b0 != b1 {
                levels = [0; 4];
            }
        }
        let mask = if self.stereo() { self.gg[6] } else { 0xff };
        let side = |shift: u8| -> i32 {
            (0..4)
                .filter(|ch| mask & (1 << (ch + shift)) != 0)
                .map(|ch| levels[ch as usize])
                .sum()
        };
        let channels = if self.stereo() { 2 } else { 1 };
        let mut out = [0i16; 2];
        let mut got = false;
        for (c, o) in out.iter_mut().enumerate().take(channels) {
            let level = if self.stereo() {
                side(if c == 0 { 4 } else { 0 })
            } else {
                levels.iter().sum::<i32>() + fm
            };
            if let Some(s) = self.audio[c].push(level - 16_382) {
                let x = i32::from(s) << 16;
                let y = x - self.dc[c][0] + ((i64::from(self.dc[c][1]) * 65_208) >> 16) as i32;
                self.dc[c] = [x, y];
                *o = (y >> 16).clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
                got = true;
            }
        }
        if got {
            self.samples.extend_from_slice(&out[..channels]);
        }
        let pixels = 1 + (self.chips_t & 1);
        for _ in 0..pixels {
            self.field_done |= self.vdp.tick();
        }
        self.chips_t += 1;
    }

    fn run_to(&mut self, to: u64) {
        while self.chips_t < to {
            self.tick();
        }
    }

    fn bios_on(&self) -> bool {
        self.bios.is_some() && self.memctl & 0x08 == 0
    }

    fn cart_on(&self) -> bool {
        self.cart.is_some() && (self.memctl & 0x40 == 0 || self.model == SmsModel::GameGear)
    }

    fn mem_peek(&self, addr: u16) -> u8 {
        if addr >= 0xc000 {
            return if self.memctl & 0x10 != 0 {
                0xff
            } else {
                self.ram[usize::from(addr & 0x1fff)]
            };
        }
        // The Game Gear's 1 KB BIOS sits over the cartridge at $0000.
        if self.model == SmsModel::GameGear && self.bios_on() && addr < 0x400 {
            return self
                .bios
                .as_ref()
                .map_or(0xff, |b| b[usize::from(addr) % b.len()]);
        }
        if self.cart_on() {
            if let Some(o) = self.mapper.ram_offset(addr) {
                return self.cart_ram[o];
            }
            let c = self.cart.as_ref().expect("cart_on");
            return c.byte(self.mapper.rom_offset(addr));
        }
        if self.bios_on() {
            let b = self.bios.as_ref().expect("bios_on");
            return b[self.mapper.rom_offset(addr) % b.len()];
        }
        0xff
    }

    fn mem_write(&mut self, addr: u16, v: u8) {
        if addr >= 0xc000 {
            if self.memctl & 0x10 == 0 {
                self.ram[usize::from(addr & 0x1fff)] = v;
            }
            self.mapper.write(addr, v);
            return;
        }
        if let Some(o) = self.mapper.ram_offset(addr) {
            self.cart_ram[o] = v;
            return;
        }
        self.mapper.write(addr, v);
    }

    /// A TH line: an input reads high; an output its level on an export
    /// machine, 0 on a Japanese one.
    fn th(&self, port_b: bool) -> bool {
        let (dir, level) = if port_b { (0x08, 0x80) } else { (0x02, 0x20) };
        if self.ioctl & dir != 0 {
            true
        } else {
            self.export && self.ioctl & level != 0
        }
    }

    fn io_peek(&self, port: u8) -> u8 {
        if port == 0xf2 && self.fm.is_some() && self.memctl & 0x04 != 0 {
            return self.fm_ctl;
        }
        match port & 0xc1 {
            0x00 | 0x01 => {
                if self.model == SmsModel::GameGear && port <= 6 {
                    self.gg_peek(port)
                } else {
                    0xff
                }
            }
            0x40 => self.vdp.v_counter(),
            0x41 => self.vdp.h_counter(),
            0x80 => self.vdp.peek().0,
            0x81 => self.vdp.peek().1,
            _ => {
                if self.memctl & 0x04 != 0 {
                    return 0xff;
                }
                if self.model == SmsModel::GameGear && !matches!(port, 0xc0 | 0xc1 | 0xdc | 0xdd) {
                    return 0xff;
                }
                let low = !self.pads;
                if port & 1 == 0 {
                    low as u8
                } else {
                    let reset = !(self.model == SmsModel::Sms && self.reset_button);
                    (low >> 8) as u8 & 0x0f
                        | u8::from(reset) << 4
                        | 0x20
                        | u8::from(self.th(false)) << 6
                        | u8::from(self.th(true)) << 7
                }
            }
        }
    }

    fn gg_peek(&self, port: u8) -> u8 {
        match port {
            0 => u8::from(!self.start_button) << 7 | u8::from(self.export) << 6,
            p => self.gg[usize::from(p)],
        }
    }

    fn io_write(&mut self, port: u8, v: u8) {
        if let (Some(fm), 0xf0..=0xf2) = (&mut self.fm, port) {
            match port {
                0xf0 => fm.write_addr(v),
                0xf1 => fm.write_data(v),
                _ => self.fm_ctl = v & if self.fm_builtin { 3 } else { 1 },
            }
            return;
        }
        match port & 0xc1 {
            0x00 | 0x01 => {
                if self.model == SmsModel::GameGear && port <= 6 {
                    if port != 0 {
                        self.gg[usize::from(port)] = v;
                    }
                } else if port & 1 == 0 {
                    self.memctl = v;
                } else {
                    let before = (self.th(false), self.th(true));
                    self.ioctl = v;
                    if (self.th(false), self.th(true)) != before {
                        self.vdp.latch_h();
                    }
                }
            }
            0x40 | 0x41 => self.psg.write(v),
            0x80 => self.vdp.write_data(v),
            0x81 => self.vdp.write_control(v),
            _ => {}
        }
    }
}

struct SmsBus<'a> {
    b: &'a mut Board,
    ports: &'a mut [Option<Box<dyn PortDevice>>; 2],
    start: u64,
    dbg: Option<&'a mut DebugHooks>,
    beam: BeamPos,
}

impl Bus for SmsBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        let v = if addr >= IO_BASE {
            let port = addr as u8;
            if port & 0xc0 == 0xc0 {
                self.b.pads = pad_row(self.ports, Cycle(self.b.chips_t));
            }
            u32::from(match port & 0xc1 {
                0x80 => self.b.vdp.read_data(),
                0x81 => self.b.vdp.read_status(),
                _ => self.b.io_peek(port),
            })
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
            self.b.io_write(addr as u8, v as u8);
        } else {
            self.b.mem_write(addr as u16, v as u8);
        }
    }

    fn addr_bits(&self) -> u8 {
        17
    }
}

impl TimedBus for SmsBus<'_> {
    fn wait_states(&mut self, _access: Access, t: u32) -> u32 {
        self.b.run_to(self.start + u64::from(t));
        0
    }
}

fn peek(b: &Board, addr: u32) -> u8 {
    if addr >= IO_BASE {
        b.io_peek(addr as u8)
    } else {
        b.mem_peek(addr as u16)
    }
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

pub struct Sms {
    model: SmsModel,
    standard: VideoStandard,
    cpu: Z80,
    board: Board,
    ports: [Option<Box<dyn PortDevice>>; 2],
    cart: CartSlot<SmsCart>,
    mapper_kind: MapperKind,
    nmi_held: bool,
    sched: Scheduler,
    input: InputQueue,
    field_done_pending: bool,
    overlay_ids: Vec<&'static str>,
    overlay_fresh: bool,
}

impl Sms {
    pub fn new(model: SmsModel, config: &MachineConfig) -> Result<Self, ConfigError> {
        let opt = |k: &str| config.options.get(k).map(String::as_str);
        let standard = match model {
            SmsModel::GameGear => VideoStandard::Ntsc,
            _ => config.video_format.unwrap_or(VideoStandard::Ntsc),
        };
        let pal = standard == VideoStandard::Pal;
        let hz = if pal { CPU_HZ_PAL } else { crate::CPU_HZ };
        let mapper_kind = match opt("mapper") {
            None | Some("sega") => MapperKind::Sega,
            Some("codemasters") => MapperKind::Codemasters,
            Some(o) => {
                return Err(ConfigError::Invalid(format!(
                    "mapper option '{o}' (sega or codemasters)"
                )))
            }
        };
        let export = match opt("region") {
            None => model != SmsModel::Mark3,
            Some("export") => true,
            Some("japan") => false,
            Some(o) => {
                return Err(ConfigError::Invalid(format!(
                    "region option '{o}' (export or japan)"
                )))
            }
        };
        let fm_on = match opt("fm") {
            None => model == SmsModel::Sms && !export,
            Some("on") if model != SmsModel::GameGear => true,
            Some("off") => false,
            Some(o) => {
                return Err(ConfigError::Invalid(format!(
                    "fm option '{o}' (on or off; not on the Game Gear)"
                )))
            }
        };
        let bios = config
            .roms
            .get(model.rom_role())
            .cloned()
            .filter(|b| !b.is_empty());
        let port = |key: &str| -> Result<Option<Box<dyn PortDevice>>, ConfigError> {
            match opt(key) {
                None | Some("joystick") => Ok(Some(Box::new(Joystick::new(PortKind::SegaJoy)))),
                Some("none") => Ok(None),
                Some(o) => Err(ConfigError::Invalid(format!(
                    "{key} option '{o}' (joystick or none)"
                ))),
            }
        };
        let ports = [port("port1")?, port("port2")?];
        let decimator = || {
            Decimator::new(hz, mimic_core::SAMPLE_RATE)
                .ok_or_else(|| ConfigError::Invalid("audio rate".into()))
        };
        let mut m = Self {
            model,
            standard,
            cpu: Z80::new(),
            board: Board {
                model,
                ram: vec![0; 0x2000],
                bios,
                cart: None,
                cart_ram: vec![0; 0x8000],
                mapper: Mapper::new(mapper_kind),
                vdp: SmsVdp::new(model.vdp(), pal),
                psg: Sn76489::sega(),
                fm: fm_on.then(Ym2413::new),
                fm_ctl: 0,
                fm_builtin: model == SmsModel::Sms,
                memctl: 0,
                ioctl: 0xff,
                gg: [0xc0, 0x7f, 0xff, 0x00, 0xff, 0x00, 0xff],
                export,
                pads: 0,
                reset_button: false,
                start_button: false,
                chips_t: 0,
                field_done: false,
                audio: [decimator()?, decimator()?],
                dc: [[0; 2]; 2],
                samples: Vec::new(),
            },
            ports,
            cart: devices_cartridge::sega::sms_slot(),
            mapper_kind,
            nmi_held: false,
            sched: Scheduler::new(),
            input: InputQueue::new(),
            field_done_pending: false,
            overlay_ids: Vec::new(),
            overlay_fresh: false,
        };
        match opt("cart") {
            None => {}
            Some("demo") => {
                let media = Media {
                    name: "demo".into(),
                    kind: MediaKind::Cartridge,
                    backend: Box::new(MemoryImage(demo::sms_rom(model == SmsModel::GameGear))),
                };
                m.cart
                    .insert(media)
                    .map_err(|e| ConfigError::Invalid(format!("demo cartridge: {e}")))?;
            }
            Some(o) => return Err(ConfigError::Invalid(format!("cart option '{o}' (demo)"))),
        }
        // The configured cartridge is in from power-on, not a change.
        m.cart.take_changed();
        m.power_up();
        Ok(m)
    }

    pub fn model(&self) -> SmsModel {
        self.model
    }

    /// Power on: with a BIOS it runs first (cartridge off); without one the
    /// machine starts the cartridge as the BIOS would leave it.
    fn power_up(&mut self) {
        self.cpu = Z80::new();
        self.cpu.reset();
        let b = &mut self.board;
        b.ram.fill(0);
        b.cart_ram.fill(0);
        b.cart = self.cart.cart().cloned();
        b.mapper = Mapper::new(self.mapper_kind);
        b.vdp = SmsVdp::new(self.model.vdp(), self.standard == VideoStandard::Pal);
        b.vdp.set_overlays(&self.overlay_ids);
        b.psg = Sn76489::sega();
        if b.fm.is_some() {
            b.fm = Some(Ym2413::new());
        }
        b.fm_ctl = 0;
        b.ioctl = 0xff;
        b.gg = [0xc0, 0x7f, 0xff, 0x00, 0xff, 0x00, 0xff];
        if b.bios.is_some() {
            b.memctl = 0xe3;
        } else {
            b.memctl = if self.model == SmsModel::GameGear {
                0xa8
            } else {
                0xab
            };
            b.ram[0] = b.memctl;
        }
    }

    fn beam_pos(&self) -> BeamPos {
        let v = &self.board.vdp;
        BeamPos {
            frame: v.fields,
            line: v.line,
            cycle: v.dot * 2 / 3,
            hpos: v.dot.min(vdp::WIDTH as u16 - 1),
        }
    }

    fn instruction(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        if self.cart.take_changed() {
            self.reset(ResetKind::Hard);
        }
        while let Some(ev) = self.input.pop_due(self.sched.now()) {
            self.apply_input(ev);
        }
        let start = self.sched.now().0;
        let beam = self.beam_pos();
        let t = {
            let mut bus = SmsBus {
                b: &mut self.board,
                ports: &mut self.ports,
                start,
                dbg: dbg.as_deref_mut(),
                beam,
            };
            self.cpu.step_timed(&mut bus)
        };
        let end = start + u64::from(t);
        self.board.run_to(end);
        self.sched.advance(u64::from(t));
        self.cpu.set_nmi(self.nmi_held);
        self.cpu.set_int(self.board.vdp.int());
        let entered = self.cpu.take_interrupt();
        let field = std::mem::take(&mut self.board.field_done);
        if field && self.board.vdp.overlays_on() {
            self.overlay_fresh = true;
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
                    match machine_key.0 {
                        crate::keys::PAUSE => self.pause(down),
                        crate::keys::RESET_BUTTON => self.board.reset_button = down,
                        _ => {}
                    }
                }
            }
            InputTarget::Panel => {
                if let InputEvent::Control { id, value } = ev.event {
                    match id.0 {
                        CONTROL_POWER if value == 1 => self.reset(ResetKind::Hard),
                        CONTROL_PAUSE => self.pause(value != 0),
                        CONTROL_RESET => self.board.reset_button = value != 0,
                        _ => {}
                    }
                }
            }
        }
    }

    /// PAUSE (the /NMI) or the Game Gear's START.
    fn pause(&mut self, down: bool) {
        if self.model == SmsModel::GameGear {
            self.board.start_button = down;
        } else {
            self.nmi_held = down;
        }
    }

    fn signal(&self) -> SignalInfo {
        let (w, _, _) = self.board.vdp.frame();
        let hz = if self.standard == VideoStandard::Pal {
            CPU_HZ_PAL
        } else {
            crate::CPU_HZ
        };
        SignalInfo {
            pixel_clock_hz: hz * 3 / 2,
            active_pixels: w,
            total_pixels: u32::from(vdp::LINE),
            lines_per_field: u32::from(self.board.vdp.lines()),
            standard: self.standard,
            output: if self.model == SmsModel::GameGear {
                OutputType::Rgb
            } else {
                OutputType::Composite
            },
            field: FieldParity::Progressive,
        }
    }
}

/// Read a byte as the CPU sees it now (tests and tools).
pub fn peek_cpu(m: &Sms, addr: u16) -> u8 {
    m.board.mem_peek(addr)
}

impl Machine for Sms {
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

    /// Hard: power cycle. Soft: the Z80 and the VDP restart (the SMS's
    /// RESET button is read by the program, see `CONTROL_RESET`).
    fn reset(&mut self, kind: ResetKind) {
        if kind == ResetKind::Hard {
            self.power_up();
        } else {
            self.cpu = Z80::new();
            self.cpu.reset();
            self.board.vdp.reset();
        }
        self.nmi_held = false;
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

    fn overlays(&self) -> Vec<OverlayInfo> {
        vdp::overlay_infos(self.board.vdp.chip_name())
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
            channels: if self.board.stereo() { 2 } else { 1 },
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
        let v = &self.board.vdp;
        let p = &self.board.psg;
        let b = &self.board;
        let reg = |name, value: u32, bits| ChipReg {
            name,
            addr: None,
            value,
            bits,
        };
        const R: [&str; 11] = [
            "R0", "R1", "R2", "R3", "R4", "R5", "R6", "R7", "R8", "R9", "R10",
        ];
        let mut vdp_regs: Vec<ChipReg> = R
            .iter()
            .zip(v.regs)
            .map(|(n, val)| reg(*n, u32::from(val), 8))
            .collect();
        vdp_regs.extend([
            reg("STATUS", u32::from(v.peek().1), 8),
            reg("ADDR", u32::from(v.addr), 14),
            reg("CODE", u32::from(v.code), 2),
            reg("LINE", u32::from(v.line), 9),
            reg("VCOUNT", u32::from(v.v_counter()), 8),
        ]);
        let psg_regs = vec![
            reg("TONE0", u32::from(p.period[0]), 10),
            reg("TONE1", u32::from(p.period[1]), 10),
            reg("TONE2", u32::from(p.period[2]), 10),
            reg("NOISE", u32::from(p.noise), 3),
            reg("ATT0", u32::from(p.atten[0]), 4),
            reg("ATT1", u32::from(p.atten[1]), 4),
            reg("ATT2", u32::from(p.atten[2]), 4),
            reg("ATT3", u32::from(p.atten[3]), 4),
        ];
        let mut sys = vec![
            reg("MEMCTL", u32::from(b.memctl), 8),
            reg("IOCTL", u32::from(b.ioctl), 8),
            reg("SLOT0", u32::from(b.mapper.regs[1]), 8),
            reg("SLOT1", u32::from(b.mapper.regs[2]), 8),
            reg("SLOT2", u32::from(b.mapper.regs[3]), 8),
            reg("RAMCTL", u32::from(b.mapper.regs[0]), 8),
        ];
        if b.model == SmsModel::GameGear {
            sys.push(reg("STEREO", u32::from(b.gg[6]), 8));
        }
        let mut chips = vec![
            ChipInfo {
                name: "vdp",
                regs: vdp_regs,
            },
            ChipInfo {
                name: "psg",
                regs: psg_regs,
            },
            ChipInfo {
                name: "system",
                regs: sys,
            },
        ];
        if let Some(fm) = &b.fm {
            const NAMES: [&str; 9] = [
                "CH0", "CH1", "CH2", "CH3", "CH4", "CH5", "CH6", "CH7", "CH8",
            ];
            let mut regs = vec![
                reg("CONTROL", u32::from(b.fm_ctl), 2),
                reg("RHYTHM", u32::from(fm.regs[0x0e]), 8),
            ];
            for (ch, name) in NAMES.iter().enumerate() {
                let freq = u32::from(fm.regs[0x20 + ch]) << 8 | u32::from(fm.regs[0x10 + ch]);
                regs.push(reg(name, freq << 8 | u32::from(fm.regs[0x30 + ch]), 24));
            }
            chips.push(ChipInfo {
                name: "ym2413",
                regs,
            });
        }
        chips
    }

    fn devices(&self) -> Vec<mimic_core::DeviceId> {
        vec![CPU]
    }

    fn debug_target(&mut self, dev: mimic_core::DeviceId) -> Option<&mut dyn DebugTarget> {
        (dev == CPU).then_some(self as &mut dyn DebugTarget)
    }

    fn input(&mut self, ev: TimedInput) {
        self.input.push(ev);
    }

    fn now(&self) -> Cycle {
        self.sched.now()
    }

    fn controls(&self) -> Vec<mimic_core::ControlInfo> {
        let mut c = vec![mimic_core::ControlInfo::button(
            CONTROL_POWER,
            "POWER",
            false,
        )];
        match self.model {
            SmsModel::GameGear => c.push(mimic_core::ControlInfo::button(
                CONTROL_PAUSE,
                "START",
                self.board.start_button,
            )),
            _ => c.push(mimic_core::ControlInfo::button(
                CONTROL_PAUSE,
                "PAUSE",
                self.nmi_held,
            )),
        }
        if self.model == SmsModel::Sms {
            c.push(mimic_core::ControlInfo::button(
                CONTROL_RESET,
                "RESET",
                self.board.reset_button,
            ));
        }
        c
    }

    fn ports(&self) -> Vec<PortInfo> {
        let names = if self.model == SmsModel::GameGear {
            [(PORT1, "controls"), (PORT2, "unused")]
        } else {
            [(PORT1, "joystick 1"), (PORT2, "joystick 2")]
        };
        names
            .into_iter()
            .map(|(id, name)| PortInfo {
                id,
                kind: PortKind::SegaJoy,
                name,
                connected: self.ports[usize::from(id.0)].is_some(),
            })
            .collect()
    }

    fn connect(&mut self, port: PortId, dev: Box<dyn PortDevice>) -> Result<(), PortError> {
        let Some(slot) = self.ports.get_mut(usize::from(port.0)) else {
            return Err(PortError::NoSuchPort(port));
        };
        if dev.kind() != PortKind::SegaJoy {
            return Err(PortError::Incompatible {
                port: PortKind::SegaJoy,
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
        vec![MediaSlotInfo {
            id: SLOT_CARTRIDGE,
            name: "cartridge",
            kind: MediaKind::Cartridge,
            inserted: self.cart.is_inserted(),
        }]
    }

    fn media_slot(&mut self, slot: SlotId) -> Option<&mut dyn MediaSlot> {
        (slot == SLOT_CARTRIDGE).then_some(&mut self.cart as &mut dyn MediaSlot)
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        let b = &self.board;
        w.chunk(*b"SMSB", 1, |w| {
            w.u8(self.model as u8);
            w.bool(self.field_done_pending);
            w.bool(self.nmi_held);
            w.bytes(&b.ram);
            w.bytes(&b.cart_ram);
            w.bool(b.cart.is_some());
            w.raw(&b.mapper.regs);
            w.u8(b.memctl);
            w.u8(b.ioctl);
            w.raw(&b.gg);
            w.u16(b.pads);
            w.bool(b.reset_button);
            w.bool(b.start_button);
            w.u64(b.chips_t);
            w.bool(b.field_done);
            for a in &b.audio {
                a.save(w);
            }
            for d in b.dc {
                w.u32(d[0] as u32);
                w.u32(d[1] as u32);
            }
            w.bool(b.fm.is_some());
            w.u8(b.fm_ctl);
        });
        self.cpu.save(&mut w);
        b.vdp.save(&mut w);
        b.psg.save(&mut w);
        if let Some(fm) = &b.fm {
            fm.save(&mut w);
        }
        self.sched.save(&mut w);
        self.input.save(&mut w);
        for (i, tag) in [*b"SMP1", *b"SMP2"].into_iter().enumerate() {
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
        let (_, mut c) = r.chunk(*b"SMSB", 1)?;
        if c.u8()? != self.model as u8 {
            return Err(StateError::Invalid("model differs"));
        }
        let field_done_pending = c.bool()?;
        let nmi_held = c.bool()?;
        let mut board = self.board.clone();
        for (dst, what) in [
            (&mut board.ram, "RAM size"),
            (&mut board.cart_ram, "cartridge RAM size"),
        ] {
            let data = c.bytes()?;
            if data.len() != dst.len() {
                return Err(StateError::Invalid(what));
            }
            dst.copy_from_slice(data);
        }
        if c.bool()? != board.cart.is_some() {
            return Err(StateError::Invalid("cartridge differs"));
        }
        c.raw_into(&mut board.mapper.regs)?;
        board.memctl = c.u8()?;
        board.ioctl = c.u8()?;
        c.raw_into(&mut board.gg)?;
        board.pads = c.u16()? & 0x0fff;
        board.reset_button = c.bool()?;
        board.start_button = c.bool()?;
        board.chips_t = c.u64()?;
        board.field_done = c.bool()?;
        for a in &mut board.audio {
            a.load(&mut c)?;
        }
        for d in &mut board.dc {
            *d = [c.u32()? as i32, c.u32()? as i32];
        }
        if c.bool()? != board.fm.is_some() {
            return Err(StateError::Invalid("FM sound differs"));
        }
        board.fm_ctl = c.u8()? & 3;
        board.samples.clear();
        let mut cpu = self.cpu.clone();
        let mut sched = self.sched.clone();
        let mut input = self.input.clone();
        cpu.load(&mut r)?;
        board.vdp.load(&mut r)?;
        board.psg.load(&mut r)?;
        if let Some(fm) = &mut board.fm {
            fm.load(&mut r)?;
        }
        sched.load(&mut r)?;
        input.load(&mut r)?;
        let mut port_states = Vec::new();
        for tag in [*b"SMP1", *b"SMP2"] {
            let (_, mut c) = r.chunk(tag, 1)?;
            port_states.push(c.bytes()?.to_vec());
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
        self.cpu = cpu;
        self.board = board;
        self.sched = sched;
        self.input = input;
        self.field_done_pending = field_done_pending;
        self.nmi_held = nmi_held;
        Ok(())
    }
}

impl DebugTarget for Sms {
    fn regs(&self) -> RegisterSet {
        self.cpu.regs()
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        self.cpu.set_reg(name, v);
    }

    fn peek(&self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
        let byte = |a: u32| -> Option<u32> {
            match space {
                CPU_SPACE => Some(u32::from(peek(&self.board, a))),
                VRAM_SPACE => Some(u32::from(self.board.vdp.vram[(a & 0x3fff) as usize])),
                _ => None,
            }
        };
        (0..size.bytes()).try_fold(0, |v, i| Some(v | byte(addr.wrapping_add(i))? << (8 * i)))
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        for i in 0..size.bytes() {
            let a = addr.wrapping_add(i);
            let byte = (v >> (8 * i)) as u8;
            match space {
                CPU_SPACE if (0xc000..0x1_0000).contains(&a) => {
                    self.board.ram[(a & 0x1fff) as usize] = byte;
                }
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
