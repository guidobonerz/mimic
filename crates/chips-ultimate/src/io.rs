//! What every block on the board's I/O bus shares: the access context, the
//! block trait and the C64 as the board sees it.

use crate::irq::Irq;
use mimic_core::{StateError, StateReader, StateWriter};

/// The board's clock: 100 MHz. [`IoCtx::now`] counts its periods.
pub const CLOCK_HZ: u64 = 100_000_000;

/// The C64's ROMs inside the FPGA, which the firmware fills through
/// windows on the I/O bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum C64Rom {
    /// 8 KB, `$A000`.
    Basic,
    /// 8 KB, `$E000`.
    Kernal,
    /// 4 KB, `$D000`.
    Char,
}

/// The C64 behind the board's registers. The machine implements this for
/// its C64; [`NullC64`] stands in where there is none (tests).
pub trait C64Side {
    /// A read on the C64's bus by the board as DMA master, with the side
    /// effects a read has there (what is mapped follows the C64's memory
    /// configuration).
    fn dma_read(&mut self, addr: u16) -> u8;
    /// The same without side effects.
    fn dma_peek(&self, addr: u16) -> u8;
    /// A write on the C64's bus by the board as DMA master.
    fn dma_write(&mut self, addr: u16, v: u8);
    /// Hold the 6510 (the board stops it to use the bus) or let it run.
    fn set_stopped(&mut self, on: bool);
    /// The 6510 stands still.
    fn stopped(&self) -> bool;
    /// Pull the C64's RESET line or release it; releasing starts the C64.
    fn set_reset(&mut self, on: bool);
    /// A byte of one of the C64's ROMs as the firmware loads it.
    fn rom_write(&mut self, rom: C64Rom, offset: u16, v: u8);
    fn rom_read(&self, rom: C64Rom, offset: u16) -> u8;
    /// The video standard the firmware selects: `true` NTSC, `false` PAL.
    fn set_ntsc(&mut self, _ntsc: bool) {}
    /// The board's keyboard path into the C64's matrix (USB keyboard,
    /// remote control): row byte for a column, low = pressed. Not the
    /// C64's own keyboard, which the machine handles.
    fn set_matrix(&mut self, _column: u8, _rows: u8) {}
    /// Force the ULTIMAX memory map on the C64's bus as the board's
    /// cartridge logic does while its menu owns the machine (C64_MODE
    /// bit 1): RAM only at `$0000`-`$0FFF`, I/O, the cartridge windows.
    fn set_ultimax(&mut self, _on: bool) {}
    /// The level of the NMI line the board drives (C64_MODE bit 4, the
    /// RESTORE key of the board's keyboard path).
    fn set_nmi(&mut self, _on: bool) {}
    /// When a stop request takes hold (C64_STOP_MODE): 0 in a bad line,
    /// 1 after a write followed by a read, 2 at once. Told before
    /// [`C64Side::set_stopped`].
    fn set_stop_condition(&mut self, _mode: u8) {}
    /// DMA with the "memory only" switch of the C64 core set: the RAM
    /// under I/O and ROM, without side effects on the chips.
    fn dma_read_ram(&mut self, addr: u16) -> u8 {
        self.dma_read(addr)
    }
    fn dma_peek_ram(&self, addr: u16) -> u8 {
        self.dma_peek(addr)
    }
    fn dma_write_ram(&mut self, addr: u16, v: u8) {
        self.dma_write(addr, v);
    }
    /// The lines the board drives into the expansion port now, `true` =
    /// pulled low: /GAME, /EXROM (the forced ULTIMAX of C64_MODE or the
    /// board's cartridge). They count at once, for the DMA that follows
    /// and for the 6510's next cycle.
    fn set_cart_lines(&mut self, _game: bool, _exrom: bool) {}
    /// Where a DMA access at `addr` lands as the C64's PLA decides it,
    /// if in a cartridge window: the board's cartridge answers it there.
    fn cart_window(&self, _addr: u16) -> Option<crate::c64port::CartWindow> {
        None
    }
}

/// No C64: 64 KB of memory that reads back what was written.
pub struct NullC64 {
    pub mem: Vec<u8>,
    pub stopped: bool,
    pub reset: bool,
    pub roms: [Vec<u8>; 3],
}

impl Default for NullC64 {
    fn default() -> Self {
        Self {
            mem: vec![0; 0x1_0000],
            stopped: false,
            reset: false,
            roms: [vec![0; 0x2000], vec![0; 0x2000], vec![0; 0x1000]],
        }
    }
}

impl C64Side for NullC64 {
    fn dma_read(&mut self, addr: u16) -> u8 {
        self.mem[usize::from(addr)]
    }

    fn dma_peek(&self, addr: u16) -> u8 {
        self.mem[usize::from(addr)]
    }

    fn dma_write(&mut self, addr: u16, v: u8) {
        self.mem[usize::from(addr)] = v;
    }

    fn set_stopped(&mut self, on: bool) {
        self.stopped = on;
    }

    fn stopped(&self) -> bool {
        self.stopped
    }

    fn set_reset(&mut self, on: bool) {
        self.reset = on;
    }

    fn rom_write(&mut self, rom: C64Rom, offset: u16, v: u8) {
        let r = &mut self.roms[rom as usize];
        let n = r.len();
        r[usize::from(offset) % n] = v;
    }

    fn rom_read(&self, rom: C64Rom, offset: u16) -> u8 {
        let r = &self.roms[rom as usize];
        r[usize::from(offset) % r.len()]
    }
}

/// What a block gets with every access and tick.
pub struct IoCtx<'a> {
    /// The board's clock now ([`CLOCK_HZ`]).
    pub now: u64,
    /// The board's memory (64 MB DDR at address 0), for blocks that move
    /// data themselves (DMA). Addresses are masked with `ram.len() - 1`.
    pub ram: &'a mut [u8],
    /// The interrupt inputs of the processor's interrupt controller.
    pub irq: &'a mut Irq,
    /// What the firmware prints on its serial console.
    pub console: &'a mut Vec<u8>,
    /// The C64.
    pub c64: &'a mut dyn C64Side,
    /// The media behind the board's storage (SD card, USB sticks);
    /// `None`: there are none.
    pub storage: Option<&'a mut dyn Storage>,
    /// What is plugged into the board's Ethernet socket; `None`: nothing
    /// (the link is down).
    pub net: Option<&'a mut dyn EtherLink>,
}

/// The cable in the board's Ethernet socket, as the MAC sees it: whole
/// Ethernet frames (destination, source, type, payload; no preamble, no
/// frame check sequence) in both directions. The machine keeps what is
/// at the other end (a virtual router onto the host's network).
pub trait EtherLink {
    /// The PHY has a link.
    fn link_up(&self) -> bool;
    /// A frame the board sends.
    fn transmit(&mut self, frame: &[u8]);
    /// The next frame for the board, if one waits.
    fn receive(&mut self) -> Option<Vec<u8>>;
    /// A frame waits (without taking it).
    fn pending(&self) -> bool;
}

/// A cable in memory, for tests: what the board sent, and frames queued
/// for it.
#[derive(Clone, Debug, Default)]
pub struct MemLink {
    pub up: bool,
    pub sent: Vec<Vec<u8>>,
    pub incoming: std::collections::VecDeque<Vec<u8>>,
}

impl EtherLink for MemLink {
    fn link_up(&self) -> bool {
        self.up
    }

    fn transmit(&mut self, frame: &[u8]) {
        self.sent.push(frame.to_vec());
    }

    fn receive(&mut self) -> Option<Vec<u8>> {
        self.incoming.pop_front()
    }

    fn pending(&self) -> bool {
        !self.incoming.is_empty()
    }
}

/// Sector size of every medium behind [`Storage`].
pub const SECTOR: usize = 512;

/// The board's units of storage.
pub mod unit {
    /// The SD card slot.
    pub const SD: u8 = 0;
    /// The first USB port of the hub; the others follow (`USB0 + 1`, `+ 2`).
    pub const USB0: u8 = 1;
    /// The hub's ports.
    pub const USB_PORTS: u8 = 3;
}

/// Block media as the board's storage blocks see them: 512-byte
/// sectors. The machine keeps them (image files, a folder made into a
/// FAT volume); a unit without a medium has no sectors. Media can come
/// and go between two calls; a block that cares compares
/// [`Storage::generation`].
pub trait Storage {
    /// The medium's size in sectors; `None`: no medium in the unit.
    fn sectors(&self, unit: u8) -> Option<u64>;
    /// Counts up whenever the medium in the unit changes (inserted,
    /// removed, replaced).
    fn generation(&self, unit: u8) -> u32;
    fn write_protected(&self, _unit: u8) -> bool {
        false
    }
    /// Read sector `lba` into `buf` (`SECTOR` bytes). `false`: no medium
    /// or past its end (or the host failed).
    fn read(&mut self, unit: u8, lba: u64, buf: &mut [u8]) -> bool;
    /// Write sector `lba` from `data` (`SECTOR` bytes). `false`: not
    /// written.
    fn write(&mut self, unit: u8, lba: u64, data: &[u8]) -> bool;
}

/// Media in memory, for tests: one optional image per unit.
#[derive(Clone, Debug, Default)]
pub struct MemStorage {
    pub units: Vec<Option<Vec<u8>>>,
    pub generations: Vec<u32>,
    pub protected: Vec<bool>,
}

impl MemStorage {
    /// Put an image (a multiple of `SECTOR` bytes) into `unit`, or take
    /// it out with `None`.
    pub fn set(&mut self, unit: u8, image: Option<Vec<u8>>) {
        let u = usize::from(unit);
        if self.units.len() <= u {
            self.units.resize(u + 1, None);
            self.generations.resize(u + 1, 0);
            self.protected.resize(u + 1, false);
        }
        self.units[u] = image;
        self.generations[u] = self.generations[u].wrapping_add(1);
    }
}

impl Storage for MemStorage {
    fn sectors(&self, unit: u8) -> Option<u64> {
        let m = self.units.get(usize::from(unit))?.as_ref()?;
        Some((m.len() / SECTOR) as u64)
    }

    fn generation(&self, unit: u8) -> u32 {
        self.generations
            .get(usize::from(unit))
            .copied()
            .unwrap_or(0)
    }

    fn write_protected(&self, unit: u8) -> bool {
        self.protected
            .get(usize::from(unit))
            .copied()
            .unwrap_or(false)
    }

    fn read(&mut self, unit: u8, lba: u64, buf: &mut [u8]) -> bool {
        let Some(Some(m)) = self.units.get(usize::from(unit)) else {
            return false;
        };
        let Ok(at) = usize::try_from(lba.saturating_mul(SECTOR as u64)) else {
            return false;
        };
        match m.get(at..at.saturating_add(SECTOR)) {
            Some(src) if buf.len() >= SECTOR => {
                buf[..SECTOR].copy_from_slice(src);
                true
            }
            _ => false,
        }
    }

    fn write(&mut self, unit: u8, lba: u64, data: &[u8]) -> bool {
        if self.write_protected(unit) || data.len() < SECTOR {
            return false;
        }
        let Some(Some(m)) = self.units.get_mut(usize::from(unit)) else {
            return false;
        };
        let Ok(at) = usize::try_from(lba.saturating_mul(SECTOR as u64)) else {
            return false;
        };
        match m.get_mut(at..at.saturating_add(SECTOR)) {
            Some(dst) => {
                dst.copy_from_slice(&data[..SECTOR]);
                true
            }
            None => false,
        }
    }
}

/// A block on the I/O bus. `addr` is the 24-bit I/O address.
pub trait IoDevice {
    /// A read with its side effects.
    fn read(&mut self, addr: u32, ctx: &mut IoCtx<'_>) -> u8;
    /// What a read would return, without side effects (debugger).
    fn peek(&self, addr: u32) -> u8;
    fn write(&mut self, addr: u32, v: u8, ctx: &mut IoCtx<'_>);
    /// The clock at which [`IoDevice::tick`] must run next; `u64::MAX`:
    /// never. Asked again after every access to the I/O bus and every
    /// tick.
    fn next_event(&self) -> u64 {
        u64::MAX
    }
    /// Called once `ctx.now >= next_event()`.
    fn tick(&mut self, _ctx: &mut IoCtx<'_>) {}
    /// Power-on state (what a reconfiguration of the FPGA leaves).
    fn reset(&mut self);
    fn save(&self, w: &mut StateWriter);
    /// Corrupt data gives an error, never a panic.
    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError>;
}
