//! One of the two floppy drives inside the FPGA as the firmware sees it:
//! drive A at `$020000`, drive B at `$024000`. The registers are the
//! drive's lines; the drive itself (processor, VIAs, head) is the
//! machine's ([`Drive`] says what it needs and takes back what it
//! reports), its disk is board memory as the track table describes it.
//!
//! The 8 KB window splits on address bits 12:11 (offsets from the drive's
//! base):
//!
//! | Off | Name | Bits |
//! |---|---|---|
//! | `$0000` | POWER | bit 0 |
//! | `$0001` | RESET | bit 0 hold the drive's processor, bit 1 follow the C64's reset, bit 2 stand while the C64 is stopped; power-on all three |
//! | `$0002` | HW_ADDR | bits 1:0: device 8 + n (the VIA's jumper inputs) |
//! | `$0003` | SENSOR | bit 0: 1 = light passes (writable); power-on 0 |
//! | `$0004` | INSERTED | bit 0; without it the head writes nothing and sets no DIRTY bit |
//! | `$0005` | RAMMAP | bits 7:1 |
//! | `$0006` | SIDE | R bit 0: the head in use |
//! | `$0007` | MAN_WRITE | W: restarts the write-busy time |
//! | `$0008` | TRACK | R bits 6:0: half-track under the head (0 = track 1) |
//! | `$0009` | STATUS | R bit 0 motor, bit 1 writing, bit 2 write busy: from the first bit written until 2047 ms after the last (or after MAN_WRITE); cleared by holding the drive in reset |
//! | `$000C` | DISKCHANGE | bit 0 disk changed, bit 1 force ready |
//! | `$000D` | DRIVETYPE | bits 1:0: 0 = 1541, 1 = 1571, 2 = 1581; reads back only where the drive can change its type |
//! | `$000E` | SOUNDS | W strobes: bit 0 insert, bit 1 remove |
//! | `$0800`-`$087F` | DIRTY | per track index `side << 6 \| half-track >> 1`: R bit 7 any track written, bit 0 this track written; W with bit 7 clears "any", without it this track's bit. The head sets both while it is in write mode with the motor on and a disk inserted; holding the drive in reset clears "any" |
//! | `$1000`-`$17FF` | track table | 512 words of 32 bits, little-endian, word `side << 8 \| half-track << 1 \| w`: w = 0 the address of the track's first byte in board memory (26 bits), w = 1 the offset of its last byte (bits 13:0) and the bit time (bits 25:16). Reads 0 |
//! | `$1800` | WD_COMMAND | W bit 0 index pulse on, bit 1 its polarity. R: the head of the command queue |
//! | `$1801` | WD_TRACK | track register (power-on 1) |
//! | `$1802`, `$1803` | WD_SECTOR, WD_DATA | R |
//! | `$1804` | WD_STATUS_CLEAR | W: clears the written bits; R status |
//! | `$1805` | WD_STATUS_SET | W: sets the written bits; R status |
//! | `$1806` | WD_IRQ_ACK | R bit 7 a command waits, bit 0 it is a completion; W takes it off the queue. **Reads 0 here**: the firmware empties the queue in a loop without a timeout |
//! | `$1807` | WD_DMA_MODE | 0 off, 1 to the drive, 2 from the drive, 3 write complete |
//! | `$1808`-`$180A` | WD_DMA_ADDR | W, 24 bits |
//! | `$180C`, `$180D` | WD_DMA_LEN | 14 bits; R what remains |
//! | `$180E` | WD_STEPPER_TRACK | W target track; R bit 0 the head moves (0 here) |
//! | `$180F` | WD_STEP_TIME | 5 bits, ms per step (power-on 12) |
//!
//! The drive's 64 KB memory image is board memory [`DRIVE_A_AREA`] /
//! [`DRIVE_B_AREA`]: the firmware loads the DOS ROM into its upper 32 KB
//! (a 16 KB ROM twice) before it lets the drive out of reset, and reads
//! the device number the DOS uses at `$78` of it.
//!
//! What the machine does with it: while [`Drive::held`] the drive is off
//! the bus; when it comes out ([`Drive::take_restart`]) it takes its ROM
//! from board memory and starts; every cycle it takes its lines
//! ([`Drive::device`], [`Drive::write_protected`],
//! [`Drive::stops_when_frozen`]), reads and writes the track under its
//! head where [`Drive::track_entry`] points, marks what it writes
//! ([`Drive::mark_written`]) and reports its head
//! ([`Drive::report_head`]) at the board's clock ([`Drive::set_clock`]).
//! The track's length decides how fast its bytes pass (a revolution is
//! 200 ms; the firmware sets the bit time to match), so the bit time is
//! kept but not used.
//!
//! Interrupts: the WD177x raises the ITU's high source 1 (drive A) or 2
//! (drive B) while a command waits in its queue. The 1541 needs none, and
//! no WD177x command ever arrives here (no 1571/1581 behind it), so
//! neither is raised.
//!
//! Source: UE2-C64U-Emulator `docs/hw/11-drives-iec-periph.md` (address
//! map "Drive A", hazards H2-H5, H9, H13, the functional model "Drive",
//! the T0 and T1 rows); masks, power-on values, the write-busy time and
//! the DIRTY rules checked in the firmware's
//! `fpga/1541/vhdl_source/drive_registers.vhd`, `floppy.vhd` and
//! `floppy_stream.vhd`, their use in `software/drive/c1541.cc`.

use mimic_core::{StateError, StateReader, StateWriter};

const POWER: usize = 0x0;
const RESET: usize = 0x1;
const HW_ADDR: usize = 0x2;
const SENSOR: usize = 0x3;
const INSERTED: usize = 0x4;
const RAMMAP: usize = 0x5;
const SIDE: usize = 0x6;
const MAN_WRITE: usize = 0x7;
const TRACK: usize = 0x8;
const STATUS: usize = 0x9;
const DISKCHANGE: usize = 0xc;
const DRIVETYPE: usize = 0xd;

const WD_TRACK: u32 = 0x1;
const WD_STATUS_CLEAR: u32 = 0x4;
const WD_STATUS_SET: u32 = 0x5;
const WD_DMA_MODE: u32 = 0x7;
const WD_DMA_ADDR: u32 = 0x8;
const WD_DMA_LEN: u32 = 0xc;
const WD_STEPPER_TRACK: u32 = 0xe;
const WD_STEP_TIME: u32 = 0xf;

/// Size of the track table in bytes.
const PARAM_BYTES: usize = 0x800;

/// How long STATUS bit 2 stays up after the last bit written: 2047 ms
/// in board clocks.
pub const WRITE_BUSY_CLOCKS: u64 = 2047 * (crate::io::CLOCK_HZ / 1000);

/// Board memory of drive A's 64 KB memory image (the DOS ROM in its
/// upper 32 KB).
pub const DRIVE_A_AREA: u32 = 0x00ee_0000;
/// The same for drive B.
pub const DRIVE_B_AREA: u32 = 0x00ed_0000;

/// The WD177x part of the window, idle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wd177x {
    /// Index pulse control (2 bits).
    pub index: u8,
    pub track: u8,
    pub status: u8,
    pub dma_mode: u8,
    /// 24 bits.
    pub dma_addr: u32,
    /// 14 bits.
    pub dma_len: u16,
    pub goto_track: u8,
    pub step_time: u8,
}

impl Default for Wd177x {
    fn default() -> Self {
        Self {
            index: 0,
            track: 1,
            status: 0,
            dma_mode: 0,
            dma_addr: 0,
            dma_len: 0,
            goto_track: 0,
            step_time: 12,
        }
    }
}

impl Wd177x {
    fn read(&self, reg: u32) -> u8 {
        match reg {
            WD_TRACK => self.track,
            WD_STATUS_CLEAR | WD_STATUS_SET => self.status,
            WD_DMA_MODE => self.dma_mode,
            WD_DMA_LEN => self.dma_len as u8,
            0xd => (self.dma_len >> 8) as u8,
            WD_STEP_TIME => self.step_time,
            // The command queue is empty, sector and data registers are
            // the drive's, the head does not move.
            _ => 0,
        }
    }

    fn write(&mut self, reg: u32, v: u8) {
        match reg {
            0 => self.index = v & 3,
            WD_TRACK => self.track = v,
            WD_STATUS_CLEAR => self.status &= !v,
            WD_STATUS_SET => self.status |= v,
            WD_DMA_MODE => self.dma_mode = v & 3,
            WD_DMA_ADDR..=0xa => {
                let sh = (reg - WD_DMA_ADDR) * 8;
                self.dma_addr = (self.dma_addr & !(0xff << sh)) | (u32::from(v) << sh);
            }
            WD_DMA_LEN => self.dma_len = (self.dma_len & 0x3f00) | u16::from(v),
            0xd => self.dma_len = (self.dma_len & 0xff) | (u16::from(v & 0x3f) << 8),
            WD_STEPPER_TRACK => self.goto_track = v & 0x7f,
            WD_STEP_TIME => self.step_time = v & 0x1f,
            // 6: taking a command off the empty queue.
            _ => {}
        }
    }
}

/// How the drive comes out of a hold ([`Drive::take_restart`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Restart {
    /// Out of reset: the processor starts again.
    Reset,
    /// Switched on: the drive's RAM starts anew too.
    PowerOn,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drive {
    /// DRIVETYPE reads back (a drive that can be a 1571 or 1581 too).
    pub multi_mode: bool,
    regs: [u8; 16],
    /// The track table as written.
    param: Vec<u8>,
    /// DIRTY: a bit per track index, and "any".
    dirty: u128,
    any_dirty: bool,
    /// What the drive reported: half-track, side, motor, writing.
    head: u8,
    side: u8,
    motor: bool,
    writing: bool,
    /// The board's clock as the machine last told it, and the clock at
    /// which write busy ends.
    clock: u64,
    busy_until: u64,
    restart: Option<Restart>,
    pub wd: Wd177x,
}

impl Drive {
    pub fn new(multi_mode: bool) -> Self {
        let mut regs = [0; 16];
        regs[RESET] = 7;
        Self {
            multi_mode,
            regs,
            param: vec![0; PARAM_BYTES],
            dirty: 0,
            any_dirty: false,
            head: 0,
            side: 0,
            motor: false,
            writing: false,
            clock: 0,
            busy_until: 0,
            restart: None,
            wd: Wd177x::default(),
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.multi_mode);
    }

    fn write_busy(&self) -> bool {
        self.clock < self.busy_until
    }

    /// `off` counts from the drive's base.
    pub fn read(&self, off: u32) -> u8 {
        match (off >> 11) & 3 {
            0 => match (off & 0xf) as usize {
                DRIVETYPE if !self.multi_mode => 0,
                SIDE => self.side & 1,
                TRACK => self.head & 0x7f,
                STATUS => {
                    u8::from(self.motor)
                        | u8::from(self.writing) << 1
                        | u8::from(self.write_busy()) << 2
                }
                // The latches; the strobes and what is not decoded read 0.
                r @ (POWER | RESET | HW_ADDR | SENSOR | INSERTED | RAMMAP | DISKCHANGE
                | DRIVETYPE) => self.regs[r],
                _ => 0,
            },
            1 => u8::from(self.any_dirty) << 7 | (self.dirty >> (off & 0x7f)) as u8 & 1,
            3 => self.wd.read(off & 0xf),
            // The track table reads 0.
            _ => 0,
        }
    }

    pub fn write(&mut self, off: u32, v: u8) {
        match (off >> 11) & 3 {
            0 => {
                let r = (off & 0xf) as usize;
                let mask = match r {
                    POWER => {
                        if v & 1 != 0 && !self.powered() {
                            self.restart = Some(Restart::PowerOn);
                        }
                        1
                    }
                    RESET => {
                        if v & 1 != 0 {
                            // Held: the processor starts again when let
                            // go; write busy and "any written" are cleared.
                            self.restart.get_or_insert(Restart::Reset);
                            self.busy_until = 0;
                            self.any_dirty = false;
                        }
                        7
                    }
                    MAN_WRITE => {
                        self.busy_until = self.clock + WRITE_BUSY_CLOCKS;
                        return;
                    }
                    SENSOR | INSERTED => 1,
                    HW_ADDR | DISKCHANGE | DRIVETYPE => 3,
                    RAMMAP => 0xfe,
                    // Read-only registers and strobes.
                    _ => return,
                };
                self.regs[r] = v & mask;
            }
            1 => {
                if v & 0x80 != 0 {
                    self.any_dirty = false;
                } else {
                    self.dirty &= !(1u128 << (off & 0x7f));
                }
            }
            2 => self.param[(off & 0x7ff) as usize] = v,
            _ => self.wd.write(off & 0xf, v),
        }
    }

    pub fn powered(&self) -> bool {
        self.regs[POWER] & 1 != 0
    }

    /// The RESET register: bit 0 hold, bit 1 follow the C64's reset,
    /// bit 2 stand while the C64 is stopped.
    pub fn reset_control(&self) -> u8 {
        self.regs[RESET]
    }

    /// Off or held in reset: the drive is not on the bus.
    pub fn held(&self) -> bool {
        !self.powered() || self.regs[RESET] & 1 != 0
    }

    /// The C64's reset resets the drive too (RESET bit 1).
    pub fn follows_c64_reset(&self) -> bool {
        self.regs[RESET] & 2 != 0
    }

    /// The drive's clock stands while the C64 is stopped (RESET bit 2).
    pub fn stops_when_frozen(&self) -> bool {
        self.regs[RESET] & 4 != 0
    }

    /// The drive came out of a hold since the last call: how it starts.
    /// While it is still held the answer waits.
    pub fn take_restart(&mut self) -> Option<Restart> {
        if self.held() {
            return None;
        }
        self.restart.take()
    }

    /// Something else holds the drive's reset line (the C64's reset with
    /// RESET bit 1): it starts again when that ends.
    pub fn note_restart(&mut self) {
        self.restart.get_or_insert(Restart::Reset);
    }

    /// The device number the jumpers select.
    pub fn device(&self) -> u8 {
        8 + self.regs[HW_ADDR]
    }

    pub fn write_protected(&self) -> bool {
        self.regs[SENSOR] & 1 == 0
    }

    pub fn inserted(&self) -> bool {
        self.regs[INSERTED] & 1 != 0
    }

    /// 0 = 1541, 1 = 1571, 2 = 1581.
    pub fn drive_type(&self) -> u8 {
        self.regs[DRIVETYPE]
    }

    /// The track table's entry for a side and half-track: the address of
    /// the track's first byte in board memory, the offset of its last
    /// byte and the bit time.
    pub fn track_entry(&self, side: u8, half_track: u8) -> (u32, u16, u16) {
        let i = ((usize::from(side & 1) << 8) | (usize::from(half_track & 0x7f) << 1)) * 4;
        let word = |i: usize| {
            u32::from_le_bytes([
                self.param[i],
                self.param[i + 1],
                self.param[i + 2],
                self.param[i + 3],
            ])
        };
        let (start, len) = (word(i), word(i + 4));
        (
            start & 0x03ff_ffff,
            (len & 0x3fff) as u16,
            ((len >> 16) & 0x3ff) as u16,
        )
    }

    /// The board's clock now (write busy counts it).
    pub fn set_clock(&mut self, now: u64) {
        self.clock = now;
    }

    /// The head is in write mode with the motor on over this track: with
    /// a disk inserted its DIRTY bit and "any" go up.
    pub fn mark_written(&mut self, side: u8, half_track: u8) {
        if self.inserted() {
            self.any_dirty = true;
            let index = (u32::from(side & 1) << 6) | u32::from((half_track >> 1) & 0x3f);
            self.dirty |= 1u128 << index;
        }
    }

    /// What the drive's head does now; writing with the motor on keeps
    /// write busy up.
    pub fn report_head(&mut self, half_track: u8, side: u8, motor: bool, writing: bool) {
        self.head = half_track & 0x7f;
        self.side = side & 1;
        self.motor = motor;
        self.writing = writing;
        if writing && motor {
            self.busy_until = self.clock + WRITE_BUSY_CLOCKS;
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.bool(self.multi_mode);
        w.raw(&self.regs);
        w.raw(&self.param);
        w.raw(&self.dirty.to_le_bytes());
        w.bool(self.any_dirty);
        w.u8(self.head);
        w.u8(self.side);
        w.bool(self.motor);
        w.bool(self.writing);
        w.u64(self.clock);
        w.u64(self.busy_until);
        w.u8(match self.restart {
            None => 0,
            Some(Restart::Reset) => 1,
            Some(Restart::PowerOn) => 2,
        });
        for v in [
            self.wd.index,
            self.wd.track,
            self.wd.status,
            self.wd.dma_mode,
            self.wd.goto_track,
            self.wd.step_time,
        ] {
            w.u8(v);
        }
        w.u32(self.wd.dma_addr);
        w.u16(self.wd.dma_len);
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        self.multi_mode = r.bool()?;
        r.raw_into(&mut self.regs)?;
        r.raw_into(&mut self.param)?;
        let mut dirty = [0; 16];
        r.raw_into(&mut dirty)?;
        self.dirty = u128::from_le_bytes(dirty);
        self.any_dirty = r.bool()?;
        self.head = r.u8()? & 0x7f;
        self.side = r.u8()? & 1;
        self.motor = r.bool()?;
        self.writing = r.bool()?;
        self.clock = r.u64()?;
        self.busy_until = r.u64()?;
        self.restart = match r.u8()? {
            0 => None,
            1 => Some(Restart::Reset),
            2 => Some(Restart::PowerOn),
            _ => return Err(StateError::Invalid("drive restart")),
        };
        self.wd.index = r.u8()? & 3;
        self.wd.track = r.u8()?;
        self.wd.status = r.u8()?;
        self.wd.dma_mode = r.u8()? & 3;
        self.wd.goto_track = r.u8()? & 0x7f;
        self.wd.step_time = r.u8()? & 0x1f;
        self.wd.dma_addr = r.u32()? & 0x00ff_ffff;
        self.wd.dma_len = r.u16()? & 0x3fff;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIRTY: u32 = 0x800;

    #[test]
    fn dirty_bits_set_by_the_head_cleared_by_the_firmware() {
        let mut d = Drive::new(true);
        // No disk: writing marks nothing.
        d.mark_written(0, 34);
        assert_eq!(d.read(DIRTY), 0);
        d.write(4, 1);
        d.mark_written(0, 34);
        d.mark_written(1, 3);
        // Index side << 6 | half-track >> 1.
        assert_eq!(d.read(DIRTY + 17), 0x81);
        assert_eq!(d.read(DIRTY + 64 + 1), 0x81);
        assert_eq!(d.read(DIRTY + 16), 0x80);
        // Bit 7 written: "any" only.
        d.write(DIRTY, 0x80);
        assert_eq!(d.read(DIRTY + 17), 0x01);
        // Without bit 7: this track's bit.
        d.write(DIRTY + 17, 0);
        assert_eq!(d.read(DIRTY + 17), 0);
        assert_eq!(d.read(DIRTY + 65), 1);
        // Holding the drive in reset clears "any".
        d.mark_written(0, 0);
        d.write(1, 1);
        assert_eq!(d.read(DIRTY), 0x01);
    }

    #[test]
    fn head_status_and_write_busy() {
        let mut d = Drive::new(true);
        d.set_clock(1000);
        d.report_head(35, 0, true, false);
        assert_eq!((d.read(8), d.read(6), d.read(9)), (35, 0, 1));
        d.report_head(35, 1, true, true);
        assert_eq!((d.read(6), d.read(9)), (1, 7));
        // Busy outlasts the writing by 2047 ms.
        d.report_head(35, 0, false, false);
        d.set_clock(1000 + WRITE_BUSY_CLOCKS - 1);
        assert_eq!(d.read(9), 4);
        d.set_clock(1000 + WRITE_BUSY_CLOCKS);
        assert_eq!(d.read(9), 0);
        // MAN_WRITE starts it again; a reset hold ends it.
        d.write(7, 1);
        assert_eq!(d.read(9), 4);
        assert_eq!(d.read(7), 0);
        d.write(1, 1);
        assert_eq!(d.read(9), 0);
    }

    #[test]
    fn power_and_reset_restart_the_drive() {
        let mut d = Drive::new(true);
        assert!(d.held());
        assert_eq!(d.take_restart(), None);
        // As the firmware does it: power on, reset pulse, let go.
        d.write(0, 1);
        d.write(1, 1);
        assert_eq!(d.take_restart(), None, "still held");
        d.write(1, 6);
        assert!(!d.held() && d.follows_c64_reset() && d.stops_when_frozen());
        assert_eq!(d.take_restart(), Some(Restart::PowerOn));
        assert_eq!(d.take_restart(), None);
        d.write(1, 1);
        d.write(1, 0);
        assert_eq!(d.take_restart(), Some(Restart::Reset));
        d.note_restart();
        assert_eq!(d.take_restart(), Some(Restart::Reset));
        d.write(0, 0);
        assert!(d.held());
    }

    #[test]
    fn state_round_trip() {
        let mut d = Drive::new(true);
        d.write(0, 1);
        d.write(4, 1);
        d.write(0x1000 + 9, 0x42);
        d.set_clock(77);
        d.report_head(20, 0, true, true);
        d.mark_written(0, 20);
        let mut w = StateWriter::new();
        d.save(&mut w);
        let bytes = w.into_bytes();
        let mut e = Drive::new(true);
        e.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(d, e);
        assert!(e.load(&mut StateReader::new(&bytes[..40])).is_err());
    }
}
