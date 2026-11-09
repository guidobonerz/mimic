//! Tandy (Radio Shack) TRS-80 Model I and Model III.
//!
//! Z80 at 1.774 MHz (the 10.6445 MHz crystal divided by six), 12 KB Level II
//! BASIC (or 4 KB Level I), the memory-mapped keyboard and 64 × 16 text
//! screen with 2 × 3 block graphics (see [`memory`], [`keyboard`],
//! [`video`]), port `$FF` for the cassette ([`cassette`], 500 baud) and
//! the 32-column mode, and the Expansion Interface ([`ei`]: real-time
//! clock, FD1771 floppy controller ([`fdc`]) with four drives).
//!
//! The Model III ([`m3`]): Z80 at 2.028 MHz, 14 KB ROM, eight-bit video RAM
//! (lowercase), the I/O on ports: interrupt and NMI masks, the 30 Hz
//! clock, an FD1793 with double density, side select and WAIT. Its ROM
//! boots drive 0 (BREAK held at power-on skips that). Its cassette is not
//! built yet.
//!
//! Media slots: "tape" (kind Tape): a ".cas" image; CLOAD plays it, CSAVE
//! appends to it. With the Expansion Interface or on the Model III "drive
//! 0" to "drive 3" (kind Disk): DMK, JV1 or JV3 images.
//!
//! Models: "1" (Model I, default), "3" (Model III, ROM role "model3").
//! Options (Model I unless said):
//! - `level`: "2" (default, ROM role "level2") or "1" (ROM role "level1").
//! - `ram`: "16k" (default), "4k", "48k" (needs the Expansion Interface on
//!   a real machine; here plain RAM).
//! - `lowercase`: "off" (default; seven-bit video RAM) or "on" (the
//!   lowercase modification).
//! - `ei`: "off" (default) or "on": the Expansion Interface. With it Level
//!   II boots from drive 0 (holding BREAK at power-on skips that; without
//!   a disk it waits, as the real machine does).
//! - `roms`: "stub" runs a small built-in program ([`stub`]).
//!
//! Port `$FF` (Model I): out bits 0–1 the cassette output, bit 2 the
//! cassette motor relay, bit 3 the 32-column mode; in bit 7 the cassette
//! input latch. Panel controls: RESET (the button behind the keyboard's
//! right side: an NMI, which the ROM takes as a warm start) and POWER.
//!
//! ROMs user-supplied, never in the repository: "level2" (12 KB, Level II
//! BASIC 1.3 or 1.2), "level1" (4 KB). No character generator ROM: the
//! font is this project's own ([`charset`]).
//!
//! Sources: TRS-80 Model I Technical Reference Manual (Radio Shack
//! 26-2103: memory map, keyboard, video, port `$FF`); TRS-80 Model III
//! Technical Reference Manual. The implementation is this project's own.

#![forbid(unsafe_code)]

pub mod cassette;
pub mod charset;
pub mod ei;
pub mod fdc;
pub mod keyboard;
pub mod m3;
pub mod memory;
pub mod stub;
pub mod video;

use cpu_z80::{Access, TimedBus, IO_BASE, Z80};
use emu_core::frame::{
    FieldParity, FrameRef, OutputType, PixelData, Rgb8, SignalInfo, VideoStandard,
};
use emu_core::input::{InputEvent, InputQueue, InputTarget, TimedInput};
use emu_core::machine::ChipReg;
use emu_core::media::{MediaKind, MediaSlot, MediaSlotInfo, SlotId};
use emu_core::port::{PortDevice, PortError, PortId, PortInfo};
use emu_core::registry::{ConfigError, MachineConfig, MachineEntry};
use emu_core::{
    BeamPos, Bus, ChipInfo, Cpu, Cycle, DebugHooks, DebugTarget, DeviceId, Instr, Machine,
    MemSpace, MemSpaceId, RegisterSet, ResetKind, Savestate, Scheduler, Size, StateError,
    StateReader, StateWriter, StopReason,
};
use memory::Memory;

pub const CPU: DeviceId = DeviceId("cpu");
/// The Z80's view: memory at 0–$FFFF, I/O ports at `IO_BASE + port`.
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);
pub const SLOT_TAPE: SlotId = SlotId(0);
/// Disk drives 0-3 (with the Expansion Interface).
pub const SLOT_DRIVE0: SlotId = SlotId(1);

/// Model I: 10.64448 MHz / 6.
pub const CPU_HZ: u32 = 1_774_080;
/// Model III: 20.2752 MHz / 10.
pub const CPU_HZ_III: u32 = 2_027_520;

/// Panel controls.
pub const CONTROL_RESET: u16 = 0;
pub const CONTROL_POWER: u16 = 1;

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "trs80",
        name: "Tandy TRS-80",
        models: &["1", "3"],
        video_formats: &[],
        create: |config| Ok(Box::new(Trs80::new(config)?)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    ModelI,
    ModelIII,
}

/// The Z80's bus for one instruction.
struct TrsBus<'a> {
    mem: &'a mut Memory,
    keys: &'a [u8; 8],
    port_ff: &'a mut u8,
    cass: &'a mut cassette::Cassette,
    ei: Option<&'a mut ei::ExpansionInterface>,
    m3: Option<&'a mut m3::Model3Io>,
    /// Clocks WAIT held the CPU in this instruction.
    extra: u64,
    cass_in: bool,
    /// Clock of the instruction's start and of the coming access.
    start: u64,
    at: u64,
    dbg: Option<&'a mut DebugHooks>,
    beam: BeamPos,
}

fn read_at(mem: &Memory, keys: &[u8; 8], cass_in: bool, addr: u32) -> u8 {
    if addr >= IO_BASE {
        return match addr as u8 {
            0xff => 0x7f | u8::from(cass_in) << 7,
            _ => 0xff,
        };
    }
    let a = addr as u16;
    match a {
        0x3800..=0x3bff => keyboard::read(keys, a),
        _ => mem.read(a),
    }
}

impl Bus for TrsBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        let v = match (addr, self.ei.as_deref_mut(), self.m3.as_deref_mut()) {
            (0x37e0..=0x37ef, Some(ei), _) => u32::from(ei.read(addr as u16, self.at)),
            (IO_BASE.., _, Some(m3)) if addr as u8 != 0xff => {
                let (v, wait) = m3.read(addr as u8, self.at);
                self.extra += wait;
                self.at += wait;
                u32::from(v)
            }
            _ => u32::from(read_at(self.mem, self.keys, self.cass_in, addr)),
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
        match (addr, self.ei.as_deref(), self.m3.as_deref()) {
            (0x37e0..=0x37ef, Some(ei), _) => u32::from(ei.peek(addr as u16, self.at)),
            (IO_BASE.., _, Some(m3)) if addr as u8 != 0xff => {
                u32::from(m3.peek(addr as u8, self.at))
            }
            _ => u32::from(read_at(self.mem, self.keys, self.cass_in, addr)),
        }
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
            if addr as u8 == 0xff {
                *self.port_ff = v as u8 & 0x0f;
                self.cass.write_port(v as u8, self.at);
            } else if let Some(m3) = self.m3.as_deref_mut() {
                let wait = m3.write(addr as u8, v as u8, self.at);
                self.extra += wait;
                self.at += wait;
            }
        } else if let (0x37e0..=0x37ef, Some(ei)) = (addr, self.ei.as_deref_mut()) {
            ei.write(addr as u16, v as u8, self.at);
        } else {
            self.mem.write(addr as u16, v as u8);
        }
    }

    fn addr_bits(&self) -> u8 {
        17
    }
}

impl TimedBus for TrsBus<'_> {
    /// No wait states on the bus itself; the Model III's WAIT for the
    /// floppy controller lengthens the instruction (see `extra`).
    fn wait_states(&mut self, _access: Access, t: u32) -> u32 {
        self.at = self.start + u64::from(t) + self.extra;
        0
    }
}

struct PeekBus<'a>(
    &'a Memory,
    &'a [u8; 8],
    bool,
    Option<&'a ei::ExpansionInterface>,
    u64,
    Option<&'a m3::Model3Io>,
);

impl Bus for PeekBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        match (addr, self.3, self.5) {
            (0x37e0..=0x37ef, Some(ei), _) => u32::from(ei.peek(addr as u16, self.4)),
            (IO_BASE.., _, Some(m3)) if addr as u8 != 0xff => {
                u32::from(m3.peek(addr as u8, self.4))
            }
            _ => u32::from(read_at(self.0, self.1, self.2, addr)),
        }
    }

    fn write(&mut self, _addr: u32, _size: Size, _v: u32) {}

    fn addr_bits(&self) -> u8 {
        17
    }
}

pub struct Trs80 {
    model: Model,
    cpu: Z80,
    mem: Memory,
    /// Keyboard rows (1 = pressed).
    keys: [u8; 8],
    /// Port `$FF` output latch (bits 0–3).
    port_ff: u8,
    /// The cassette input latch (port `$FF` bit 7), as of the last
    /// instruction.
    cass_in: bool,
    cassette: cassette::Cassette,
    ei: Option<ei::ExpansionInterface>,
    m3: Option<m3::Model3Io>,
    timing: video::Timing,
    /// CPU clocks since power-on.
    sched: Scheduler,
    input: InputQueue,
    /// Clock of the next field's start.
    next_field: u64,
    fields: u64,
    pixels: Vec<u8>,
    palette: [Rgb8; 2],
    field_done_pending: bool,
}

impl Trs80 {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        let model = match config.model.as_deref() {
            None | Some("1") => Model::ModelI,
            Some("3") => Model::ModelIII,
            Some(other) => return Err(ConfigError::UnknownModel(other.into())),
        };
        let three = model == Model::ModelIII;
        let opt = |k: &str| config.options.get(k).map(String::as_str);
        let ram = match opt("ram") {
            None if three => 0xc000,
            None | Some("16k") => 0x4000,
            Some("4k") => 0x1000,
            Some("48k") => 0xc000,
            Some(o) => {
                return Err(ConfigError::Invalid(format!(
                    "ram option '{o}' (4k, 16k, 48k)"
                )))
            }
        };
        let lowercase = match opt("lowercase") {
            None | Some("off") => false,
            Some("on") => true,
            Some(o) => return Err(ConfigError::Invalid(format!("lowercase option '{o}'"))),
        } || three;
        let ei = match opt("ei") {
            None | Some("off") => None,
            Some("on") if !three => Some(ei::ExpansionInterface::new(CPU_HZ)),
            Some(o) => return Err(ConfigError::Invalid(format!("ei option '{o}' (on, off)"))),
        };
        let (role, size) = match opt("level") {
            _ if three => ("model3", 0x3800),
            None | Some("2") => ("level2", 0x3000),
            Some("1") => ("level1", 0x1000),
            Some(o) => return Err(ConfigError::Invalid(format!("level option '{o}' (1 or 2)"))),
        };
        let rom = match opt("roms") {
            Some("stub") => {
                let mut r = stub::rom();
                if three {
                    r.resize(0x3800, 0xff);
                }
                r
            }
            Some(o) => return Err(ConfigError::Invalid(format!("roms option '{o}'"))),
            None => {
                let r = config
                    .roms
                    .get(role)
                    .ok_or_else(|| ConfigError::MissingRom(role.into()))?;
                if r.len() != size {
                    return Err(ConfigError::Invalid(format!(
                        "{role} ROM must be {size} bytes, got {}",
                        r.len()
                    )));
                }
                r.clone()
            }
        };
        let timing = if three {
            video::MODEL_III
        } else {
            video::MODEL_I
        };
        let mut m = Self {
            model,
            cpu: Z80::new(),
            mem: Memory::new(rom, ram, lowercase),
            keys: [0; 8],
            port_ff: 0,
            cass_in: false,
            cassette: cassette::Cassette::default(),
            ei,
            m3: three.then(|| m3::Model3Io::new(CPU_HZ_III)),
            timing,
            sched: Scheduler::new(),
            input: InputQueue::new(),
            next_field: timing.t_per_field(),
            fields: 0,
            pixels: vec![0; (video::WIDTH * video::HEIGHT) as usize],
            // A white P4 phosphor.
            palette: [
                Rgb8 { r: 0, g: 0, b: 0 },
                Rgb8 {
                    r: 0xe8,
                    g: 0xec,
                    b: 0xf4,
                },
            ],
            field_done_pending: false,
        };
        m.cpu.reset();
        Ok(m)
    }

    pub fn model(&self) -> Model {
        self.model
    }

    fn beam_pos(&self) -> BeamPos {
        let t = self.timing;
        let into = self.sched.now().0 + t.t_per_field() - self.next_field;
        let dots = t.dot_hz / t.cpu_hz;
        BeamPos {
            frame: self.fields,
            line: (into / t.t_per_line) as u16,
            cycle: (into % t.t_per_line) as u16,
            hpos: ((into % t.t_per_line) * u64::from(dots)) as u16,
        }
    }

    /// One instruction (or interrupt response). Returns `Some` if
    /// emulation must stop.
    fn instruction(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        while let Some(ev) = self.input.pop_due(self.sched.now()) {
            self.apply_input(ev);
        }
        let beam = self.beam_pos();
        let start = self.sched.now().0;
        self.cass_in = self.cassette.latch();
        let t = {
            let mut bus = TrsBus {
                mem: &mut self.mem,
                keys: &self.keys,
                port_ff: &mut self.port_ff,
                cass: &mut self.cassette,
                ei: self.ei.as_mut(),
                m3: self.m3.as_mut(),
                extra: 0,
                cass_in: self.cass_in,
                start,
                at: start,
                dbg: dbg.as_deref_mut(),
                beam,
            };
            let t = self.cpu.step_timed(&mut bus);
            u64::from(t) + bus.extra
        };
        self.sched.advance(t);
        self.cassette.run(t);
        if let Some(ei) = &mut self.ei {
            ei.tick(self.sched.now().0);
            self.cpu.set_int(ei.int());
        }
        if let Some(m3) = &mut self.m3 {
            m3.tick(self.sched.now().0);
            self.cpu.set_int(m3.int());
            self.cpu.set_nmi(m3.nmi());
        }
        let entered = self.cpu.take_interrupt();
        let field = self.sched.now().0 >= self.next_field;
        if field {
            self.next_field += self.timing.t_per_field();
            self.fields += 1;
            let wide = match &self.m3 {
                Some(m3) => m3.wide(),
                None => self.port_ff & 0x08 != 0,
            };
            video::render(&self.mem.vram, wide, self.mem.lowercase, &mut self.pixels);
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
        match (ev.target, ev.event) {
            (InputTarget::Keyboard, InputEvent::Key { machine_key, down }) => {
                let (row, bit) = (usize::from(machine_key.0 / 8), machine_key.0 % 8);
                if let Some(r) = self.keys.get_mut(row) {
                    if down {
                        *r |= 1 << bit;
                    } else {
                        *r &= !(1 << bit);
                    }
                }
            }
            (InputTarget::Panel, InputEvent::Control { id, value: 1 }) => match id.0 {
                CONTROL_RESET => Machine::reset(self, ResetKind::Soft),
                CONTROL_POWER => Machine::reset(self, ResetKind::Hard),
                _ => {}
            },
            _ => {}
        }
    }

    fn signal(&self) -> SignalInfo {
        SignalInfo {
            pixel_clock_hz: self.timing.dot_hz,
            active_pixels: video::WIDTH,
            total_pixels: self.timing.dots_per_line,
            lines_per_field: self.timing.lines,
            standard: VideoStandard::Ntsc,
            output: OutputType::Monochrome,
            field: FieldParity::Progressive,
        }
    }

    /// Queue key presses that type `text` (tests and tools): each key held
    /// for three fields and released for three.
    pub fn type_text(&mut self, text: &str) {
        let hold = self.timing.t_per_field() * 3;
        let (events, _) = keyboard::typed_events(text, self.sched.now().0, hold, hold);
        for e in events {
            self.input.push(e);
        }
    }

    /// The text screen as a string, 16 lines of 64 characters (graphics
    /// as `#`, unprintable as `.`), for tests.
    pub fn screen_text(&self) -> String {
        let mut s = String::new();
        for row in 0..video::ROWS {
            for col in 0..video::COLUMNS {
                let c = self.mem.vram_read(row * video::COLUMNS + col);
                s.push(match c {
                    0x80.. => '#',
                    0x20..=0x7e => c as char,
                    0..=0x1f => (c + 0x40) as char,
                    _ => '.',
                });
            }
            s.push('\n');
        }
        s
    }

    /// A byte as the CPU reads it now (tests and tools).
    pub fn peek_cpu(&self, addr: u16) -> u8 {
        read_at(&self.mem, &self.keys, self.cass_in, u32::from(addr))
    }
}

impl Machine for Trs80 {
    fn run_frame(&mut self, dbg: &mut DebugHooks) -> StopReason {
        if std::mem::take(&mut self.field_done_pending) {
            return StopReason::FrameDone;
        }
        for f in dbg.per_frame_freezes() {
            if f.space == CPU_SPACE && f.addr < IO_BASE {
                for i in 0..f.size.bytes() {
                    self.mem
                        .write((f.addr + i) as u16, (f.value >> (8 * i)) as u8);
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
        self.instruction(None);
        StopReason::StepDone
    }

    /// Soft: the RESET button, an NMI (the ROM's warm start). Hard: power
    /// off and on.
    fn reset(&mut self, kind: ResetKind) {
        match kind {
            // The Model III's RESET restarts the CPU at 0 (RAM kept).
            ResetKind::Soft if self.m3.is_some() => self.cpu.reset(),
            ResetKind::Soft => {
                self.cpu.set_nmi(true);
                self.cpu.set_nmi(false);
            }
            _ => {
                self.cpu = Z80::new();
                self.cpu.reset();
                self.mem.clear();
                self.keys = [0; 8];
                self.port_ff = 0;
                self.cass_in = false;
                if let Some(ei) = &mut self.ei {
                    ei.reset();
                }
                if let Some(m3) = &mut self.m3 {
                    m3.reset();
                }
                self.input.clear();
                self.field_done_pending = false;
            }
        }
    }

    fn framebuffer(&self) -> FrameRef<'_> {
        FrameRef {
            width: video::WIDTH,
            height: video::HEIGHT,
            pixels: PixelData::Indexed {
                pixels: &self.pixels,
                palette: &self.palette,
            },
            luma_chroma: None,
            signal: self.signal(),
        }
    }

    fn audio(&mut self, _out: &mut Vec<i16>) -> emu_core::AudioFormat {
        emu_core::AudioFormat {
            sample_rate: emu_core::SAMPLE_RATE,
            channels: 1,
        }
    }

    fn beam(&self) -> Option<BeamPos> {
        Some(self.beam_pos())
    }

    fn memory_spaces(&self) -> Vec<MemSpace> {
        vec![MemSpace {
            id: CPU_SPACE,
            name: "cpu",
            device: CPU,
            addr_bits: 17,
            big_endian: false,
        }]
    }

    fn chips(&self) -> Vec<ChipInfo> {
        vec![ChipInfo {
            name: "io",
            regs: vec![
                ChipReg {
                    name: "PORT_FF",
                    addr: Some(IO_BASE + 0xff),
                    value: u32::from(self.port_ff),
                    bits: 8,
                },
                ChipReg {
                    name: "CASS_IN",
                    addr: None,
                    value: u32::from(self.cass_in),
                    bits: 1,
                },
            ],
        }]
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
            emu_core::ControlInfo::button(CONTROL_RESET, "RESET", false),
            emu_core::ControlInfo::button(CONTROL_POWER, "POWER", false),
        ]
    }

    fn ports(&self) -> Vec<PortInfo> {
        Vec::new()
    }

    fn connect(&mut self, port: PortId, _dev: Box<dyn PortDevice>) -> Result<(), PortError> {
        Err(PortError::NoSuchPort(port))
    }

    fn disconnect(&mut self, _port: PortId) -> Option<Box<dyn PortDevice>> {
        None
    }

    fn media_slots(&self) -> Vec<MediaSlotInfo> {
        let mut v = vec![MediaSlotInfo {
            id: SLOT_TAPE,
            name: "tape",
            kind: MediaKind::Tape,
            inserted: self.cassette.inserted(),
        }];
        let drives = match (&self.ei, &self.m3) {
            (Some(ei), _) => Some(&ei.drives),
            (_, Some(m3)) => Some(&m3.drives),
            _ => None,
        };
        if let Some(drives) = drives {
            const NAMES: [&str; ei::DRIVES] = ["drive 0", "drive 1", "drive 2", "drive 3"];
            for (d, name) in NAMES.iter().enumerate() {
                v.push(MediaSlotInfo {
                    id: SlotId(SLOT_DRIVE0.0 + d as u8),
                    name,
                    kind: MediaKind::Disk,
                    inserted: drives[d].image.is_some(),
                });
            }
        }
        v
    }

    fn media_slot(&mut self, slot: SlotId) -> Option<&mut dyn MediaSlot> {
        if slot == SLOT_TAPE {
            return Some(&mut self.cassette as &mut dyn MediaSlot);
        }
        let d = usize::from(slot.0.checked_sub(SLOT_DRIVE0.0)?);
        let drives = match (&mut self.ei, &mut self.m3) {
            (Some(ei), _) => &mut ei.drives,
            (_, Some(m3)) => &mut m3.drives,
            _ => return None,
        };
        drives.get_mut(d).map(|d| d as &mut dyn MediaSlot)
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(*b"TRS8", 1, |w| {
            w.u8(self.model as u8);
            w.raw(&self.keys);
            w.u8(self.port_ff);
            w.bool(self.cass_in);
            w.u64(self.next_field);
            w.u64(self.fields);
            w.bool(self.field_done_pending);
            w.raw(&self.pixels);
            self.cassette.save(w);
            w.bool(self.ei.is_some());
            if let Some(ei) = &self.ei {
                ei.save(w);
            }
            if let Some(m3) = &self.m3 {
                m3.save(w);
            }
        });
        self.cpu.save(&mut w);
        self.mem.save(&mut w);
        self.sched.save(&mut w);
        self.input.save(&mut w);
        w.into_bytes()
    }

    /// Atomic: everything loads into copies first.
    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"TRS8", 1)?;
        if c.u8()? != self.model as u8 {
            return Err(StateError::Invalid("model differs"));
        }
        let mut keys = [0u8; 8];
        c.raw_into(&mut keys)?;
        let port_ff = c.u8()? & 0x0f;
        let cass_in = c.bool()?;
        let next_field = c.u64()?;
        let fields = c.u64()?;
        let field_done_pending = c.bool()?;
        let mut pixels = vec![0u8; self.pixels.len()];
        c.raw_into(&mut pixels)?;
        let mut cass = cassette::Cassette::default();
        cass.load(&mut c)?;
        if c.bool()? != self.ei.is_some() {
            return Err(StateError::Invalid("Expansion Interface differs"));
        }
        let ei_state = match &self.ei {
            Some(_) => {
                let mut e = ei::ExpansionInterface::new(CPU_HZ);
                e.load(&mut c)?;
                Some(e)
            }
            None => None,
        };
        let m3_state = match &self.m3 {
            Some(_) => {
                let mut io = m3::Model3Io::new(CPU_HZ_III);
                io.load(&mut c)?;
                Some(io)
            }
            None => None,
        };
        if pixels.iter().any(|&p| p > 1) {
            return Err(StateError::Invalid("TRS-80 pixels"));
        }
        let mut cpu = self.cpu.clone();
        let mut mem = self.mem.clone();
        let mut sched = self.sched.clone();
        let mut input = self.input.clone();
        cpu.load(&mut r)?;
        mem.load(&mut r)?;
        sched.load(&mut r)?;
        input.load(&mut r)?;
        if next_field > sched.now().0 + self.timing.t_per_field() {
            return Err(StateError::Invalid("TRS-80 field clock"));
        }
        self.cpu = cpu;
        self.mem = mem;
        self.sched = sched;
        self.input = input;
        self.keys = keys;
        self.port_ff = port_ff;
        self.cass_in = cass_in;
        self.next_field = next_field;
        self.fields = fields;
        self.field_done_pending = field_done_pending;
        self.pixels = pixels;
        // The tape's contents stay; its position and the recording load.
        self.cassette.restore(cass);
        if let (Some(ei), Some(loaded)) = (&mut self.ei, ei_state) {
            ei.restore(loaded);
        }
        if let (Some(io), Some(loaded)) = (&mut self.m3, m3_state) {
            io.restore(loaded);
        }
        Ok(())
    }
}

impl DebugTarget for Trs80 {
    fn regs(&self) -> RegisterSet {
        self.cpu.regs()
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        self.cpu.set_reg(name, v);
    }

    fn peek(&self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
        if space != CPU_SPACE {
            return None;
        }
        let bus = PeekBus(
            &self.mem,
            &self.keys,
            self.cass_in,
            self.ei.as_ref(),
            self.sched.now().0,
            self.m3.as_ref(),
        );
        Some((0..size.bytes()).fold(0, |v, i| {
            v | bus.peek(addr.wrapping_add(i), Size::Byte) << (8 * i)
        }))
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        if space != CPU_SPACE || addr >= IO_BASE {
            return false;
        }
        for i in 0..size.bytes() {
            let a = (addr.wrapping_add(i) & 0xffff) as u16;
            if a < memory::VRAM_BASE {
                return false;
            }
            self.mem.write(a, (v >> (8 * i)) as u8);
        }
        true
    }

    fn disassemble(&self, addr: u32) -> Instr {
        self.cpu.disassemble(
            &PeekBus(
                &self.mem,
                &self.keys,
                self.cass_in,
                self.ei.as_ref(),
                self.sched.now().0,
                self.m3.as_ref(),
            ),
            addr,
        )
    }
}

#[cfg(test)]
mod tests;
