//! IMSAI 8080 (1975): an Intel 8080A at 2 MHz on the S-100 bus, memory
//! cards, serial cards and the front panel. The machine has no screen: its
//! picture ([`Machine::framebuffer`]) is the front panel with the lamps
//! and paddle switches ([`panel`]); clicking a paddle in the picture works
//! it (the host pointer comes as `InputEvent::LightPen` to port 0). The
//! serial terminal is the second output, "terminal" ([`terminal`]; F12 in
//! the desktop), and the host keyboard types on it.
//!
//! **Memory.** RAM from 0 up to option `ram` (default 64 KB), reads above
//! it give $FF. 1 KB blocks can be protected (the RAM cards' protection,
//! which blocks writes from the CPU and from DMA alike): the controls
//! PROTECT and UNPROTECT (in the panel strip, not paddles in the picture:
//! the IMSAI's panel has none) set it for the block the address switches
//! A10-A15 select, as the Altair's paddles of that name do.
//!
//! **I/O.** Port $FF: IN reads the left eight switches (sense), OUT sets
//! PROGRAMMED OUTPUT, whose lamps show the inverse. The console and the
//! tape reader answer on the usual card addresses, so software for either
//! layout runs:
//!
//! | Ports | Card | Connected to |
//! | --- | --- | --- |
//! | $02 data, $03 status | IMSAI SIO-2 channel A (8251: bit 0 TxRDY, 1 RxRDY, 2 TxE) | terminal |
//! | $04, $05 | SIO-2 channel B | tape reader |
//! | $00 status, $01 data | MITS 88-SIO (bit 0 low: byte waiting, bit 7 low: ready to send) | terminal |
//! | $06, $07 | MITS 88-ACR, as the 88-SIO | tape reader |
//! | $10 status, $11 data | MITS 88-2SIO port A (6850) | terminal |
//! | $12, $13 | 88-2SIO port B | tape reader and punch |
//! | $FD | IMSAI FIF floppy disk interface ([`fif`]) | drives A:-D: |
//!
//! The reader cards' channels also punch: a byte written to their data
//! port goes to the paper tape punch (slot "punch"). A byte takes 10 bit
//! times at option `baud` (default 9600); on the SIO-2 the 8251's mode word
//! sets the character's length instead (`usart`: after a reset the 8251
//! waits for its mode and command words, with transmitter and receiver
//! off, as on the card). With option `reader=console` the tape plays into
//! the terminal's line instead, as on a Teletype's reader. The 6850's
//! receive interrupt pulls INT (the CPU fetches RST 7 from the idle bus).
//!
//! **Disks.** The FIF with four 8-inch drives (slots "A:"-"D:", raw IBM
//! 3740 images of 256 256 bytes, as z80pack's IMSAI CP/M disks). At
//! power-up and RESET the FIF puts its bootstrap into memory when drive A
//! holds a disk, so RESET and RUN boot it (a disk inserted before the
//! first frame counts as in the drive at power-up; with `autostart=on`
//! the machine then runs). An OUT to the FIF while it works holds the CPU
//! in WAIT until it takes the byte.
//!
//! **Front panel** ([`Imsai::controls`], ids in [`ctl`]). As the 8080
//! has no direct memory access for the panel, EXAMINE sets PC and the
//! lamps show the fetch the CPU stands at. A stopped CPU sits in the WAIT
//! state of that fetch: MEMR, M1 and /WO lit. EXAMINE, EXAMINE NEXT,
//! DEPOSIT, DEPOSIT NEXT work only while stopped; DEPOSIT writes the low
//! eight switches. RESET sets PC to 0 (and a running program goes on
//! there). SINGLE STEP runs one instruction. PWR OFF darkens the machine,
//! PWR ON powers it up again with RAM cleared. Bus lamps are lit by the
//! share of the frame the line was high ([`panel`]).
//!
//! **MPU-A / MPU-B ROM** (optional; the machine needs no ROM). Option
//! `mpu=a` or `b` fits the 2 KB monitor ROM (roles "mpu-a", "mpu-b"): it
//! appears at $D800 and, after power-on and RESET, also at 0, so the CPU
//! starts in it. OUT $F3: bit 6 removes the copy at 0 (at the next read
//! from $D800-$DFFF, or at once with bit 7), bit 7 the one at $D800. With a ROM fitted, $12/$13 are the MPU's 8251 (nothing connected)
//! and the parallel port $14/$15 reports nothing, instead of the 88-2SIO's
//! port B. The MPU-B also has an 8253 timer at $D100-$D103 (memory-mapped,
//! 2 MHz, `pit`), the clock of its 8251, there while the ROM's upper copy
//! is. As elsewhere RUN starts the
//! CPU, `autostart=on` at power-on. The mapping is read from Bruce Hall's
//! commented disassemblies of these ROMs (z80pack's images); the MPU-B
//! boots a disk in drive A through the FIF, else it asks for the space bar
//! on the console.
//!
//! Media: slot "program" (a binary loaded to option `load`, default 0, or
//! Intel HEX by its addresses), slot "tape" (paper tape image for the
//! reader), slot "punch" (the punched bytes go into this file from its
//! start, until its end: a blank reel of the size wanted; "rewind" starts
//! over), slots "A:"-"D:" (disks), slot "cards" (a card deck for the
//! card reader, [`cardreader`]: option `cardreader=text` plays it into the
//! reader line after the tape, `raw` hands its columns out at ports
//! $20-$22). Option `autostart=on` runs a loaded
//! program. Loaded programs return after a power cycle.
//!
//! Sources: Thomas Fischer's edition of the IMSAI 8080 System manual,
//! "General Assembly and Test Instructions" (panel use, status after
//! RESET, test programs); "The IMSAI 8080 front panel",
//! RetroComputerInstructionManual (programmed output inverse at port 255,
//! sense switches on IN FFh); the photograph named in `docs/sources.md`
//! (layout); Intel 8251A and Motorola MC6850 data sheets; MITS 88-SIO
//! documentation; Bruce Hall's listings at github.com/bhall66/IMSAI-8080
//! (console ports, MPU ROMs). No ROM is needed. The implementation is this
//! project's own.

#![forbid(unsafe_code)]

pub mod cardreader;
pub mod fif;
pub mod panel;
mod pit;
pub mod terminal;
mod usart;

use std::collections::VecDeque;

use cardreader::CardReader;
use cpu_8080::{status, StatusBus, I8080, IO_BASE};
use emu_core::frame::{FrameRef, PixelData, ScreenId, ScreenInfo};
use emu_core::input::{InputEvent, InputQueue, InputTarget, TimedInput};
use emu_core::machine::ChipReg;
use emu_core::media::{Media, MediaError, MediaKind, MediaSlot, MediaSlotInfo, SlotId};
use emu_core::port::{PortDevice, PortError, PortId, PortInfo};
use emu_core::registry::{ConfigError, MachineConfig, MachineEntry};
use emu_core::{
    BeamPos, Bus, ChipInfo, ControlInfo, Cpu, Cycle, DebugHooks, DebugTarget, DeviceId, Instr,
    Machine, MemSpace, MemSpaceId, RegisterSet, ResetKind, Savestate, Size, StateError,
    StateReader, StateWriter, StopReason,
};
use fif::{Disk, Fif};
use panel::{Hit, Lamps, Paddle, Panel};
use pit::Pit;
use terminal::Terminal;
use usart::Usart;

pub const CPU: DeviceId = DeviceId("cpu");
/// The 8080's view: memory at 0–$FFFF, I/O ports at `IO_BASE + port`.
pub const CPU_SPACE: MemSpaceId = MemSpaceId(0);
pub const SLOT_PROGRAM: SlotId = SlotId(0);
pub const SLOT_TAPE: SlotId = SlotId(1);
pub const SLOT_PUNCH: SlotId = SlotId(2);
/// Drives A:-D: of the FIF.
pub const SLOT_DISKS: [SlotId; fif::DRIVES] = [SlotId(3), SlotId(4), SlotId(5), SlotId(6)];
const DISK_NAMES: [&str; fif::DRIVES] = ["A:", "B:", "C:", "D:"];
/// The card reader ([`cardreader`]).
pub const SLOT_CARDS: SlotId = SlotId(7);
pub const CPU_HZ: u32 = 2_000_000;
/// Fields a second of the panel picture: nothing in the machine sets this.
pub const FPS: u32 = 60;
const FRAME: u64 = (CPU_HZ / FPS) as u64;
pub const TERMINAL_SCREEN: ScreenId = ScreenId(1);

/// Panel control ids.
pub mod ctl {
    /// PWR ON/OFF (a switch).
    pub const POWER: u16 = 0;
    /// Address/data switches: id `SWITCH + bit`.
    pub const SWITCH: u16 = 1;
    pub const EXAMINE: u16 = 20;
    pub const EXAMINE_NEXT: u16 = 21;
    pub const DEPOSIT: u16 = 22;
    pub const DEPOSIT_NEXT: u16 = 23;
    pub const RESET: u16 = 24;
    pub const EXT_CLR: u16 = 25;
    pub const RUN: u16 = 26;
    pub const STOP: u16 = 27;
    pub const STEP: u16 = 28;
    /// Memory protection of the block the switches A10-A15 select.
    pub const PROTECT: u16 = 29;
    pub const UNPROTECT: u16 = 30;
}

/// Machine keys on the terminal: what a key sends unshifted (the Apple II
/// host keymap's codes), and the modifiers.
pub mod keys {
    pub const SHIFT: u16 = 0x100;
    pub const CTRL: u16 = 0x101;
    pub const CAPS: u16 = 0x102;
}

const SWITCH_NAMES: [&str; 16] = [
    "A0", "A1", "A2", "A3", "A4", "A5", "A6", "A7", "A8", "A9", "A10", "A11", "A12", "A13", "A14",
    "A15",
];

pub fn entry() -> MachineEntry {
    MachineEntry {
        id: "imsai",
        name: "IMSAI 8080",
        models: &["8080"],
        video_formats: &[],
        create: |config| Ok(Box::new(Imsai::new(config)?)),
    }
}

/// Time each bus line was high in the frame, as tables over the byte (a
/// table costs one add per machine cycle, whatever the pattern).
#[derive(Clone, Debug)]
struct BusTime {
    addr_lo: [u32; 256],
    addr_hi: [u32; 256],
    data: [u32; 256],
    status: [u32; 256],
    total: u32,
    wait: u32,
}

impl BusTime {
    fn new() -> Self {
        Self {
            addr_lo: [0; 256],
            addr_hi: [0; 256],
            data: [0; 256],
            status: [0; 256],
            total: 0,
            wait: 0,
        }
    }

    fn add(&mut self, st: u8, addr: u16, data: u8, t: u32) {
        self.addr_lo[usize::from(addr as u8)] += t;
        self.addr_hi[usize::from(addr >> 8)] += t;
        self.data[usize::from(data)] += t;
        self.status[usize::from(st)] += t;
        self.total += t;
    }

    /// Brightness of bit `bit` from a table.
    fn level(&self, table: &[u32; 256], bit: u8) -> u8 {
        let on: u64 = table
            .iter()
            .enumerate()
            .filter(|(v, _)| v >> bit & 1 != 0)
            .map(|(_, &t)| u64::from(t))
            .sum();
        duty(on, u64::from(self.total))
    }
}

/// Lamp level 0–15 for `on` of `total`; the eye sees short pulses brighter
/// than their share.
fn duty(on: u64, total: u64) -> u8 {
    if on == 0 || total == 0 {
        return 0;
    }
    let share = on as f64 / total as f64;
    (share.sqrt() * 15.0).round().clamp(1.0, 15.0) as u8
}

/// A serial line's receive side.
#[derive(Clone, Debug, Default)]
struct Line {
    /// The byte waiting in the receive register.
    data: Option<u8>,
    /// What a read returns with nothing waiting.
    last: u8,
    /// No new byte before this clock.
    next: u64,
}

impl Line {
    fn take(&mut self) -> u8 {
        if let Some(b) = self.data.take() {
            self.last = b;
        }
        self.last
    }

    fn peek(&self) -> u8 {
        self.data.unwrap_or(self.last)
    }

    fn save(&self, w: &mut StateWriter) {
        w.bool(self.data.is_some());
        w.u8(self.data.unwrap_or(0));
        w.u8(self.last);
        w.u64(self.next);
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let some = r.bool()?;
        let b = r.u8()?;
        self.data = some.then_some(b);
        self.last = r.u8()?;
        self.next = r.u64()?;
        Ok(())
    }
}

/// Everything on the bus except the CPU and the tape.
#[derive(Clone, Debug)]
struct Board {
    mem: Vec<u8>,
    ram_size: u32,
    /// 1 KB blocks protected, one bit each.
    protect: u64,
    /// Clocks since power-on.
    t: u64,
    switches: u16,
    output: u8,
    /// The bus as last driven: the panel's lamps while stopped.
    latch: (u8, u16, u8),
    time: Box<BusTime>,
    terminal: Terminal,
    /// Keys typed, waiting for the line.
    typed: VecDeque<u8>,
    console: Line,
    reader: Line,
    /// The console's transmitter is busy until this clock.
    tx_busy: u64,
    /// 6850 control registers, ports A and B.
    acia: [u8; 2],
    char_clocks: u64,
    reader_console: bool,
    /// The MPU's monitor ROM, 2 KB.
    rom: Option<Vec<u8>>,
    /// Its copies at 0 and at $D800 are mapped in.
    rom_low: bool,
    rom_high: bool,
    /// OUT $F3 bit 6 was written: the copy at 0 goes at the next read from
    /// the ROM's own range ([S] a jump-start latch: the listings' start-up
    /// code fetches its JP from the copy at 0 right after that OUT).
    rom_low_leaving: bool,
    /// The MPU-B's 8253 at $D100 (with `mpu=b`).
    timer: bool,
    pit: Pit,
    /// The 8251s: SIO-2 channel A (console), B (tape), the MPU's.
    usart: [Usart; 3],
    /// The punch is busy until this clock.
    punch_busy: u64,
    /// The MPU's 8251 sends until this clock.
    mpu_tx_busy: u64,
    /// Bytes for the punch, taken by the machine into the punch slot.
    punched: Vec<u8>,
    fif: Fif,
    /// The CPU waits in an OUT to the FIF until this clock.
    stall_until: u64,
}

/// The MPU-B's 8253: $D100-$D103.
const PIT_BASE: u16 = 0xd100;

impl Board {
    fn protected(&self, addr: u16) -> bool {
        self.protect >> (addr >> 10) & 1 != 0
    }

    fn rom_at(&self, addr: u16) -> Option<u8> {
        let rom = self.rom.as_ref()?;
        let at = match addr {
            0xd800..=0xdfff if self.rom_high => addr - 0xd800,
            0x0000..=0x07ff if self.rom_low => addr,
            _ => return None,
        };
        rom.get(usize::from(at)).copied()
    }

    fn mem_read(&self, addr: u16) -> u8 {
        if self.pit_at(addr) {
            return self.pit.peek((addr - PIT_BASE) as u8, self.t);
        }
        if let Some(v) = self.rom_at(addr) {
            v
        } else if u32::from(addr) < self.ram_size {
            self.mem[usize::from(addr)]
        } else {
            0xff
        }
    }

    /// The 8253 answers while the MPU-B's upper ROM copy is mapped in
    /// ([S]: the listings do not say how the card decodes it; with OUT $F3
    /// bit 7 the page is RAM again, as a 54K CP/M with its BIOS up to $D7FF
    /// needs, which the MPU-B boots after removing its ROM).
    fn pit_at(&self, addr: u16) -> bool {
        self.timer && self.rom_high && (PIT_BASE..PIT_BASE + 4).contains(&addr)
    }

    /// A CPU read of memory (the 8253's counters have read effects).
    fn mem_read_cpu(&mut self, addr: u16) -> u8 {
        if self.rom_low_leaving && (0xd800..=0xdfff).contains(&addr) {
            self.rom_low_leaving = false;
            self.rom_low = false;
        }
        if self.pit_at(addr) {
            return self.pit.read((addr - PIT_BASE) as u8, self.t);
        }
        self.mem_read(addr)
    }

    fn mem_write(&mut self, addr: u16, v: u8) {
        if self.pit_at(addr) {
            self.pit.write((addr - PIT_BASE) as u8, v, self.t);
            return;
        }
        let rom = self.rom.is_some() && self.rom_high && (0xd800..=0xdfff).contains(&addr);
        if !rom && u32::from(addr) < self.ram_size && !self.protected(addr) {
            self.mem[usize::from(addr)] = v;
        }
    }

    fn send(&mut self, v: u8, clocks: u64) {
        self.terminal.put(v);
        self.tx_busy = self.t + clocks;
    }

    fn tx_ready(&self) -> bool {
        self.t >= self.tx_busy
    }

    fn punch_ready(&self) -> bool {
        self.t >= self.punch_busy
    }

    fn punch(&mut self, v: u8, clocks: u64) {
        self.punched.push(v);
        self.punch_busy = self.t + clocks;
    }

    /// The MPU 8251's character time: its clock is the 8253's counter 0
    /// (without the timer, or with it stopped, option `baud`'s).
    fn mpu_clocks(&self) -> u64 {
        let u = &self.usart[2];
        match self.pit.period0() {
            Some(p) if self.timer => {
                u64::from(p) * u64::from(u.factor()) * u64::from(u.half_bits()) / 2
            }
            _ => self.char_clocks,
        }
    }

    /// An 8251's character time at the line rate of option `baud`.
    fn usart_clocks(&self, n: usize) -> u64 {
        self.char_clocks * u64::from(self.usart[n].half_bits()) / 20
    }

    /// SIO-2 channel `n`'s 8251 status: TxRDY and TxE with the buffer
    /// empty, RxRDY with a byte waiting and the receiver on, DSR.
    fn sio2_status(&self, n: usize, line: &Line, ready: bool) -> u8 {
        let u = &self.usart[n];
        let tx = ready && !u.holding();
        0x80 | u8::from(tx)
            | u8::from(line.data.is_some() && u.rx_enabled()) << 1
            | u8::from(tx) << 2
    }

    /// The 88-SIO status: active low.
    fn sio_status(&self, line: &Line, tx_ready: bool) -> u8 {
        0x7e | u8::from(line.data.is_none()) | u8::from(!tx_ready) << 7
    }

    /// The 6850 status: bit 0 RDRF, 1 TDRE, 7 IRQ.
    fn acia_status(&self, line: &Line, tx_ready: bool, control: u8) -> u8 {
        let rx = line.data.is_some();
        u8::from(rx) | u8::from(tx_ready) << 1 | u8::from(rx && control & 0x80 != 0) << 7
    }

    /// The I/O ports' read value, without its side effects.
    fn io_peek(&self, port: u8) -> u8 {
        let tx = self.tx_ready();
        match port {
            // The MPU's own 8251 and parallel port: nothing connected.
            // TxRDY and TxE once its character time (the 8253's counter 0
            // as its clock) is over.
            0x13 if self.rom.is_some() => {
                let tx = self.t >= self.mpu_tx_busy && !self.usart[2].holding();
                0x80 | u8::from(tx) | u8::from(tx) << 2
            }
            0x12 | 0x14 | 0x15 if self.rom.is_some() => 0x00,
            0x00 => self.sio_status(&self.console, tx),
            0x06 => self.sio_status(&self.reader, self.punch_ready()),
            0x03 => self.sio2_status(0, &self.console, tx),
            0x05 => self.sio2_status(1, &self.reader, self.punch_ready()),
            0x10 => self.acia_status(&self.console, tx, self.acia[0]),
            0x12 => self.acia_status(&self.reader, self.punch_ready(), self.acia[1]),
            0x02 => self.console.peek() & self.usart[0].mask(),
            0x04 => self.reader.peek() & self.usart[1].mask(),
            0x01 | 0x11 => self.console.peek(),
            0x07 | 0x13 => self.reader.peek(),
            0xff => (self.switches >> 8) as u8,
            _ => 0xff,
        }
    }

    fn io_read(&mut self, port: u8) -> u8 {
        match port {
            0x12..=0x15 if self.rom.is_some() => self.io_peek(port),
            0x02 => self.console.take() & self.usart[0].mask(),
            0x04 => self.reader.take() & self.usart[1].mask(),
            0x01 | 0x11 => self.console.take(),
            0x07 | 0x13 => self.reader.take(),
            _ => self.io_peek(port),
        }
    }

    fn io_write(&mut self, port: u8, v: u8) {
        match port {
            // The MPU's 8251: set up, sends into nothing.
            0x13 if self.rom.is_some() => self.usart[2].control(v),
            0x12 if self.rom.is_some() => {
                if self.usart[2].transmit(v).is_some() {
                    self.mpu_tx_busy = self.t + self.mpu_clocks();
                }
            }
            0xf3 => {
                self.rom_low_leaving |= self.rom_low && v & 0x40 != 0;
                self.rom_high &= v & 0x80 == 0;
                // Both copies off at once (the MPU-B's boot, OUT $C0): the
                // CPU goes on from RAM, so the copy at 0 goes now.
                if v & 0xc0 == 0xc0 {
                    self.rom_low = false;
                    self.rom_low_leaving = false;
                }
            }
            fif::PORT => {
                let go_on = self.fif.out(v, self.t);
                self.stall_until = self.stall_until.max(go_on);
            }
            0x03 => {
                self.usart[0].control(v);
                if let Some(b) = self.usart[0].release() {
                    let c = self.usart_clocks(0);
                    self.send(b, c);
                }
            }
            0x05 => {
                self.usart[1].control(v);
                if let Some(b) = self.usart[1].release() {
                    let c = self.usart_clocks(1);
                    self.punch(b, c);
                }
            }
            0x02 => {
                if let Some(b) = self.usart[0].transmit(v) {
                    let c = self.usart_clocks(0);
                    self.send(b, c);
                }
            }
            0x04 => {
                if let Some(b) = self.usart[1].transmit(v) {
                    let c = self.usart_clocks(1);
                    self.punch(b, c);
                }
            }
            0x07 | 0x13 => self.punch(v, self.char_clocks),
            0x01 | 0x11 => self.send(v, self.char_clocks),
            0x10 | 0x12 => {
                let n = usize::from(port == 0x12);
                self.acia[n] = v;
                if v & 3 == 3 {
                    // Master reset.
                    self.acia[n] = 0;
                    if n == 0 {
                        self.console.data = None;
                    } else {
                        self.reader.data = None;
                    }
                }
            }
            0xff => self.output = v,
            _ => {}
        }
    }
}

impl fif::Dma for Board {
    fn dma_read(&self, addr: u16) -> u8 {
        self.mem_read(addr)
    }

    fn dma_write(&mut self, addr: u16, v: u8) {
        self.mem_write(addr, v);
    }
}

/// The 8080's bus for one instruction.
struct ImsaiBus<'a> {
    b: &'a mut Board,
    cards: &'a mut CardReader,
    dbg: Option<&'a mut DebugHooks>,
    beam: BeamPos,
}

impl Bus for ImsaiBus<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        let v = if addr >= IO_BASE && self.cards.decodes(addr as u8) {
            u32::from(self.cards.read(addr as u8, self.b.t))
        } else if addr >= IO_BASE {
            u32::from(self.b.io_read(addr as u8))
        } else {
            u32::from(self.b.mem_read_cpu(addr as u16))
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
        if addr >= IO_BASE && self.cards.decodes(addr as u8) {
            self.cards.write(addr as u8, v as u8, self.b.t);
        } else if addr >= IO_BASE {
            self.b.io_write(addr as u8, v as u8);
        } else {
            self.b.mem_write(addr as u16, v as u8);
        }
    }

    fn addr_bits(&self) -> u8 {
        17
    }
}

impl StatusBus for ImsaiBus<'_> {
    fn cycle(&mut self, st: u8, addr: u16, data: u8, t: u32) {
        let b = &mut *self.b;
        b.t += u64::from(t);
        b.latch = (st, addr, data);
        b.time.add(st, addr, data, t);
        if st & status::HLTA != 0 {
            b.time.wait += t;
        }
    }
}

fn peek(b: &Board, addr: u32) -> u8 {
    if addr >= IO_BASE {
        b.io_peek(addr as u8)
    } else {
        b.mem_read(addr as u16)
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

/// A program in memory: blocks by address.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Image {
    blocks: Vec<(u16, Vec<u8>)>,
    start: u16,
}

/// Intel HEX records, or `None` if `bytes` is not such a file.
fn parse_hex(bytes: &[u8]) -> Option<Image> {
    if bytes.first() != Some(&b':') {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let mut image = Image::default();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let hex = line.strip_prefix(':')?;
        if hex.len() % 2 != 0 || !hex.is_ascii() {
            return None;
        }
        let rec: Vec<u8> = (0..hex.len() / 2)
            .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok())
            .collect::<Option<_>>()?;
        let len = usize::from(*rec.first()?);
        if rec.len() != len + 5 || rec.iter().fold(0u8, |a, b| a.wrapping_add(*b)) != 0 {
            return None;
        }
        let addr = u16::from_be_bytes([rec[1], rec[2]]);
        match rec[3] {
            0 => {
                if image.blocks.is_empty() {
                    image.start = addr;
                }
                image.blocks.push((addr, rec[4..4 + len].to_vec()));
            }
            1 => break,
            _ => {}
        }
    }
    Some(image)
}

/// The program slot: a file's bytes go into memory at the next instruction.
struct ProgramSlot {
    load: u16,
    media: Option<Media>,
    image: Image,
    pending: bool,
}

impl MediaSlot for ProgramSlot {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Program {
            return Err(MediaError::Unsupported(media.kind));
        }
        let mut bytes = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut bytes)?;
        self.image = parse_hex(&bytes).unwrap_or_else(|| Image {
            start: self.load,
            blocks: vec![(self.load, bytes)],
        });
        self.pending = true;
        self.media = Some(media);
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        self.pending = false;
        self.image = Image::default();
        self.media.take()
    }

    fn write_protect(&mut self, _on: bool) {}

    fn flush(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

/// The paper tape reader.
#[derive(Default)]
struct TapeSlot {
    media: Option<Media>,
    bytes: Vec<u8>,
    pos: usize,
}

impl MediaSlot for TapeSlot {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Tape {
            return Err(MediaError::Unsupported(media.kind));
        }
        let mut bytes = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut bytes)?;
        self.bytes = bytes;
        self.pos = 0;
        self.media = Some(media);
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        self.bytes.clear();
        self.pos = 0;
        self.media.take()
    }

    fn write_protect(&mut self, _on: bool) {}

    fn flush(&mut self) -> Result<(), MediaError> {
        Ok(())
    }

    fn control(&mut self, command: &str) -> Result<(), MediaError> {
        match command {
            "rewind" => {
                self.pos = 0;
                Ok(())
            }
            other => Err(MediaError::Control(format!("'{other}': only rewind"))),
        }
    }
}

/// The paper tape punch: bytes go into the file from its start.
#[derive(Default)]
struct PunchSlot {
    media: Option<Media>,
    pos: u64,
}

impl PunchSlot {
    fn punch(&mut self, bytes: &[u8]) {
        let Some(m) = &mut self.media else {
            return;
        };
        for &b in bytes {
            // Past the reel's end the bytes are lost.
            if self.pos < m.backend.len() && m.backend.write(self.pos, &[b]).is_ok() {
                self.pos += 1;
            }
        }
    }
}

impl MediaSlot for PunchSlot {
    fn insert(&mut self, media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Tape {
            return Err(MediaError::Unsupported(media.kind));
        }
        self.media = Some(media);
        self.pos = 0;
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        self.pos = 0;
        self.media.take()
    }

    fn write_protect(&mut self, _on: bool) {}

    fn flush(&mut self) -> Result<(), MediaError> {
        match &mut self.media {
            Some(m) => m.backend.commit(),
            None => Ok(()),
        }
    }

    fn dirty(&self) -> bool {
        self.media.as_ref().is_some_and(|m| m.backend.dirty())
    }

    fn control(&mut self, command: &str) -> Result<(), MediaError> {
        match command {
            "rewind" => {
                self.pos = 0;
                Ok(())
            }
            other => Err(MediaError::Control(format!("'{other}': only rewind"))),
        }
    }
}

/// A drive of the FIF as a media slot: the disk and what the drive shows.
struct DiskSlot {
    disk: Disk,
    shown: emu_core::media::DriveStatus,
}

impl MediaSlot for DiskSlot {
    fn insert(&mut self, media: Media) -> Result<(), MediaError> {
        self.disk.insert(media)
    }

    fn eject(&mut self) -> Option<Media> {
        self.disk.eject()
    }

    fn write_protect(&mut self, on: bool) {
        self.disk.write_protect(on);
    }

    fn flush(&mut self) -> Result<(), MediaError> {
        self.disk.flush()
    }

    fn dirty(&self) -> bool {
        self.disk.dirty()
    }

    fn status(&self) -> emu_core::media::DriveStatus {
        emu_core::media::DriveStatus {
            error: self.disk.write_error(),
            ..self.shown
        }
    }
}

pub struct Imsai {
    cpu: I8080,
    board: Board,
    program: ProgramSlot,
    tape: TapeSlot,
    punch: PunchSlot,
    disks: [DiskSlot; fif::DRIVES],
    cards: CardReader,
    /// A disk was in drive A at power-up (or before the first frame).
    booted: bool,
    input: InputQueue,
    power: bool,
    /// The panel's RUN flip-flop.
    running: bool,
    /// Clock of the next frame end.
    frame_end: u64,
    frames: u64,
    /// Buttons held, by control id.
    held: u64,
    /// The pointer's button, and the control a click holds.
    pointer_down: bool,
    pointer_held: Option<u16>,
    shift: bool,
    ctrl: bool,
    upper: bool,
    autostart: bool,
    panel: Panel,
    /// The lamps as last drawn (derived, not state).
    shown: Lamps,
}

fn parse_ram(s: &str) -> Option<u32> {
    let kb: u32 = s.strip_suffix(['k', 'K'])?.parse().ok()?;
    (1..=64).contains(&kb).then_some(kb * 1024)
}

impl Imsai {
    pub fn new(config: &MachineConfig) -> Result<Self, ConfigError> {
        match config.model.as_deref() {
            None | Some("8080") => {}
            Some(other) => return Err(ConfigError::UnknownModel(other.into())),
        }
        let get = |k: &str| config.options.get(k).map(String::as_str);
        let ram_size = match get("ram") {
            None => 0x1_0000,
            Some(s) => parse_ram(s)
                .ok_or_else(|| ConfigError::Invalid(format!("ram option '{s}' (1k…64k)")))?,
        };
        let baud: u32 = match get("baud") {
            None => 9600,
            Some(s) => s
                .parse()
                .ok()
                .filter(|b| (50..=115_200).contains(b))
                .ok_or_else(|| ConfigError::Invalid(format!("baud option '{s}'")))?,
        };
        let load = match get("load") {
            None => 0,
            Some(s) => u16::from_str_radix(s.trim_start_matches("0x"), 16)
                .map_err(|_| ConfigError::Invalid(format!("load option '{s}' (hex)")))?,
        };
        let reader_console = match get("reader") {
            None | Some("card") => false,
            Some("console") => true,
            Some(s) => {
                return Err(ConfigError::Invalid(format!(
                    "reader option '{s}' (card or console)"
                )))
            }
        };
        let rom = match get("mpu") {
            None | Some("none") => None,
            Some(s @ ("a" | "b")) => {
                let role = format!("mpu-{s}");
                let r = config
                    .roms
                    .get(&role)
                    .ok_or_else(|| ConfigError::MissingRom(role.clone()))?;
                if r.len() != 0x800 {
                    return Err(ConfigError::Invalid(format!(
                        "{role} ROM must be 2048 bytes, got {}",
                        r.len()
                    )));
                }
                Some(r.clone())
            }
            Some(s) => {
                return Err(ConfigError::Invalid(format!(
                    "mpu option '{s}' (a, b or none)"
                )))
            }
        };
        let autostart = get("autostart") == Some("on");
        let card_raw = match get("cardreader") {
            None | Some("text") => false,
            Some("raw") => true,
            Some(s) => {
                return Err(ConfigError::Invalid(format!(
                    "cardreader option '{s}' (text or raw)"
                )))
            }
        };
        let boot = autostart && rom.is_some();
        let mut m = Self {
            cpu: I8080::new(),
            board: Board {
                mem: vec![0; 0x1_0000],
                ram_size,
                protect: 0,
                t: 0,
                switches: 0,
                output: 0,
                latch: (status::FETCH, 0, 0),
                time: Box::new(BusTime::new()),
                terminal: Terminal::new(),
                typed: VecDeque::new(),
                console: Line::default(),
                reader: Line::default(),
                tx_busy: 0,
                acia: [0; 2],
                char_clocks: u64::from(CPU_HZ) * 10 / u64::from(baud),
                reader_console,
                timer: get("mpu") == Some("b"),
                rom,
                rom_low: true,
                rom_high: true,
                rom_low_leaving: false,
                pit: Pit::new(),
                usart: std::array::from_fn(|_| Usart::new()),
                punch_busy: 0,
                mpu_tx_busy: 0,
                punched: Vec::new(),
                fif: Fif::new(),
                stall_until: 0,
            },
            program: ProgramSlot {
                load,
                media: None,
                image: Image::default(),
                pending: false,
            },
            tape: TapeSlot::default(),
            punch: PunchSlot::default(),
            disks: std::array::from_fn(|_| DiskSlot {
                disk: Disk::default(),
                shown: Default::default(),
            }),
            cards: CardReader::new(card_raw),
            booted: false,
            input: InputQueue::new(),
            power: true,
            running: boot,
            frame_end: FRAME,
            frames: 0,
            held: 0,
            pointer_down: false,
            pointer_held: None,
            shift: false,
            ctrl: false,
            upper: get("uppercase") != Some("off"),
            autostart,
            panel: Panel::new(),
            shown: Lamps::default(),
        };
        m.show_fetch();
        m.render();
        Ok(m)
    }

    /// The lamps as last drawn (tests and tools).
    pub fn shown_lamps(&self) -> Lamps {
        self.shown
    }

    /// The terminal's text (tests and tools).
    pub fn terminal(&self) -> &Terminal {
        &self.board.terminal
    }

    /// RAM as the CPU sees it (tests and tools).
    pub fn peek_mem(&self, addr: u16) -> u8 {
        self.board.mem_read(addr)
    }

    pub fn running(&self) -> bool {
        self.running
    }

    pub fn pc(&self) -> u16 {
        self.cpu.pc
    }

    /// A key typed on the terminal, for tests: as the terminal sends it.
    pub fn type_byte(&mut self, b: u8) {
        self.board.typed.push_back(b);
    }

    /// The bus stands at the fetch from PC, in WAIT: what the panel shows
    /// after a stop, RESET or EXAMINE.
    fn show_fetch(&mut self) {
        let pc = self.cpu.pc;
        let st = if self.cpu.halted {
            status::HALT
        } else {
            status::FETCH
        };
        self.board.latch = (st, pc, self.board.mem_read(pc));
    }

    fn lamps(&self) -> Lamps {
        let b = &self.board;
        if !self.power {
            return Lamps::default();
        }
        let time = &b.time;
        let mut l = Lamps::default();
        // A frame just begun, or a stop before any bus activity: the latch.
        let (st, addr, data) = b.latch;
        let live = time.total > 0;
        for i in 0..8u8 {
            l.output[usize::from(i)] = if b.output >> (7 - i) & 1 == 0 { 15 } else { 0 };
            let bit = 7 - i;
            l.status[usize::from(i)] = if live {
                time.level(&time.status, bit)
            } else {
                (st >> bit & 1) * 15
            };
            l.data[usize::from(i)] = if live {
                time.level(&time.data, bit)
            } else {
                (data >> bit & 1) * 15
            };
            l.address[usize::from(i)] = if live {
                time.level(&time.addr_hi, bit)
            } else {
                ((addr >> (8 + bit)) & 1) as u8 * 15
            };
            l.address[usize::from(i) + 8] = if live {
                time.level(&time.addr_lo, bit)
            } else {
                ((addr >> bit) & 1) as u8 * 15
            };
        }
        // MEMR INP M1 OUT HLTA STACK /WO INTA is bits 7…0: as ordered.
        let wait = if live {
            duty(u64::from(time.wait), u64::from(time.total))
        } else if self.running && !self.cpu.halted {
            0
        } else {
            15
        };
        l.run = [
            if self.cpu.inte { 15 } else { 0 },
            if self.running { 15 } else { 0 },
            wait,
            0,
        ];
        l
    }

    fn controls_shown(&self) -> [Paddle; 6] {
        let pos = |up: u16, down: u16| {
            if self.held >> up & 1 != 0 {
                Paddle::Up
            } else if self.held >> down & 1 != 0 {
                Paddle::Down
            } else {
                Paddle::Middle
            }
        };
        [
            pos(ctl::EXAMINE, ctl::EXAMINE_NEXT),
            pos(ctl::DEPOSIT, ctl::DEPOSIT_NEXT),
            pos(ctl::RESET, ctl::EXT_CLR),
            pos(ctl::RUN, ctl::STOP),
            pos(ctl::STEP, ctl::STEP),
            if self.power { Paddle::Up } else { Paddle::Down },
        ]
    }

    /// Bring the panel picture and the terminal up to date.
    fn render(&mut self) {
        let lamps = self.lamps();
        self.shown = lamps;
        let controls = self.controls_shown();
        self.panel.draw_lamps(&lamps);
        self.panel.draw_switches(self.board.switches, &controls);
        // About 1.5 blinks a second, from the machine's clock.
        let cursor = self.power && self.board.t >> 19 & 1 == 0;
        self.board.terminal.render(cursor);
    }

    fn power_up(&mut self) {
        self.cpu = I8080::new();
        self.board.mem.fill(0);
        self.board.protect = 0;
        self.board.output = 0;
        self.board.terminal = Terminal::new();
        self.board.typed.clear();
        self.board.console = Line::default();
        self.board.reader = Line::default();
        self.board.acia = [0; 2];
        self.tape.pos = 0;
        self.board.rom_low = true;
        self.board.rom_high = true;
        self.board.rom_low_leaving = false;
        self.board.pit = Pit::new();
        self.board.usart = std::array::from_fn(|_| Usart::new());
        self.board.punch_busy = 0;
        self.board.mpu_tx_busy = 0;
        self.board.punched.clear();
        self.board.stall_until = 0;
        self.running = self.autostart && self.board.rom.is_some();
        self.program.pending = !self.program.image.blocks.is_empty();
        self.fif_restart();
        self.show_fetch();
    }

    /// The FIF's IFM restarts (power-up, RESET): its bootstrap goes into
    /// memory when drive A holds a disk.
    fn fif_restart(&mut self) {
        self.board.fif.reset();
        self.board.stall_until = 0;
        self.booted = self.disks[0].disk.media.is_some();
        if self.booted {
            Fif::bootstrap(&mut self.board);
        }
    }

    /// Let the FIF work up to now and show the drives' state.
    fn fif_run(&mut self) {
        let mut fif = std::mem::take(&mut self.board.fif);
        let mut disks: [Disk; fif::DRIVES] =
            std::array::from_fn(|d| std::mem::take(&mut self.disks[d].disk));
        fif.run(self.board.t, &mut self.board, &mut disks);
        for (d, disk) in disks.into_iter().enumerate() {
            self.disks[d].disk = disk;
            self.disks[d].shown = fif.drive_status(d);
        }
        self.board.fif = fif;
    }

    /// One press of a control (rising edge).
    fn press(&mut self, id: u16) {
        if !self.power {
            return;
        }
        let stopped = !self.running;
        match id {
            ctl::RUN => self.running = true,
            ctl::STOP => {
                self.running = false;
                self.show_fetch();
            }
            ctl::STEP if stopped => {
                self.instruction(None);
                self.show_fetch();
            }
            ctl::EXAMINE if stopped => {
                self.cpu.pc = self.board.switches;
                self.cpu.halted = false;
                self.show_fetch();
            }
            ctl::EXAMINE_NEXT if stopped => {
                self.cpu.pc = self.cpu.pc.wrapping_add(1);
                self.show_fetch();
            }
            ctl::DEPOSIT | ctl::DEPOSIT_NEXT if stopped => {
                if id == ctl::DEPOSIT_NEXT {
                    self.cpu.pc = self.cpu.pc.wrapping_add(1);
                }
                self.board.mem_write(self.cpu.pc, self.board.switches as u8);
                self.show_fetch();
            }
            ctl::RESET => {
                self.cpu.reset();
                self.board.rom_low = true;
                self.board.rom_high = true;
                self.board.rom_low_leaving = false;
                self.fif_restart();
                self.show_fetch();
            }
            ctl::EXT_CLR => {
                self.board.acia = [0; 2];
                self.board.usart = std::array::from_fn(|_| Usart::new());
                self.board.console = Line::default();
                self.board.reader = Line::default();
            }
            ctl::PROTECT | ctl::UNPROTECT => {
                let block = self.board.switches >> 10;
                if id == ctl::PROTECT {
                    self.board.protect |= 1 << block;
                } else {
                    self.board.protect &= !(1 << block);
                }
            }
            _ => {}
        }
    }

    /// Set a control to a value (panel event or a click on the picture).
    fn set_control(&mut self, id: u16, value: u8) {
        match id {
            ctl::POWER => {
                let on = value != 0;
                if on && !self.power {
                    self.power_up();
                }
                self.power = on;
                if !on {
                    self.running = false;
                }
            }
            1..=16 => {
                let bit = 1u16 << (id - ctl::SWITCH);
                if value != 0 {
                    self.board.switches |= bit;
                } else {
                    self.board.switches &= !bit;
                }
            }
            ctl::EXAMINE..=ctl::UNPROTECT => {
                let mask = 1u64 << id;
                if value != 0 {
                    if self.held & mask == 0 {
                        self.press(id);
                    }
                    self.held |= mask;
                } else {
                    self.held &= !mask;
                }
            }
            _ => {}
        }
    }

    /// The host pointer over the picture.
    fn pointer(&mut self, x: u16, y: u16, trigger: bool) {
        let was = std::mem::replace(&mut self.pointer_down, trigger);
        if trigger && !was {
            match panel::hit(x, y) {
                Some(Hit::Sense(bit)) => {
                    let up = self.board.switches >> bit & 1 != 0;
                    self.set_control(ctl::SWITCH + u16::from(bit), u8::from(!up));
                }
                Some(Hit::Control(5, _)) => self.set_control(ctl::POWER, u8::from(!self.power)),
                Some(Hit::Control(n, upper)) => {
                    let id = match (n, upper) {
                        (0, true) => ctl::EXAMINE,
                        (0, false) => ctl::EXAMINE_NEXT,
                        (1, true) => ctl::DEPOSIT,
                        (1, false) => ctl::DEPOSIT_NEXT,
                        (2, true) => ctl::RESET,
                        (2, false) => ctl::EXT_CLR,
                        (3, true) => ctl::RUN,
                        (3, false) => ctl::STOP,
                        _ => ctl::STEP,
                    };
                    self.set_control(id, 1);
                    self.pointer_held = Some(id);
                }
                None => {}
            }
        } else if !trigger {
            if let Some(id) = self.pointer_held.take() {
                self.set_control(id, 0);
            }
        }
    }

    /// A key on the terminal.
    fn key(&mut self, key: u16, down: bool) {
        match key {
            keys::SHIFT => self.shift = down,
            keys::CTRL => self.ctrl = down,
            keys::CAPS if down => self.upper = !self.upper,
            k @ 0..=0x7f if down => {
                let b = self.ascii(k as u8);
                self.board.typed.push_back(b);
            }
            _ => {}
        }
    }

    fn ascii(&self, k: u8) -> u8 {
        let shifted = |k: u8| match k {
            b'a'..=b'z' => k - 0x20,
            b'1' => b'!',
            b'2' => b'@',
            b'3' => b'#',
            b'4' => b'$',
            b'5' => b'%',
            b'6' => b'^',
            b'7' => b'&',
            b'8' => b'*',
            b'9' => b'(',
            b'0' => b')',
            b'-' => b'_',
            b'=' => b'+',
            b'[' => b'{',
            b']' => b'}',
            b'\\' => b'|',
            b';' => b':',
            b'\'' => b'"',
            b',' => b'<',
            b'.' => b'>',
            b'/' => b'?',
            b'`' => b'~',
            other => other,
        };
        if self.ctrl {
            return shifted(k) & 0x1f;
        }
        if self.shift {
            shifted(k)
        } else if self.upper {
            k.to_ascii_uppercase()
        } else {
            k
        }
    }

    fn apply_input(&mut self, ev: TimedInput) {
        match (ev.target, ev.event) {
            (InputTarget::Panel, InputEvent::Control { id, value }) => {
                self.set_control(id.0, value);
            }
            (InputTarget::Keyboard, InputEvent::Key { machine_key, down }) => {
                self.key(machine_key.0, down);
            }
            (InputTarget::Port(_), InputEvent::LightPen { x, y, trigger }) => {
                self.pointer(x, y, trigger);
            }
            _ => {}
        }
    }

    /// Input, loading and the serial lines: between instructions.
    fn service(&mut self) {
        while let Some(ev) = self.input.pop_due(Cycle(self.board.t)) {
            self.apply_input(ev);
        }
        // A disk put into drive A before the first frame was there at
        // power-up.
        if !self.booted && self.frames == 0 && self.power && self.disks[0].disk.media.is_some() {
            self.fif_restart();
            if self.autostart && self.board.rom.is_none() {
                self.cpu.pc = 0;
                self.running = true;
            }
            self.show_fetch();
        }
        if !self.board.punched.is_empty() {
            let bytes = std::mem::take(&mut self.board.punched);
            self.punch.punch(&bytes);
        }
        if self.board.fif.pending() {
            self.fif_run();
        }
        if self.program.pending && self.power {
            self.program.pending = false;
            for (addr, data) in &self.program.image.blocks {
                for (i, &byte) in data.iter().enumerate() {
                    self.board.mem_write(addr.wrapping_add(i as u16), byte);
                }
            }
            if self.autostart {
                self.cpu.pc = self.program.image.start;
                self.running = true;
            }
            self.show_fetch();
        }
        let b = &mut self.board;
        let char_clocks = b.char_clocks;
        if b.console.data.is_none() && b.t >= b.console.next {
            let mut byte = b.typed.pop_front();
            if byte.is_none() && b.reader_console {
                byte = self.tape.bytes.get(self.tape.pos).copied();
                self.tape.pos += usize::from(byte.is_some());
                if byte.is_none() {
                    byte = self.cards.text_byte();
                }
            }
            if byte.is_some() {
                b.console.data = byte;
                b.console.next = b.t + char_clocks;
            }
        }
        if !b.reader_console && b.reader.data.is_none() && b.t >= b.reader.next {
            let tape = self.tape.bytes.get(self.tape.pos).copied();
            self.tape.pos += usize::from(tape.is_some());
            // The tape first, then the card reader's deck.
            if let Some(byte) = tape.or_else(|| self.cards.text_byte()) {
                b.reader.data = Some(byte);
                b.reader.next = b.t + char_clocks;
            }
        }
        let irq = (b.console.data.is_some() && b.acia[0] & 0x80 != 0)
            || (b.reader.data.is_some() && b.acia[1] & 0x80 != 0);
        self.cpu.set_int(irq);
    }

    /// One instruction. Returns `Some` if emulation must stop.
    fn instruction(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        let beam = self.beam_pos();
        {
            let mut bus = ImsaiBus {
                b: &mut self.board,
                cards: &mut self.cards,
                dbg: dbg.as_deref_mut(),
                beam,
            };
            self.cpu.step(&mut bus);
        }
        let dbg = dbg?;
        let next_pc = u32::from(self.cpu.pc);
        if dbg.trace_instr_armed() {
            dbg.trace_instr(CPU, next_pc, beam);
        }
        if dbg.instr_step_armed() {
            dbg.check_instr_step();
        }
        if dbg.exec_armed(CPU_SPACE) {
            dbg.check_exec(CPU_SPACE, next_pc);
        }
        dbg.take_stop()
    }

    fn beam_pos(&self) -> BeamPos {
        BeamPos {
            frame: self.frames,
            ..BeamPos::default()
        }
    }

    /// The CPU waits in an OUT to the FIF (READY low): the bus stands still
    /// until the FIF takes the byte or the frame ends.
    fn wait_stall(&mut self) {
        let b = &mut self.board;
        let t = b.stall_until.min(self.frame_end).saturating_sub(b.t).max(1);
        let (st, addr, data) = b.latch;
        b.t += t;
        b.time.add(st, addr, data, t as u32);
        b.time.wait += t as u32;
    }

    /// The rest of the frame with the CPU stopped: the bus stands still.
    fn stand_still(&mut self) {
        let b = &mut self.board;
        let t = self.frame_end.saturating_sub(b.t);
        let (st, addr, data) = b.latch;
        b.t += t;
        if self.power {
            b.time.add(st, addr, data, t as u32);
            b.time.wait += t as u32;
        }
    }

    fn end_frame(&mut self) {
        self.render();
        *self.board.time = BusTime::new();
        self.frame_end += FRAME;
        self.frames += 1;
    }
}

impl Machine for Imsai {
    fn run_frame(&mut self, dbg: &mut DebugHooks) -> StopReason {
        for f in dbg.per_frame_freezes() {
            if f.space == CPU_SPACE && f.addr < IO_BASE {
                for i in 0..f.size.bytes() {
                    self.board
                        .mem_write((f.addr + i) as u16, (f.value >> (8 * i)) as u8);
                }
            }
        }
        loop {
            self.service();
            if self.board.t >= self.frame_end {
                self.end_frame();
                return StopReason::FrameDone;
            }
            if self.power && self.board.t < self.board.stall_until {
                self.wait_stall();
            } else if self.power && (self.running || dbg.instr_step_armed()) {
                if let Some(stop) = self.instruction(Some(dbg)) {
                    self.show_fetch();
                    self.render();
                    return stop;
                }
            } else {
                self.stand_still();
            }
        }
    }

    /// One instruction, running or not (the debugger's step).
    fn step_cycle(&mut self) -> StopReason {
        self.service();
        if self.power && self.board.t < self.board.stall_until {
            self.wait_stall();
        } else if self.power {
            self.instruction(None);
            self.show_fetch();
        } else {
            self.board.t += 1;
        }
        self.render();
        StopReason::StepDone
    }

    /// Hard: power cycle. Soft: the RESET paddle.
    fn reset(&mut self, kind: ResetKind) {
        match kind {
            ResetKind::Hard => {
                self.power_up();
                self.power = true;
            }
            ResetKind::Soft => {
                self.cpu.reset();
                self.board.rom_low = true;
                self.board.rom_high = true;
                self.board.rom_low_leaving = false;
                self.fif_restart();
            }
        }
        self.input.clear();
        self.show_fetch();
    }

    fn framebuffer(&self) -> FrameRef<'_> {
        FrameRef {
            width: panel::WIDTH,
            height: panel::HEIGHT,
            pixels: PixelData::Indexed {
                pixels: self.panel.pixels(),
                palette: &panel::PALETTE,
            },
            luma_chroma: None,
            signal: panel::signal(FPS),
        }
    }

    fn screens(&self) -> Vec<ScreenInfo> {
        vec![
            ScreenInfo {
                id: ScreenId::MAIN,
                name: "panel",
                active: true,
            },
            ScreenInfo {
                id: TERMINAL_SCREEN,
                name: "terminal",
                active: false,
            },
        ]
    }

    fn screen(&self, id: ScreenId) -> Option<FrameRef<'_>> {
        if id == ScreenId::MAIN {
            return Some(self.framebuffer());
        }
        (id == TERMINAL_SCREEN).then(|| FrameRef {
            width: terminal::WIDTH,
            height: terminal::HEIGHT,
            pixels: PixelData::Indexed {
                pixels: self.board.terminal.pixels(),
                palette: &terminal::PALETTE,
            },
            luma_chroma: None,
            signal: terminal::signal(FPS),
        })
    }

    fn audio(&mut self, _out: &mut Vec<i16>) -> emu_core::AudioFormat {
        emu_core::AudioFormat {
            sample_rate: emu_core::SAMPLE_RATE,
            channels: 1,
        }
    }

    fn beam(&self) -> Option<BeamPos> {
        None
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
        let b = &self.board;
        let reg = |name, value: u32, bits| ChipReg {
            name,
            addr: None,
            value,
            bits,
        };
        let port = |name, port: u8, value: u32| ChipReg {
            name,
            addr: Some(IO_BASE + u32::from(port)),
            value,
            bits: 8,
        };
        vec![
            ChipInfo {
                name: "panel",
                regs: vec![
                    reg("SWITCHES", u32::from(b.switches), 16),
                    port("OUTPUT", 0xff, u32::from(b.output)),
                    reg("ADDRESS", u32::from(b.latch.1), 16),
                    reg("DATA", u32::from(b.latch.2), 8),
                    reg("STATUS", u32::from(b.latch.0), 8),
                    reg("RUN", u32::from(self.running), 1),
                    reg("POWER", u32::from(self.power), 1),
                ],
            },
            ChipInfo {
                name: "serial",
                regs: vec![
                    port("SIO2A", 0x03, u32::from(b.io_peek(0x03))),
                    port("SIO2B", 0x05, u32::from(b.io_peek(0x05))),
                    port("88SIO", 0x00, u32::from(b.io_peek(0x00))),
                    reg("TAPEPOS", self.tape.pos as u32, 32),
                    reg("PUNCHPOS", self.punch.pos as u32, 32),
                ],
            },
            ChipInfo {
                name: "memory",
                regs: vec![
                    reg("PROTECT", b.protect as u32, 32),
                    reg("PROTECTHI", (b.protect >> 32) as u32, 32),
                ],
            },
            ChipInfo {
                name: "fif",
                regs: (0..fif::DRIVES)
                    .map(|d| {
                        let s = b.fif.drive_status(d);
                        reg(
                            ["TRACKA", "TRACKB", "TRACKC", "TRACKD"][d],
                            u32::from(s.track.unwrap_or(1) - 1),
                            8,
                        )
                    })
                    .chain([reg("STALL", u32::from(b.t < b.stall_until), 1)])
                    .collect(),
            },
            ChipInfo {
                name: "8253",
                regs: if b.timer {
                    vec![
                        reg("OUT0", u32::from(b.pit.out(0, b.t)), 1),
                        reg("OUT1", u32::from(b.pit.out(1, b.t)), 1),
                        reg("OUT2", u32::from(b.pit.out(2, b.t)), 1),
                        reg("PERIOD0", b.pit.period0().unwrap_or(0), 17),
                    ]
                } else {
                    Vec::new()
                },
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
        Cycle(self.board.t)
    }

    fn controls(&self) -> Vec<ControlInfo> {
        const UP_DOWN: &[&str] = &["DOWN", "UP"];
        const OFF_ON: &[&str] = &["OFF", "ON"];
        let mut c = vec![ControlInfo::switch(
            ctl::POWER,
            "PWR",
            OFF_ON,
            u8::from(self.power),
        )];
        for bit in (0..16u16).rev() {
            c.push(ControlInfo::switch(
                ctl::SWITCH + bit,
                SWITCH_NAMES[usize::from(bit)],
                UP_DOWN,
                (self.board.switches >> bit & 1) as u8,
            ));
        }
        let buttons = [
            (ctl::EXAMINE, "EXAMINE"),
            (ctl::EXAMINE_NEXT, "EXAMINE NEXT"),
            (ctl::DEPOSIT, "DEPOSIT"),
            (ctl::DEPOSIT_NEXT, "DEPOSIT NEXT"),
            (ctl::RESET, "RESET"),
            (ctl::EXT_CLR, "EXT CLR"),
            (ctl::RUN, "RUN"),
            (ctl::STOP, "STOP"),
            (ctl::STEP, "SINGLE STEP"),
        ];
        for (id, name) in buttons {
            c.push(ControlInfo::button(id, name, self.held >> id & 1 != 0));
        }
        // All of them are paddles in the picture; PROTECT and UNPROTECT
        // are not (the IMSAI's panel has no such paddles).
        let mut c: Vec<ControlInfo> = c.into_iter().map(ControlInfo::shown_on_picture).collect();
        for (id, name) in [(ctl::PROTECT, "PROTECT"), (ctl::UNPROTECT, "UNPROTECT")] {
            c.push(ControlInfo::button(id, name, self.held >> id & 1 != 0));
        }
        c
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
        vec![
            MediaSlotInfo {
                id: SLOT_PROGRAM,
                name: "program",
                kind: MediaKind::Program,
                inserted: self.program.media.is_some(),
            },
            MediaSlotInfo {
                id: SLOT_TAPE,
                name: "tape",
                kind: MediaKind::Tape,
                inserted: self.tape.media.is_some(),
            },
            MediaSlotInfo {
                id: SLOT_PUNCH,
                name: "punch",
                kind: MediaKind::Tape,
                inserted: self.punch.media.is_some(),
            },
        ]
        .into_iter()
        .chain((0..fif::DRIVES).map(|d| MediaSlotInfo {
            id: SLOT_DISKS[d],
            name: DISK_NAMES[d],
            kind: MediaKind::Disk,
            inserted: self.disks[d].disk.media.is_some(),
        }))
        .chain([MediaSlotInfo {
            id: SLOT_CARDS,
            name: "cards",
            kind: MediaKind::Cards,
            inserted: self.cards.media.is_some(),
        }])
        .collect()
    }

    fn media_slot(&mut self, slot: SlotId) -> Option<&mut dyn MediaSlot> {
        match slot {
            SLOT_PROGRAM => Some(&mut self.program),
            SLOT_TAPE => Some(&mut self.tape),
            SLOT_PUNCH => Some(&mut self.punch),
            SlotId(n @ 3..=6) => Some(&mut self.disks[usize::from(n) - 3]),
            SLOT_CARDS => Some(&mut self.cards),
            _ => None,
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        let b = &self.board;
        // Version 2: the 8251s, the 8253, the punch, the FIF.
        w.chunk(*b"IMSA", 3, |w| {
            w.bytes(&b.mem);
            w.u32(b.ram_size);
            w.u64(b.protect);
            w.u64(b.t);
            w.u16(b.switches);
            w.u8(b.output);
            w.u8(b.latch.0);
            w.u16(b.latch.1);
            w.u8(b.latch.2);
            b.terminal.save(w);
            w.bytes(&b.typed.iter().copied().collect::<Vec<u8>>());
            b.console.save(w);
            b.reader.save(w);
            w.u64(b.tx_busy);
            w.u8(b.acia[0]);
            w.u8(b.acia[1]);
            w.bool(b.rom_low);
            w.bool(b.rom_high);
            w.u32(self.tape.pos as u32);
            w.bool(self.power);
            w.bool(self.running);
            w.u64(self.frame_end);
            w.u64(self.frames);
            w.u64(self.held);
            w.bool(self.pointer_down);
            w.u16(self.pointer_held.unwrap_or(0xffff));
            w.bool(self.shift);
            w.bool(self.ctrl);
            w.bool(self.upper);
            for u in &b.usart {
                u.save(w);
            }
            b.pit.save(w);
            w.u64(b.punch_busy);
            w.u64(b.mpu_tx_busy);
            w.bytes(&b.punched);
            w.u64(self.punch.pos);
            b.fif.save(w);
            w.u64(b.stall_until);
            w.bool(self.booted);
            w.bool(b.rom_low_leaving);
            self.cards.save(w);
        });
        self.cpu.save(&mut w);
        self.input.save(&mut w);
        w.into_bytes()
    }

    /// Atomic: everything loads into copies first.
    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (version, mut c) = r.chunk(*b"IMSA", 3)?;
        let mut b = self.board.clone();
        let mem = c.bytes()?;
        if mem.len() != b.mem.len() {
            return Err(StateError::Invalid("memory size"));
        }
        b.mem = mem.to_vec();
        if c.u32()? != b.ram_size {
            return Err(StateError::Invalid("RAM size differs"));
        }
        b.protect = c.u64()?;
        b.t = c.u64()?;
        b.switches = c.u16()?;
        b.output = c.u8()?;
        b.latch = (c.u8()?, c.u16()?, c.u8()?);
        b.terminal.load(&mut c)?;
        b.typed = c.bytes()?.iter().copied().collect();
        b.console.load(&mut c)?;
        b.reader.load(&mut c)?;
        b.tx_busy = c.u64()?;
        b.acia = [c.u8()?, c.u8()?];
        b.rom_low = c.bool()?;
        b.rom_high = c.bool()?;
        let tape_pos = c.u32()? as usize;
        let power = c.bool()?;
        let running = c.bool()?;
        let frame_end = c.u64()?;
        let frames = c.u64()?;
        let held = c.u64()?;
        let pointer_down = c.bool()?;
        let pointer_held = c.u16()?;
        let shift = c.bool()?;
        let ctrl = c.bool()?;
        let upper = c.bool()?;
        let mut punch_pos = self.punch.pos;
        let mut card_pos = None;
        let mut booted = self.booted;
        if version >= 2 {
            for u in &mut b.usart {
                u.load(&mut c)?;
            }
            b.pit.load(&mut c)?;
            b.punch_busy = c.u64()?;
            b.mpu_tx_busy = c.u64()?;
            b.punched = c.bytes()?.to_vec();
            punch_pos = c.u64()?;
            b.fif.load(&mut c)?;
            b.stall_until = c.u64()?;
            booted = c.bool()?;
            b.rom_low_leaving = c.bool()?;
            if version >= 3 {
                card_pos = Some(cardreader::Position::load(&mut c)?);
            }
        } else {
            // Version 1 had the 8251s working without set-up and no FIF.
            b.usart =
                std::array::from_fn(|_| Usart::with(0x4e, usart::cmd::TXEN | usart::cmd::RXE));
            b.fif = Fif::new();
            b.stall_until = 0;
        }
        if frame_end < b.t.saturating_sub(FRAME) || frame_end > b.t + 2 * FRAME {
            return Err(StateError::Invalid("frame position"));
        }
        let mut cpu = self.cpu.clone();
        let mut input = self.input.clone();
        cpu.load(&mut r)?;
        input.load(&mut r)?;
        *b.time = BusTime::new();
        self.cpu = cpu;
        self.input = input;
        self.board = b;
        self.tape.pos = tape_pos.min(self.tape.bytes.len());
        self.punch.pos = punch_pos;
        if let Some(p) = card_pos {
            self.cards.set_position(p);
        }
        self.booted = booted;
        self.power = power;
        self.running = running;
        self.frame_end = frame_end;
        self.frames = frames;
        self.held = held;
        self.pointer_down = pointer_down;
        self.pointer_held = (pointer_held != 0xffff).then_some(pointer_held);
        self.shift = shift;
        self.ctrl = ctrl;
        self.upper = upper;
        self.render();
        Ok(())
    }
}

impl DebugTarget for Imsai {
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
        Some((0..size.bytes()).fold(0, |v, i| {
            let a = addr.wrapping_add(i);
            let byte = if a >= IO_BASE && self.cards.decodes(a as u8) {
                self.cards.peek(a as u8, self.board.t)
            } else {
                peek(&self.board, a)
            };
            v | u32::from(byte) << (8 * i)
        }))
    }

    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool {
        if space != CPU_SPACE {
            return false;
        }
        for i in 0..size.bytes() {
            let a = addr.wrapping_add(i);
            let byte = (v >> (8 * i)) as u8;
            if a >= IO_BASE {
                self.board.io_write(a as u8, byte);
            } else if a < self.board.ram_size {
                self.board.mem[a as usize] = byte;
            } else {
                return false;
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
