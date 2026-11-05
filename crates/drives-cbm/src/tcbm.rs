//! The TCBM bus of the C16/Plus/4: a 1551 disk drive and its interface
//! cartridge ("paddle", schematic 251925) in the expansion port.
//!
//! The paddle holds a 6523A-28 TIA (only PA0–7, PB0–1, PC6–7 bonded out;
//! the other input bits read 0) and a PLA that maps it at `$FEE0–$FEFF`
//! when the drive holds DEV high (device 8; the KERNAL uses `$FEF0`) or at
//! `$FEC0–$FEDF` when DEV is low (device 9); registers repeat every
//! 8 bytes. The cable joins:
//!
//! | Line      | Host (paddle TIA)  | Drive (TIA at `$4000`) |
//! | --------- | ------------------ | ---------------------- |
//! | DIO1–8    | PA0–7              | PA0–7                  |
//! | STATUS0/1 | PB0/PB1 (in)       | PC0/PC1 (out)          |
//! | DAV       | PC6 (out)          | PC7 (in)               |
//! | ACK       | PC7 (in)           | PC3 (out)              |
//! | DEV       | PLA (address)      | PC2 (out)              |
//! | RESET     | the host's reset   | RESET                  |
//!
//! Every line has a pull-up; a line is low when either side drives it low
//! (released inputs read high). The host's reset resets the drive.
//! The protocol on these lines is the KERNAL's and the DOS's business
//! (described in "Commodore Peripheral Bus: Part 5: TCBM", pagetable.com).
//!
//! The drive's 6510T runs from its own 16 MHz crystal at 2 MHz; it is
//! clocked per microsecond next to the host like the serial drives.
//!
//! Sources: 1551 schematic 251860, paddle schematic 251925 and parts list
//! (zimmers.net); Plus/4 KERNAL 318004-05 (`$EC8B–$EE10`, `$F313`).

use emu_core::media::{DriveStatus, MediaSlot};
use emu_core::{
    ClockDivider, CycleResult, DebugHooks, DebugTarget, DeviceId, MemSpaceId, Savestate,
    StateError, StateReader, StateWriter, StopReason,
};

use crate::c1541::{CbmDrive, DriveModel, TcbmHost};
use crate::iec::IecLines;
use chips_cbm::tia::Tia6523;

const DRIVE_HZ: u32 = 1_000_000;
/// Port bits the paddle's 28-pin TIA has: PA0–7, PB0–1, PC6–7.
const BONDED: [u8; 3] = [0xff, 0x03, 0xc0];

pub struct Tcbm1551 {
    pub drive: CbmDrive,
    /// The interface cartridge's TIA.
    pub paddle: Tia6523,
    /// The drive's debug device ("drive8").
    pub device: DeviceId,
    clock: ClockDivider,
}

impl Tcbm1551 {
    /// A 1551 with device number 8 or 9 (the jumper), its CPU memory as
    /// debug space `space`, next to a host clocked at `host_hz`.
    pub fn new(
        rom: Box<[u8]>,
        number: u8,
        device: DeviceId,
        space: MemSpaceId,
        host_hz: u32,
    ) -> Self {
        Self {
            drive: CbmDrive::new(DriveModel::C1551, rom, number.clamp(8, 9), space),
            paddle: Tia6523::new(),
            device,
            clock: ClockDivider::new(DRIVE_HZ, host_hz).expect("non-zero clock"),
        }
    }

    /// Run the drive's crystal `ppm` parts per million off nominal.
    pub fn set_crystal_ppm(&mut self, ppm: i32, host_hz: u32) {
        let hz = DRIVE_HZ.saturating_add_signed(ppm.clamp(-1000, 1000));
        self.clock = ClockDivider::new(hz, host_hz).expect("non-zero clock");
    }

    /// The paddle's register for a host address, if the PLA selects it.
    fn register(&self, addr: u16) -> Option<u8> {
        let base = if self.drive.tcbm_out().dev {
            0xfee0
        } else {
            0xfec0
        };
        (addr & 0xffe0 == base).then_some(addr as u8 & 7)
    }

    /// Levels at the paddle's ports A, B, C.
    fn host_pins(&self) -> [u8; 3] {
        let d = self.drive.tcbm_out();
        let pa = self.paddle.outputs(0) & d.dio;
        let pb = d.status;
        let pc = (self.paddle.outputs(2) & 0x40) | if d.ack { 0x80 } else { 0 };
        [pa & BONDED[0], pb & BONDED[1], pc & BONDED[2]]
    }

    /// A host read without side effects (the TIA has none); `None` where
    /// the paddle does not answer.
    pub fn peek(&self, addr: u16) -> Option<u8> {
        self.register(addr)
            .map(|r| self.paddle.read(r, self.host_pins()))
    }

    /// A host write; false where the paddle does not answer.
    pub fn write(&mut self, addr: u16, v: u8) -> bool {
        match self.register(addr) {
            Some(r) => {
                self.paddle.write(r, v);
                true
            }
            None => false,
        }
    }

    /// RESET from the host: the paddle and the drive restart; `hard` =
    /// power cycle.
    pub fn reset(&mut self, hard: bool) {
        self.paddle.reset();
        self.drive.reset(hard);
    }

    /// Run the drive microseconds that fall into one host cycle.
    pub fn run(&mut self, mut dbg: Option<&mut DebugHooks>) -> Option<StopReason> {
        let mut stop = None;
        for _ in 0..self.clock.tick() {
            self.drive.set_tcbm_host(TcbmHost {
                dio: self.paddle.outputs(0),
                dav: self.paddle.outputs(2) & 0x40 != 0,
            });
            if let CycleResult::Jammed = self.drive.tick(IecLines::default(), dbg.as_deref_mut()) {
                stop = Some(StopReason::CpuJam {
                    device: self.device,
                    pc: u32::from(self.drive.pc()),
                });
            }
        }
        stop
    }

    pub fn has_disk(&self) -> bool {
        self.drive.has_disk()
    }

    pub fn status(&self) -> DriveStatus {
        self.drive.status()
    }

    pub fn media_slot(&mut self) -> &mut dyn MediaSlot {
        &mut self.drive
    }

    pub fn debug_target(&mut self) -> &mut dyn DebugTarget {
        &mut self.drive
    }

    pub fn space(&self) -> MemSpaceId {
        self.drive.space()
    }

    /// The media slot's name: "drive 8 (1551)" or "drive 9 (1551)".
    pub fn slot_name(&self) -> &'static str {
        if self.register(0xfee0).is_some() {
            "drive 8 (1551)"
        } else {
            "drive 9 (1551)"
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        self.drive.save(w);
        self.paddle.save(w);
        self.clock.save(w);
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let mut paddle = self.paddle;
        let mut clock = self.clock;
        self.drive.load(r)?;
        paddle.load(r)?;
        clock.load(r)?;
        self.paddle = paddle;
        self.clock = clock;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1551 whose ROM idles in a loop; the test drives its TIA by hand.
    fn tcbm() -> Tcbm1551 {
        let mut rom = vec![0xea; 0x4000];
        // $C000: JMP $C000; reset vector $C000.
        rom[..3].copy_from_slice(&[0x4c, 0x00, 0xc0]);
        rom[0x3ffc] = 0x00;
        rom[0x3ffd] = 0xc0;
        Tcbm1551::new(rom.into(), 8, DeviceId("drive8"), MemSpaceId(3), 1_773_447)
    }

    #[test]
    fn paddle_appears_where_dev_selects() {
        let mut t = tcbm();
        // DEV released: pulled high, device 8's window.
        assert!(t.peek(0xfef3).is_some());
        assert!(t.peek(0xfec3).is_none());
        // The drive pulls DEV low (PC2 output, 0): the device 9 window.
        t.drive.tia.write(5, 0x1f);
        t.drive.tia.write(2, 0x00);
        assert!(t.peek(0xfef3).is_none());
        assert!(t.peek(0xfec3).is_some());
        assert_eq!(t.slot_name(), "drive 9 (1551)");
    }

    #[test]
    fn lines_join_both_tias() {
        let mut t = tcbm();
        // Host drives DIO with $55, DAV low.
        assert!(t.write(0xfef3, 0xff));
        assert!(t.write(0xfef0, 0x55));
        assert!(t.write(0xfef5, 0x40));
        assert!(t.write(0xfef2, 0x00));
        // Drive: STATUS1 high, STATUS0 low, ACK low.
        t.drive.tia.write(5, 0x1f);
        t.drive.tia.write(2, 0x06);
        for _ in 0..4 {
            t.run(None);
        }
        let d = t.drive.peek(MemSpaceId(3), 0x4000, emu_core::Size::Byte);
        assert_eq!(d, Some(0x55), "drive PA sees DIO");
        let pc = t
            .drive
            .peek(MemSpaceId(3), 0x4002, emu_core::Size::Byte)
            .unwrap();
        assert_eq!(pc & 0x80, 0, "DAV low at the drive");
        assert_eq!(t.peek(0xfef1), Some(0x02), "status at the host's PB");
        assert_eq!(t.peek(0xfef2).map(|v| v & 0x80), Some(0), "ACK low");
        // Unbonded host pins: output bits read their latch, inputs 0.
        assert!(t.write(0xfef4, 0xfc));
        assert!(t.write(0xfef1, 0xfc));
        assert_eq!(t.peek(0xfef1).map(|v| v & 0xfc), Some(0xfc));
        assert!(t.write(0xfef4, 0x00));
        assert_eq!(t.peek(0xfef1).map(|v| v & 0xfc), Some(0));
    }

    #[test]
    fn irq_pulses_every_8330_microseconds() {
        let mut t = tcbm();
        // Count IRQ entries: vector $FFFE points at an RTI.
        let mut rom = vec![0xea; 0x4000];
        rom[..4].copy_from_slice(&[0x58, 0x4c, 0x01, 0xc0]); // CLI; JMP $C001
        rom[0x100] = 0xee; // $C100: INC $0010; RTI
        rom[0x101] = 0x10;
        rom[0x102] = 0x00;
        rom[0x103] = 0x40;
        rom[0x3ffc] = 0x00;
        rom[0x3ffd] = 0xc0;
        rom[0x3ffe] = 0x00;
        rom[0x3fff] = 0xc1;
        t.drive = CbmDrive::new(DriveModel::C1551, rom.into(), 8, MemSpaceId(3));
        // Pulses at 0, 8330 and 16 660 µs; the handler outlasts the 7 µs
        // low time, so each is taken once.
        for _ in 0..(8330 * 2 + 100) {
            t.drive.tick(IecLines::default(), None);
        }
        let n = t.drive.peek(MemSpaceId(3), 0x10, emu_core::Size::Byte);
        assert_eq!(n, Some(3), "one IRQ per period");
    }

    #[test]
    fn savestate_round_trip() {
        let mut t = tcbm();
        t.write(0xfef3, 0xff);
        t.write(0xfef0, 0x81);
        for _ in 0..100 {
            t.run(None);
        }
        let mut w = StateWriter::new();
        t.save(&mut w);
        let bytes = w.into_bytes();
        let mut u = tcbm();
        u.load(&mut StateReader::new(&bytes)).unwrap();
        let mut w2 = StateWriter::new();
        u.save(&mut w2);
        assert_eq!(bytes, w2.into_bytes());
    }
}
