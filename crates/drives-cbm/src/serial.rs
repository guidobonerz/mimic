//! A device on the serial bus clocked next to the host computer: a
//! true-emulated 1541, 1571 or 1581, or the virtual drive, ticked per
//! microsecond against the host's clock. Machines own one per drive slot
//! and only map their port pins to [`IecLines`].
//!
//! The drive mode is a slot setting chosen when the machine is built
//! (`drive8` = "1541", "1571", "1581" or "virtual"); a true drive needs its
//! DOS ROM.
//!
//! The drive has its own crystal, which is never exactly on its nominal
//! frequency (quartz tolerance is some ±50 ppm), so on real hardware the
//! drive's phase against the computer drifts and timing races between the
//! two (e.g. a fast serial byte arriving while a halted CPU repeats its
//! ICR read) happen only now and then. With exactly nominal clocks such a
//! race would hit at the same point every time; the drive therefore runs
//! [`DEFAULT_CRYSTAL_PPM`] off nominal unless `drive8_ppm` says otherwise.

use emu_core::media::MediaSlot;
use emu_core::registry::ConfigError;
use emu_core::CycleResult as Cycle;
use emu_core::{
    ClockDivider, CycleResult, DebugHooks, DebugTarget, DeviceId, MemSpaceId, Savestate,
    StateError, StateReader, StateWriter, StopReason,
};

use crate::c1541::{CbmDrive, DriveModel};
use crate::c1581::Drive1581;
use crate::iec::IecLines;
use crate::virtual_drive::VirtualDrive;

const DRIVE_HZ: u32 = 1_000_000;

/// Default deviation of the drive crystal from nominal, in ppm.
pub const DEFAULT_CRYSTAL_PPM: i32 = 30;

/// The `drive8_ppm` option: crystal deviation in ppm (−1000 to 1000).
pub fn crystal_ppm(option: Option<&str>) -> Result<i32, ConfigError> {
    let Some(v) = option else {
        return Ok(DEFAULT_CRYSTAL_PPM);
    };
    v.parse::<i32>()
        .ok()
        .filter(|p| (-1000..=1000).contains(p))
        .ok_or_else(|| ConfigError::Invalid(format!("drive8_ppm '{v}' (-1000 to 1000)")))
}

/// A drive with its own CPU and DOS.
pub trait TrueDrive: MediaSlot + DebugTarget + Savestate + Send {
    fn model(&self) -> DriveModel;
    /// The debug memory space of the drive's CPU.
    fn space(&self) -> MemSpaceId;
    fn pc(&self) -> u16;
    fn has_disk(&self) -> bool;
    /// RESET from the serial bus; `hard` = power cycle.
    fn reset(&mut self, hard: bool);
    /// Lines the drive pulls, given the bus ATN level.
    fn iec_out(&self, atn_low: bool) -> IecLines;
    /// One microsecond with the lines the host pulls.
    fn tick(&mut self, host: IecLines, dbg: Option<&mut DebugHooks>) -> Cycle;
}

impl TrueDrive for CbmDrive {
    fn model(&self) -> DriveModel {
        CbmDrive::model(self)
    }
    fn space(&self) -> MemSpaceId {
        CbmDrive::space(self)
    }
    fn pc(&self) -> u16 {
        CbmDrive::pc(self)
    }
    fn has_disk(&self) -> bool {
        CbmDrive::has_disk(self)
    }
    fn reset(&mut self, hard: bool) {
        CbmDrive::reset(self, hard);
    }
    fn iec_out(&self, atn_low: bool) -> IecLines {
        CbmDrive::iec_out(self, atn_low)
    }
    fn tick(&mut self, host: IecLines, dbg: Option<&mut DebugHooks>) -> Cycle {
        CbmDrive::tick(self, host, dbg)
    }
}

impl TrueDrive for Drive1581 {
    fn model(&self) -> DriveModel {
        DriveModel::C1581
    }
    fn space(&self) -> MemSpaceId {
        Drive1581::space(self)
    }
    fn pc(&self) -> u16 {
        Drive1581::pc(self)
    }
    fn has_disk(&self) -> bool {
        Drive1581::has_disk(self)
    }
    fn reset(&mut self, hard: bool) {
        Drive1581::reset(self, hard);
    }
    fn iec_out(&self, atn_low: bool) -> IecLines {
        Drive1581::iec_out(self, atn_low)
    }
    fn tick(&mut self, host: IecLines, dbg: Option<&mut DebugHooks>) -> Cycle {
        Drive1581::tick(self, host, dbg)
    }
}

pub enum DriveKind {
    /// Its own CPU and DOS: fast loaders and copy protection work.
    True(Box<dyn TrueDrive>),
    /// KERNAL-level file access served directly from the image.
    Virtual(Box<VirtualDrive>),
}

/// The drive DOS ROMs a machine configuration has.
#[derive(Clone, Copy, Debug, Default)]
pub struct DriveRoms<'a> {
    pub dos1541: Option<&'a [u8]>,
    pub dos1571: Option<&'a [u8]>,
    pub dos1581: Option<&'a [u8]>,
}

impl<'a> DriveRoms<'a> {
    /// The ROMs from a configuration's ROM map.
    pub fn from_map(roms: &'a std::collections::BTreeMap<String, Vec<u8>>) -> Self {
        Self {
            dos1541: roms.get("dos1541").map(Vec::as_slice),
            dos1571: roms.get("dos1571").map(Vec::as_slice),
            dos1581: roms.get("dos1581").map(Vec::as_slice),
        }
    }

    fn get(&self, model: DriveModel) -> Option<&'a [u8]> {
        match model {
            DriveModel::C1541 => self.dos1541,
            DriveModel::C1571 => self.dos1571,
            DriveModel::C1581 => self.dos1581,
        }
    }
}

pub struct SerialDrive {
    pub kind: DriveKind,
    /// The debug device of a true drive ("drive8").
    pub device: DeviceId,
    clock: ClockDivider,
}

impl SerialDrive {
    /// A true drive with device number `number`, CPU memory as debug space
    /// `space`, next to a host clocked at `host_hz`.
    pub fn new_true(
        model: DriveModel,
        rom: Box<[u8]>,
        number: u8,
        device: DeviceId,
        space: MemSpaceId,
        host_hz: u32,
    ) -> Self {
        Self {
            kind: DriveKind::True(match model {
                DriveModel::C1581 => Box::new(Drive1581::new(rom, number, space)),
                _ => Box::new(CbmDrive::new(model, rom, number, space)),
            }),
            device,
            clock: ClockDivider::new(DRIVE_HZ, host_hz).expect("non-zero clock"),
        }
    }

    /// The drive a machine option asks for: "1541", "1571" or "1581" (true
    /// drive, needs its DOS ROM), "virtual" or "none". Without the option:
    /// the `preferred` true drive if its ROM is there, else the other 5.25"
    /// drive, else the virtual drive (a 1581 only when asked for).
    pub fn from_option(
        option: Option<&str>,
        roms: DriveRoms<'_>,
        preferred: DriveModel,
        number: u8,
        device: DeviceId,
        space: MemSpaceId,
        host_hz: u32,
    ) -> Result<Option<Self>, ConfigError> {
        let true_drive = |model: DriveModel| -> Result<Option<Self>, ConfigError> {
            let rom = roms
                .get(model)
                .ok_or_else(|| ConfigError::MissingRom(model.rom_role().into()))?;
            if rom.len() != model.rom_size() {
                return Err(ConfigError::Invalid(format!(
                    "{} ROM must be {} bytes, got {}",
                    model.rom_role(),
                    model.rom_size(),
                    rom.len()
                )));
            }
            Ok(Some(Self::new_true(
                model,
                rom.into(),
                number,
                device,
                space,
                host_hz,
            )))
        };
        let other = match preferred {
            DriveModel::C1541 => DriveModel::C1571,
            DriveModel::C1571 | DriveModel::C1581 => DriveModel::C1541,
        };
        match option {
            Some("none") => Ok(None),
            Some("virtual") => Ok(Some(Self::new_virtual(number, device, host_hz))),
            Some("1541") => true_drive(DriveModel::C1541),
            Some("1571") => true_drive(DriveModel::C1571),
            Some("1581") => true_drive(DriveModel::C1581),
            None if roms.get(preferred).is_some() => true_drive(preferred),
            None if roms.get(other).is_some() => true_drive(other),
            None => Ok(Some(Self::new_virtual(number, device, host_hz))),
            Some(other) => Err(ConfigError::Invalid(format!(
                "drive{number} option '{other}' (1541, 1571, 1581, virtual or none)"
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

    /// Run the drive's crystal `ppm` parts per million off nominal, next
    /// to a host clocked at `host_hz`.
    pub fn set_crystal_ppm(&mut self, ppm: i32, host_hz: u32) {
        let hz = DRIVE_HZ.saturating_add_signed(ppm.clamp(-1000, 1000));
        self.clock = ClockDivider::new(hz, host_hz).expect("non-zero clock");
    }

    /// True drive emulation (a debug target with its own CPU).
    pub fn is_true(&self) -> bool {
        matches!(self.kind, DriveKind::True(_))
    }

    /// The model of a true drive.
    pub fn model(&self) -> Option<DriveModel> {
        match &self.kind {
            DriveKind::True(d) => Some(d.model()),
            DriveKind::Virtual(_) => None,
        }
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
            match &mut self.kind {
                DriveKind::True(d) => {
                    if let CycleResult::Jammed = d.tick(host, dbg.as_deref_mut()) {
                        stop = Some(StopReason::CpuJam {
                            device: self.device,
                            pc: u32::from(d.pc()),
                        });
                    }
                }
                DriveKind::Virtual(d) => {
                    let bus = IecLines::combine(&[host, d.iec_out()]);
                    d.tick(bus);
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn pick(option: Option<&str>, roms: DriveRoms<'_>, preferred: DriveModel) -> Option<String> {
        SerialDrive::from_option(
            option,
            roms,
            preferred,
            8,
            DeviceId("drive8"),
            MemSpaceId(3),
            985_248,
        )
        .map(|d| {
            d.map(|d| d.model().map_or("virtual".into(), |m| m.name().to_string()))
                .unwrap_or_else(|| "none".into())
        })
        .ok()
    }

    #[test]
    fn drive_choice_follows_option_and_roms() {
        let (r41, r71) = (vec![0; 0x4000], vec![0; 0x8000]);
        let both = DriveRoms {
            dos1541: Some(&r41),
            dos1571: Some(&r71),
            dos1581: None,
        };
        let only41 = DriveRoms {
            dos1571: None,
            ..both
        };
        let pick = |o, r, p| pick(o, r, p).unwrap_or_else(|| "error".into());
        assert_eq!(pick(None, both, DriveModel::C1571), "1571");
        assert_eq!(pick(None, both, DriveModel::C1541), "1541");
        assert_eq!(pick(None, only41, DriveModel::C1571), "1541");
        assert_eq!(
            pick(None, DriveRoms::default(), DriveModel::C1571),
            "virtual"
        );
        assert_eq!(pick(Some("1571"), only41, DriveModel::C1541), "error");
        assert_eq!(pick(Some("1541"), both, DriveModel::C1571), "1541");
        assert_eq!(pick(Some("none"), both, DriveModel::C1571), "none");
        assert_eq!(pick(Some("1581"), both, DriveModel::C1571), "error");
        let r81 = vec![0; 0x8000];
        let with81 = DriveRoms {
            dos1581: Some(&r81),
            ..both
        };
        assert_eq!(pick(Some("1581"), with81, DriveModel::C1571), "1581");
        let only81 = DriveRoms {
            dos1581: Some(&r81),
            ..DriveRoms::default()
        };
        assert_eq!(pick(None, only81, DriveModel::C1571), "virtual");
        let short = DriveRoms {
            dos1571: Some(&r41),
            ..both
        };
        assert_eq!(pick(Some("1571"), short, DriveModel::C1571), "error");
    }

    #[test]
    fn crystal_ppm_option() {
        assert_eq!(crystal_ppm(None).unwrap(), DEFAULT_CRYSTAL_PPM);
        assert_eq!(crystal_ppm(Some("-25")).unwrap(), -25);
        assert!(crystal_ppm(Some("5000")).is_err());
        assert!(crystal_ppm(Some("fast")).is_err());
    }
}
