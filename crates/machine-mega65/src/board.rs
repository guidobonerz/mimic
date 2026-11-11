//! Everything on the bus but the CPU: memory, the C64 and `$D030` banking,
//! the I/O area with its personalities.
//!
//! **Physical addresses (28 bits).** `$0000000`/`$0000001` the CPU port;
//! `$0000002`–`$005FFFF` chip RAM (384 KB), `$20000`–`$3FFFF` holding the
//! ROM, read-only unless the hypervisor lifts the protection; writes to
//! `$1F800`–`$1FFFF` also go to the first 2 KB of colour RAM and back;
//! `$FF7E000`–`$FF7EFFF` the VIC-IV's character ROM (RAM); `$FF80000`–
//! `$FF87FFF` colour RAM (32 KB); `$FFD0000`, `$FFD1000`, `$FFD2000`,
//! `$FFD3000` the I/O area in the C64, C65, Ethernet and MEGA65
//! personality. Everything else reads 0 (attic RAM comes later).
//!
//! **CPU addresses MAP leaves alone** (16 bits): the C64 port's LORAM,
//! HIRAM, CHAREN as on the C64 (BASIC `$2A000` at `$A000`, KERNAL `$2E000`
//! at `$E000`, I/O or the character ROM `$2D000` at `$D000`), and the
//! VIC-III's `$D030` bits: ROM8 `$38000` at `$8000`, ROMA `$3A000` at
//! `$A000`, ROMC `$2C000` at `$C000`, ROME `$3E000` at `$E000` (the C65
//! specification's mode table: C65 BASIC's graphics code at `$8000`–
//! `$BFFF`, the interface ROM at `$C000`; the guide's table says `$28000`
//! for ROM8, which contradicts that). CRAM2K puts colour RAM's second KB
//! over the CIAs at `$DC00`–`$DFFF`. Writes under ROM go to RAM.
//!
//! Sources: the MEGA65 user guide (`appendix-memorymap.tex`,
//! `appendix-45gs02-registers.tex`); the `@IO` register descriptions of the
//! MEGA65 core (`gs4510.vhdl`: the CPU port, `$00` = 65/64 forcing 40 MHz);
//! the C65 system specification (sections 1.5.3, 1.5.4: the ROM layout
//! and memory configurations).

use chips_cbm::cia::{Cia, CiaModel};
use chips_cbm::sid::{Sid, SidModel};
use emu_core::audio::Decimator;

use crate::keyboard::{self, Keyboard};
use cpu_45gs02::{Speed, ADDR_MASK, UNMAPPED};
use emu_core::{StateError, StateReader, StateWriter};

use crate::vic::{Personality, Vic};

pub const CHIP_RAM: usize = 0x6_0000;
/// A page of [`Board`]'s read table that is not plain memory.
const NO_PAGE: u32 = u32::MAX;
/// The SIDs' clock: 63 cycles a line, 312 lines, 50 fields a second.
pub const SID_HZ: u32 = 63 * 312 * 50;

/// The pot lines the SIDs see: CIA 1 PA6 switches control port 1's
/// POTX/POTY to them, PA7 port 2's (as on the C64).
fn pot_lines(cia1_pa: u8, ports: [[u16; 2]; 2]) -> [u16; 2] {
    let mut pots = [u16::MAX; 2];
    for (bit, port) in [(0x40, ports[0]), (0x80, ports[1])] {
        if cia1_pa & bit != 0 {
            for (p, r) in pots.iter_mut().zip(port) {
                *p = (*p).min(r);
            }
        }
    }
    pots
}
pub const COLOUR_RAM: usize = 0x8000;
pub const ROM_START: usize = 0x2_0000;
pub const ROM_END: usize = 0x4_0000;
const COLOUR_BASE: u32 = 0x0ff8_0000;
const CHARROM_BASE: u32 = 0x0ff7_e000;
const IO_BASE: u32 = 0x0ffd_0000;
/// The sector buffers as normal programs see them (repeated 8 times).
const SECTOR_BUFFER: u32 = 0x0ffd_6000;

#[derive(Clone, Debug)]
pub struct Board {
    pub chip: Vec<u8>,
    pub colour: Vec<u8>,
    /// The VIC-IV's character generator (written by the hypervisor).
    pub charrom: Vec<u8>,
    /// `$20000`–`$3FFFF` read-only.
    pub rom_protect: bool,
    pub port_ddr: u8,
    pub port_data: u8,
    /// `$00` = 65 forces 40 MHz (64 releases it).
    pub force_fast: bool,
    pub vic: Vic,
    pub cia1: Cia,
    pub cia2: Cia,
    /// The four SIDs: right #1 (`$D400`), right #2 (`$D420`), left #1
    /// (`$D440`), left #2 (`$D460`), mirrored at `$D480`–`$D4FF`; clocked
    /// at the C64's rate.
    pub sids: [Sid; 4],
    /// Left and right output, resampled to 48 kHz.
    audio: [Decimator; 2],
    /// Interleaved stereo samples not yet taken.
    pub samples: Vec<i16>,
    pub keyboard: Keyboard,
    /// Control ports 1 and 2: lines pulled low, POTX/POTY.
    pub port_lines: [u32; 2],
    pub port_analog: [[u16; 2]; 2],
    /// The F011 and SD card controller ([`crate::storage`]).
    pub storage: crate::storage::Storage,
    /// The serial bus (IEC) as the drives leave it (true = pulled low);
    /// CIA 2 PA3–PA5 drive ATN, CLK, DATA through inverters, PA6/PA7 read
    /// CLK and DATA.
    pub iec: drives_cbm::IecLines,
    /// MEGA65 devices at `$D600`–`$D7FF` as written (the UART, the
    /// keyboard, DMAgic, the math unit come later).
    pub devices: [u8; 0x200],
    /// `$D629`: the model ID (`$03` MEGA65 R3; the guide's
    /// `appendix-target-specific.tex`), read only.
    pub model_id: u8,
    /// Reads of unmapped CPU addresses `$8000`–`$FFFF` by 4 KB page: the
    /// chip RAM address the banking selects, or [`NO_PAGE`] for I/O.
    /// Recomputed when the port or `$D030` change ([`Board::remap`]).
    pages: [u32; 16],
    /// A DMA job was started (run by the machine after the instruction).
    pub dma: Option<crate::dma::Trigger>,
    /// Hypervisor traps the program asked for (`$D640`–`$D67F` writes).
    pub traps: u32,
    /// A trap was written: its number (0–63) and A (run by the machine
    /// after the instruction, [`crate::hyper`]).
    pub trap: Option<(u8, u8)>,
}

impl Board {
    pub fn new(rom: &[u8]) -> Self {
        let mut chip = vec![0; CHIP_RAM];
        let n = rom.len().min(ROM_END - ROM_START);
        chip[ROM_START..ROM_START + n].copy_from_slice(&rom[..n]);
        // The hypervisor loads the VIC-IV's character ROM; here the C64
        // character set of the ROM ($2D000).
        let mut charrom = vec![0; 0x1000];
        charrom.copy_from_slice(&chip[0x2_d000..0x2_e000]);
        let mut b = Self {
            chip,
            colour: vec![0; COLOUR_RAM],
            charrom,
            rom_protect: true,
            port_ddr: 0,
            port_data: 0,
            force_fast: false,
            vic: Vic::new(),
            cia1: Cia::new(CiaModel::Mos6526A),
            cia2: Cia::new(CiaModel::Mos6526A),
            sids: std::array::from_fn(|_| Sid::new(SidModel::Mos6581, SID_HZ)),
            audio: [0, 1]
                .map(|_| Decimator::new(SID_HZ, emu_core::SAMPLE_RATE).expect("valid rates")),
            samples: Vec::new(),
            keyboard: Keyboard::default(),
            port_lines: [0; 2],
            port_analog: [[u16::MAX; 2]; 2],
            iec: drives_cbm::IecLines::default(),
            storage: crate::storage::Storage::default(),
            devices: Self::devices_at_start(),
            model_id: 3,
            pages: [NO_PAGE; 16],
            dma: None,
            traps: 0,
            trap: None,
        };
        b.remap();
        b
    }

    /// The MEGA65 devices as the hypervisor leaves them: the DMAgic in the
    /// F018B list format (the guide: the hypervisor picks it for a newer
    /// ROM).
    fn devices_at_start() -> [u8; 0x200] {
        let mut d = [0; 0x200];
        d[0x103] = 1;
        d
    }

    /// The CPU's speed as the registers select it.
    pub fn speed(&self) -> Speed {
        let r = &self.vic.regs;
        if self.force_fast || (r[0x31] & 0x40 != 0 && r[0x54] & 0x40 != 0) {
            Speed::Mhz40
        } else if r[0x31] & 0x40 != 0 {
            Speed::Mhz3_5
        } else if self.vic.c128_fast {
            Speed::Mhz2
        } else {
            Speed::Mhz1
        }
    }

    /// The port's LORAM, HIRAM, CHAREN lines (inputs read high).
    fn port_lines(&self) -> u8 {
        (self.port_data | !self.port_ddr) & 7
    }

    /// The port's pins: inputs pulled high, bit 6 the C65's CAPS LOCK (low
    /// while locked).
    fn port_read(&self) -> u8 {
        let caps = if self.keyboard.caps { 0 } else { 0x40 };
        (self.port_data & self.port_ddr) | (!self.port_ddr & (0x17 | caps))
    }

    /// CIA 1's port levels: the joysticks (port 1 on B, port 2 on A) and
    /// the keyboard matrix with column 8 (`$D607` bit 1 driven low).
    fn cia1_pins(&self) -> (u8, u8) {
        let joy = |lines: u32| -> u8 { !((lines & 0x0f) as u8 | ((lines >> 1) & 0x10) as u8) };
        let a = self.cia1.port_a_out() & joy(self.port_lines[1]);
        let b = self.cia1.port_b_out() & joy(self.port_lines[0]);
        let col8 = self.devices[0x08] & 2 != 0 && self.devices[0x07] & 2 == 0;
        self.keyboard.resolve(a, b, col8)
    }

    /// CIA 2 port A's inputs: PA6 CLK and PA7 DATA (1 = released).
    fn cia2_pa_in(&self) -> u8 {
        0x3f | if self.iec.clk { 0 } else { 0x40 } | if self.iec.data { 0 } else { 0x80 }
    }

    /// What the MEGA65 pulls on the serial bus (from CIA 2 port A).
    pub fn iec_out(&self) -> drives_cbm::IecLines {
        let pa = self.cia2.port_a_out();
        drives_cbm::IecLines {
            atn: pa & 0x08 != 0,
            clk: pa & 0x10 != 0,
            data: pa & 0x20 != 0,
            srq: false,
        }
    }

    /// One SID cycle (the C64's rate): the four SIDs with the pot lines,
    /// mixed to left and right.
    pub fn clock_sids(&mut self) {
        let pots = pot_lines(self.cia1.port_a_out(), self.port_analog);
        let mut out = [0i32; 4];
        for (o, s) in out.iter_mut().zip(self.sids.iter_mut()) {
            s.set_pots(pots);
            *o = s.clock();
        }
        let left = (out[2] + out[3]) / 2;
        let right = (out[0] + out[1]) / 2;
        let l = self.audio[0].push(left);
        let r = self.audio[1].push(right);
        if let (Some(l), Some(r)) = (l, r) {
            if self.samples.len() < emu_core::SAMPLE_RATE as usize * 8 {
                self.samples.push(l);
                self.samples.push(r);
            }
        }
    }

    /// The multiplier and divider (`$D770`–`$D777` in, the outputs always
    /// up to date): MULTOUT (`$D778`, 64 bits) = A × B unsigned, DIVOUT
    /// (`$D768`, 64 bits) = A / B with 32 bits of fraction below the whole
    /// part (all ones for B = 0, an assumption). The divider is done at
    /// once: DIVBUSY (`$D70F` bit 7) is never seen set.
    fn math(&mut self) {
        let d = &self.devices;
        let a = u64::from(u32::from_le_bytes([d[0x170], d[0x171], d[0x172], d[0x173]]));
        let b = u64::from(u32::from_le_bytes([d[0x174], d[0x175], d[0x176], d[0x177]]));
        let product = a * b;
        let quotient = if b == 0 {
            u64::MAX
        } else {
            ((u128::from(a) << 32) / u128::from(b)) as u64
        };
        self.devices[0x178..0x180].copy_from_slice(&product.to_le_bytes());
        self.devices[0x168..0x170].copy_from_slice(&quotient.to_le_bytes());
    }

    /// The MEGA65 devices' registers read (`$D600`–`$D7FF`).
    fn device_peek(&self, reg: u16) -> u8 {
        let k = &self.keyboard;
        match reg {
            0x60a => k.first().map_or(0, |t| t.mods | 0x80),
            0x60f => (self.devices[0x0f] & !3) | u8::from(k.left) | u8::from(k.up) << 1,
            0x610 => k.first().map_or(0, |t| t.ascii),
            0x611 => k.mods(),
            0x613 => k.column(k.select),
            0x614 => k.select,
            0x619 => k.first().map_or(0, |t| keyboard::petscii(t.ascii)),
            0x629 => self.model_id,
            // The multiplier and divider are never busy here.
            0x70f => self.devices[0x10f] & 0x3f,
            _ => self.devices[usize::from(reg - 0x600)],
        }
    }

    fn port_write(&mut self, reg: u8, v: u8) {
        if reg == 0 {
            match v {
                65 => self.force_fast = true,
                64 => self.force_fast = false,
                _ => self.port_ddr = v,
            }
        } else {
            self.port_data = v;
        }
        self.remap();
    }

    /// The read page table after a banking change.
    pub fn remap(&mut self) {
        for page in 8..16u16 {
            self.pages[usize::from(page)] = match self.decode16(page << 12) {
                Target::Ram(a) | Target::Rom(a) => a,
                _ => NO_PAGE,
            };
        }
    }

    /// Where an unmapped CPU address goes.
    fn decode16(&self, a: u16) -> Target {
        // The VIC-III's register keeps its value while the C64
        // personality hides it.
        let d030 = self.vic.regs[0x30];
        let lines = self.port_lines();
        let (loram, hiram, charen) = (lines & 1 != 0, lines & 2 != 0, lines & 4 != 0);
        let off = u32::from(a);
        match a {
            0x0000 | 0x0001 => Target::Port(a as u8),
            0x8000..=0x9fff if d030 & 0x08 != 0 => Target::Rom(0x3_8000 + off - 0x8000),
            0xa000..=0xbfff if d030 & 0x10 != 0 => Target::Rom(0x3_a000 + off - 0xa000),
            0xa000..=0xbfff if loram && hiram => Target::Rom(0x2_a000 + off - 0xa000),
            0xc000..=0xcfff if d030 & 0x20 != 0 => Target::Rom(0x2_c000 + off - 0xc000),
            0xd000..=0xdfff if loram || hiram => {
                if charen {
                    Target::Io(self.vic.personality, a & 0x0fff)
                } else {
                    Target::Rom(0x2_d000 + off - 0xd000)
                }
            }
            0xe000..=0xffff if d030 & 0x80 != 0 => Target::Rom(0x3_e000 + off - 0xe000),
            0xe000..=0xffff if hiram => Target::Rom(0x2_e000 + off - 0xe000),
            _ => Target::Ram(off),
        }
    }

    fn decode28(&self, a: u32) -> Target {
        let a = a & ADDR_MASK;
        match a {
            0 | 1 => Target::Port(a as u8),
            _ if (a as usize) < CHIP_RAM => Target::Ram(a),
            _ if (CHARROM_BASE..CHARROM_BASE + 0x1000).contains(&a) => {
                Target::CharRom(a - CHARROM_BASE)
            }
            _ if (COLOUR_BASE..COLOUR_BASE + COLOUR_RAM as u32).contains(&a) => {
                Target::Colour(a - COLOUR_BASE)
            }
            _ if (SECTOR_BUFFER..SECTOR_BUFFER + 0x1000).contains(&a) => {
                Target::SectorBuf((a & 0x1ff) as u16)
            }
            _ if (IO_BASE..IO_BASE + 0x4000).contains(&a) => {
                let p = Personality::from_index(((a - IO_BASE) >> 12) as u8)
                    .unwrap_or(Personality::Mega65);
                Target::Io(p, (a & 0x0fff) as u16)
            }
            _ => Target::None,
        }
    }

    fn decode(&self, addr: u32) -> Target {
        if addr & UNMAPPED != 0 {
            self.decode16(addr as u16)
        } else {
            self.decode28(addr)
        }
    }

    #[inline]
    pub fn read(&mut self, addr: u32) -> u8 {
        if let Some(v) = self.fast_ram(addr) {
            return v;
        }
        match self.decode(addr) {
            Target::Io(p, reg) => self.io_read(p, reg),
            t => self.peek_target(t),
        }
    }

    pub fn peek(&self, addr: u32) -> u8 {
        if let Some(v) = self.fast_ram(addr) {
            return v;
        }
        self.peek_target(self.decode(addr))
    }

    /// Plain RAM without the decoding: `$0002`–`$7FFF` of the CPU's view
    /// (no banking reaches it) and chip RAM by its 28-bit address.
    #[inline]
    fn fast_ram(&self, addr: u32) -> Option<u8> {
        let a = if addr & UNMAPPED != 0 {
            let a = addr & 0xffff;
            if a >= 0x8000 {
                let base = self.pages[(a >> 12) as usize];
                return (base != NO_PAGE).then(|| self.chip[(base + (a & 0x0fff)) as usize]);
            }
            a
        } else {
            addr
        };
        (2..CHIP_RAM as u32)
            .contains(&a)
            .then(|| self.chip[a as usize])
    }

    fn peek_target(&self, t: Target) -> u8 {
        match t {
            Target::Port(0) => self.port_ddr,
            Target::Port(_) => self.port_read(),
            Target::Ram(a) | Target::Rom(a) => self.chip[a as usize],
            Target::Colour(a) => self.colour[a as usize],
            Target::CharRom(a) => self.charrom[a as usize],
            Target::SectorBuf(i) => self.storage.buffer_peek(i),
            Target::Io(p, reg) => self.io_peek(p, reg),
            Target::None => 0,
        }
    }

    pub fn write(&mut self, addr: u32, v: u8) {
        let t = self.decode(addr);
        match t {
            Target::Port(r) => self.port_write(r, v),
            // Under ROM the RAM of bank 0 takes the write.
            Target::Rom(_) => {
                if addr & UNMAPPED != 0 {
                    self.write_ram(u32::from(addr as u16), v);
                }
            }
            Target::Ram(a) => self.write_ram(a, v),
            Target::Colour(a) => self.write_colour(a as usize, v),
            Target::CharRom(a) => self.charrom[a as usize] = v,
            Target::SectorBuf(i) => self.storage.buffer_write(i, v),
            Target::Io(p, reg) => self.io_write(p, reg, v),
            Target::None => {}
        }
    }

    fn write_ram(&mut self, a: u32, v: u8) {
        let a = a as usize;
        if self.rom_protect && (ROM_START..ROM_END).contains(&a) {
            return;
        }
        self.chip[a] = v;
        if (0x1_f800..0x2_0000).contains(&a) {
            self.colour[a - 0x1_f800] = v;
        }
    }

    fn write_colour(&mut self, a: usize, v: u8) {
        self.colour[a] = v;
        if a < 0x800 {
            self.chip[0x1_f800 + a] = v;
        }
    }

    fn io_peek(&self, p: Personality, reg: u16) -> u8 {
        let cram2k = p != Personality::C64 && self.vic.regs[0x30] & 1 != 0;
        let mega = matches!(p, Personality::Mega65 | Personality::Ethernet);
        match reg {
            0x080..=0x08f if p != Personality::C64 => self.storage.fdc_peek(reg),
            0x680..=0x693 if mega => self.storage.sd_peek(reg - 0x680),
            0xe00..=0xfff if self.storage.mapped => self.storage.buffer_peek(reg),
            0x000..=0x0ff => self.vic.peek(p, reg & 0x7f),
            0x100..=0x3ff if p != Personality::C64 => self.vic.palette_peek(reg - 0x100),
            0x100..=0x3ff => self.vic.peek(p, reg & 0x3f),
            0x400..=0x4ff => self.sids[usize::from(reg >> 5 & 3)].peek((reg & 0x1f) as u8),
            0x600..=0x7ff if p != Personality::C64 => self.device_peek(reg),
            0x800..=0xbff => self.colour[usize::from(reg - 0x800)],
            0xc00..=0xfff if cram2k => self.colour[usize::from(reg - 0x800)],
            0xc00..=0xcff => {
                let (a, b) = self.cia1_pins();
                self.cia1.peek((reg & 15) as u8, a, b)
            }
            0xd00..=0xdff => self.cia2.peek((reg & 15) as u8, self.cia2_pa_in(), 0xff),
            _ => 0xff,
        }
    }

    fn io_read(&mut self, p: Personality, reg: u16) -> u8 {
        let cram2k = p != Personality::C64 && self.vic.regs[0x30] & 1 != 0;
        match reg {
            0x080..=0x08f if p != Personality::C64 => self.storage.fdc_read(reg),
            0x000..=0x0ff => self.vic.read(p, reg & 0x7f),
            0x400..=0x4ff => self.sids[usize::from(reg >> 5 & 3)].read((reg & 0x1f) as u8),
            0xc00..=0xcff if !cram2k => {
                let (a, b) = self.cia1_pins();
                self.cia1.read((reg & 15) as u8, a, b)
            }
            0xd00..=0xdff if !cram2k => {
                let pa = self.cia2_pa_in();
                self.cia2.read((reg & 15) as u8, pa, 0xff)
            }
            _ => self.io_peek(p, reg),
        }
    }

    fn io_write(&mut self, p: Personality, reg: u16, v: u8) {
        let cram2k = p != Personality::C64 && self.vic.regs[0x30] & 1 != 0;
        let mega = matches!(p, Personality::Mega65 | Personality::Ethernet);
        match reg {
            0x080..=0x08f if p != Personality::C64 => self.storage.fdc_write(reg, v),
            0x680..=0x693 if mega => self.storage.sd_write(reg - 0x680, v),
            0xe00..=0xfff if self.storage.mapped => self.storage.buffer_write(reg, v),
            0x000..=0x0ff => {
                self.vic.write(p, reg & 0x7f, v);
                if reg & 0x7f == 0x30 {
                    self.remap();
                }
            }
            0x100..=0x3ff if p != Personality::C64 => self.vic.palette_write(reg - 0x100, v),
            0x100..=0x3ff => self.vic.write(p, reg & 0x3f, v),
            0x400..=0x4ff => self.sids[usize::from(reg >> 5 & 3)].write((reg & 0x1f) as u8, v),
            0x610 | 0x619 if p != Personality::C64 => self.keyboard.dequeue(),
            0x614 if p != Personality::C64 => self.keyboard.select = v & 0x0f,
            0x615..=0x617 if p != Personality::C64 => {
                self.keyboard.virt[usize::from(reg - 0x615)] = v & 0x7f;
            }
            0x63c if p != Personality::C64 => {
                self.devices[0x3c] = v;
                for (i, s) in self.sids.iter_mut().enumerate() {
                    s.set_model(if v >> i & 1 != 0 {
                        SidModel::Mos8580
                    } else {
                        SidModel::Mos6581
                    });
                }
            }
            0x640..=0x67f if matches!(p, Personality::Mega65 | Personality::Ethernet) => {
                self.traps += 1;
                self.trap = Some(((reg - 0x640) as u8, v));
            }
            0x770..=0x777 if p != Personality::C64 => {
                self.devices[usize::from(reg - 0x600)] = v;
                self.math();
            }
            0x700..=0x70f if p != Personality::C64 => {
                use crate::dma::Trigger;
                self.devices[usize::from(reg - 0x600)] = v;
                let r = reg & 0x0f;
                if r == 2 {
                    // Writing the bank clears the megabyte.
                    self.devices[0x104] = 0;
                }
                self.dma = match r {
                    0x0 => Some(Trigger::F018),
                    0x5 => Some(Trigger::Enhanced),
                    0x6 => Some(Trigger::Mapped),
                    0x7 => Some(Trigger::Inline),
                    _ => self.dma,
                };
                if matches!(r, 0x5 | 0x6 | 0xe) {
                    // The LSB lives at $D700.
                    self.devices[0x100] = v;
                }
            }
            0x600..=0x7ff if p != Personality::C64 => self.devices[usize::from(reg - 0x600)] = v,
            0x800..=0xbff => self.write_colour(usize::from(reg - 0x800), v),
            0xc00..=0xfff if cram2k => self.write_colour(usize::from(reg - 0x800), v),
            0xc00..=0xcff => self.cia1.write((reg & 15) as u8, v),
            0xd00..=0xdff => {
                self.cia2.write((reg & 15) as u8, v);
                // The VIC bank bits (a hot register).
                self.vic.set_cia2_pa(self.cia2.port_a_out());
            }
            _ => {}
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        use emu_core::Savestate;
        w.raw(&self.chip);
        w.raw(&self.colour);
        w.raw(&self.charrom);
        w.bool(self.rom_protect);
        w.u8(self.port_ddr);
        w.u8(self.port_data);
        w.bool(self.force_fast);
        self.vic.save(w);
        self.cia1.save(w);
        self.cia2.save(w);
        for s in &self.sids {
            s.save(w);
        }
        self.keyboard.save(w);
        self.storage.save(w);
        w.bool(self.iec.atn);
        w.bool(self.iec.clk);
        w.bool(self.iec.data);
        w.u32(self.port_lines[0]);
        w.u32(self.port_lines[1]);
        for p in self.port_analog {
            w.u16(p[0]);
            w.u16(p[1]);
        }
        w.raw(&self.devices);
        w.u8(match self.dma {
            None => 0,
            Some(crate::dma::Trigger::F018) => 1,
            Some(crate::dma::Trigger::Enhanced) => 2,
            Some(crate::dma::Trigger::Mapped) => 3,
            Some(crate::dma::Trigger::Inline) => 4,
        });
        w.u32(self.traps);
        w.bool(self.trap.is_some());
        let (t, a) = self.trap.unwrap_or((0, 0));
        w.u8(t);
        w.u8(a);
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        use emu_core::Savestate;
        r.raw_into(&mut self.chip)?;
        r.raw_into(&mut self.colour)?;
        r.raw_into(&mut self.charrom)?;
        self.rom_protect = r.bool()?;
        self.port_ddr = r.u8()?;
        self.port_data = r.u8()?;
        self.force_fast = r.bool()?;
        self.vic.load(r)?;
        self.cia1.load(r)?;
        self.cia2.load(r)?;
        for s in &mut self.sids {
            s.load(r)?;
        }
        self.keyboard.load(r)?;
        self.storage.load(r)?;
        self.iec = drives_cbm::IecLines {
            atn: r.bool()?,
            clk: r.bool()?,
            data: r.bool()?,
            srq: false,
        };
        self.port_lines = [r.u32()?, r.u32()?];
        for p in &mut self.port_analog {
            *p = [r.u16()?, r.u16()?];
        }
        r.raw_into(&mut self.devices)?;
        self.dma = match r.u8()? {
            0 => None,
            1 => Some(crate::dma::Trigger::F018),
            2 => Some(crate::dma::Trigger::Enhanced),
            3 => Some(crate::dma::Trigger::Mapped),
            4 => Some(crate::dma::Trigger::Inline),
            _ => return Err(StateError::Invalid("DMA trigger")),
        };
        self.traps = r.u32()?;
        let pending = r.bool()?;
        let (t, a) = (r.u8()? & 0x3f, r.u8()?);
        self.trap = pending.then_some((t, a));
        self.remap();
        Ok(())
    }
}

/// What an address selects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    Port(u8),
    Ram(u32),
    Rom(u32),
    Colour(u32),
    CharRom(u32),
    /// The sector buffer BUFSEL shows.
    SectorBuf(u16),
    Io(Personality, u16),
    None,
}
