//! A device on the serial bus clocked next to the host computer: a
//! true-emulated 1541 or the virtual drive, at 1 MHz against the host's
//! clock. Machines own one per drive slot and only map their port pins to
//! [`IecLines`].
//!
//! The drive mode is a slot setting chosen when the machine is built
//! (`drive8` = "1541" or "virtual"); the true drive needs the DOS ROM.

use emu_core::media::MediaSlot;
use emu_core::registry::ConfigError;
use emu_core::{
    ClockDivider, CycleResult, DebugHooks, DebugTarget, DeviceId, MemSpaceId, Savestate,
    StateError, StateReader, StateWriter, StopReason,
};

use crate::c1541::{self, Drive1541};
use crate::iec::IecLines;
use crate::virtual_drive::VirtualDrive;

const DRIVE_HZ: u32 = 1_000_000;

pub enum DriveKind {
    /// Its own CPU and DOS: fast loaders and copy protection work.
    True(Box<Drive1541>),
    /// KERNAL-level file access served directly from the image.
    Virtual(Box<VirtualDrive>),
}

pub struct SerialDrive {
    pub kind: DriveKind,
    /// The debug device of a true drive ("drive8").
    pub device: DeviceId,
    clock: ClockDivider,
}

impl SerialDrive {
    /// A true 1541 with device number `number`, CPU memory as debug space
    /// `space`, next to a host clocked at `host_hz`.
    pub fn new_1541(
        rom: Box<[u8; c1541::ROM_SIZE]>,
        number: u8,
        device: DeviceId,
        space: MemSpaceId,
        host_hz: u32,
    ) -> Self {
        Self {
            kind: DriveKind::True(Box::new(Drive1541::new(rom, number, space))),
            device,
            clock: ClockDivider::new(DRIVE_HZ, host_hz).expect("non-zero clock"),
        }
    }

    /// The drive a machine option asks for: "1541" (true drive, needs the
    /// DOS ROM), "virtual" or "none"; without the option the true drive if
    /// `rom` is given, else the virtual one.
    pub fn from_option(
        option: Option<&str>,
        rom: Option<&[u8]>,
        number: u8,
        device: DeviceId,
        space: MemSpaceId,
        host_hz: u32,
    ) -> Result<Option<Self>, ConfigError> {
        let true_drive = |rom: &[u8]| -> Result<Option<Self>, ConfigError> {
            let rom: Box<[u8; c1541::ROM_SIZE]> =
                rom.to_vec().into_boxed_slice().try_into().map_err(|_| {
                    ConfigError::Invalid(format!(
                        "dos1541 ROM must be {} bytes, got {}",
                        c1541::ROM_SIZE,
                        rom.len()
                    ))
                })?;
            Ok(Some(Self::new_1541(rom, number, device, space, host_hz)))
        };
        match (option, rom) {
            (Some("none"), _) => Ok(None),
            (Some("virtual"), _) | (None, None) => {
                Ok(Some(Self::new_virtual(number, device, host_hz)))
            }
            (Some("1541") | None, Some(rom)) => true_drive(rom),
            (Some("1541"), None) => Err(ConfigError::MissingRom("dos1541".into())),
            (Some(other), _) => Err(ConfigError::Invalid(format!(
                "drive{number} option '{other}' (1541, virtual or none)"
            ))),
        }
    }

    pub fn new_virtual(number: u8, device: DeviceId, host_hz: u32) -> Self {
        Self {
            kind: DriveKind::Virtual(Box::new(VirtualDrive::new(number))),
            device,
            clock: ClockDivider::new(DRIVE_HZ, host_hz).expect("non-zero clock"),
        }
    }

    /// True drive emulation (a debug target with its own CPU).
    pub fn is_true(&self) -> bool {
        matches!(self.kind, DriveKind::True(_))
    }

    pub fn has_disk(&self) -> bool {
        match &self.kind {
            DriveKind::True(d) => d.has_disk(),
            DriveKind::Virtual(d) => d.has_disk(),
        }
    }

    /// RESET from the serial bus; `hard` = power cycle.
    pub fn reset(&mut self, hard: bool) {
        match &mut self.kind {
            DriveKind::True(d) => d.reset(hard),
            DriveKind::Virtual(d) => d.reset(),
        }
    }

    pub fn media_slot(&mut self) -> &mut dyn MediaSlot {
        match &mut self.kind {
            DriveKind::True(d) => d.as_mut(),
            DriveKind::Virtual(d) => d.as_mut(),
        }
    }

    /// The drive's CPU for the debugger (true drives only).
    pub fn debug_target(&mut self) -> Option<&mut dyn DebugTarget> {
        match &mut self.kind {
            DriveKind::True(d) => Some(d.as_mut()),
            DriveKind::Virtual(_) => None,
        }
    }

    /// The debug memory space of a true drive.
    pub fn space(&self) -> Option<MemSpaceId> {
        match &self.kind {
            DriveKind::True(d) => Some(d.space()),
            DriveKind::Virtual(_) => None,
        }
    }

    fn out(&self, atn: bool) -> IecLines {
        match &self.kind {
            DriveKind::True(d) => d.iec_out(atn),
            DriveKind::Virtual(d) => d.iec_out(),
        }
    }

    /// Run the drive cycles that fall into one host cycle. Returns the bus
    /// levels afterwards and a stop reason from the drive's debug hooks.
    pub fn run(
        &mut self,
        host: IecLines,
        mut dbg: Option<&mut DebugHooks>,
    ) -> (IecLines, Option<StopReason>) {
        let mut stop = None;
        for _ in 0..self.clock.tick() {
            let bus = IecLines::combine(&[host, self.out(host.atn)]);
            match &mut self.kind {
                DriveKind::True(d) => match d.tick(bus, dbg.as_deref_mut()) {
                    CycleResult::InstrDone { next_pc } => {
                        if let Some(dbg) = dbg.as_deref_mut() {
                            if dbg.exec_armed(d.space()) {
                                dbg.check_exec(d.space(), next_pc);
                            }
                        }
                    }
                    CycleResult::Jammed => {
                        stop = Some(StopReason::CpuJam {
                            device: self.device,
                            pc: u32::from(d.pc()),
                        });
                    }
                    CycleResult::Busy => {}
                },
                DriveKind::Virtual(d) => d.tick(bus),
            }
        }
        (IecLines::combine(&[host, self.out(host.atn)]), stop)
    }

    pub fn save(&self, w: &mut StateWriter) {
        match &self.kind {
            DriveKind::True(d) => d.save(w),
            DriveKind::Virtual(d) => d.save(w),
        }
        self.clock.save(w);
    }

    /// The drive kind comes from the configuration; a state of the other
    /// kind is rejected (its chunk is missing).
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        match &mut self.kind {
            DriveKind::True(d) => d.load(r)?,
            DriveKind::Virtual(d) => d.load(r)?,
        }
        self.clock.load(r)
    }
}
