//! Drive 8: on the serial bus a [`SerialDrive`] (true 1541, 1571 or 1581,
//! or the virtual drive), or a 1551 on the TCBM bus with its interface
//! cartridge at `$FEE0–$FEFF` ([`Tcbm1551`]), clocked next to the TED's
//! double clock (1 773 447 Hz PAL, 1 789 772 Hz NTSC). The CPU port wiring
//! is in the crate docs.

use drives_cbm::{DriveModel, DriveRoms, IecLines, SerialDrive, Tcbm1551};
use emu_core::media::{DriveStatus, MediaSlot};
use emu_core::registry::ConfigError;
use emu_core::{
    DebugHooks, DebugTarget, DeviceId, MemSpaceId, StateError, StateReader, StateWriter, StopReason,
};

pub const DRIVE8: DeviceId = DeviceId("drive8");
/// The drive CPU's address space.
pub const DRIVE_SPACE: MemSpaceId = MemSpaceId(3);

pub enum Drive8 {
    Serial(SerialDrive),
    Tcbm(Box<Tcbm1551>),
}

impl Drive8 {
    /// The `drive8` option: "1551" (needs `roms["dos1551"]`), else what
    /// [`SerialDrive::from_option`] takes.
    pub fn from_option(
        option: Option<&str>,
        roms: DriveRoms<'_>,
        dos1551: Option<&[u8]>,
        host_hz: u32,
    ) -> Result<Option<Self>, ConfigError> {
        if option == Some("1551") {
            let rom = dos1551.ok_or_else(|| ConfigError::MissingRom("dos1551".into()))?;
            let size = DriveModel::C1551.rom_size();
            if rom.len() != size {
                return Err(ConfigError::Invalid(format!(
                    "dos1551 ROM must be {size} bytes, got {}",
                    rom.len()
                )));
            }
            return Ok(Some(Self::Tcbm(Box::new(Tcbm1551::new(
                rom.into(),
                8,
                DRIVE8,
                DRIVE_SPACE,
                host_hz,
            )))));
        }
        Ok(SerialDrive::from_option(
            option,
            roms,
            DriveModel::C1541,
            8,
            DRIVE8,
            DRIVE_SPACE,
            host_hz,
        )
        .map_err(|e| match (e, option) {
            (ConfigError::Invalid(m), Some(o)) if m.contains("option") => ConfigError::Invalid(
                format!("drive8 option '{o}' (1541, 1571, 1581, 1551, virtual or none)"),
            ),
            (e, _) => e,
        })?
        .map(Self::Serial))
    }

    pub fn set_crystal_ppm(&mut self, ppm: i32, host_hz: u32) {
        match self {
            Self::Serial(d) => d.set_crystal_ppm(ppm, host_hz),
            Self::Tcbm(d) => d.set_crystal_ppm(ppm, host_hz),
        }
    }

    pub fn tcbm(&self) -> Option<&Tcbm1551> {
        match self {
            Self::Tcbm(d) => Some(d),
            Self::Serial(_) => None,
        }
    }

    pub fn tcbm_mut(&mut self) -> Option<&mut Tcbm1551> {
        match self {
            Self::Tcbm(d) => Some(d),
            Self::Serial(_) => None,
        }
    }

    /// Run the drive cycles of one host cycle; the serial bus levels
    /// afterwards (a 1551 leaves the bus to the host).
    pub fn run(
        &mut self,
        host: IecLines,
        dbg: Option<&mut DebugHooks>,
    ) -> (IecLines, Option<StopReason>) {
        match self {
            Self::Serial(d) => d.run(host, dbg),
            Self::Tcbm(d) => (host, d.run(dbg)),
        }
    }

    pub fn is_true(&self) -> bool {
        match self {
            Self::Serial(d) => d.is_true(),
            Self::Tcbm(_) => true,
        }
    }

    pub fn has_disk(&self) -> bool {
        match self {
            Self::Serial(d) => d.has_disk(),
            Self::Tcbm(d) => d.has_disk(),
        }
    }

    pub fn slot_name(&self) -> &'static str {
        match self {
            Self::Serial(d) => d.slot_name(),
            Self::Tcbm(d) => d.slot_name(),
        }
    }

    pub fn status(&self) -> DriveStatus {
        match self {
            Self::Serial(d) => d.status(),
            Self::Tcbm(d) => d.status(),
        }
    }

    pub fn reset(&mut self, hard: bool) {
        match self {
            Self::Serial(d) => d.reset(hard),
            Self::Tcbm(d) => d.reset(hard),
        }
    }

    pub fn media_slot(&mut self) -> &mut dyn MediaSlot {
        match self {
            Self::Serial(d) => d.media_slot(),
            Self::Tcbm(d) => d.media_slot(),
        }
    }

    pub fn debug_target(&mut self) -> Option<&mut dyn DebugTarget> {
        match self {
            Self::Serial(d) => d.debug_target(),
            Self::Tcbm(d) => Some(d.debug_target()),
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        match self {
            Self::Serial(d) => d.save(w),
            Self::Tcbm(d) => d.save(w),
        }
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        match self {
            Self::Serial(d) => d.load(r),
            Self::Tcbm(d) => d.load(r),
        }
    }
}
