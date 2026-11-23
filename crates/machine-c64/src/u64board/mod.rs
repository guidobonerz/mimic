//! The Ultimate 64 Elite II board around the C64: the RISC-V processor
//! that runs the board's firmware, its memory, and the bus to the blocks
//! of `chips-ultimate`.
//!
//! - Memory: 64 MB at address 0, mirrored wherever address bit 28 is
//!   clear; `$8000xxxx` is the boot ROM's block RAM (not present here,
//!   reads 0). Instruction fetches ignore bit 28.
//! - I/O: `$10000000`–`$10FFFFFF`, bytes; a 16- or 32-bit access is its
//!   bytes at ascending addresses. Nothing faults: what is not there reads
//!   0.
//! - Time: the board's clock is 100 MHz. The processor here takes four
//!   clocks for every instruction (25 million instructions a second, what
//!   the firmware's timing loops and its idle task were measured to need
//!   on the board); the firmware's own time comes from the ITU's timers,
//!   which count the clock.
//! - Start: the boot ROM is replaced by what it does ([`loader`]): the
//!   application record is copied out of the flash and started. An update
//!   file can be started in its place, as the firmware does for "Run
//!   Update": its record is the updater.
//!
//! Sources: UE2-C64U-Emulator `docs/hw/00-memory-map.md` (bus rules,
//! address map), `docs/hw/01-cpu-boot-memory.md`, `docs/ARCHITECTURE.md`
//! ("Hardware facts that shape the design": 25 MIPS).

pub(crate) mod cart;
pub(crate) mod drives;
pub(crate) mod glue;
pub mod loader;
pub mod net;
pub mod storage;

use chips_ultimate::board::Board;
use chips_ultimate::c64port::C64Port;
use chips_ultimate::flash::{FlashChip, FlashLayout, SpiFlash};
use chips_ultimate::itu::Itu;
use chips_ultimate::overlay::Overlay;
use chips_ultimate::rmii::Rmii;
use chips_ultimate::sdcard::SdCard;
use chips_ultimate::u64io::U64Io;
use chips_ultimate::usb::UsbHost;
use chips_ultimate::wifi::{PowerEvent, Wifi};
use chips_ultimate::{C64Side, EtherLink, IoCtx, IoDevice, Irq, Storage};
use cpu_rv32::{Rv32, Rv32Bus, StepResult};
use mimic_core::{StateError, StateReader, StateWriter};

/// 64 MB.
pub const RAM_SIZE: usize = 0x400_0000;
const RAM_MASK: usize = RAM_SIZE - 1;
/// Board clocks per instruction.
pub const CLOCKS_PER_INSTR: u64 = 4;
/// The most the console keeps (the oldest bytes go first).
const CONSOLE_KEEP: usize = 0x1_0000;
/// The most a saved target lies ahead of the clock (a run is a few
/// hundred clocks).
const CLOCK_SLACK: u64 = 1_000_000_000;

/// Which board: they differ in the FPGA (and with it the flash layout),
/// the flash chip and the second drive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Ultimate 64 Elite II: XC7A100T (FPGA type 3), S25FL128L.
    Elite2,
    /// Commodore 64 Ultimate: XC7A50T (FPGA type 2), W25Q128JV.
    C64Ultimate,
}

impl Kind {
    pub fn fpga_type(self) -> u8 {
        match self {
            Kind::Elite2 => 3,
            Kind::C64Ultimate => 2,
        }
    }

    pub fn flash_chip(self) -> FlashChip {
        match self {
            Kind::Elite2 => FlashChip::S25fl128l,
            Kind::C64Ultimate => FlashChip::W25q128jv,
        }
    }

    /// The capability word the firmware reads from the ITU: Ultimate 64,
    /// the FPGA type, cartridge port, hardware IEC, drive A (and B on the
    /// Commodore 64 Ultimate), the USB host (bit 23), Ethernet (bit 24). The bits of blocks that are not functional
    /// here stay clear, so the firmware does not use them.
    pub fn capabilities(self) -> u32 {
        match self {
            Kind::Elite2 => 0x3580_0222,
            Kind::C64Ultimate => 0x2580_0226,
        }
    }
}

/// Why the board's processor stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Halt {
    /// It runs.
    Running,
    /// The flash holds no application and no update file was given.
    NoFirmware,
    /// The firmware switched the machine off.
    PowerOff,
    /// An instruction the processor does not have (the word).
    Illegal(u32),
}

/// What the firmware asked of the machine during a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// Switch off.
    PowerOff,
    /// Start again from the flash (reboot, or the FPGA reloading).
    Reboot,
}

pub struct Elite2 {
    kind: Kind,
    pub cpu: Rv32,
    ram: Vec<u8>,
    /// The board's clock.
    now: u64,
    /// The clock the processor may run to.
    target: u64,
    irq: Irq,
    pub itu: Itu,
    pub flash: SpiFlash,
    pub overlay: Overlay,
    pub u64io: U64Io,
    pub misc: Board,
    pub wifi: Wifi,
    pub sd: SdCard,
    pub usb: UsbHost,
    pub rmii: Rmii,
    pub c64: C64Port,
    console: Vec<u8>,
    /// The earliest clock a block wants its tick.
    deadline: u64,
    halt: Halt,
}

/// The bus as the processor sees it during a run.
struct Sys<'a> {
    ram: &'a mut [u8],
    now: u64,
    irq: &'a mut Irq,
    itu: &'a mut Itu,
    flash: &'a mut SpiFlash,
    overlay: &'a mut Overlay,
    u64io: &'a mut U64Io,
    misc: &'a mut Board,
    wifi: &'a mut Wifi,
    sd: &'a mut SdCard,
    usb: &'a mut UsbHost,
    rmii: &'a mut Rmii,
    net: &'a mut dyn EtherLink,
    c64: &'a mut C64Port,
    console: &'a mut Vec<u8>,
    side: &'a mut dyn C64Side,
    storage: &'a mut dyn Storage,
    /// An I/O access was made: deadlines and the interrupt line may have
    /// changed.
    touched: bool,
}

/// The block an I/O address belongs to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Block {
    Itu,
    Flash,
    Wifi,
    Sd,
    Usb,
    Rmii,
    Misc,
    Overlay,
    U64Io,
    C64,
    None,
}

fn block(addr: u32) -> Block {
    match addr >> 16 {
        0x00 if addr < 0x100 => Block::Itu,
        0x06 => match addr >> 8 {
            0x602 => Block::Flash,
            0x609 => Block::Wifi,
            0x600 => Block::Sd,
            0x608 => Block::Rmii,
            _ if Board::claims(addr) => Block::Misc,
            _ => Block::None,
        },
        0x10 => match addr >> 8 {
            0x1003 => Block::C64,
            0x1004 => Block::U64Io,
            _ if Board::claims(addr) => Block::Misc,
            _ => Block::None,
        },
        0x14 => Block::Overlay,
        0x08 if UsbHost::claims(addr) => Block::Usb,
        0x20 if Board::claims(addr) => Block::Misc,
        _ if C64Port::claims(addr) => Block::C64,
        _ => Block::None,
    }
}

macro_rules! ctx {
    ($s:expr) => {
        IoCtx {
            now: $s.now,
            ram: &mut *$s.ram,
            irq: &mut *$s.irq,
            console: &mut *$s.console,
            c64: &mut *$s.side,
            storage: Some(&mut *$s.storage),
            net: Some(&mut *$s.net),
        }
    };
}

impl Sys<'_> {
    fn io_read(&mut self, addr: u32) -> u8 {
        self.touched = true;
        let mut ctx = ctx!(self);
        match block(addr) {
            Block::Itu => self.itu.read(addr, &mut ctx),
            Block::Flash => self.flash.read(addr, &mut ctx),
            Block::Wifi => self.wifi.read(addr, &mut ctx),
            Block::Sd => self.sd.read(addr, &mut ctx),
            Block::Usb => self.usb.read(addr, &mut ctx),
            Block::Rmii => self.rmii.read(addr, &mut ctx),
            Block::Misc => self.misc.read(addr, &mut ctx),
            Block::Overlay => self.overlay.read(addr, &mut ctx),
            Block::U64Io => self.u64io.read(addr, &mut ctx),
            Block::C64 => self.c64.read(addr, &mut ctx),
            Block::None => 0,
        }
    }

    fn io_write(&mut self, addr: u32, v: u8) {
        self.touched = true;
        let mut ctx = ctx!(self);
        match block(addr) {
            Block::Itu => self.itu.write(addr, v, &mut ctx),
            Block::Flash => self.flash.write(addr, v, &mut ctx),
            Block::Wifi => self.wifi.write(addr, v, &mut ctx),
            Block::Sd => self.sd.write(addr, v, &mut ctx),
            Block::Usb => self.usb.write(addr, v, &mut ctx),
            Block::Rmii => self.rmii.write(addr, v, &mut ctx),
            Block::Misc => self.misc.write(addr, v, &mut ctx),
            Block::Overlay => self.overlay.write(addr, v, &mut ctx),
            Block::U64Io => {
                self.u64io.write(addr, v, &mut ctx);
                // LEDSTRIP_EN: the pins the LED strip hangs on.
                let on = self.u64io.latch(chips_ultimate::u64io::reg::LEDSTRIP_EN) & 1 != 0;
                self.misc.set_led_enable(on);
            }
            Block::C64 => self.c64.write(addr, v, &mut ctx),
            Block::None => {}
        }
    }

    /// Where a data address leads: memory, I/O or nothing.
    fn is_ram(addr: u32) -> bool {
        addr & 0x1000_0000 == 0 && addr >> 16 != 0x8000
    }

    fn is_io(addr: u32) -> bool {
        addr >> 24 == 0x10
    }
}

impl Rv32Bus for Sys<'_> {
    #[inline]
    fn fetch(&mut self, addr: u32) -> u32 {
        if addr >> 16 == 0x8000 {
            return 0;
        }
        let a = addr as usize & RAM_MASK & !3;
        u32::from_le_bytes([
            self.ram[a],
            self.ram[a + 1],
            self.ram[a + 2],
            self.ram[a + 3],
        ])
    }

    #[inline]
    fn read8(&mut self, addr: u32) -> u8 {
        if Self::is_ram(addr) {
            self.ram[addr as usize & RAM_MASK]
        } else if Self::is_io(addr) {
            self.io_read(addr & 0xff_ffff)
        } else {
            0
        }
    }

    #[inline]
    fn read16(&mut self, addr: u32) -> u16 {
        u16::from_le_bytes([self.read8(addr), self.read8(addr.wrapping_add(1))])
    }

    #[inline]
    fn read32(&mut self, addr: u32) -> u32 {
        let a = addr as usize & RAM_MASK;
        if Self::is_ram(addr) && a <= RAM_SIZE - 4 {
            return u32::from_le_bytes([
                self.ram[a],
                self.ram[a + 1],
                self.ram[a + 2],
                self.ram[a + 3],
            ]);
        }
        u32::from_le_bytes([
            self.read8(addr),
            self.read8(addr.wrapping_add(1)),
            self.read8(addr.wrapping_add(2)),
            self.read8(addr.wrapping_add(3)),
        ])
    }

    #[inline]
    fn write8(&mut self, addr: u32, v: u8) {
        if Self::is_ram(addr) {
            self.ram[addr as usize & RAM_MASK] = v;
        } else if Self::is_io(addr) {
            self.io_write(addr & 0xff_ffff, v);
        }
    }

    #[inline]
    fn write16(&mut self, addr: u32, v: u16) {
        let b = v.to_le_bytes();
        self.write8(addr, b[0]);
        self.write8(addr.wrapping_add(1), b[1]);
    }

    #[inline]
    fn write32(&mut self, addr: u32, v: u32) {
        let a = addr as usize & RAM_MASK;
        if Self::is_ram(addr) && a <= RAM_SIZE - 4 {
            self.ram[a..a + 4].copy_from_slice(&v.to_le_bytes());
            return;
        }
        for (i, b) in v.to_le_bytes().into_iter().enumerate() {
            self.write8(addr.wrapping_add(i as u32), b);
        }
    }
}

impl Elite2 {
    pub fn new(kind: Kind) -> Self {
        let mut b = Self {
            kind,
            cpu: Rv32::default(),
            ram: vec![0; RAM_SIZE],
            now: 0,
            target: 0,
            irq: Irq::default(),
            itu: Itu::new(kind.capabilities()),
            flash: SpiFlash::new(kind.flash_chip()),
            overlay: Overlay::new(),
            u64io: U64Io::new(),
            misc: Board::new(),
            wifi: Wifi::new(),
            sd: SdCard::new(),
            usb: UsbHost::new(),
            rmii: Rmii::new(),
            c64: C64Port::new(),
            console: Vec::new(),
            deadline: 0,
            halt: Halt::NoFirmware,
        };
        b.apply_kind();
        b
    }

    /// What the model fixes in the blocks (configuration; again after a
    /// state was loaded).
    ///
    /// The Commodore 64 Ultimate carries the Blingboard (LED and
    /// keyboard controller). Its "blackboard" register reads 0: Gideon's
    /// board reads 1 there, and Commodore's updater (1.2.1) reads it
    /// right after its heading and refuses the board as "not from
    /// Commodore" for every other value tried (1, 2, 3, 5, $80, $FF).
    /// Found by trying, 2026-11-23; what the register means is not known.
    fn apply_kind(&mut self) {
        if self.kind == Kind::C64Ultimate {
            self.misc.set_blingboard(true);
            self.u64io.set_blackboard(0);
        }
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    pub fn halt(&self) -> Halt {
        self.halt
    }

    /// The board's clock (100 MHz) since power-on.
    pub fn now(&self) -> u64 {
        self.now
    }

    pub fn ram(&self) -> &[u8] {
        &self.ram
    }

    pub fn ram_mut(&mut self) -> &mut [u8] {
        &mut self.ram
    }

    /// Board memory and the C64 port at once (for its drives, whose disks
    /// and ROMs are board memory).
    pub fn drive_parts(&mut self) -> (&mut [u8], &mut C64Port) {
        (&mut self.ram, &mut self.c64)
    }

    /// The cartridge logic and board memory at once (the board's
    /// cartridge on the C64's port reads its ROM from there).
    pub fn cart_parts(&mut self) -> (&mut chips_ultimate::c64port::CartLogic, &mut [u8]) {
        (&mut self.c64.carts, &mut self.ram)
    }

    /// The C64 was reset by something other than the board's own
    /// register (the reset button): the firmware is told.
    pub fn c64_reset_event(&mut self) {
        self.c64.c64_reset_event(&mut self.irq);
    }

    /// What the firmware printed on its serial console, and forget it.
    pub fn take_console(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.console)
    }

    /// Power-on: every block and the memory as after the FPGA has loaded,
    /// then what the boot ROM does. With `update` (an update file) its
    /// updater is started in place of the application in the flash.
    /// The flash keeps its contents.
    pub fn power_on(&mut self, update: Option<&[u8]>, side: &mut dyn C64Side) {
        self.ram.fill(0);
        self.now = 0;
        self.target = 0;
        self.deadline = 0;
        self.console.clear();
        self.irq.reset();
        self.itu.reset();
        self.flash.reset();
        self.flash.power_cycle();
        self.overlay.reset();
        self.u64io.reset();
        self.misc.reset();
        self.misc.led.power_on();
        self.misc.bling_led.power_on();
        self.wifi.reset();
        self.sd.reset();
        self.usb.reset();
        self.rmii.reset();
        self.c64.reset();
        self.c64.apply_lines(side);
        let start = match update {
            Some(file) => {
                loader::records(file).and_then(|r| loader::load_records(&mut self.ram, &r))
            }
            None => {
                let layout = FlashLayout::for_fpga_type(self.kind.fpga_type());
                loader::boot_from_flash(
                    &mut self.ram,
                    self.flash.image(),
                    layout.application.start as usize,
                )
            }
        };
        self.cpu = Rv32::default();
        match start {
            Some(pc) => {
                self.cpu.reset(pc);
                self.halt = Halt::Running;
            }
            None => self.halt = Halt::NoFirmware,
        }
    }

    /// One access of the I/O bus as the processor makes it, from outside
    /// (tests, tools): `Some(v)` writes, `None` reads. The storage blocks
    /// see no media.
    pub fn io_access(&mut self, addr: u32, v: Option<u8>, side: &mut dyn C64Side) -> u8 {
        let mut storage = chips_ultimate::MemStorage::default();
        let mut link = chips_ultimate::MemLink::default();
        let mut sys = Sys {
            ram: &mut self.ram,
            now: self.now,
            irq: &mut self.irq,
            itu: &mut self.itu,
            flash: &mut self.flash,
            overlay: &mut self.overlay,
            u64io: &mut self.u64io,
            misc: &mut self.misc,
            wifi: &mut self.wifi,
            sd: &mut self.sd,
            usb: &mut self.usb,
            rmii: &mut self.rmii,
            net: &mut link,
            c64: &mut self.c64,
            console: &mut self.console,
            side,
            storage: &mut storage,
            touched: false,
        };
        let addr = addr & 0xff_ffff;
        let out = match v {
            Some(v) => {
                sys.io_write(addr, v);
                v
            }
            None => sys.io_read(addr),
        };
        // Deadlines may have moved.
        self.deadline = 0;
        out
    }

    /// Let the board run for `clocks` of its clock. What the firmware
    /// asked of the machine meanwhile.
    pub fn run(
        &mut self,
        clocks: u64,
        side: &mut dyn C64Side,
        storage: &mut dyn Storage,
        net: &mut dyn EtherLink,
    ) -> Option<Request> {
        self.target += clocks;
        if self.halt != Halt::Running {
            self.now = self.target;
            return None;
        }
        let mut sys = Sys {
            ram: &mut self.ram,
            now: self.now,
            irq: &mut self.irq,
            itu: &mut self.itu,
            flash: &mut self.flash,
            overlay: &mut self.overlay,
            u64io: &mut self.u64io,
            misc: &mut self.misc,
            wifi: &mut self.wifi,
            sd: &mut self.sd,
            usb: &mut self.usb,
            rmii: &mut self.rmii,
            net,
            c64: &mut self.c64,
            console: &mut self.console,
            side,
            storage,
            touched: false,
        };
        let mut deadline = self.deadline;
        let mut illegal = None;
        while sys.now < self.target {
            if sys.now >= deadline {
                let mut ctx = ctx!(sys);
                if sys.itu.next_event() <= ctx.now {
                    sys.itu.tick(&mut ctx);
                }
                if sys.u64io.next_event() <= ctx.now {
                    sys.u64io.tick(&mut ctx);
                }
                if sys.usb.next_event() <= ctx.now {
                    sys.usb.tick(&mut ctx);
                }
                if sys.rmii.next_event() <= ctx.now {
                    sys.rmii.tick(&mut ctx);
                }
                if sys.wifi.next_event() <= ctx.now {
                    sys.wifi.tick(&mut ctx);
                }
                sys.touched = true;
            }
            if sys.touched {
                sys.touched = false;
                deadline = sys
                    .itu
                    .next_event()
                    .min(sys.u64io.next_event())
                    .min(sys.wifi.next_event())
                    .min(sys.usb.next_event())
                    .min(sys.rmii.next_event());
                self.cpu.set_irq(sys.irq.line());
            }
            if let StepResult::Illegal(word) = self.cpu.step(&mut sys) {
                // The processor traps; the firmware's handler decides.
                illegal.get_or_insert(word);
            }
            sys.now += CLOCKS_PER_INSTR;
        }
        self.now = sys.now;
        self.deadline = deadline;
        let _ = illegal;
        if self.console.len() > CONSOLE_KEEP {
            let cut = self.console.len() - CONSOLE_KEEP;
            self.console.drain(..cut);
        }
        match self.wifi.take_power_event() {
            Some(PowerEvent::Off) => {
                self.halt = Halt::PowerOff;
                return Some(Request::PowerOff);
            }
            Some(PowerEvent::Reboot) => return Some(Request::Reboot),
            None => {}
        }
        if self.misc.take_reconfigure() {
            return Some(Request::Reboot);
        }
        None
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.u8(match self.halt {
            Halt::Running => 0,
            Halt::NoFirmware => 1,
            Halt::PowerOff => 2,
            Halt::Illegal(_) => 3,
        });
        w.u64(self.now);
        w.u64(self.target);
        mimic_core::Savestate::save(&self.cpu, w);
        self.irq.save(w);
        self.itu.save(w);
        self.flash.save(w);
        self.overlay.save(w);
        self.u64io.save(w);
        self.misc.save(w);
        self.wifi.save(w);
        self.sd.save(w);
        self.usb.save(w);
        self.rmii.save(w);
        self.c64.save(w);
        // Memory: 4 KB pages, those that are all zero left out.
        let pages = RAM_SIZE / 0x1000;
        let mut map = vec![0u8; pages / 8];
        for (n, page) in self.ram.as_chunks::<0x1000>().0.iter().enumerate() {
            if page.iter().any(|&b| b != 0) {
                map[n / 8] |= 1 << (n % 8);
            }
        }
        w.raw(&map);
        for (n, page) in self.ram.as_chunks::<0x1000>().0.iter().enumerate() {
            if map[n / 8] & 1 << (n % 8) != 0 {
                w.raw(page);
            }
        }
    }

    /// Corrupt data gives an error; the board may then hold a part of the
    /// new state, so the caller loads into a copy or powers on again.
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let halt = match r.u8()? {
            0 => Halt::Running,
            1 => Halt::NoFirmware,
            2 => Halt::PowerOff,
            3 => Halt::Illegal(0),
            _ => return Err(StateError::Invalid("board halt state")),
        };
        let now = r.u64()?;
        let target = r.u64()?;
        // The clock runs past the target by less than an instruction.
        if now > target.saturating_add(CLOCKS_PER_INSTR) || target > now.saturating_add(CLOCK_SLACK)
        {
            return Err(StateError::Invalid("board clock"));
        }
        mimic_core::Savestate::load(&mut self.cpu, r)?;
        self.irq.load(r)?;
        self.itu.load(r)?;
        self.flash.load(r)?;
        self.overlay.load(r)?;
        self.u64io.load(r)?;
        self.misc.load(r)?;
        self.wifi.load(r)?;
        self.sd.load(r)?;
        self.usb.load(r)?;
        self.rmii.load(r)?;
        self.c64.load(r)?;
        let pages = RAM_SIZE / 0x1000;
        let map = r.raw(pages / 8)?.to_vec();
        for (n, page) in self.ram.as_chunks_mut::<0x1000>().0.iter_mut().enumerate() {
            if map[n / 8] & 1 << (n % 8) != 0 {
                r.raw_into(page)?;
            } else {
                page.fill(0);
            }
        }
        self.apply_kind();
        self.halt = halt;
        self.now = now;
        self.target = target;
        self.deadline = 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
