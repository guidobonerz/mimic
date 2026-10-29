//! RAM Expansion Unit: Commodore 1700 (128 KB), 1764 (256 KB), 1750
//! (512 KB) and the larger units built the same way (1 MB to 16 MB, as
//! the Ultimate offers), with the REC 8726 DMA controller.
//!
//! Registers in I/O2, decoded by A0–A4 (mirrored every 32 bytes over
//! `$DF00–$DFFF`); source: Commodore 1764 RAM Expansion Module User's
//! Guide, appendix "REC registers", and the 1700/1750 user's guide:
//!
//! | Reg | Read                               | Write                    |
//! | --- | ---------------------------------- | ------------------------ |
//! | 0   | status: 7 IRQ, 6 end of block, 5 fault (verify), 4 size (1 = 256K chips), 3–0 version | – |
//! | 1   | command                            | 7 execute, 5 autoload, 4 no `$FF00` trigger, 1–0 type: 00 C64→REU, 01 REU→C64, 10 swap, 11 verify |
//! | 2–3 | C64 address                        | C64 address              |
//! | 4–6 | REU address, bank                  | REU address, bank        |
//! | 7–8 | transfer length (0 = 64 KB)        | transfer length          |
//! | 9   | interrupt mask: 7 enable, 6 end of block, 5 verify error | same |
//! | 10  | address control: 7 fix C64, 6 fix REU | same                  |
//!
//! Reading the status clears bits 7–5 and releases /IRQ. A command with
//! bit 7 and bit 4 runs at once; with bit 4 clear it waits for the CPU's
//! next write to `$FF00` (so the transfer can start with the I/O area
//! banked out). During the transfer the REU holds /DMA: the CPU stops and
//! the REU moves one byte a cycle (swap: two cycles a byte) through the
//! C64's memory map as the CPU would see it, waiting while the VIC-II
//! holds BA. At the end the addresses point past the block and the length
//! reads 1, unless autoload restores the values last written.
//!
//! Unverified (the guides do not say, chosen plausibly): unused register
//! bits read 1 (bank bits 3–7 on the 128–512 KB units, mask bits 4–0,
//! address-control bits 5–0, registers 11–31 `$FF`); after a command the
//! execute bit reads 0 and bit 4 reads 1; a verify error stops after the
//! failing byte with both addresses past it; a transfer ending on a
//! verify error with length 1 sets end of block too; DMA reads of the
//! REU's own registers see the open bus.

use emu_core::{Bus, Size, StateError, StateReader, StateWriter};

/// Command register bits.
pub mod cmd {
    pub const EXECUTE: u8 = 1 << 7;
    pub const AUTOLOAD: u8 = 1 << 5;
    pub const NO_FF00: u8 = 1 << 4;
    pub const TYPE: u8 = 0x03;
}

/// Status register bits.
pub mod status {
    pub const IRQ: u8 = 1 << 7;
    pub const END_OF_BLOCK: u8 = 1 << 6;
    pub const FAULT: u8 = 1 << 5;
    pub const SIZE: u8 = 1 << 4;
}

/// Interrupt mask bits.
pub mod mask {
    pub const ENABLE: u8 = 1 << 7;
    pub const END_OF_BLOCK: u8 = 1 << 6;
    pub const VERIFY: u8 = 1 << 5;
}

/// Sizes: 128 KB (1700), 256 KB (1764), 512 KB (1750), 1–16 MB.
pub const SIZES: [usize; 8] = [
    128 << 10,
    256 << 10,
    512 << 10,
    1 << 20,
    2 << 20,
    4 << 20,
    8 << 20,
    16 << 20,
];

/// The REU option value (`"128k"`, `"1764"`, `"16m"` …) as a size.
pub fn size_option(v: &str) -> Option<usize> {
    Some(match v.to_ascii_lowercase().as_str() {
        "128k" | "1700" => 128 << 10,
        "256k" | "1764" => 256 << 10,
        "512k" | "1750" => 512 << 10,
        "1m" => 1 << 20,
        "2m" => 2 << 20,
        "4m" => 4 << 20,
        "8m" => 8 << 20,
        "16m" => 16 << 20,
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dma {
    Idle,
    /// Waiting for the CPU's write to `$FF00`.
    Armed,
    /// Moving bytes; `swap` holds the byte pair read in a swap's first
    /// cycle.
    Running {
        swap: Option<(u8, u8)>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reu {
    ram: Vec<u8>,
    status: u8,
    command: u8,
    c64_addr: u16,
    /// 24-bit REU address (bank in bits 16–23).
    reu_addr: u32,
    length: u16,
    /// The values last written, for autoload.
    base_c64: u16,
    base_reu: u32,
    base_length: u16,
    int_mask: u8,
    addr_ctrl: u8,
    dma: Dma,
}

impl Reu {
    /// A REU of `size` bytes (one of [`SIZES`]); RAM starts cleared.
    pub fn new(size: usize) -> Self {
        let size = SIZES
            .iter()
            .copied()
            .find(|&s| s >= size)
            .unwrap_or(16 << 20);
        Self {
            ram: vec![0; size],
            status: 0,
            command: cmd::NO_FF00,
            c64_addr: 0,
            reu_addr: 0,
            length: 0xffff,
            base_c64: 0,
            base_reu: 0,
            base_length: 0xffff,
            int_mask: 0,
            addr_ctrl: 0,
            dma: Dma::Idle,
        }
    }

    pub fn size(&self) -> usize {
        self.ram.len()
    }

    /// The expansion's memory (the UCI loads and saves REU images).
    pub fn ram(&self) -> &[u8] {
        &self.ram
    }

    pub fn ram_mut(&mut self) -> &mut [u8] {
        &mut self.ram
    }

    /// Bank bits the unit keeps: three on the Commodore units, as many
    /// as the size needs above.
    fn bank_bits(&self) -> u8 {
        let banks = (self.ram.len() >> 16).max(8);
        (banks - 1) as u8
    }

    /// /RESET: the registers return to their power-on values; the RAM
    /// keeps its contents.
    pub fn reset(&mut self) {
        let ram = std::mem::take(&mut self.ram);
        *self = Self::new(ram.len());
        self.ram = ram;
    }

    /// Side-effect-free register read (`addr` in I/O2).
    pub fn peek(&self, addr: u16) -> u8 {
        match addr & 0x1f {
            0 => {
                let size = if self.ram.len() > 128 << 10 {
                    status::SIZE
                } else {
                    0
                };
                self.status | size
            }
            1 => self.command,
            2 => self.c64_addr as u8,
            3 => (self.c64_addr >> 8) as u8,
            4 => self.reu_addr as u8,
            5 => (self.reu_addr >> 8) as u8,
            6 => (self.reu_addr >> 16) as u8 | !self.bank_bits(),
            7 => self.length as u8,
            8 => (self.length >> 8) as u8,
            9 => self.int_mask | 0x1f,
            10 => self.addr_ctrl | 0x3f,
            _ => 0xff,
        }
    }

    /// CPU read: the status register clears its flags.
    pub fn read(&mut self, addr: u16) -> u8 {
        let v = self.peek(addr);
        if addr & 0x1f == 0 {
            self.status = 0;
        }
        v
    }

    pub fn write(&mut self, addr: u16, v: u8) {
        match addr & 0x1f {
            1 => {
                self.command = v;
                if v & cmd::EXECUTE != 0 {
                    self.dma = if v & cmd::NO_FF00 != 0 {
                        Dma::Running { swap: None }
                    } else {
                        Dma::Armed
                    };
                }
            }
            2 => self.set_c64(self.base_c64 & 0xff00 | u16::from(v)),
            3 => self.set_c64(self.base_c64 & 0x00ff | u16::from(v) << 8),
            4 => self.set_reu(self.base_reu & 0xff_ff00 | u32::from(v)),
            5 => self.set_reu(self.base_reu & 0xff_00ff | u32::from(v) << 8),
            6 => self.set_reu(self.base_reu & 0x00_ffff | u32::from(v & self.bank_bits()) << 16),
            7 => self.set_length(self.base_length & 0xff00 | u16::from(v)),
            8 => self.set_length(self.base_length & 0x00ff | u16::from(v) << 8),
            9 => {
                self.int_mask = v & 0xe0;
                self.update_irq();
            }
            10 => self.addr_ctrl = v & 0xc0,
            _ => {}
        }
    }

    fn set_c64(&mut self, a: u16) {
        self.base_c64 = a;
        self.c64_addr = a;
    }

    fn set_reu(&mut self, a: u32) {
        self.base_reu = a;
        self.reu_addr = a;
    }

    fn set_length(&mut self, n: u16) {
        self.base_length = n;
        self.length = n;
    }

    /// A CPU write anywhere: a write to `$FF00` starts an armed command.
    #[inline]
    pub fn cpu_write(&mut self, addr: u16) {
        if addr == 0xff00 && self.dma == Dma::Armed {
            self.dma = Dma::Running { swap: None };
        }
    }

    /// The REU holds /DMA: the CPU must not run this cycle.
    #[inline]
    pub fn dma_active(&self) -> bool {
        matches!(self.dma, Dma::Running { .. })
    }

    /// /IRQ.
    pub fn irq(&self) -> bool {
        self.status & status::IRQ != 0
    }

    fn update_irq(&mut self) {
        let pending = self.int_mask & mask::ENABLE != 0
            && ((self.int_mask & mask::END_OF_BLOCK != 0
                && self.status & status::END_OF_BLOCK != 0)
                || (self.int_mask & mask::VERIFY != 0 && self.status & status::FAULT != 0));
        if pending {
            self.status |= status::IRQ;
        }
    }

    fn reu_index(&self) -> usize {
        self.reu_addr as usize & (self.ram.len() - 1)
    }

    /// One DMA cycle on the C64's bus (call only while BA is high).
    pub fn step(&mut self, bus: &mut dyn Bus) {
        let Dma::Running { swap } = self.dma else {
            return;
        };
        let at = self.c64_addr;
        let r = self.reu_index();
        let mut fault = false;
        match self.command & cmd::TYPE {
            0 => {
                self.ram[r] = bus.read(u32::from(at), Size::Byte) as u8;
            }
            1 => bus.write(u32::from(at), Size::Byte, u32::from(self.ram[r])),
            2 => match swap {
                None => {
                    let c = bus.read(u32::from(at), Size::Byte) as u8;
                    self.dma = Dma::Running {
                        swap: Some((c, self.ram[r])),
                    };
                    return;
                }
                Some((c, e)) => {
                    bus.write(u32::from(at), Size::Byte, u32::from(e));
                    self.ram[r] = c;
                }
            },
            _ => fault = bus.read(u32::from(at), Size::Byte) as u8 != self.ram[r],
        }
        self.advance();
        if fault {
            self.status |= status::FAULT;
            if self.length == 1 && self.base_length != 1 {
                self.status |= status::END_OF_BLOCK;
            }
            self.end();
            return;
        }
        if self.dma_active() {
            self.dma = Dma::Running { swap: None };
        }
    }

    /// Past one byte; the block ends after the last.
    fn advance(&mut self) {
        let last = self.length == 1;
        if self.addr_ctrl & 0x80 == 0 {
            self.c64_addr = self.c64_addr.wrapping_add(1);
        }
        if self.addr_ctrl & 0x40 == 0 {
            self.reu_addr = (self.reu_addr + 1) & 0xff_ffff;
        }
        if last {
            self.status |= status::END_OF_BLOCK;
            self.end();
        } else {
            self.length = self.length.wrapping_sub(1);
        }
    }

    fn end(&mut self) {
        self.dma = Dma::Idle;
        self.command = (self.command & !cmd::EXECUTE) | cmd::NO_FF00;
        if self.command & cmd::AUTOLOAD != 0 {
            self.c64_addr = self.base_c64;
            self.reu_addr = self.base_reu;
            self.length = self.base_length;
        }
        self.update_irq();
    }

    /// The whole REU with its RAM.
    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"REU0", 1, |w| {
            w.bytes(&self.ram);
            w.u8(self.status);
            w.u8(self.command);
            w.u16(self.c64_addr);
            w.u32(self.reu_addr);
            w.u16(self.length);
            w.u16(self.base_c64);
            w.u32(self.base_reu);
            w.u16(self.base_length);
            w.u8(self.int_mask);
            w.u8(self.addr_ctrl);
            match self.dma {
                Dma::Idle => w.u8(0),
                Dma::Armed => w.u8(1),
                Dma::Running { swap: None } => w.u8(2),
                Dma::Running { swap: Some((c, e)) } => {
                    w.u8(3);
                    w.u8(c);
                    w.u8(e);
                }
            }
        });
    }

    /// Atomic; the size must match the configured REU.
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"REU0", 1)?;
        let ram = c.bytes()?;
        if ram.len() != self.ram.len() {
            return Err(StateError::Invalid("REU size differs"));
        }
        let mut n = Self::new(ram.len());
        n.status = c.u8()?;
        n.command = c.u8()?;
        n.c64_addr = c.u16()?;
        n.reu_addr = c.u32()? & 0xff_ffff;
        n.length = c.u16()?;
        n.base_c64 = c.u16()?;
        n.base_reu = c.u32()? & 0xff_ffff;
        n.base_length = c.u16()?;
        n.int_mask = c.u8()? & 0xe0;
        n.addr_ctrl = c.u8()? & 0xc0;
        n.dma = match c.u8()? {
            0 => Dma::Idle,
            1 => Dma::Armed,
            2 => Dma::Running { swap: None },
            3 => Dma::Running {
                swap: Some((c.u8()?, c.u8()?)),
            },
            _ => return Err(StateError::Invalid("REU DMA state")),
        };
        n.ram = ram.to_vec();
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 64 KB of C64 memory as a bus.
    struct Mem(Vec<u8>);

    impl Bus for Mem {
        fn read(&mut self, a: u32, _: Size) -> u32 {
            u32::from(self.0[a as usize & 0xffff])
        }
        fn peek(&self, a: u32, _: Size) -> u32 {
            u32::from(self.0[a as usize & 0xffff])
        }
        fn write(&mut self, a: u32, _: Size, v: u32) {
            self.0[a as usize & 0xffff] = v as u8;
        }
        fn addr_bits(&self) -> u8 {
            16
        }
    }

    fn setup(reu: &mut Reu, c64: u16, at: u32, len: u16) {
        reu.write(0xdf02, c64 as u8);
        reu.write(0xdf03, (c64 >> 8) as u8);
        reu.write(0xdf04, at as u8);
        reu.write(0xdf05, (at >> 8) as u8);
        reu.write(0xdf06, (at >> 16) as u8);
        reu.write(0xdf07, len as u8);
        reu.write(0xdf08, (len >> 8) as u8);
    }

    /// Run the DMA to the end; the cycles it took.
    fn run(reu: &mut Reu, mem: &mut Mem) -> u32 {
        let mut n = 0;
        while reu.dma_active() {
            reu.step(mem);
            n += 1;
            assert!(n < 1_000_000, "DMA does not end");
        }
        n
    }

    #[test]
    fn stash_fetch_swap_and_verify() {
        let mut mem = Mem((0..65536).map(|i| i as u8).collect());
        let mut reu = Reu::new(512 << 10);
        assert_eq!(reu.peek(0xdf00) & status::SIZE, status::SIZE);
        assert_eq!(reu.peek(0xdf06), 0xf8);
        // C64 $1000 → REU $07FFF0, 32 bytes: one cycle a byte, the REU
        // address wraps at 512 KB.
        setup(&mut reu, 0x1000, 0x07_fff0, 32);
        reu.write(0xdf01, cmd::EXECUTE | cmd::NO_FF00);
        assert_eq!(run(&mut reu, &mut mem), 32);
        assert_eq!(reu.ram()[0x7_fff0], 0x00);
        assert_eq!(reu.ram()[0x0f], 0x1f);
        // Registers after the block: past it, length 1.
        assert_eq!((reu.peek(0xdf02), reu.peek(0xdf03)), (0x20, 0x10));
        assert_eq!(reu.peek(0xdf06), 0xf8);
        assert_eq!((reu.peek(0xdf07), reu.peek(0xdf08)), (1, 0));
        assert_eq!(
            reu.peek(0xdf01) & (cmd::EXECUTE | cmd::NO_FF00),
            cmd::NO_FF00
        );
        assert_eq!(
            reu.read(0xdf00) & status::END_OF_BLOCK,
            status::END_OF_BLOCK
        );
        assert_eq!(reu.peek(0xdf00) & 0xe0, 0);
        // Fetch with autoload and a fixed C64 address.
        setup(&mut reu, 0x2000, 0x07_fff0, 4);
        reu.write(0xdf0a, 0x80);
        reu.write(0xdf01, cmd::EXECUTE | cmd::NO_FF00 | cmd::AUTOLOAD | 1);
        run(&mut reu, &mut mem);
        assert_eq!(mem.0[0x2000], 0x03);
        assert_eq!(mem.0[0x2001], 0x01);
        assert_eq!((reu.peek(0xdf07), reu.peek(0xdf02)), (4, 0x00));
        reu.write(0xdf0a, 0);
        // Swap: two cycles a byte.
        mem.0[0x3000..0x3002].copy_from_slice(&[0xaa, 0xbb]);
        reu.ram_mut()[0x100..0x102].copy_from_slice(&[1, 2]);
        setup(&mut reu, 0x3000, 0x100, 2);
        reu.write(0xdf01, cmd::EXECUTE | cmd::NO_FF00 | 2);
        assert_eq!(run(&mut reu, &mut mem), 4);
        assert_eq!((mem.0[0x3000], mem.0[0x3001]), (1, 2));
        assert_eq!(reu.ram()[0x100..0x102], [0xaa, 0xbb]);
        // Verify stops at the first difference and interrupts (the status
        // keeps its flags until read).
        reu.read(0xdf00);
        reu.write(0xdf09, mask::ENABLE | mask::VERIFY);
        reu.ram_mut()[0x100..0x103].copy_from_slice(&[1, 2, 0x55]);
        mem.0[0x3002] = 0x56;
        setup(&mut reu, 0x3000, 0x100, 10);
        reu.write(0xdf01, cmd::EXECUTE | cmd::NO_FF00 | 3);
        assert_eq!(run(&mut reu, &mut mem), 3);
        assert!(reu.irq());
        assert_eq!(reu.peek(0xdf02), 0x03);
        assert_eq!(reu.read(0xdf00) & 0xe0, status::IRQ | status::FAULT);
        assert!(!reu.irq());
    }

    #[test]
    fn ff00_trigger_reset_and_state() {
        let mut mem = Mem(vec![7; 65536]);
        let mut reu = Reu::new(128 << 10);
        assert_eq!(reu.peek(0xdf00) & status::SIZE, 0);
        // Registers mirror every 32 bytes.
        reu.write(0xdf22, 0x34);
        assert_eq!(reu.peek(0xdf02), 0x34);
        assert_eq!(reu.peek(0xdf1f), 0xff);
        setup(&mut reu, 0, 0, 3);
        reu.write(0xdf01, cmd::EXECUTE);
        assert!(!reu.dma_active());
        reu.cpu_write(0xfeff);
        assert!(!reu.dma_active());
        reu.cpu_write(0xff00);
        assert!(reu.dma_active());
        reu.step(&mut mem);
        let mut w = StateWriter::new();
        reu.save(&mut w);
        let bytes = w.into_bytes();
        let mut other = Reu::new(128 << 10);
        other.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(other, reu);
        assert!(Reu::new(256 << 10)
            .load(&mut StateReader::new(&bytes))
            .is_err());
        let before = other.clone();
        assert!(other
            .load(&mut StateReader::new(&bytes[..bytes.len() / 2]))
            .is_err());
        assert_eq!(other, before);
        run(&mut reu, &mut mem);
        assert_eq!(reu.ram()[..3], [7, 7, 7]);
        reu.reset();
        assert_eq!(reu.ram()[..3], [7, 7, 7]);
        assert_eq!(reu.peek(0xdf07), 0xff);
        assert_eq!(size_option("1764"), Some(256 << 10));
        assert_eq!(size_option("3m"), None);
    }
}
