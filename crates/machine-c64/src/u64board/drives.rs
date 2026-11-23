//! The board's floppy drives on the C64's serial bus: drive A is the
//! machine's drive 8, drive B (Commodore 64 Ultimate) a second drive.
//!
//! On the board both are 1541s inside the FPGA; the firmware keeps their
//! ROMs and disks in board memory and drives them through the registers
//! of [`chips_ultimate::c64port::drive`]. Here each is mimic's own 1541
//! ([`drives_cbm::CbmDrive`]) in its outside-surface mode:
//!
//! - Power and reset: while POWER is off, RESET bit 0 holds it, or the
//!   C64's reset is pulled with RESET bit 1 set, the drive is off the bus
//!   and stands. Coming out it takes its DOS ROM from board memory (the
//!   upper 16 KB of its 64 KB area, [`DRIVE_A_AREA`] / [`DRIVE_B_AREA`],
//!   where the firmware has loaded it) and starts; after power-on with
//!   new RAM. RESET bit 2: its clock stands while the board stops the
//!   C64 (the open menu). DRIVETYPE other than 1541: off (no 1571/1581
//!   behind it).
//! - The jumpers are HW_ADDR, the write-protect sensor is SENSOR (the
//!   firmware darkens it while it changes the disk, which is how the DOS
//!   notices).
//! - The disk is board memory: the track under the head is where the
//!   track table points, read and written in place; the head in write
//!   mode sets the track's DIRTY bit, so the firmware writes the track
//!   back to the image file. TRACK, SIDE and STATUS (motor, writing,
//!   write busy) report the head.
//! - Once a frame the drive's 2 KB RAM goes to the bottom of its area,
//!   where the firmware reads the device number the DOS uses (`$78`).
//!
//! Everything above is in a register, in board memory or in the drive's
//! savestate; after a state is loaded the ROMs are taken from board
//! memory again.
//!
//! Sources: UE2-C64U-Emulator `docs/hw/11-drives-iec-periph.md`
//! (functional model "Drive"); the firmware's `software/drive/c1541.cc`
//! (how it powers, resets, loads the ROM, changes disks and writes back)
//! and `fpga/1541/vhdl_source/` (`drive_registers.vhd`, `floppy.vhd`,
//! `floppy_stream.vhd`: what the lines and DIRTY bits do).

use super::glue::BoardPart;
use crate::drive::{Drive8, DRIVE9, DRIVE9_SPACE};
use crate::C64;
use chips_ultimate::c64port::drive::{Drive as Regs, Restart, DRIVE_A_AREA, DRIVE_B_AREA};
use drives_cbm::c1541::ROM_SIZE;
use drives_cbm::{DiskSurface, DriveModel, IecLines, SerialDrive};
use mimic_core::{DebugHooks, DeviceId, MemSpaceId, StopReason};

/// A drive's disk: board memory where its track table points.
struct BoardSurface<'a> {
    regs: &'a mut Regs,
    ram: &'a mut [u8],
}

impl BoardSurface<'_> {
    fn range(&self, side: u8, half_track: u8) -> Option<std::ops::Range<usize>> {
        let (addr, last, _) = self.regs.track_entry(side, half_track);
        let start = addr as usize & (self.ram.len() - 1);
        let end = start + usize::from(last) + 1;
        (end <= self.ram.len()).then_some(start..end)
    }
}

impl DiskSurface for BoardSurface<'_> {
    fn track(&self, side: u8, half_track: u8) -> Option<&[u8]> {
        let r = self.range(side, half_track)?;
        Some(&self.ram[r])
    }

    /// Without a disk inserted the head writes nothing.
    fn track_mut(&mut self, side: u8, half_track: u8) -> Option<&mut [u8]> {
        if !self.regs.inserted() {
            return None;
        }
        let r = self.range(side, half_track)?;
        Some(&mut self.ram[r])
    }

    fn writing(&mut self, side: u8, half_track: u8) {
        self.regs.mark_written(side, half_track);
    }
}

/// A board drive: a 1541 with no ROM yet whose disk is board memory.
pub(crate) fn new_board_drive(
    number: u8,
    device: DeviceId,
    space: MemSpaceId,
    cpu_hz: u32,
    ppm: i32,
) -> Drive8 {
    let mut d = SerialDrive::new_true(
        DriveModel::C1541,
        vec![0xff; ROM_SIZE].into_boxed_slice(),
        number,
        device,
        space,
        cpu_hz,
    );
    d.set_crystal_ppm(ppm, cpu_hz);
    if let Some(c) = d.cbm_mut() {
        c.set_external_surface(true);
    }
    d
}

/// Drive B of the Commodore 64 Ultimate (the Elite II's firmware has
/// none).
pub(crate) fn new_drive_b(cpu_hz: u32) -> Drive8 {
    new_board_drive(
        9,
        DRIVE9,
        DRIVE9_SPACE,
        cpu_hz,
        drives_cbm::serial::DEFAULT_CRYSTAL_PPM,
    )
}

/// The drive answers on the bus now.
fn on_bus(regs: &Regs, c64_reset: bool) -> bool {
    !regs.held() && regs.drive_type() == 0 && !(c64_reset && regs.follows_c64_reset())
}

/// The DOS ROM from the drive's area of board memory.
fn take_rom(d: &mut Drive8, ram: &[u8], area: u32) {
    let a = area as usize;
    if let (Some(c), Some(rom)) = (d.cbm_mut(), ram.get(a + 0xc000..a + 0x1_0000)) {
        c.set_rom(rom);
    }
}

/// One C64 cycle of a board drive with the lines the others pull. `None`:
/// it is off the bus.
#[allow(clippy::too_many_arguments)]
fn drive_cycle(
    d: &mut Drive8,
    regs: &mut Regs,
    ram: &mut [u8],
    area: u32,
    c64_stopped: bool,
    c64_reset: bool,
    host: IecLines,
    dbg: Option<&mut DebugHooks>,
) -> Option<Option<StopReason>> {
    if c64_reset && regs.follows_c64_reset() {
        regs.note_restart();
    }
    if !on_bus(regs, c64_reset) {
        return None;
    }
    if let Some(how) = regs.take_restart() {
        take_rom(d, ram, area);
        d.reset(how == Restart::PowerOn);
    }
    let c = d.cbm_mut()?;
    c.set_device(regs.device());
    c.set_surface_protected(regs.write_protected());
    if c64_stopped && regs.stops_when_frozen() {
        return Some(None);
    }
    let (_, stop) = d.run_on(
        host,
        dbg,
        &mut BoardSurface {
            regs: &mut *regs,
            ram: &mut *ram,
        },
    );
    if let Some(c) = d.cbm() {
        regs.report_head(c.half_track(), c.side(), c.motor_on(), c.head_writing());
    }
    Some(stop)
}

impl C64 {
    /// The board's drives' share of a C64 cycle on the serial bus: `c64`
    /// is what the C64 (and the SD2IEC) pull, `host` the C64 alone. The
    /// bus afterwards, what the drives and the C64 pull (for the SD2IEC),
    /// and a stop from a drive's debug hooks.
    pub(crate) fn board_drives(
        &mut self,
        c64: IecLines,
        host: IecLines,
        mut dbg: Option<&mut DebugHooks>,
    ) -> (IecLines, IecLines, Option<StopReason>) {
        let Some(part) = self.board.as_deref_mut() else {
            return (c64, host, None);
        };
        let BoardPart {
            board,
            lines,
            drive_b,
            ..
        } = part;
        let now = board.now();
        let (stopped, reset) = (lines.stop, lines.reset);
        let (ram, port) = board.drive_parts();
        port.drive_a.set_clock(now);
        port.drive_b.set_clock(now);
        // Drive A sees what drive B pulled last.
        let b_before = drive_b
            .as_ref()
            .filter(|_| on_bus(&port.drive_b, reset))
            .map_or(IecLines::default(), |b| b.iec_out(c64.atn));
        let mut stop = None;
        let a_in = IecLines::combine(&[c64, b_before]);
        let mut a_out = IecLines::default();
        if let Some(a) = self.drive.as_mut() {
            let ran = drive_cycle(
                a,
                &mut port.drive_a,
                ram,
                DRIVE_A_AREA,
                stopped,
                reset,
                a_in,
                dbg.as_deref_mut(),
            );
            if let Some(s) = ran {
                stop = stop.or(s);
                a_out = a.iec_out(c64.atn);
            }
        }
        let mut b_out = IecLines::default();
        if let Some(b) = drive_b.as_mut() {
            let b_in = IecLines::combine(&[c64, a_out]);
            let ran = drive_cycle(
                b,
                &mut port.drive_b,
                ram,
                DRIVE_B_AREA,
                stopped,
                reset,
                b_in,
                dbg,
            );
            if let Some(s) = ran {
                stop = stop.or(s);
                b_out = b.iec_out(c64.atn);
            }
        }
        (
            IecLines::combine(&[c64, a_out, b_out]),
            IecLines::combine(&[host, a_out, b_out]),
            stop,
        )
    }

    /// Once a frame: the drives' RAM into their areas of board memory.
    pub(crate) fn board_drives_frame(&mut self) {
        let Some(part) = self.board.as_deref_mut() else {
            return;
        };
        let (ram, port) = part.board.drive_parts();
        for (d, regs, area) in [
            (self.drive.as_ref(), &port.drive_a, DRIVE_A_AREA),
            (part.drive_b.as_ref(), &port.drive_b, DRIVE_B_AREA),
        ] {
            if let (Some(c), true) = (d.and_then(|d| d.cbm()), regs.powered()) {
                let a = area as usize;
                ram[a..a + c.ram().len()].copy_from_slice(c.ram());
            }
        }
    }

    /// After a state was loaded: the drives' ROMs from board memory.
    pub(crate) fn board_drives_loaded(&mut self) {
        let Some(part) = self.board.as_deref_mut() else {
            return;
        };
        let (ram, _) = part.board.drive_parts();
        if let Some(a) = self.drive.as_mut() {
            take_rom(a, ram, DRIVE_A_AREA);
        }
        if let Some(b) = part.drive_b.as_mut() {
            take_rom(b, ram, DRIVE_B_AREA);
        }
    }

    /// The C64's reset reaches a board drive whose RESET bit 1 is set.
    pub(crate) fn board_drives_follow_reset(&mut self, hard: bool) {
        let Some(part) = self.board.as_deref_mut() else {
            return;
        };
        let port = &part.board.c64;
        if let Some(a) = self.drive.as_mut() {
            if port.drive_a.follows_c64_reset() {
                a.reset(hard);
            }
        }
        if let Some(b) = part.drive_b.as_mut() {
            if port.drive_b.follows_c64_reset() {
                b.reset(hard);
            }
        }
    }
}
