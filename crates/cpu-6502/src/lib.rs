//! 6502 family: 6502, 6507, 6510, 8502, 7501/8501, 6502C (SALLY) and the
//! NES 2A03/2A07 via variant flags.
//!
//! The core is cycle-stepped: every [`Cpu::tick`] performs exactly one bus
//! access, including dummy reads and writes. Only NMOS 6502 behaviour is
//! implemented so far; the variants share it and differ in on-chip
//! peripherals (see [`IoPort`]) that machines wire up themselves.

#![forbid(unsafe_code)]

mod disasm;
mod io_port;
mod micro;
mod table;

use emu_core::savestate::{Savestate, StateError, StateReader, StateWriter};
use emu_core::{Bus, Cpu, CycleResult, Instr, InterruptKind, Register, RegisterSet, Size};

pub use crate::io_port::IoPort;
use crate::micro::{Core, Seq, B, I, U, V};

/// Members of the 6502 family.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Variant {
    /// NMOS 6502 (PET, VIC-20, 1541 drive).
    #[default]
    Nmos6502,
    /// 6510 with 6-bit I/O port at $00/$01 (C64).
    Mos6510,
    /// 8502 with 7-bit I/O port, 1/2 MHz (C128).
    Mos8502,
    /// 7501/8501 with I/O port, TED machines (C16/Plus/4).
    Mos7501,
    /// 6502C "SALLY" with HALT input (Atari 8-bit).
    Sally,
    /// 6507 (Atari 2600): the NMOS core in a 28-pin package with 13
    /// address lines and no IRQ/NMI pins. The machine's bus ignores
    /// A13–A15 and never raises an interrupt (MOS MCS6500 data sheet,
    /// "MCS6507").
    Mos6507,
    /// Ricoh 2A03 (NES NTSC) / 2A07 (PAL): the NMOS core without decimal
    /// mode; D is stored and pushed but ADC/SBC stay binary (NESdev wiki,
    /// "CPU"; Tom Harte's `nes6502` tests). APU, OAM DMA and the joypad
    /// strobe on the same die are emulated by the machine.
    Rp2A03,
    /// CMOS 65C02 as in the enhanced Apple IIe (NCR/GTE; no Rockwell bit
    /// instructions, no WAI/STP): new instructions and addressing modes,
    /// every undefined opcode a NOP, fixed JMP (abs), valid N/Z in
    /// decimal mode at the cost of a cycle, D cleared on interrupts.
    Cmos65C02,
}

impl Variant {
    /// Width mask of the on-chip I/O port, if the variant has one.
    pub const fn io_port_mask(self) -> Option<u8> {
        match self {
            Variant::Mos6510 => Some(0x3f),
            Variant::Mos8502 => Some(0x7f),
            Variant::Mos7501 => Some(0xdf),
            Variant::Nmos6502
            | Variant::Sally
            | Variant::Mos6507
            | Variant::Rp2A03
            | Variant::Cmos65C02 => None,
        }
    }
}

/// Construction-time configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Config {
    pub variant: Variant,
    /// "Magic" constant of the unstable ANE ($8B); chip-dependent.
    pub ane_magic: u8,
    /// "Magic" constant of the unstable LXA ($AB); chip-dependent.
    pub lxa_magic: u8,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            variant: Variant::default(),
            ane_magic: 0xee,
            lxa_magic: 0xee,
        }
    }
}

const STATE_TAG: [u8; 4] = *b"6502";
/// Version 2 added `polled` (version 1 loads with it clear).
const STATE_VERSION: u16 = 2;

/// A 6502-family CPU.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cpu6502 {
    core: Core,
    variant: Variant,
    rdy: bool,
    /// SALLY HALT input (true = halted). Driven by ANTIC every cycle, so
    /// it is not part of the savestate.
    halt: bool,
    /// `rdy && !halt`: the fast path of `tick` tests one flag.
    free: bool,
    /// The last tick with RDY low or HALT high did not run (see
    /// [`Cpu6502::halted`]). Only written on that slow path.
    halted: bool,
}

impl Cpu6502 {
    /// A CPU at the start of an opcode fetch at PC = 0. Machines call
    /// [`Cpu6502::reset`] to run the reset sequence.
    pub fn new(config: Config) -> Self {
        Self {
            core: Core::new(
                config.ane_magic,
                config.lxa_magic,
                config.variant != Variant::Rp2A03,
                config.variant != Variant::Rp2A03,
                config.variant == Variant::Cmos65C02,
            ),
            variant: config.variant,
            rdy: true,
            halt: false,
            free: true,
            halted: false,
        }
    }

    pub fn variant(&self) -> Variant {
        self.variant
    }

    /// Start the 7-cycle reset sequence at the next tick (vector $FFFC).
    /// Also releases a jammed CPU.
    pub fn reset(&mut self) {
        let c = &mut self.core;
        c.jammed = false;
        c.t = 0;
        c.reset_pending = true;
        c.nmi_pending = false;
        c.poll = false;
    }

    /// IRQ input level (true = asserted). Level-triggered, masked by I.
    pub fn set_irq(&mut self, active: bool) {
        self.core.irq_line = active;
    }

    /// NMI input level (true = asserted). Edge-triggered on assertion.
    pub fn set_nmi(&mut self, active: bool) {
        if active && !self.core.nmi_line {
            self.core.nmi_pending = true;
        }
        self.core.nmi_line = active;
    }

    /// RDY input (false = halt). The NMOS 6502 only stops on read cycles;
    /// write cycles proceed (VIC-II bad lines rely on this).
    pub fn set_rdy(&mut self, ready: bool) {
        self.rdy = ready;
        self.free = self.rdy && !self.halt;
    }

    /// HALT input of the 6502C "SALLY" (true = halted). Unlike RDY it stops
    /// the CPU on any cycle, writes included: ANTIC asserts it for each DMA
    /// cycle it takes, and the CPU simply does not run in that cycle
    /// (Altirra Hardware Reference Manual, "6502C"). Other variants have no
    /// HALT pin and ignore it.
    pub fn set_halt(&mut self, halted: bool) {
        self.halt = halted && self.variant == Variant::Sally;
        self.free = self.rdy && !self.halt;
    }

    /// Interrupt inputs as they are at the end of the cycle just run, for
    /// machines whose chips change them during the cycle after the CPU's
    /// bus access (the NES: a PPUSTATUS read in the cycle the vertical
    /// blank flag rises cancels the NMI). Like [`Cpu6502::set_nmi`] and
    /// [`Cpu6502::set_irq`], but the levels also count for the interrupt
    /// poll of that cycle, as if they had been set before it.
    pub fn sample_interrupts(&mut self, nmi: bool, irq: bool) {
        self.set_nmi(nmi);
        self.set_irq(irq);
        let c = &mut self.core;
        if c.polled {
            c.poll = c.nmi_pending || (c.irq_line && c.p & I == 0);
        }
    }

    /// Whether the last tick, made with RDY low (or HALT high), stopped the
    /// CPU: true when it would have read (the cycle repeats), false when
    /// it wrote and so went ahead. Meaningless after a tick with RDY high.
    /// The NES's DMA unit takes the bus only in a cycle the CPU gave up.
    pub fn halted(&self) -> bool {
        self.halted
    }

    /// SO input: a falling edge sets the V flag (used by the 1541).
    pub fn set_overflow(&mut self) {
        self.core.p |= V;
    }

    /// The hardware interrupt whose entry sequence completed in the last
    /// tick, if any (the next instruction is the handler's first). Machines
    /// call this after every tick that finished an instruction.
    pub fn take_interrupt(&mut self) -> Option<InterruptKind> {
        self.core.entered.take()
    }

    pub fn is_jammed(&self) -> bool {
        self.core.jammed
    }

    /// True between instructions, i.e. the next tick fetches an opcode.
    pub fn at_instruction_boundary(&self) -> bool {
        self.core.t == 0 && !self.core.jammed
    }

    pub fn pc(&self) -> u16 {
        self.core.pc
    }
}

impl Default for Cpu6502 {
    fn default() -> Self {
        Self::new(Config::default())
    }
}

/// The bus of a cycle with RDY low: writes go through (the 6502 does not
/// stop on writes) and are recorded; the read the CPU will repeat is a
/// `peek`, without side effects. Only the access once RDY is high again
/// counts, so a halted `LDA $DC0D` acknowledges the CIA's interrupts once.
/// (C128 fast serial transfers lose bytes otherwise, which real machines
/// do not.)
struct WriteProbe<'a> {
    bus: &'a mut dyn Bus,
    wrote: bool,
}

impl Bus for WriteProbe<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.bus.peek(addr, size)
    }

    fn peek(&self, addr: u32, size: Size) -> u32 {
        self.bus.peek(addr, size)
    }

    fn write(&mut self, addr: u32, size: Size, v: u32) {
        self.wrote = true;
        self.bus.write(addr, size, v);
    }

    fn addr_bits(&self) -> u8 {
        self.bus.addr_bits()
    }
}

impl Cpu for Cpu6502 {
    fn tick(&mut self, bus: &mut dyn Bus) -> CycleResult {
        if self.free {
            return self.core.cycle(bus);
        }
        if self.halt {
            self.halted = true;
            return CycleResult::Busy;
        }
        // RDY low: the CPU repeats a read cycle, so roll the state back
        // unless the cycle was a write.
        let saved = self.core;
        let mut probe = WriteProbe { bus, wrote: false };
        let result = self.core.cycle(&mut probe);
        if probe.wrote || self.core.jammed {
            self.halted = false;
            result
        } else {
            self.halted = true;
            // Interrupt recognition keeps running while the cycle repeats:
            // an NMI that arrives during the stall is taken after the
            // stalled instruction even when that was its last cycle
            // (Acid800 `antic_dlitiming`, DLI during WSYNC).
            let poll = saved.t != 0 && saved.op != 0x00 && !saved.no_poll;
            self.core = saved;
            self.core.polled = poll;
            if poll {
                let c = &mut self.core;
                c.poll |= c.nmi_pending || (c.irq_line && c.p & I == 0);
            }
            CycleResult::Busy
        }
    }

    fn regs(&self) -> RegisterSet {
        let c = &self.core;
        let reg = |name, value: u32, bits| Register { name, value, bits };
        RegisterSet(vec![
            reg("PC", u32::from(c.pc), 16),
            reg("A", u32::from(c.a), 8),
            reg("X", u32::from(c.x), 8),
            reg("Y", u32::from(c.y), 8),
            reg("S", u32::from(c.s), 8),
            // Bit 5 always reads as 1; B does not exist in the register.
            reg("P", u32::from(c.p | U), 8),
        ])
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        let c = &mut self.core;
        let byte = v as u8;
        match name.to_ascii_uppercase().as_str() {
            "PC" => c.pc = v as u16,
            "A" => c.a = byte,
            "X" => c.x = byte,
            "Y" => c.y = byte,
            "S" | "SP" => c.s = byte,
            "P" => c.p = byte & !(B | U),
            _ => {}
        }
    }

    fn disassemble(&self, bus: &dyn Bus, addr: u32) -> Instr {
        disasm::disassemble(bus, addr as u16, self.core.cmos)
    }
}

fn seq_to_u8(seq: Seq) -> u8 {
    match seq {
        Seq::Brk => 0,
        Seq::Interrupt => 1,
        Seq::Reset => 2,
    }
}

fn seq_from_u8(v: u8) -> Result<Seq, StateError> {
    match v {
        0 => Ok(Seq::Brk),
        1 => Ok(Seq::Interrupt),
        2 => Ok(Seq::Reset),
        _ => Err(StateError::Invalid("6502 interrupt sequence")),
    }
}

impl Savestate for Cpu6502 {
    fn save(&self, w: &mut StateWriter) {
        let c = &self.core;
        w.chunk(STATE_TAG, STATE_VERSION, |w| {
            for v in [c.a, c.x, c.y, c.s, c.p, c.op, c.t, c.ptr, c.data] {
                w.u8(v);
            }
            for v in [c.pc, c.ea, c.base, c.vector] {
                w.u16(v);
            }
            w.u8(seq_to_u8(c.seq));
            for v in [
                c.crossed,
                c.irq_line,
                c.nmi_line,
                c.nmi_pending,
                c.reset_pending,
                c.poll,
                c.jammed,
                self.rdy,
                c.polled,
            ] {
                w.bool(v);
            }
        });
    }

    /// Restores registers and mid-instruction state. Configuration (variant,
    /// magic constants) belongs to the machine and is not part of the state.
    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (version, mut s) = r.chunk(STATE_TAG, STATE_VERSION)?;
        let mut c = self.core;
        c.a = s.u8()?;
        c.x = s.u8()?;
        c.y = s.u8()?;
        c.s = s.u8()?;
        c.p = s.u8()? & !(B | U);
        c.op = s.u8()?;
        c.t = s.u8()?;
        c.ptr = s.u8()?;
        c.data = s.u8()?;
        c.pc = s.u16()?;
        c.ea = s.u16()?;
        c.base = s.u16()?;
        c.vector = s.u16()?;
        c.seq = seq_from_u8(s.u8()?)?;
        c.crossed = s.bool()?;
        c.irq_line = s.bool()?;
        c.nmi_line = s.bool()?;
        c.nmi_pending = s.bool()?;
        c.reset_pending = s.bool()?;
        c.poll = s.bool()?;
        c.jammed = s.bool()?;
        let rdy = s.bool()?;
        c.polled = version >= 2 && s.bool()?;
        if c.t > 8 && !c.jammed {
            return Err(StateError::Invalid("6502 cycle counter"));
        }
        c.no_poll = false;
        self.core = c;
        self.rdy = rdy;
        self.free = self.rdy && !self.halt;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::micro::I;
    use emu_core::testing::FlatBus;

    /// Run until the CPU reaches an instruction boundary; returns cycles used.
    fn step(cpu: &mut Cpu6502, bus: &mut FlatBus) -> u32 {
        let mut cycles = 0;
        loop {
            cycles += 1;
            match cpu.tick(bus) {
                CycleResult::InstrDone { .. } => return cycles,
                CycleResult::Jammed => panic!("CPU jammed"),
                CycleResult::Busy => {}
            }
        }
    }

    fn cpu_at(pc: u16, program: &[u8]) -> (Cpu6502, FlatBus) {
        let mut bus = FlatBus::new(16);
        bus.load(u32::from(pc), program);
        let mut cpu = Cpu6502::default();
        cpu.set_reg("PC", u32::from(pc));
        cpu.set_reg("S", 0xff);
        (cpu, bus)
    }

    #[test]
    fn reset_sequence_reads_vector_and_takes_7_cycles() {
        let mut bus = FlatBus::new(16);
        bus.load(0xfffc, &[0x00, 0xe0]);
        let mut cpu = Cpu6502::default();
        cpu.set_reg("S", 0x00);
        cpu.reset();
        assert_eq!(step(&mut cpu, &mut bus), 7);
        assert_eq!(cpu.pc(), 0xe000);
        assert_eq!(
            cpu.regs().get("S"),
            Some(0xfd),
            "reset decrements S by 3 without writing"
        );
        assert_eq!(bus.mem[0x0100], 0, "no stack writes during reset");
        assert_ne!(cpu.regs().get("P").unwrap() & u32::from(I), 0);
    }

    #[test]
    fn instruction_cycle_counts() {
        // (program, expected cycles of the first instruction)
        let cases: &[(&[u8], u32)] = &[
            (&[0xa9, 0x01], 2),       // LDA #
            (&[0xa5, 0x10], 3),       // LDA zp
            (&[0xbd, 0x00, 0x20], 4), // LDA abs,X  (X=0, no cross)
            (&[0x9d, 0x00, 0x20], 5), // STA abs,X  (always 5)
            (&[0xfe, 0x00, 0x20], 7), // INC abs,X
            (&[0x20, 0x00, 0x30], 6), // JSR
            (&[0x6c, 0x00, 0x20], 5), // JMP (ind)
            (&[0x00], 7),             // BRK
            (&[0x48], 3),             // PHA
            (&[0x68], 4),             // PLA
            (&[0x03, 0x10], 8),       // SLO (zp,X)
        ];
        for (program, expected) in cases {
            let (mut cpu, mut bus) = cpu_at(0x0400, program);
            assert_eq!(
                step(&mut cpu, &mut bus),
                *expected,
                "opcode {:02x}",
                program[0]
            );
        }
    }

    #[test]
    fn page_crossing_adds_a_cycle_to_reads() {
        let (mut cpu, mut bus) = cpu_at(0x0400, &[0xbd, 0xff, 0x20]); // LDA $20FF,X
        cpu.set_reg("X", 1);
        assert_eq!(step(&mut cpu, &mut bus), 5);
    }

    #[test]
    fn branch_timing() {
        // BNE +2 with Z clear: taken, no page cross = 3 cycles.
        let (mut cpu, mut bus) = cpu_at(0x0400, &[0xd0, 0x02]);
        cpu.set_reg("P", 0);
        assert_eq!(step(&mut cpu, &mut bus), 3);
        assert_eq!(cpu.pc(), 0x0404);
        // Not taken = 2 cycles.
        let (mut cpu, mut bus) = cpu_at(0x0400, &[0xd0, 0x02]);
        cpu.set_reg("P", 0x02);
        assert_eq!(step(&mut cpu, &mut bus), 2);
        // Taken across a page = 4 cycles.
        let (mut cpu, mut bus) = cpu_at(0x04f0, &[0xd0, 0x7f]);
        cpu.set_reg("P", 0);
        assert_eq!(step(&mut cpu, &mut bus), 4);
        assert_eq!(cpu.pc(), 0x0571);
    }

    #[test]
    fn irq_is_taken_after_the_current_instruction() {
        let (mut cpu, mut bus) = cpu_at(0x0400, &[0xea, 0xea]);
        bus.load(0xfffe, &[0x00, 0x90]);
        cpu.set_reg("P", 0); // I clear
        cpu.set_irq(true);
        assert_eq!(step(&mut cpu, &mut bus), 2, "NOP completes");
        assert_eq!(step(&mut cpu, &mut bus), 7, "IRQ sequence");
        assert_eq!(cpu.pc(), 0x9000);
        assert_eq!(bus.mem[0x01ff], 0x04, "PCH pushed");
        assert_eq!(bus.mem[0x01fe], 0x01, "PC of the next instruction pushed");
        assert_eq!(bus.mem[0x01fd] & B, 0, "B clear for hardware interrupts");
    }

    #[test]
    fn interrupt_entry_is_reported_once_per_sequence() {
        let (mut cpu, mut bus) = cpu_at(0x0400, &[0x00, 0x00, 0xea]);
        bus.load(0xfffa, &[0x00, 0xa0, 0x00, 0xb0, 0x00, 0x90]);
        step(&mut cpu, &mut bus); // BRK: software, not reported
        assert_eq!(cpu.take_interrupt(), None);

        let (mut cpu, mut bus) = cpu_at(0x0400, &[0xea, 0xea]);
        bus.load(0xfffa, &[0x00, 0xa0, 0x00, 0xb0, 0x00, 0x90]);
        cpu.set_reg("P", 0);
        cpu.set_irq(true);
        step(&mut cpu, &mut bus);
        assert_eq!(cpu.take_interrupt(), None, "NOP");
        step(&mut cpu, &mut bus);
        assert_eq!(cpu.take_interrupt(), Some(InterruptKind::Irq));
        assert_eq!(cpu.take_interrupt(), None, "taken once");

        let (mut cpu, mut bus) = cpu_at(0x0400, &[0x00, 0x00]);
        bus.load(0xfffa, &[0x00, 0xa0, 0x00, 0xb0, 0x00, 0x90]);
        for _ in 0..3 {
            cpu.tick(&mut bus);
        }
        cpu.set_nmi(true);
        step(&mut cpu, &mut bus);
        assert_eq!(
            cpu.take_interrupt(),
            Some(InterruptKind::Nmi),
            "BRK hijacked"
        );

        let mut cpu = Cpu6502::default();
        cpu.reset();
        step(&mut cpu, &mut bus);
        assert_eq!(cpu.take_interrupt(), None, "reset is not an interrupt");
    }

    #[test]
    fn cli_delays_irq_by_one_instruction() {
        let (mut cpu, mut bus) = cpu_at(0x0400, &[0x58, 0xea, 0xea]); // CLI; NOP; NOP
        bus.load(0xfffe, &[0x00, 0x90]);
        cpu.set_irq(true);
        step(&mut cpu, &mut bus); // CLI
        step(&mut cpu, &mut bus); // NOP still runs
        assert_eq!(cpu.pc(), 0x0402);
        step(&mut cpu, &mut bus); // IRQ
        assert_eq!(cpu.pc(), 0x9000);
    }

    #[test]
    fn nmi_hijacks_brk() {
        let (mut cpu, mut bus) = cpu_at(0x0400, &[0x00, 0x00]);
        bus.load(0xfffa, &[0x00, 0xa0, 0x00, 0xb0, 0x00, 0x90]);
        for _ in 0..3 {
            cpu.tick(&mut bus);
        }
        cpu.set_nmi(true); // arrives before the vector is chosen
        step(&mut cpu, &mut bus);
        assert_eq!(cpu.pc(), 0xa000, "NMI vector used");
        assert_ne!(bus.mem[0x01fd] & B, 0, "B flag of the BRK still pushed");
    }

    #[test]
    fn late_nmi_during_brk_waits_for_the_first_handler_instruction() {
        // Lorenz "nmi": an NMI arriving after the vector is chosen is taken
        // only after the first instruction of the BRK handler.
        let (mut cpu, mut bus) = cpu_at(0x0400, &[0x00, 0x00]);
        bus.load(0xfffa, &[0x00, 0xa0, 0x00, 0xb0, 0x00, 0x90]);
        bus.load(0x9000, &[0xea, 0xea]); // NOP; NOP
        for _ in 0..5 {
            cpu.tick(&mut bus);
        }
        cpu.set_nmi(true);
        step(&mut cpu, &mut bus); // rest of BRK
        assert_eq!(cpu.pc(), 0x9000, "IRQ/BRK vector, no hijack");
        step(&mut cpu, &mut bus); // first handler instruction
        assert_eq!(cpu.pc(), 0x9001);
        step(&mut cpu, &mut bus); // now the NMI sequence
        assert_eq!(cpu.pc(), 0xa000);
    }

    #[test]
    fn nmi_is_edge_triggered() {
        let (mut cpu, mut bus) = cpu_at(0x0400, &[0xea; 8]);
        bus.load(0xfffa, &[0x00, 0xa0]);
        bus.load(0xa000, &[0xea; 8]);
        cpu.set_nmi(true);
        step(&mut cpu, &mut bus);
        step(&mut cpu, &mut bus);
        assert_eq!(cpu.pc(), 0xa000);
        cpu.set_nmi(true); // still asserted: no new edge
        step(&mut cpu, &mut bus);
        step(&mut cpu, &mut bus);
        assert_eq!(cpu.pc(), 0xa002);
    }

    #[test]
    fn rdy_stalls_reads_but_not_writes() {
        let (mut cpu, mut bus) = cpu_at(0x0400, &[0x8d, 0x00, 0x20]); // STA $2000
        cpu.set_reg("A", 0x42);
        cpu.set_rdy(false);
        for _ in 0..10 {
            cpu.tick(&mut bus);
        }
        assert_eq!(cpu.pc(), 0x0400, "opcode fetch repeats while RDY is low");
        cpu.set_rdy(true);
        cpu.tick(&mut bus);
        cpu.tick(&mut bus);
        cpu.tick(&mut bus);
        cpu.set_rdy(false);
        cpu.tick(&mut bus); // the write cycle proceeds
        assert!(!cpu.halted());
        assert_eq!(bus.mem[0x2000], 0x42);
        cpu.tick(&mut bus); // the next opcode fetch stops
        assert!(cpu.halted());
        assert!(cpu.at_instruction_boundary());
    }

    /// A bus counting reads with side effects.
    struct CountingBus {
        inner: FlatBus,
        reads: u32,
    }

    impl Bus for CountingBus {
        fn read(&mut self, addr: u32, size: Size) -> u32 {
            self.reads += 1;
            self.inner.read(addr, size)
        }
        fn peek(&self, addr: u32, size: Size) -> u32 {
            self.inner.peek(addr, size)
        }
        fn write(&mut self, addr: u32, size: Size, v: u32) {
            self.inner.write(addr, size, v);
        }
        fn addr_bits(&self) -> u8 {
            16
        }
    }

    #[test]
    fn sally_halt_stops_writes_too() {
        let (_, mut bus) = cpu_at(0x0400, &[0x8d, 0x00, 0x20]); // STA $2000
        let mut cpu = Cpu6502::new(Config {
            variant: Variant::Sally,
            ..Config::default()
        });
        cpu.set_reg("PC", 0x0400);
        cpu.set_reg("A", 0x42);
        for _ in 0..3 {
            cpu.tick(&mut bus);
        }
        // The write cycle is next: HALT freezes it.
        cpu.set_halt(true);
        for _ in 0..5 {
            assert_eq!(cpu.tick(&mut bus), CycleResult::Busy);
        }
        assert_eq!(bus.mem[0x2000], 0);
        cpu.set_halt(false);
        cpu.tick(&mut bus);
        assert_eq!(bus.mem[0x2000], 0x42);
        assert!(cpu.at_instruction_boundary());

        // Other variants have no HALT pin.
        let (mut nmos, mut bus) = cpu_at(0x0400, &[0xea]);
        nmos.set_halt(true);
        nmos.tick(&mut bus);
        nmos.tick(&mut bus);
        assert!(nmos.at_instruction_boundary());
        assert_eq!(nmos.pc(), 0x0401);
    }

    #[test]
    fn stalled_reads_have_no_side_effects() {
        let (mut cpu, inner) = cpu_at(0x0400, &[0xea]); // NOP
        let mut bus = CountingBus { inner, reads: 0 };
        cpu.set_rdy(false);
        for _ in 0..5 {
            cpu.tick(&mut bus);
        }
        assert_eq!(bus.reads, 0, "repeated reads are peeks");
        cpu.set_rdy(true);
        cpu.tick(&mut bus);
        assert_eq!(bus.reads, 1, "the opcode fetch happens once");
    }

    #[test]
    fn jam_halts_until_reset() {
        let (mut cpu, mut bus) = cpu_at(0x0400, &[0x02]);
        bus.load(0xfffc, &[0x00, 0x04]);
        cpu.tick(&mut bus);
        assert_eq!(cpu.tick(&mut bus), CycleResult::Jammed);
        assert_eq!(cpu.tick(&mut bus), CycleResult::Jammed);
        assert!(cpu.is_jammed());
        cpu.reset();
        assert!(!cpu.is_jammed());
        assert_eq!(step(&mut cpu, &mut bus), 7);
    }

    #[test]
    fn decimal_mode_adc_and_sbc() {
        // SED; CLC; LDA #$58; ADC #$46  -> $04, C set (58 + 46 = 104)
        let (mut cpu, mut bus) = cpu_at(0x0400, &[0xf8, 0x18, 0xa9, 0x58, 0x69, 0x46]);
        for _ in 0..4 {
            step(&mut cpu, &mut bus);
        }
        assert_eq!(cpu.regs().get("A"), Some(0x04));
        assert_eq!(cpu.regs().get("P").unwrap() & 1, 1);
        // SED; SEC; LDA #$12; SBC #$21  -> $91, C clear (12 - 21 = -9)
        let (mut cpu, mut bus) = cpu_at(0x0400, &[0xf8, 0x38, 0xa9, 0x12, 0xe9, 0x21]);
        for _ in 0..4 {
            step(&mut cpu, &mut bus);
        }
        assert_eq!(cpu.regs().get("A"), Some(0x91));
        assert_eq!(cpu.regs().get("P").unwrap() & 1, 0);
    }

    #[test]
    fn late_sampled_nmi_counts_for_the_cycle_just_run() {
        // NOP takes 2 cycles; the poll at the end of its first decides.
        let (mut cpu, mut bus) = cpu_at(0x0400, &[0xea, 0xea, 0xea]);
        bus.load(0xfffa, &[0x00, 0xa0]);
        cpu.tick(&mut bus); // opcode fetch
        cpu.sample_interrupts(true, false);
        cpu.tick(&mut bus); // last cycle of the NOP
        step(&mut cpu, &mut bus);
        assert_eq!(cpu.pc(), 0xa000, "taken right after the first NOP");
        // Set the same way with set_nmi, it waits one more instruction.
        let (mut cpu, mut bus) = cpu_at(0x0400, &[0xea, 0xea, 0xea]);
        bus.load(0xfffa, &[0x00, 0xa0]);
        cpu.tick(&mut bus);
        cpu.set_nmi(true);
        cpu.tick(&mut bus);
        step(&mut cpu, &mut bus);
        assert_eq!(cpu.pc(), 0x0402);
    }

    #[test]
    fn rp2a03_has_no_decimal_mode() {
        // SED; CLC; LDA #$58; ADC #$46 -> $9E binary, D still set
        let program = [0xf8, 0x18, 0xa9, 0x58, 0x69, 0x46];
        let mut bus = FlatBus::new(16);
        bus.load(0x0400, &program);
        let mut cpu = Cpu6502::new(Config {
            variant: Variant::Rp2A03,
            ..Config::default()
        });
        cpu.set_reg("PC", 0x0400);
        for _ in 0..4 {
            step(&mut cpu, &mut bus);
        }
        assert_eq!(cpu.regs().get("A"), Some(0x9e));
        assert_ne!(cpu.regs().get("P").unwrap() & 0x08, 0, "D kept");
    }

    /// Savestate at every cycle of a program, restore into a fresh CPU and
    /// check both continue identically (determinism, golden rule 3).
    #[test]
    fn savestate_round_trip_at_every_cycle() {
        let program = [
            0xa2, 0x05, // LDX #5
            0xbd, 0xff, 0x04, // LDA $04FF,X (page cross)
            0x20, 0x20, 0x04, // JSR $0420
            0xca, // DEX
            0xd0, 0xf7, // BNE -9
            0x02, // JAM
        ];
        let (mut cpu, mut bus) = cpu_at(0x0400, &program);
        bus.load(0x0420, &[0xee, 0x00, 0x30, 0x60]); // INC $3000; RTS
        for cycle in 0..200 {
            let mut w = StateWriter::new();
            cpu.save(&mut w);
            let data = w.into_bytes();
            let mut copy = Cpu6502::default();
            copy.load(&mut StateReader::new(&data)).unwrap();
            assert_eq!(copy, cpu, "state differs after load at cycle {cycle}");

            let mut bus_copy = bus.clone();
            let a = cpu.tick(&mut bus);
            let b = copy.tick(&mut bus_copy);
            assert_eq!(a, b);
            assert_eq!(bus.mem, bus_copy.mem);
        }
        assert!(cpu.is_jammed());
        assert_eq!(bus.mem[0x3000], 5);
    }

    #[test]
    fn corrupt_savestate_is_rejected() {
        let cpu = Cpu6502::default();
        let mut w = StateWriter::new();
        cpu.save(&mut w);
        let mut data = w.into_bytes();
        let last = data.len() - 1;
        data[last] = 7; // invalid bool
        let mut target = Cpu6502::default();
        target.set_reg("A", 0x99);
        assert!(target.load(&mut StateReader::new(&data)).is_err());
        assert_eq!(
            target.regs().get("A"),
            Some(0x99),
            "failed load leaves state unchanged"
        );
    }

    #[test]
    fn registers_round_trip() {
        let mut cpu = Cpu6502::default();
        cpu.set_reg("pc", 0x1234);
        cpu.set_reg("sp", 0x80);
        cpu.set_reg("P", 0xff);
        let regs = cpu.regs();
        assert_eq!(regs.get("PC"), Some(0x1234));
        assert_eq!(regs.get("S"), Some(0x80));
        assert_eq!(
            regs.get("P"),
            Some(0xef),
            "B never set in the register, bit 5 always"
        );
    }
}
