//! Commodore 64.
//!
//! Phase 2 builds this up chip by chip. Present so far: 6510 with its I/O
//! port, PLA banking, RAM, ROMs, colour RAM, both CIAs and the IRQ/NMI
//! wiring. The VIC-II and SID follow; until the VIC-II exists the frame
//! timing is a fixed PAL 312 × 63 cycle count and the picture is black.
//!
//! CIA revision: options["cia"] = "6526" (default, original C64) or
//! "6526a" (C64C).
//!
//! ROMs come from `MachineConfig::roms` ("kernal", "basic", "chargen";
//! user-supplied, never in the repo). With `options["roms"] = "stub"` a
//! tiny built-in KERNAL replacement is used instead, for tests without ROMs.

#![forbid(unsafe_code)]

pub mod io;
pub mod memory;
pub mod pla;

use chips_cbm::cia::CiaModel;
use cpu_6502::{Config as CpuConfig, Cpu6502, Variant};
use emu_core::frame::{
    FieldParity, FrameRef, OutputType, PixelData, Rgb8, SignalInfo, VideoStandard,
};
use emu_core::machine::ChipReg;
use emu_core::media::{MediaSlot, MediaSlotInfo, SlotId};
use emu_core::port::{PortDevice, PortError, PortId, PortInfo};
use emu_core::registry::{ConfigError, MachineConfig, MachineEntry};
use emu_core::{
    BeamPos, Bus, ChipInfo, Cpu, CycleResult, DebugHooks, DebugTarget, DeviceId, HookedBus, Instr,
    Machine, MemSpace, MemSpaceId, RegisterSet, Savestate, Scheduler, Size, StateError,
    StateReader, StateWriter, StopReason,
};

use io::{Io, OPEN_BUS};
use memory::{Memory, Roms, BASIC_SIZE, CHARGEN_SIZE, KERNAL_SIZE};
use pla::Bank;

pub const CPU: DeviceId = DeviceId("cpu");
/// The CPU's banked view of memory.
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);
/// The 64 KB of RAM, independent of banking.
pub const RAM_SPACE: MemSpaceId = MemSpaceId(1);
/// The 1 KB × 4 bit colour RAM.
pub const COLOR_SPACE: MemSpaceId = MemSpaceId(2);

const LINES: u32 = 312;
const CYCLES_PER_LINE: u32 = 63;
const CYCLES_PER_FRAME: u32 = LINES * CYCLES_PER_LINE;
const WIDTH: u32 = 384;
const HEIGHT: u32 = 272;

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "c64",
        name: "Commodore 64",
        models: &["pal"],
        create: |config| Ok(Box::new(C64::new(config)?)),
    }
}

/// Interrupt sources on the open-collector IRQ and NMI lines.
pub mod irq {
    pub const VIC: u8 = 1 << 0;
    pub const CIA1: u8 = 1 << 1;
    pub const CARTRIDGE: u8 = 1 << 2;
}

pub mod nmi {
    pub const CIA2: u8 = 1 << 0;
    pub const RESTORE: u8 = 1 << 1;
    pub const CARTRIDGE: u8 = 1 << 2;
}

/// Wired-OR interrupt lines: a line is active while any source pulls it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Interrupts {
    pub irq: u8,
    pub nmi: u8,
}

impl Interrupts {
    pub fn set_irq(&mut self, source: u8, active: bool) {
        if active {
            self.irq |= source;
        } else {
            self.irq &= !source;
        }
    }

    pub fn set_nmi(&mut self, source: u8, active: bool) {
        if active {
            self.nmi |= source;
        } else {
            self.nmi &= !source;
        }
    }
}

/// Side-effect-free CPU-view read.
fn cpu_peek(mem: &Memory, io: &Io, addr: u16) -> u8 {
    match mem.read(addr) {
        Some(v) => v,
        None if mem.bank(addr) == Bank::Io => io.peek(mem, addr),
        None => OPEN_BUS,
    }
}

/// The CPU's bus: PLA-resolved memory plus I/O.
struct C64Bus<'a> {
    mem: &'a mut Memory,
    io: &'a mut Io,
}

impl Bus for C64Bus<'_> {
    fn read(&mut self, addr: u32, _size: Size) -> u32 {
        let a = addr as u16;
        u32::from(match self.mem.read(a) {
            Some(v) => v,
            None if self.mem.bank(a) == Bank::Io => self.io.read(self.mem, a),
            None => OPEN_BUS,
        })
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(cpu_peek(self.mem, self.io, addr as u16))
    }

    fn write(&mut self, addr: u32, _size: Size, v: u32) {
        let a = addr as u16;
        if !self.mem.write(a, v as u8) {
            self.io.write(self.mem, a, v as u8);
        }
    }

    fn addr_bits(&self) -> u8 {
        16
    }
}

/// Read-only view for disassembly and debugger peeks.
struct PeekBus<'a>(&'a Memory, &'a Io);

impl Bus for PeekBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(cpu_peek(self.0, self.1, addr as u16))
    }

    fn write(&mut self, _addr: u32, _size: Size, _v: u32) {}

    fn addr_bits(&self) -> u8 {
        16
    }
}

/// Built-in KERNAL replacement for runs without ROMs: initialises stack and
/// CPU port, then idles; IRQ and NMI return immediately. Written for this
/// project, not derived from the Commodore KERNAL.
fn stub_kernal() -> Box<[u8; KERNAL_SIZE]> {
    let mut k = Box::new([0xff; KERNAL_SIZE]);
    let code = [
        0x78, // E000 SEI
        0xa2, 0xff, // E001 LDX #$FF
        0x9a, // E003 TXS
        0xd8, // E004 CLD
        // Data latch before DDR, or the ROMs vanish when the pins turn to outputs.
        0xa9, 0x37, // E005 LDA #$37
        0x85, 0x01, // E007 STA $01
        0xa9, 0x2f, // E009 LDA #$2F
        0x85, 0x00, // E00B STA $00
        0x4c, 0x0d, 0xe0, // E00D JMP $E00D
        0x40, // E010 RTI
    ];
    k[..code.len()].copy_from_slice(&code);
    k[0x1ffa..].copy_from_slice(&[0x10, 0xe0, 0x00, 0xe0, 0x10, 0xe0]);
    k
}

fn rom<const N: usize>(config: &MachineConfig, role: &str) -> Result<Box<[u8; N]>, ConfigError> {
    let data = config
        .roms
        .get(role)
        .ok_or_else(|| ConfigError::MissingRom(role.into()))?;
    let boxed: Box<[u8]> = data.clone().into_boxed_slice();
    boxed.try_into().map_err(|_| {
        ConfigError::Invalid(format!("{role} ROM must be {N} bytes, got {}", data.len()))
    })
}

fn load_roms(config: &MachineConfig) -> Result<Roms, ConfigError> {
    match config.options.get("roms").map(String::as_str) {
        Some("stub") => Ok(Roms {
            kernal: stub_kernal(),
            basic: Box::new([0xff; BASIC_SIZE]),
            chargen: Box::new([0; CHARGEN_SIZE]),
        }),
        Some(other) => Err(ConfigError::Invalid(format!("roms option '{other}'"))),
        None => Ok(Roms {
            kernal: rom(config, "kernal")?,
            basic: rom(config, "basic")?,
            chargen: rom(config, "chargen")?,
        }),
    }
}

const PALETTE: [Rgb8; 1] = [Rgb8 { r: 0, g: 0, b: 0 }];

#[derive(Clone)]
pub struct C64 {
    cpu: Cpu6502,
    mem: Memory,
    io: Io,
    ints: Interrupts,
    sched: Scheduler,
    frame: u64,
    frame_cycle: u32,
    pixels: Vec<u8>,
    field_done_pending: bool,
}

impl C64 {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        if let Some(model) = config.model.as_deref() {
            if model != "pal" {
                return Err(ConfigError::UnknownModel(model.into()));
            }
        }
        let mut cpu = Cpu6502::new(CpuConfig {
            variant: Variant::Mos6510,
            ..CpuConfig::default()
        });
        cpu.reset();
        let cia = match config.options.get("cia").map(String::as_str) {
            None | Some("6526") => CiaModel::Mos6526,
            Some("6526a") => CiaModel::Mos6526A,
            Some(other) => return Err(ConfigError::Invalid(format!("cia option '{other}'"))),
        };
        Ok(Self {
            cpu,
            mem: Memory::new(load_roms(config)?),
            io: Io::new(cia),
            ints: Interrupts::default(),
            sched: Scheduler::new(),
            frame: 0,
            frame_cycle: 0,
            pixels: vec![0; (WIDTH * HEIGHT) as usize],
            field_done_pending: false,
        })
    }

    pub fn interrupts_mut(&mut self) -> &mut Interrupts {
        &mut self.ints
    }

    fn beam_pos(&self) -> BeamPos {
        let cycle = (self.frame_cycle % CYCLES_PER_LINE) as u16;
        BeamPos {
            frame: self.frame,
            line: (self.frame_cycle / CYCLES_PER_LINE) as u16,
            cycle,
            hpos: cycle * 8,
        }
    }

    /// One CPU cycle (φ2). Returns `Some` if emulation must stop.
    fn cycle(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        // Chips clock first: a CPU read in this cycle sees this cycle's state.
        self.io.tick();
        self.update_interrupts();
        self.cpu.set_irq(self.ints.irq != 0);
        self.cpu.set_nmi(self.ints.nmi != 0);
        let beam = self.beam_pos();
        let result = {
            let mut bus = C64Bus {
                mem: &mut self.mem,
                io: &mut self.io,
            };
            let mut hooked = HookedBus {
                inner: &mut bus,
                dbg: dbg.as_deref_mut(),
                space: CPU_SPACE,
                beam,
            };
            self.cpu.tick(&mut hooked)
        };
        self.update_interrupts();
        self.sched.tick();
        self.frame_cycle += 1;
        let new_field = self.frame_cycle == CYCLES_PER_FRAME;
        if new_field {
            self.frame_cycle = 0;
            self.frame += 1;
        }

        if let Some(dbg) = dbg {
            if let CycleResult::InstrDone { next_pc } = result {
                if dbg.trace_instr_armed() {
                    dbg.trace_instr(CPU, next_pc, self.beam_pos());
                }
                if dbg.instr_step_armed() {
                    dbg.check_instr_step();
                }
                if dbg.exec_armed(CPU_SPACE) {
                    dbg.check_exec(CPU_SPACE, next_pc);
                }
            }
            if dbg.raster_armed() {
                let b = self.beam_pos();
                dbg.check_raster(b.line, b.cycle);
            }
            if let Some(stop) = dbg.take_stop() {
                self.field_done_pending |= new_field;
                return Some(stop);
            }
        }
        match result {
            CycleResult::Jammed => Some(StopReason::CpuJam {
                device: CPU,
                pc: u32::from(self.cpu.pc()),
            }),
            _ if new_field => Some(StopReason::FrameDone),
            _ => None,
        }
    }

    fn update_interrupts(&mut self) {
        let (cia1, cia2) = (self.io.cia1.irq(), self.io.cia2.irq());
        self.ints.set_irq(irq::CIA1, cia1);
        self.ints.set_nmi(nmi::CIA2, cia2);
    }

    fn apply_frame_freezes(&mut self, dbg: &DebugHooks) {
        for f in dbg.per_frame_freezes() {
            for i in 0..f.size.bytes() {
                let addr = f.addr.wrapping_add(i);
                let v = (f.value >> (8 * i)) as u8;
                match f.space {
                    CPU_SPACE => C64Bus {
                        mem: &mut self.mem,
                        io: &mut self.io,
                    }
                    .write(addr & 0xffff, Size::Byte, u32::from(v)),
                    RAM_SPACE => self.mem.ram[(addr & 0xffff) as usize] = v,
                    COLOR_SPACE => self.mem.color[(addr & 0x3ff) as usize] = v & 0x0f,
                    _ => {}
                }
            }
        }
    }
}

impl Machine for C64 {
    fn run_frame(&mut self, dbg: &mut DebugHooks) -> StopReason {
        if std::mem::take(&mut self.field_done_pending) {
            return StopReason::FrameDone;
        }
        if self.frame_cycle == 0 {
            self.apply_frame_freezes(dbg);
        }
        loop {
            if let Some(stop) = self.cycle(Some(dbg)) {
                return stop;
            }
        }
    }

    fn step_cycle(&mut self) -> StopReason {
        match self.cycle(None) {
            Some(stop @ StopReason::CpuJam { .. }) => stop,
            _ => StopReason::StepDone,
        }
    }

    fn framebuffer(&self) -> FrameRef<'_> {
        FrameRef {
            width: WIDTH,
            height: HEIGHT,
            pixels: PixelData::Indexed {
                pixels: &self.pixels,
                palette: &PALETTE,
            },
            luma_chroma: None,
            signal: SignalInfo {
                pixel_clock_hz: 7_881_984,
                active_pixels: WIDTH,
                total_pixels: CYCLES_PER_LINE * 8,
                lines_per_field: LINES,
                standard: VideoStandard::Pal,
                output: OutputType::SVideo,
                field: FieldParity::Progressive,
            },
        }
    }

    fn beam(&self) -> Option<BeamPos> {
        Some(self.beam_pos())
    }

    fn memory_spaces(&self) -> Vec<MemSpace> {
        let space = |id, name, addr_bits| MemSpace {
            id,
            name,
            device: CPU,
            addr_bits,
            big_endian: false,
        };
        vec![
            space(CPU_SPACE, "cpu", 16),
            space(RAM_SPACE, "ram", 16),
            space(COLOR_SPACE, "colorram", 10),
        ]
    }

    fn chips(&self) -> Vec<ChipInfo> {
        let reg = |name, addr: Option<u32>, value: u8, bits| ChipReg {
            name,
            addr,
            value: u32::from(value),
            bits,
        };
        let cia = |name, base: u32, c: &chips_cbm::cia::Cia| {
            let r16 = |name, addr: u32, value: u16| ChipReg {
                name,
                addr: Some(addr),
                value: u32::from(value),
                bits: 16,
            };
            ChipInfo {
                name,
                regs: vec![
                    reg("PRA", Some(base), c.pra, 8),
                    reg("PRB", Some(base + 1), c.prb, 8),
                    reg("DDRA", Some(base + 2), c.ddra, 8),
                    reg("DDRB", Some(base + 3), c.ddrb, 8),
                    r16("TA", base + 4, c.ta.counter),
                    r16("TA_LATCH", base + 4, c.ta.latch),
                    r16("TB", base + 6, c.tb.counter),
                    r16("TB_LATCH", base + 6, c.tb.latch),
                    reg("TOD_10THS", Some(base + 8), c.tod.time[0], 8),
                    reg("TOD_SEC", Some(base + 9), c.tod.time[1], 8),
                    reg("TOD_MIN", Some(base + 10), c.tod.time[2], 8),
                    reg("TOD_HR", Some(base + 11), c.tod.time[3], 8),
                    reg("ICR", Some(base + 13), c.peek(13, 0xff, 0xff), 8),
                    reg("IMR", None, c.mask(), 8),
                    reg("CRA", Some(base + 14), c.ta.cr, 8),
                    reg("CRB", Some(base + 15), c.tb.cr, 8),
                ],
            }
        };
        vec![
            ChipInfo {
                name: "pla",
                regs: vec![
                    reg("DDR", Some(0), self.mem.port.ddr, 8),
                    reg("DATA", Some(1), self.mem.port.data, 8),
                    reg("PINS", None, self.mem.port.outputs(), 6),
                    reg("MODE", None, self.mem.mode(), 5),
                    reg("GAME", None, u8::from(self.mem.game), 1),
                    reg("EXROM", None, u8::from(self.mem.exrom), 1),
                ],
            },
            cia("cia1", 0xdc00, &self.io.cia1),
            cia("cia2", 0xdd00, &self.io.cia2),
        ]
    }

    fn devices(&self) -> Vec<DeviceId> {
        vec![CPU]
    }

    fn debug_target(&mut self, dev: DeviceId) -> Option<&mut dyn DebugTarget> {
        (dev == CPU).then_some(self as &mut dyn DebugTarget)
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
        Vec::new()
    }

    fn media_slot(&mut self, _slot: SlotId) -> Option<&mut dyn MediaSlot> {
        None
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(*b"C64S", 1, |w| {
            w.u8(self.ints.irq);
            w.u8(self.ints.nmi);
            w.u64(self.frame);
            w.u32(self.frame_cycle);
            w.bool(self.field_done_pending);
        });
        self.cpu.save(&mut w);
        self.mem.save(&mut w);
        self.io.save(&mut w);
        self.sched.save(&mut w);
        w.into_bytes()
    }

    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut next = self.clone();
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"C64S", 1)?;
        next.ints = Interrupts {
            irq: c.u8()?,
            nmi: c.u8()?,
        };
        next.frame = c.u64()?;
        next.frame_cycle = c.u32()?;
        if next.frame_cycle >= CYCLES_PER_FRAME {
            return Err(StateError::Invalid("frame cycle"));
        }
        next.field_done_pending = c.bool()?;
        next.cpu.load(&mut r)?;
        next.mem.load(&mut r)?;
        next.io.load(&mut r)?;
        next.sched.load(&mut r)?;
        *self = next;
        Ok(())
    }
}

impl DebugTarget for C64 {
    fn regs(&self) -> RegisterSet {
        self.cpu.regs()
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        self.cpu.set_reg(name, v);
    }

    fn peek(&self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
        let byte = |a: u32| -> Option<u32> {
            match space {
                CPU_SPACE => Some(PeekBus(&self.mem, &self.io).peek(a & 0xffff, Size::Byte)),
                RAM_SPACE => Some(u32::from(self.mem.ram[(a & 0xffff) as usize])),
                COLOR_SPACE => Some(u32::from(self.mem.color[(a & 0x3ff) as usize])),
                _ => None,
            }
        };
        let mut v = 0;
        for i in 0..size.bytes() {
            v |= byte(addr.wrapping_add(i))? << (8 * i);
        }
        Some(v)
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        for i in 0..size.bytes() {
            let a = addr.wrapping_add(i);
            let b = (v >> (8 * i)) as u8;
            match space {
                CPU_SPACE => {
                    let a = (a & 0xffff) as u16;
                    match self.mem.bank(a) {
                        Bank::Basic | Bank::Kernal | Bank::Char | Bank::RomL | Bank::RomH => {
                            return false
                        }
                        _ => C64Bus {
                            mem: &mut self.mem,
                            io: &mut self.io,
                        }
                        .write(u32::from(a), Size::Byte, u32::from(b)),
                    }
                }
                RAM_SPACE => self.mem.ram[(a & 0xffff) as usize] = b,
                COLOR_SPACE => self.mem.color[(a & 0x3ff) as usize] = b & 0x0f,
                _ => return false,
            }
        }
        true
    }

    fn disassemble(&self, addr: u32) -> Instr {
        self.cpu.disassemble(&PeekBus(&self.mem, &self.io), addr)
    }
}

#[cfg(test)]
mod tests;
