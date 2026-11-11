//! MEGA65.
//!
//! The 45GS02 (`cpu-45gs02`) at 1, 2, 3.5 or 40.5 MHz, 384 KB chip RAM
//! holding the 128 KB ROM at `$20000`–`$3FFFF`, 32 KB colour RAM, the
//! VIC-IV with its C64, C65 and MEGA65 I/O personalities, two CIAs
//! (`chips-cbm`), four SIDs ([`board`]).
//!
//! **Start.** Direct (the user's choice for now): the ROM file goes to
//! `$20000` and the CPU resets as the hypervisor leaves it, C64 banking
//! with the C64 KERNAL's vector at `$2FFFC`, the C64 I/O personality, full
//! speed. The real hypervisor (Hyppo) comes in a later milestone.
//!
//! **Timing.** The master clock is the 40.5 MHz of the CPU's full speed.
//! A PAL frame is 312 raster lines of 2596 master clocks (49.997 Hz). The
//! CIAs count at the C64's rate, 63 cycles a line (a 1 MHz cycle every 41
//! or 42 master clocks); at 1 MHz the CPU runs on those cycles, at 2 MHz on
//! twice as many, at 3.5 MHz on 227 a line, at 40.5 MHz on every master
//! clock.
//!
//! Options: `roms`: "stub" ([`stub`]); `board`: "r1" to "r15", the model ID
//! at `$D629` (default "r3", the MEGA65 R3); `port1`, `port2`: the control
//! ports' devices as on the C64 ("joystick", "paddles", "mouse1351", the
//! proportional mice and keypads, "none"; default: a joystick in port 2).
//!
//! Keyboard: [`keyboard`] (the C65 matrix, the typing event queue). Sound:
//! the four SIDs in stereo ([`board::Board::clock_sids`]).
//!
//! ROMs (user-supplied): role "mega65-rom", 128 KB (`mega65.rom`: the
//! MEGA65 Open ROMs or the official 920xxx ROM).
//!
//! Sources: the MEGA65 user guide (github.com/MEGA65/mega65-user-guide);
//! the register descriptions (`@IO`) of the MEGA65 core; the C65 system
//! specification. The implementation is this project's own.

#![forbid(unsafe_code)]

pub mod board;
pub mod dma;
mod gfx;
pub mod hyper;
pub mod keyboard;
pub mod keytable;
pub mod render;
pub mod storage;
pub mod stub;
#[cfg(test)]
mod tests;
pub mod vic;

use board::Board;
use cpu_45gs02::{Cpu45gs02, Speed, ADDR_MASK, UNMAPPED};
use emu_core::frame::{
    FieldParity, FrameRef, OutputType, PixelData, Rgb8, SignalInfo, VideoStandard,
};
use emu_core::input::{InputEvent, InputQueue, InputTarget, TimedInput};
use emu_core::machine::ChipReg;
use emu_core::media::{MediaSlot, MediaSlotInfo, SlotId};
use emu_core::port::{PortDevice, PortError, PortId, PortInfo};
use emu_core::registry::{ConfigError, MachineConfig, MachineEntry};
use emu_core::{
    BeamPos, Bus, ChipInfo, Cpu, Cycle, CycleResult, DebugHooks, DebugTarget, DeviceId, Instr,
    Machine, MemSpace, MemSpaceId, RegisterSet, ResetKind, Savestate, Size, StateError,
    StateReader, StateWriter, StopReason,
};

pub const CPU: DeviceId = DeviceId("cpu");
/// The true drive on the serial bus.
pub const DRIVE8: DeviceId = DeviceId("drive8");
/// Its CPU's memory.
pub const DRIVE_SPACE: MemSpaceId = MemSpaceId(2);
/// Media slots.
pub const SLOT_DRIVE8: SlotId = SlotId(0);
/// The internal floppy drive (F011 drive 0) and drive 1.
pub const SLOT_FLOPPY0: SlotId = SlotId(1);
pub const SLOT_FLOPPY1: SlotId = SlotId(2);
pub const SLOT_SDCARD: SlotId = SlotId(3);
/// The CPU's 16-bit view (through MAP and the banking).
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);
/// The 28-bit physical address space.
pub const PHYS_SPACE: MemSpaceId = MemSpaceId(1);

pub const MASTER_HZ: u32 = 40_500_000;
pub const LINE_CLOCKS: u32 = 2596;
pub const LINES: u32 = vic::LINES as u32;
/// C64 cycles a raster line (1 MHz).
const SLOW_PER_LINE: u32 = 63;
/// C65 cycles a raster line (3.5 MHz).
const C65_PER_LINE: u32 = 227;

/// The picture: 720 × 576 (the PAL mode of the VIC-IV).
pub const WIDTH: u32 = vic::WIDTH as u32;
pub const HEIGHT: u32 = vic::HEIGHT as u32;

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "mega65",
        name: "MEGA65",
        models: &["mega65"],
        video_formats: &[],
        create: |config| Ok(Box::new(Mega65::new(config)?)),
    }
}

/// The CPU's bus: the board, with the debugger's hooks on the 16-bit view
/// for unmapped accesses and on the physical space for the others.
struct M65Bus<'a> {
    board: &'a mut Board,
    dbg: Option<&'a mut DebugHooks>,
    beam: BeamPos,
}

impl M65Bus<'_> {
    fn space(addr: u32) -> (MemSpaceId, u32) {
        if addr & UNMAPPED != 0 {
            (CPU_SPACE, addr & 0xffff)
        } else {
            (PHYS_SPACE, addr & ADDR_MASK)
        }
    }
}

impl Bus for M65Bus<'_> {
    fn read(&mut self, addr: u32, _size: Size) -> u32 {
        let v = u32::from(self.board.read(addr));
        match self.dbg.as_deref_mut() {
            Some(dbg) => {
                let (space, a) = Self::space(addr);
                if dbg.read_armed(space) {
                    let seen = dbg.on_read(space, a, Size::Byte, v);
                    dbg.trace_read(space, a, Size::Byte, seen, self.beam);
                    seen
                } else {
                    v
                }
            }
            None => v,
        }
    }

    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(self.board.peek(addr))
    }

    fn write(&mut self, addr: u32, _size: Size, v: u32) {
        if let Some(dbg) = self.dbg.as_deref_mut() {
            let (space, a) = Self::space(addr);
            if dbg.write_armed(space) {
                dbg.trace_write(space, a, Size::Byte, v & 0xff, self.beam);
                if !dbg.on_write(space, a, Size::Byte, v & 0xff) {
                    return;
                }
            }
        }
        self.board.write(addr, v as u8);
    }

    fn addr_bits(&self) -> u8 {
        28
    }
}

/// Side-effect free view for the disassembler.
struct PeekBus<'a>(&'a Board);

impl Bus for PeekBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }
    fn peek(&self, addr: u32, _size: Size) -> u32 {
        u32::from(self.0.peek(addr))
    }
    fn write(&mut self, _addr: u32, _size: Size, _v: u32) {}
    fn addr_bits(&self) -> u8 {
        28
    }
}

pub struct Mega65 {
    pub cpu: Cpu45gs02,
    pub board: Board,
    rom: Vec<u8>,
    model_id: u8,
    /// Control ports 1 and 2.
    ports: [Option<Box<dyn PortDevice>>; 2],
    /// Drive 8 on the serial bus (`--option drive8`).
    drive: Option<drives_cbm::SerialDrive>,
    /// The hypervisor services' state ([`hyper`]).
    pub hyper: hyper::Hyper,
    /// Bytes written to the hypervisor's serial monitor (tests, tools).
    pub serial_out: Vec<u8>,
    input: InputQueue,
    /// Master clocks since power-on.
    clocks: u64,
    /// Position in the raster line (master clocks) and the line.
    line_clock: u32,
    raster: u32,
    /// Phase accumulators of the 1, 2 and 3.5 MHz clocks.
    phase_slow: u32,
    phase_c128: u32,
    phase_c65: u32,
    fields: u64,
    /// The picture (720 × 576).
    fb: Vec<Rgb8>,
    /// Master clocks the CPU still waits for a DMA job.
    stall: u32,
    field_done_pending: bool,
}

fn load_rom(config: &MachineConfig) -> Result<Vec<u8>, ConfigError> {
    if config.options.get("roms").map(String::as_str) == Some("stub") {
        return Ok(stub::rom());
    }
    let rom = config
        .roms
        .get("mega65-rom")
        .ok_or_else(|| ConfigError::MissingRom("mega65-rom".into()))?;
    if rom.len() != 0x2_0000 {
        return Err(ConfigError::Invalid(format!(
            "mega65-rom: {} bytes, expected 131072",
            rom.len()
        )));
    }
    Ok(rom.clone())
}

impl Mega65 {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        match config.model.as_deref() {
            None | Some("mega65") => {}
            Some(other) => return Err(ConfigError::UnknownModel(other.into())),
        }
        let mut model_id = 3;
        let option = |name: &str| config.options.get(name).map(String::as_str);
        let port1 = devices_input::control_port_option("port1", option("port1"), "none", true)?;
        let port2 = devices_input::control_port_option("port2", option("port2"), "joystick", true)?;
        for (k, v) in &config.options {
            match k.as_str() {
                "roms" if v == "stub" => {}
                "port1" | "port2" | "drive8" => {}
                "board" => {
                    model_id = v
                        .strip_prefix('r')
                        .and_then(|n| n.parse::<u8>().ok())
                        .filter(|n| (1..=15).contains(n))
                        .ok_or_else(|| ConfigError::Invalid(format!("board '{v}' (r1 to r15)")))?;
                }
                _ => return Err(ConfigError::Invalid(format!("option {k}={v}"))),
            }
        }
        // Drive 8 as on the C64: "1541", "1571", "1581" (true drive with
        // its DOS ROM), "virtual" or "none"; by default a 1541 when its ROM
        // is there.
        let drive = drives_cbm::SerialDrive::from_option(
            option("drive8"),
            drives_cbm::DriveRoms::from_map(&config.roms),
            drives_cbm::DriveModel::C1541,
            8,
            DRIVE8,
            DRIVE_SPACE,
            board::SID_HZ,
        )?;
        let rom = load_rom(config)?;
        let mut m = Self {
            cpu: Cpu45gs02::new(),
            board: Board::new(&rom),
            rom,
            model_id,
            ports: [port1, port2],
            drive,
            hyper: hyper::Hyper::default(),
            serial_out: Vec::new(),
            input: InputQueue::default(),
            clocks: 0,
            line_clock: 0,
            raster: 0,
            phase_slow: 0,
            phase_c128: 0,
            phase_c65: 0,
            fields: 0,
            stall: 0,
            fb: vec![Rgb8::default(); (WIDTH * HEIGHT) as usize],
            field_done_pending: false,
        };
        m.power_on();
        Ok(m)
    }

    fn power_on(&mut self) {
        self.board = Board::new(&self.rom);
        self.board.model_id = self.model_id;
        self.cpu = Cpu45gs02::new();
        self.reset_cpu();
    }

    fn reset_cpu(&mut self) {
        let speed = self.board.speed();
        self.cpu.set_speed(speed);
        let mut bus = M65Bus {
            board: &mut self.board,
            dbg: None,
            beam: BeamPos::default(),
        };
        self.cpu.reset(&mut bus);
    }

    fn beam_pos(&self) -> BeamPos {
        BeamPos {
            frame: self.fields,
            line: self.raster as u16,
            cycle: (self.line_clock * SLOW_PER_LINE / LINE_CLOCKS) as u16,
            hpos: (self.line_clock * WIDTH / LINE_CLOCKS) as u16,
        }
    }

    /// Draw the two physical lines of VIC-II raster `raster`, which just
    /// ended.
    fn render_raster(&mut self, raster: u32) {
        let blink = self.fields & 16 == 0;
        for phys in [2 * raster, 2 * raster + 1] {
            let Some(y) = phys.checked_sub(u32::from(vic::FIRST_PHYSICAL)) else {
                continue;
            };
            if y < HEIGHT {
                let row = &mut self.fb[(y * WIDTH) as usize..((y + 1) * WIDTH) as usize];
                let (ss, sd) = render::line(&self.board, y as u16, blink, row);
                self.board.vic.collide(ss, sd);
            }
        }
    }

    /// Draw a whole field from the state now (after loading a state).
    fn render_field(&mut self) {
        for raster in 0..LINES {
            self.render_raster(raster);
        }
    }

    /// One master clock. Returns (the CPU ticked, its result, a field
    /// ended).
    #[inline]
    fn master(&mut self, dbg: Option<&mut DebugHooks>) -> (Option<CycleResult>, bool) {
        while let Some(ev) = self.input.pop_due(Cycle(self.clocks)) {
            self.apply_input(ev);
        }
        let mut field = false;
        self.line_clock += 1;
        if self.line_clock == LINE_CLOCKS {
            self.line_clock = 0;
            self.render_raster(self.raster);
            if self.ports.iter().any(Option::is_some) {
                self.poll_ports();
            }
            self.raster += 1;
            if self.raster == LINES {
                self.raster = 0;
                self.fields += 1;
                self.board.cia1.tod_tick();
                self.board.cia2.tod_tick();
                self.board.storage.update_activity();
                field = true;
            }
            self.board.vic.line(self.raster as u16);
        }
        self.phase_slow += SLOW_PER_LINE;
        let slow = self.phase_slow >= LINE_CLOCKS;
        if slow {
            self.phase_slow -= LINE_CLOCKS;
            self.board.cia1.tick();
            self.board.cia2.tick();
            self.board.clock_sids();
            self.serial_bus();
        }
        let step = |phase: &mut u32, per_line: u32| {
            *phase += per_line;
            let due = *phase >= LINE_CLOCKS;
            if due {
                *phase -= LINE_CLOCKS;
            }
            due
        };
        let c128 = step(&mut self.phase_c128, 2 * SLOW_PER_LINE);
        let c65 = step(&mut self.phase_c65, C65_PER_LINE);
        let run = match self.cpu.speed() {
            Speed::Mhz40 => true,
            Speed::Mhz3_5 => c65,
            Speed::Mhz2 => c128,
            Speed::Mhz1 => slow,
        };
        self.clocks += 1;
        if self.stall > 0 {
            self.stall -= 1;
            return (None, field);
        }
        if !run {
            return (None, field);
        }
        (Some(self.cpu_cycle(dbg)), field)
    }

    /// Master clocks until the next one that has more to do than a CPU
    /// cycle at full speed: a line ends, a 1 MHz cycle (CIAs, SIDs), the
    /// CPU's own 2 or 3.5 MHz cycle when it runs at that speed, an input
    /// event (at least 1).
    fn clocks_to_event(&self) -> u32 {
        let until = |phase: u32, per_line: u32| (LINE_CLOCKS - phase).div_ceil(per_line);
        let mut k = (LINE_CLOCKS - self.line_clock).min(until(self.phase_slow, SLOW_PER_LINE));
        match self.cpu.speed() {
            Speed::Mhz2 => k = k.min(until(self.phase_c128, 2 * SLOW_PER_LINE)),
            Speed::Mhz3_5 => k = k.min(until(self.phase_c65, C65_PER_LINE)),
            _ => {}
        }
        if let Some(at) = self.input.next_at() {
            k = k.min(
                at.0.saturating_sub(self.clocks)
                    .saturating_add(1)
                    .min(u64::from(u32::MAX)) as u32,
            );
        }
        k.max(1)
    }

    /// Up to `n` master clocks in which nothing but the CPU at full speed
    /// can happen (`n` < [`Mega65::clocks_to_event`]): the same as as many
    /// calls of [`Mega65::master`] without the debugger, at the cost of the
    /// CPU's cycles alone. Stops early when the CPU changes its speed (the
    /// caller plans again).
    fn quiet(&mut self, n: u32) {
        let mut used = 0;
        if self.cpu.speed() == Speed::Mhz40 {
            while used < n {
                let left = n - used;
                if self.stall > 0 {
                    let k = self.stall.min(left);
                    self.stall -= k;
                    used += k;
                } else if self.cpu.speed() != Speed::Mhz40 {
                    break;
                } else if self.cpu.idle() > 0 {
                    let k = self.cpu.idle().min(left);
                    self.cpu.skip(k);
                    used += k;
                } else {
                    self.cpu_cycle(None);
                    used += 1;
                }
            }
        } else {
            // A slower CPU runs only on its own cycles, which are events.
            used = n;
            self.stall = self.stall.saturating_sub(n);
        }
        self.clocks += u64::from(used);
        self.line_clock += used;
        self.phase_slow += used * SLOW_PER_LINE;
        // These two cross the line length inside the window when the CPU
        // does not use them; the remainder is the same as clock by clock.
        self.phase_c128 = (self.phase_c128 + used * 2 * SLOW_PER_LINE) % LINE_CLOCKS;
        self.phase_c65 = (self.phase_c65 + used * C65_PER_LINE) % LINE_CLOCKS;
    }

    /// One CPU cycle, with the DMA job it may start and a change of speed.
    #[inline]
    fn cpu_cycle(&mut self, dbg: Option<&mut DebugHooks>) -> CycleResult {
        if self.cpu.at_boundary() {
            // The lines matter only where an instruction may start.
            let b = &self.board;
            self.cpu.set_irq(b.vic.irq() || b.cia1.irq());
            self.cpu.set_nmi(b.cia2.irq() || b.keyboard.restore);
        }
        // The bus sees the hooks only while a watchpoint, freeze or trace
        // wants accesses.
        let hooks = dbg.filter(|d| {
            d.read_armed(CPU_SPACE)
                || d.read_armed(PHYS_SPACE)
                || d.write_armed(CPU_SPACE)
                || d.write_armed(PHYS_SPACE)
        });
        let beam = if hooks.is_some() {
            self.beam_pos()
        } else {
            BeamPos::default()
        };
        let result = {
            let mut bus = M65Bus {
                board: &mut self.board,
                dbg: hooks,
                beam,
            };
            self.cpu.tick(&mut bus)
        };
        if let Some(trigger) = self.board.dma.take() {
            // The CPU has made its accesses; the job runs before it goes on.
            let map = self.cpu.map;
            self.stall += dma::run(&mut self.board, trigger, &map, &mut self.cpu.pc);
        }
        if let Some((trap, _)) = self.board.trap.take() {
            // The service is chosen by A, whichever register was written.
            self.hypervisor(trap, self.cpu.a);
            self.stall += hyper::TRAP_CLOCKS;
        }
        if self.cpu.at_boundary() {
            let speed = self.board.speed();
            if speed != self.cpu.speed() {
                self.cpu.set_speed(speed);
            }
        }
        result
    }

    /// Run until the CPU ticked or a field ended; debugger checks.
    fn step(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        loop {
            let (result, field) = self.master(dbg.as_deref_mut());
            if let (Some(dbg), Some(CycleResult::InstrDone { next_pc })) =
                (dbg.as_deref_mut(), result)
            {
                if dbg.trace_instr_armed() {
                    dbg.trace_instr(CPU, next_pc, self.beam_pos());
                }
                if dbg.instr_step_armed() {
                    dbg.check_instr_step();
                }
                if dbg.exec_armed(CPU_SPACE) {
                    dbg.check_exec(CPU_SPACE, next_pc);
                }
                if let Some(stop) = dbg.take_stop() {
                    self.field_done_pending |= field;
                    return Some(stop);
                }
            }
            if field {
                return Some(StopReason::FrameDone);
            }
            if result.is_some() {
                return None;
            }
        }
    }

    fn apply_input(&mut self, ev: TimedInput) {
        match (ev.target, ev.event) {
            (InputTarget::Keyboard, InputEvent::Key { machine_key, down }) => {
                self.board.keyboard.key(machine_key.0, down);
            }
            (InputTarget::Panel, InputEvent::Control { id, value: 1 }) => match id.0 {
                0 => self.reset(ResetKind::Hard),
                1 => self.reset(ResetKind::Soft),
                _ => {}
            },
            _ => {}
        }
    }

    /// One 1 MHz cycle of the serial bus: the drive runs its cycles and
    /// leaves the bus levels the CIA reads.
    fn serial_bus(&mut self) {
        let host = self.board.iec_out();
        self.board.iec = match &mut self.drive {
            Some(d) => d.run(host, None).0,
            None => host,
        };
    }

    /// Take the control ports' lines as they are now (once a raster line;
    /// a proportional mouse steps its phases over a few hundred cycles).
    fn poll_ports(&mut self) {
        let now = Cycle(self.clocks);
        for i in 0..2 {
            let lines = self.ports[i].as_mut().map(|d| {
                d.write_lines(emu_core::port::PortLines::default(), now);
                d.read_lines(now)
            });
            self.board.port_lines[i] = lines.map_or(0, |l| l.pulled_low);
            self.board.port_analog[i] = lines.map_or([u16::MAX; 2], |l| l.analog);
        }
    }

    /// A byte as the CPU sees it now (tests and tools).
    pub fn peek_cpu(&self, addr: u16) -> u8 {
        self.board.peek(self.cpu.map.translate(addr))
    }

    /// The bytes of `len` screen codes at a CPU address, as text (tests).
    pub fn screen_text(&self, addr: u32, len: usize) -> String {
        (0..len)
            .map(|i| {
                let c = self.board.chip[(addr as usize + i) % board::CHIP_RAM] & 0x7f;
                match c {
                    0x00 => '@',
                    0x01..=0x1a => (b'A' + c - 1) as char,
                    0x20..=0x3f => c as char,
                    _ => ' ',
                }
            })
            .collect()
    }
}

const STATE_TAG: [u8; 4] = *b"M65S";
const STATE_VERSION: u16 = 1;

impl Machine for Mega65 {
    fn run_frame(&mut self, dbg: &mut DebugHooks) -> StopReason {
        if std::mem::take(&mut self.field_done_pending) {
            return StopReason::FrameDone;
        }
        // What the debugger wants, once a field: with nothing armed the
        // loop below is the bare machine (golden rule 5).
        let watch = dbg.read_armed(CPU_SPACE)
            || dbg.read_armed(PHYS_SPACE)
            || dbg.write_armed(CPU_SPACE)
            || dbg.write_armed(PHYS_SPACE);
        let per_instr =
            dbg.trace_instr_armed() || dbg.instr_step_armed() || dbg.exec_armed(CPU_SPACE);
        if !watch && !per_instr {
            loop {
                let n = self.clocks_to_event() - 1;
                self.quiet(n);
                if let (_, true) = self.master(None) {
                    return StopReason::FrameDone;
                }
            }
        }
        loop {
            if let Some(stop) = self.step(Some(dbg)) {
                return stop;
            }
        }
    }

    fn step_cycle(&mut self) -> StopReason {
        if let Some(StopReason::FrameDone) = self.step(None) {
            self.field_done_pending = true;
        }
        StopReason::StepDone
    }

    /// Hard: power on (RAM cleared, the ROM loaded again). Soft: the reset
    /// button, the CPU and the I/O registers.
    fn reset(&mut self, kind: ResetKind) {
        if let Some(d) = &mut self.drive {
            // The serial bus' RESET line (a power cycle on power-on).
            d.reset(kind == ResetKind::Hard);
        }
        if kind == ResetKind::Hard {
            self.power_on();
        } else {
            let b = &mut self.board;
            b.vic = vic::Vic::new();
            b.cia1.reset();
            b.cia2.reset();
            b.port_ddr = 0;
            b.port_data = 0;
            b.force_fast = false;
            b.remap();
            self.reset_cpu();
        }
        self.input.clear();
        self.field_done_pending = false;
    }

    fn framebuffer(&self) -> FrameRef<'_> {
        FrameRef {
            width: WIDTH,
            height: HEIGHT,
            pixels: PixelData::Rgb(&self.fb),
            luma_chroma: None,
            signal: SignalInfo {
                pixel_clock_hz: 27_000_000,
                active_pixels: WIDTH,
                total_pixels: 864,
                lines_per_field: 625,
                standard: VideoStandard::Pal,
                output: OutputType::Rgb,
                field: FieldParity::Progressive,
            },
        }
    }

    fn audio(&mut self, out: &mut Vec<i16>) -> emu_core::AudioFormat {
        out.append(&mut self.board.samples);
        emu_core::AudioFormat {
            sample_rate: emu_core::SAMPLE_RATE,
            channels: 2,
        }
    }

    fn beam(&self) -> Option<BeamPos> {
        Some(self.beam_pos())
    }

    fn memory_spaces(&self) -> Vec<MemSpace> {
        let mut spaces = vec![
            MemSpace {
                id: CPU_SPACE,
                name: "cpu",
                device: CPU,
                addr_bits: 16,
                big_endian: false,
            },
            MemSpace {
                id: PHYS_SPACE,
                name: "phys",
                device: CPU,
                addr_bits: 28,
                big_endian: false,
            },
        ];
        if self.drive.as_ref().is_some_and(|d| d.is_true()) {
            spaces.push(MemSpace {
                id: DRIVE_SPACE,
                name: "drive8",
                device: DRIVE8,
                addr_bits: 16,
                big_endian: false,
            });
        }
        spaces
    }

    fn chips(&self) -> Vec<ChipInfo> {
        let b = &self.board;
        let reg = |name, addr: u32, value: u32, bits| ChipReg {
            name,
            addr: Some(addr),
            value,
            bits,
        };
        const VIC: [(&str, u16); 14] = [
            ("CTRL1", 0x11),
            ("RASTER", 0x12),
            ("CTRL2", 0x16),
            ("MEMPTR", 0x18),
            ("IRQ", 0x19),
            ("IRQMASK", 0x1a),
            ("BORDER", 0x20),
            ("BACKGND", 0x21),
            ("KEY", 0x2f),
            ("CTRLA", 0x30),
            ("CTRLB", 0x31),
            ("CTRLC", 0x54),
            ("SCRNPTR0", 0x60),
            ("SCRNPTR1", 0x61),
        ];
        let p = vic::Personality::Mega65;
        let mut vic_regs: Vec<ChipReg> = VIC
            .iter()
            .map(|&(n, r)| reg(n, 0xd000 + u32::from(r), u32::from(b.vic.peek(p, r)), 8))
            .collect();
        vic_regs.push(ChipReg {
            name: "IOMODE",
            addr: None,
            value: u32::from(b.vic.personality.index()),
            bits: 2,
        });
        let cia = |name, base: u32, c: &chips_cbm::cia::Cia| ChipInfo {
            name,
            regs: (0..16u8)
                .map(|r| {
                    const N: [&str; 16] = [
                        "PRA", "PRB", "DDRA", "DDRB", "TALO", "TAHI", "TBLO", "TBHI", "TOD10",
                        "TODSEC", "TODMIN", "TODHR", "SDR", "ICR", "CRA", "CRB",
                    ];
                    reg(
                        N[usize::from(r)],
                        base + u32::from(r),
                        u32::from(c.peek(r, 0xff, 0xff)),
                        8,
                    )
                })
                .collect(),
        };
        let mut chips = vec![
            ChipInfo {
                name: "vic",
                regs: vic_regs,
            },
            cia("cia1", 0xdc00, &b.cia1),
            cia("cia2", 0xdd00, &b.cia2),
            ChipInfo {
                name: "cpuport",
                regs: vec![
                    reg("DDR", 0, u32::from(b.port_ddr), 8),
                    reg("DATA", 1, u32::from(b.port_data), 8),
                ],
            },
        ];
        const SID: [&str; 4] = ["sid_r1", "sid_r2", "sid_l1", "sid_l2"];
        const SID_REGS: [&str; 29] = [
            "V1FREQLO", "V1FREQHI", "V1PWLO", "V1PWHI", "V1CTRL", "V1AD", "V1SR", "V2FREQLO",
            "V2FREQHI", "V2PWLO", "V2PWHI", "V2CTRL", "V2AD", "V2SR", "V3FREQLO", "V3FREQHI",
            "V3PWLO", "V3PWHI", "V3CTRL", "V3AD", "V3SR", "FCLO", "FCHI", "RESFILT", "MODEVOL",
            "POTX", "POTY", "OSC3", "ENV3",
        ];
        for (i, s) in b.sids.iter().enumerate() {
            let base = 0xd400 + 0x20 * i as u32;
            chips.push(ChipInfo {
                name: SID[i],
                regs: SID_REGS
                    .iter()
                    .enumerate()
                    .map(|(r, &n)| reg(n, base + r as u32, u32::from(s.peek(r as u8)), 8))
                    .collect(),
            });
        }
        let st = &b.storage;
        const FDC: [&str; 10] = [
            "CONTROL", "COMMAND", "STATA", "STATB", "TRACK", "SECTOR", "SIDE", "DATA", "CLOCK",
            "STEP",
        ];
        chips.push(ChipInfo {
            name: "f011",
            regs: FDC
                .iter()
                .enumerate()
                // DATA would move the buffer pointer only through a read;
                // the peek leaves it.
                .map(|(r, &n)| reg(n, 0xd080 + r as u32, u32::from(st.fdc_peek(r as u16)), 8))
                .collect(),
        });
        chips.push(ChipInfo {
            name: "sdcard",
            regs: [
                ("STATUS", 0u16),
                ("ADDR0", 1),
                ("ADDR1", 2),
                ("ADDR2", 3),
                ("ADDR3", 4),
                ("FILL", 6),
                ("BUFSEL", 9),
            ]
            .iter()
            .map(|&(n, r)| reg(n, 0xd680 + u32::from(r), u32::from(st.sd_peek(r)), 8))
            .collect(),
        });
        let d = &b.devices;
        let dev = |n: &'static str, r: usize| reg(n, 0xd600 + r as u32, u32::from(d[r]), 8);
        chips.push(ChipInfo {
            name: "dmagic",
            regs: vec![
                dev("ADDRLSB", 0x100),
                dev("ADDRMSB", 0x101),
                dev("ADDRBANK", 0x102),
                dev("EN018B", 0x103),
                dev("ADDRMB", 0x104),
            ],
        });
        let word = |at: usize| u32::from_le_bytes([d[at], d[at + 1], d[at + 2], d[at + 3]]);
        chips.push(ChipInfo {
            name: "math",
            regs: vec![
                reg("MULTINA", 0xd770, word(0x170), 32),
                reg("MULTINB", 0xd774, word(0x174), 32),
                reg("MULTOUTLO", 0xd778, word(0x178), 32),
                reg("MULTOUTHI", 0xd77c, word(0x17c), 32),
                reg("DIVOUTFRAC", 0xd768, word(0x168), 32),
                reg("DIVOUTWHOLE", 0xd76c, word(0x16c), 32),
            ],
        });
        chips.push(ChipInfo {
            name: "keyboard",
            regs: (0..9u32)
                .map(|c| {
                    const N: [&str; 9] = [
                        "COL0", "COL1", "COL2", "COL3", "COL4", "COL5", "COL6", "COL7", "COL8",
                    ];
                    ChipReg {
                        name: N[c as usize],
                        addr: None,
                        value: u32::from(b.keyboard.column(c as u8)),
                        bits: 8,
                    }
                })
                .chain([reg("MODKEYS", 0xd611, u32::from(b.keyboard.mods()), 8)])
                .collect(),
        });
        chips.push(ChipInfo {
            name: "hypervisor",
            regs: vec![
                ChipReg {
                    name: "TRAPS",
                    addr: None,
                    value: b.traps,
                    bits: 32,
                },
                ChipReg {
                    name: "ERROR",
                    addr: None,
                    value: u32::from(self.hyper.error),
                    bits: 8,
                },
                ChipReg {
                    name: "ROMPROT",
                    addr: None,
                    value: u32::from(b.rom_protect),
                    bits: 1,
                },
            ],
        });
        chips
    }

    fn gfx_views(&self) -> Vec<emu_core::GfxViewInfo> {
        gfx::views()
    }

    /// The display area, character set, sprites and palette ([`gfx`]).
    fn gfx_view(&self, id: &str, opts: &emu_core::GfxOptions) -> Option<emu_core::GfxView> {
        gfx::view(&self.board, id, opts)
    }

    fn devices(&self) -> Vec<DeviceId> {
        let mut d = vec![CPU];
        if self.drive.as_ref().is_some_and(|d| d.is_true()) {
            d.push(DRIVE8);
        }
        d
    }

    fn debug_target(&mut self, dev: DeviceId) -> Option<&mut dyn DebugTarget> {
        if dev == CPU {
            return Some(self as &mut dyn DebugTarget);
        }
        if dev == DRIVE8 {
            return self.drive.as_mut().and_then(|d| d.debug_target());
        }
        None
    }

    fn input(&mut self, ev: TimedInput) {
        self.input.push(ev);
    }

    fn now(&self) -> Cycle {
        Cycle(self.clocks)
    }

    fn controls(&self) -> Vec<emu_core::ControlInfo> {
        vec![
            emu_core::ControlInfo::button(0, "POWER", false),
            emu_core::ControlInfo::button(1, "RESET", false),
        ]
    }

    fn ports(&self) -> Vec<PortInfo> {
        (0..2)
            .map(|i| PortInfo {
                id: PortId(i as u8),
                kind: emu_core::port::PortKind::CbmControl,
                name: ["control port 1", "control port 2"][i],
                connected: self.ports[i].is_some(),
            })
            .collect()
    }

    fn connect(&mut self, port: PortId, dev: Box<dyn PortDevice>) -> Result<(), PortError> {
        let slot = self
            .ports
            .get_mut(usize::from(port.0))
            .ok_or(PortError::NoSuchPort(port))?;
        if dev.kind() != emu_core::port::PortKind::CbmControl {
            return Err(PortError::Incompatible {
                port: emu_core::port::PortKind::CbmControl,
                device: dev.kind(),
            });
        }
        if slot.is_some() {
            return Err(PortError::Occupied(port));
        }
        *slot = Some(dev);
        self.poll_ports();
        Ok(())
    }

    fn disconnect(&mut self, port: PortId) -> Option<Box<dyn PortDevice>> {
        let dev = self.ports.get_mut(usize::from(port.0))?.take();
        self.poll_ports();
        dev
    }

    fn media_slots(&self) -> Vec<MediaSlotInfo> {
        let mut slots = Vec::new();
        if let Some(d) = &self.drive {
            slots.push(MediaSlotInfo {
                id: SLOT_DRIVE8,
                name: d.slot_name(),
                kind: emu_core::media::MediaKind::Disk,
                inserted: d.has_disk(),
            });
        }
        let st = &self.board.storage;
        for (id, name, inserted) in [
            (
                SLOT_FLOPPY0,
                "drive 0 (internal)",
                st.drives[0].status().track.is_some(),
            ),
            (
                SLOT_FLOPPY1,
                "drive 1",
                st.drives[1].status().track.is_some(),
            ),
        ] {
            slots.push(MediaSlotInfo {
                id,
                name,
                kind: emu_core::media::MediaKind::Disk,
                inserted,
            });
        }
        slots.push(MediaSlotInfo {
            id: SLOT_SDCARD,
            name: "SD card",
            kind: emu_core::media::MediaKind::Disk,
            inserted: st.sd_inserted(),
        });
        slots
    }

    fn media_slot(&mut self, slot: SlotId) -> Option<&mut dyn MediaSlot> {
        match slot {
            SLOT_FLOPPY0 => Some(&mut self.board.storage.drives[0] as &mut dyn MediaSlot),
            SLOT_FLOPPY1 => Some(&mut self.board.storage.drives[1] as &mut dyn MediaSlot),
            SLOT_SDCARD => Some(&mut self.board.storage.sd as &mut dyn MediaSlot),
            SLOT_DRIVE8 => self.drive.as_mut().map(|d| d.media_slot()),
            _ => None,
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(STATE_TAG, STATE_VERSION, |w| {
            w.u64(self.clocks);
            w.u32(self.line_clock);
            w.u32(self.raster);
            w.u32(self.phase_slow);
            w.u32(self.phase_c128);
            w.u32(self.phase_c65);
            w.u64(self.fields);
            w.u32(self.stall);
            w.bool(self.hyper.transfer.is_some());
            w.u8(self.hyper.transfer.unwrap_or(0));
            w.u8(self.hyper.error);
            self.board.save(w);
            self.input.save(w);
        });
        self.cpu.save(&mut w);
        if let Some(d) = &self.drive {
            d.save(&mut w);
        }
        w.into_bytes()
    }

    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(STATE_TAG, STATE_VERSION)?;
        let clocks = c.u64()?;
        let line_clock = c.u32()?;
        let raster = c.u32()?;
        let phase_slow = c.u32()?;
        let phase_c128 = c.u32()?;
        let phase_c65 = c.u32()?;
        let fields = c.u64()?;
        let stall = c.u32()?;
        let transfer = c.bool()?;
        let transfer_page = c.u8()?;
        let hyper_error = c.u8()?;
        if line_clock >= LINE_CLOCKS
            || raster >= LINES
            || phase_slow >= LINE_CLOCKS
            || phase_c128 >= LINE_CLOCKS
            || phase_c65 >= LINE_CLOCKS
        {
            return Err(StateError::Invalid("MEGA65 timing"));
        }
        // The media stay outside the copy the state loads into.
        let media = self.board.storage.take_media();
        let mut board = self.board.clone();
        if let Err(e) = board.load(&mut c) {
            self.board.storage.put_media(media);
            return Err(e);
        }
        board.storage.put_media(media);
        let mut input = InputQueue::default();
        input.load(&mut c)?;
        let mut cpu = Cpu45gs02::new();
        cpu.load(&mut r)?;
        if let Some(d) = &mut self.drive {
            d.load(&mut r)?;
        }
        self.clocks = clocks;
        self.line_clock = line_clock;
        self.raster = raster;
        self.phase_slow = phase_slow;
        self.phase_c128 = phase_c128;
        self.phase_c65 = phase_c65;
        self.fields = fields;
        self.stall = stall;
        self.hyper = hyper::Hyper {
            transfer: transfer.then_some(transfer_page),
            error: hyper_error,
        };
        self.board = board;
        self.input = input;
        self.cpu = cpu;
        self.field_done_pending = false;
        self.render_field();
        Ok(())
    }
}

impl DebugTarget for Mega65 {
    fn regs(&self) -> RegisterSet {
        self.cpu.regs()
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        self.cpu.set_reg(name, v);
    }

    fn peek(&self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
        let byte = |a: u32| -> Option<u32> {
            match space {
                CPU_SPACE => Some(u32::from(self.peek_cpu(a as u16))),
                PHYS_SPACE => Some(u32::from(self.board.peek(a & ADDR_MASK))),
                _ => None,
            }
        };
        match size {
            Size::Byte => byte(addr),
            Size::Word => Some(byte(addr)? | byte(addr.wrapping_add(1))? << 8),
            Size::Long => Some(
                byte(addr)?
                    | byte(addr.wrapping_add(1))? << 8
                    | byte(addr.wrapping_add(2))? << 16
                    | byte(addr.wrapping_add(3))? << 24,
            ),
        }
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        let n = match size {
            Size::Byte => 1,
            Size::Word => 2,
            Size::Long => 4,
        };
        for i in 0..n {
            let byte = (v >> (8 * i)) as u8;
            let a = addr.wrapping_add(i);
            let bus = match space {
                CPU_SPACE => self.cpu.map.translate(a as u16),
                PHYS_SPACE => a & ADDR_MASK,
                _ => return false,
            };
            self.board.write(bus, byte);
        }
        true
    }

    fn disassemble(&self, addr: u32) -> Instr {
        self.cpu.disassemble(&PeekBus(&self.board), addr)
    }
}
