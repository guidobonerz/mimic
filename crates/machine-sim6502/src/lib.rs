//! Reference machine: NMOS 6502, 64 KB RAM and a minimal raster video chip.
//!
//! Not modelled on any real computer. It exists to exercise the `Machine`
//! contract end to end (scheduler, debug hooks, beam, partial frames,
//! savestates, headless runner, debug server) and stays as the smallest
//! machine for protocol and frontend tests.
//!
//! Memory map: RAM everywhere except `$D000–$D0FF` (video registers, see
//! [`video`]). The program comes from `MachineConfig::roms["program"]` as a
//! PRG (2-byte load address); execution starts at the load address unless
//! `options["start"]` (hex) says otherwise. Without a program a built-in
//! raster-bar demo runs.

#![forbid(unsafe_code)]

pub mod video;

use cpu_6502::Cpu6502;
use emu_core::input::TimedInput;
use emu_core::machine::ChipReg;
use emu_core::media::{MediaSlot, MediaSlotInfo, SlotId};
use emu_core::port::{PortDevice, PortError, PortId, PortInfo};
use emu_core::registry::{ConfigError, MachineConfig, MachineEntry};
use emu_core::Cycle;
use emu_core::{
    BeamPos, Bus, ChipInfo, Cpu, CycleResult, DebugHooks, DebugTarget, DeviceId, FrameRef, Instr,
    Machine, MemSpace, MemSpaceId, RegisterSet, ResetKind, Savestate, Scheduler, Size, StateError,
    StateReader, StateWriter, StopReason,
};

use video::{Video, IO_BASE, IO_END};

pub const CPU: DeviceId = DeviceId("cpu");
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);

/// Built-in demo at `$0800`: copy the raster line into the border colour
/// forever, giving one colour band per line with visible mid-line changes.
const DEMO: [u8; 11] = [
    0x00, 0x08, // PRG load address $0800
    0xad, 0x10, 0xd0, // LDA $D010
    0x8d, 0x00, 0xd0, // STA $D000
    0x4c, 0x00, 0x08, // JMP $0800
];

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "sim6502",
        name: "6502 reference machine",
        models: &["default"],
        create: |config| Ok(Box::new(Sim6502::new(config)?)),
    }
}

/// RAM + video as the CPU sees them, with optional debug interception.
struct SimBus<'a> {
    ram: &'a mut [u8; 0x10000],
    video: &'a mut Video,
    dbg: Option<&'a mut DebugHooks>,
}

impl SimBus<'_> {
    fn raw_read(&self, addr: u16) -> u8 {
        if (IO_BASE..=IO_END).contains(&addr) {
            self.video.peek(addr)
        } else {
            self.ram[usize::from(addr)]
        }
    }

    fn raw_write(&mut self, addr: u16, v: u8) {
        if (IO_BASE..=IO_END).contains(&addr) {
            self.video.write(addr, v);
        } else {
            self.ram[usize::from(addr)] = v;
        }
    }
}

impl Bus for SimBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        debug_assert_eq!(size, Size::Byte);
        let a = addr as u16;
        let v = u32::from(self.raw_read(a));
        let beam = self.video.beam();
        match self.dbg.as_deref_mut() {
            Some(dbg) if dbg.read_armed(CPU_SPACE) => {
                let seen = dbg.on_read(CPU_SPACE, addr & 0xffff, Size::Byte, v);
                dbg.trace_read(CPU_SPACE, addr & 0xffff, Size::Byte, seen, beam);
                seen
            }
            _ => v,
        }
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(self.raw_read(addr as u16))
    }

    fn write(&mut self, addr: u32, _size: Size, v: u32) {
        let a = addr as u16;
        let beam = self.video.beam();
        if let Some(dbg) = self.dbg.as_deref_mut() {
            if dbg.write_armed(CPU_SPACE) {
                dbg.trace_write(CPU_SPACE, addr & 0xffff, Size::Byte, v & 0xff, beam);
                if !dbg.on_write(CPU_SPACE, addr & 0xffff, Size::Byte, v & 0xff) {
                    return;
                }
            }
        }
        self.raw_write(a, v as u8);
    }

    fn addr_bits(&self) -> u8 {
        16
    }
}

/// Side-effect-free view for the disassembler.
struct PeekBus<'a> {
    ram: &'a [u8; 0x10000],
    video: &'a Video,
}

impl Bus for PeekBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        let a = addr as u16;
        if (IO_BASE..=IO_END).contains(&a) {
            u32::from(self.video.peek(a))
        } else {
            u32::from(self.ram[usize::from(a)])
        }
    }

    fn write(&mut self, _addr: u32, _size: Size, _v: u32) {}

    fn addr_bits(&self) -> u8 {
        16
    }
}

#[derive(Clone)]
pub struct Sim6502 {
    cpu: Cpu6502,
    ram: Box<[u8; 0x10000]>,
    /// RAM after loading the program, restored by a hard reset.
    power_on_ram: Box<[u8; 0x10000]>,
    video: Video,
    sched: Scheduler,
    field_done_pending: bool,
}

impl Sim6502 {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        if let Some(model) = config.model.as_deref() {
            if model != "default" {
                return Err(ConfigError::UnknownModel(model.to_string()));
            }
        }
        let prg = config.roms.get("program").map_or(&DEMO[..], Vec::as_slice);
        if prg.len() < 2 {
            return Err(ConfigError::Invalid(
                "program shorter than its load address".into(),
            ));
        }
        let load = u16::from_le_bytes([prg[0], prg[1]]);
        let body = &prg[2..];
        if usize::from(load) + body.len() > 0x10000 {
            return Err(ConfigError::Invalid(
                "program does not fit into 64 KB".into(),
            ));
        }
        let start = match config.options.get("start") {
            Some(s) => u16::from_str_radix(s.trim_start_matches('$'), 16)
                .map_err(|_| ConfigError::Invalid(format!("start address '{s}'")))?,
            None => load,
        };

        let mut ram = Box::new([0u8; 0x10000]);
        ram[usize::from(load)..usize::from(load) + body.len()].copy_from_slice(body);
        // Reset vector unless the program brings its own.
        if usize::from(load) + body.len() <= 0xfffc {
            ram[0xfffc..0xfffe].copy_from_slice(&start.to_le_bytes());
        }
        let mut cpu = Cpu6502::default();
        cpu.reset();
        Ok(Self {
            cpu,
            power_on_ram: ram.clone(),
            ram,
            video: Video::default(),
            sched: Scheduler::new(),
            field_done_pending: false,
        })
    }

    /// One master cycle. Returns `Some` if emulation must stop.
    fn cycle(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        self.cpu.set_irq(self.video.irq());
        let result = self.cpu.tick(&mut SimBus {
            ram: &mut self.ram,
            video: &mut self.video,
            dbg: dbg.as_deref_mut(),
        });
        let entered = self.cpu.take_interrupt();
        self.sched.tick();
        let new_field = self.video.cycle(&self.ram);

        if let Some(dbg) = dbg {
            if let Some(kind) = entered {
                if dbg.interrupt_armed(kind) {
                    dbg.check_interrupt(kind);
                }
            }
            if let CycleResult::InstrDone { next_pc } = result {
                if dbg.trace_instr_armed() {
                    dbg.trace_instr(CPU, next_pc, self.video.beam());
                }
                if dbg.instr_step_armed() {
                    dbg.check_instr_step();
                }
                if dbg.exec_armed(CPU_SPACE) {
                    dbg.check_exec(CPU_SPACE, next_pc);
                }
            }
            if dbg.raster_armed() {
                let b = self.video.beam();
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

    fn apply_frame_freezes(&mut self, dbg: &DebugHooks) {
        for f in dbg.per_frame_freezes() {
            if f.space != CPU_SPACE {
                continue;
            }
            let mut bus = SimBus {
                ram: &mut self.ram,
                video: &mut self.video,
                dbg: None,
            };
            for i in 0..f.size.bytes() {
                bus.raw_write(f.addr.wrapping_add(i) as u16, (f.value >> (8 * i)) as u8);
            }
        }
    }

    fn peek_bus(&self) -> PeekBus<'_> {
        PeekBus {
            ram: &self.ram,
            video: &self.video,
        }
    }
}

impl Machine for Sim6502 {
    fn run_frame(&mut self, dbg: &mut DebugHooks) -> StopReason {
        // A debug stop on the last cycle of a field: report the field first.
        if std::mem::take(&mut self.field_done_pending) {
            return StopReason::FrameDone;
        }
        if self.video.line == 0 && self.video.cycle == 0 {
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

    fn reset(&mut self, kind: ResetKind) {
        self.cpu.reset();
        if kind == ResetKind::Hard {
            self.ram.copy_from_slice(&self.power_on_ram[..]);
            self.video = Video::default();
        }
        self.field_done_pending = false;
    }

    fn framebuffer(&self) -> FrameRef<'_> {
        self.video.frame()
    }

    fn audio(&mut self, _out: &mut Vec<i16>) -> emu_core::AudioFormat {
        emu_core::AudioFormat {
            sample_rate: emu_core::SAMPLE_RATE,
            channels: 1,
        }
    }

    fn beam(&self) -> Option<BeamPos> {
        Some(self.video.beam())
    }

    fn memory_spaces(&self) -> Vec<MemSpace> {
        vec![MemSpace {
            id: CPU_SPACE,
            name: "cpu",
            device: CPU,
            addr_bits: 16,
            big_endian: false,
        }]
    }

    fn chips(&self) -> Vec<ChipInfo> {
        let reg = |name, addr: u16| ChipReg {
            name,
            addr: Some(u32::from(addr)),
            value: u32::from(self.video.peek(addr)),
            bits: 8,
        };
        vec![ChipInfo {
            name: "video",
            regs: vec![
                reg("BORDER", 0xd000),
                reg("BACKGROUND", 0xd001),
                reg("FOREGROUND", 0xd002),
                reg("RASTER_LO", 0xd010),
                reg("RASTER_HI", 0xd011),
                reg("IRQ_LINE_LO", 0xd012),
                reg("IRQ_LINE_HI", 0xd013),
                reg("IRQ_STATUS", 0xd019),
                reg("IRQ_ENABLE", 0xd01a),
            ],
        }]
    }

    fn devices(&self) -> Vec<DeviceId> {
        vec![CPU]
    }

    fn debug_target(&mut self, dev: DeviceId) -> Option<&mut dyn DebugTarget> {
        (dev == CPU).then_some(self as &mut dyn DebugTarget)
    }

    /// The reference machine has no keyboard or ports: events are dropped.
    fn input(&mut self, _ev: TimedInput) {}

    fn now(&self) -> Cycle {
        self.sched.now()
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
        w.chunk(*b"SIM6", 1, |w| {
            w.raw(&self.ram[..]);
            w.bool(self.field_done_pending);
        });
        self.cpu.save(&mut w);
        self.video.save(&mut w);
        self.sched.save(&mut w);
        w.into_bytes()
    }

    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut next = self.clone();
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"SIM6", 1)?;
        c.raw_into(&mut next.ram[..])?;
        next.field_done_pending = c.bool()?;
        next.cpu.load(&mut r)?;
        next.video.load(&mut r)?;
        next.sched.load(&mut r)?;
        *self = next;
        Ok(())
    }
}

impl DebugTarget for Sim6502 {
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
        let bus = self.peek_bus();
        let mut v = 0;
        for i in 0..size.bytes() {
            v |= bus.peek(addr.wrapping_add(i) & 0xffff, Size::Byte) << (8 * i);
        }
        Some(v)
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        if space != CPU_SPACE {
            return false;
        }
        let mut bus = SimBus {
            ram: &mut self.ram,
            video: &mut self.video,
            dbg: None,
        };
        for i in 0..size.bytes() {
            bus.raw_write((addr.wrapping_add(i) & 0xffff) as u16, (v >> (8 * i)) as u8);
        }
        true
    }

    fn disassemble(&self, addr: u32) -> Instr {
        self.cpu.disassemble(&self.peek_bus(), addr)
    }
}

#[cfg(test)]
mod tests;
