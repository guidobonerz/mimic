//! Drive 8 on the serial bus: a true-emulated 1541 clocked at 1 MHz next to
//! the C128's 985 248 Hz (PAL) or 1 022 727 Hz (NTSC). The 1571 and fast
//! serial transfers follow with the drive work (milestone 6).
//!
//! C128 side (as on the C64): CIA2 PA3 ATN out, PA4 CLK out, PA5 DATA
//! out through 7406 inverters (1 = line pulled low); PA6 CLK in, PA7 DATA
//! in read the line level (1 = released).

use drives_cbm::{Drive1541, IecLines};
use emu_core::{
    ClockDivider, CycleResult, DebugHooks, DeviceId, MemSpaceId, Savestate, StateError,
    StateReader, StateWriter, StopReason,
};

pub const DRIVE8: DeviceId = DeviceId("drive8");
/// The 1541's CPU address space.
pub const DRIVE_SPACE: MemSpaceId = MemSpaceId(3);

const DRIVE_HZ: u32 = 1_000_000;

/// What the C64 pulls on the bus, from CIA2 port A.
pub fn c64_lines(cia2_pa: u8) -> IecLines {
    IecLines {
        atn: cia2_pa & 0x08 != 0,
        clk: cia2_pa & 0x10 != 0,
        data: cia2_pa & 0x20 != 0,
    }
}

pub struct Drive8 {
    pub drive: Drive1541,
    clock: ClockDivider,
}

impl Drive8 {
    pub fn new(rom: Box<[u8; drives_cbm::c1541::ROM_SIZE]>, c64_hz: u32) -> Self {
        Self {
            drive: Drive1541::new(rom, 8, DRIVE_SPACE),
            clock: ClockDivider::new(DRIVE_HZ, c64_hz).expect("non-zero clock"),
        }
    }

    /// Run the drive cycles that fall into one C64 cycle. Returns the bus
    /// levels afterwards and a stop reason from the drive's debug hooks.
    pub fn run(
        &mut self,
        c64: IecLines,
        mut dbg: Option<&mut DebugHooks>,
    ) -> (IecLines, Option<StopReason>) {
        let mut stop = None;
        for _ in 0..self.clock.tick() {
            let bus = IecLines::combine(&[c64, self.drive.iec_out(c64.atn)]);
            let r = self.drive.tick(bus, dbg.as_deref_mut());
            match r {
                CycleResult::InstrDone { next_pc } => {
                    if let Some(d) = dbg.as_deref_mut() {
                        if d.exec_armed(DRIVE_SPACE) {
                            d.check_exec(DRIVE_SPACE, next_pc);
                        }
                    }
                }
                CycleResult::Jammed => {
                    stop = Some(StopReason::CpuJam {
                        device: DRIVE8,
                        pc: u32::from(self.drive.pc()),
                    });
                }
                CycleResult::Busy => {}
            }
        }
        (IecLines::combine(&[c64, self.drive.iec_out(c64.atn)]), stop)
    }

    pub fn save(&self, w: &mut StateWriter) {
        self.drive.save(w);
        self.clock.save(w);
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        self.drive.load(r)?;
        self.clock.load(r)
    }
}
